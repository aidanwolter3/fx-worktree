use crate::config::Config;
use crate::utils::{find_worktrees, clean_worktree, get_file_mtime, set_file_mtime};
use crate::environment::EnvironmentInfo;
use anyhow::{Context, Result, anyhow};
use rayon::prelude::*;
use std::fs;
use std::path::Path;

pub fn free_environment_by_id(config: &Config, id: &str) -> Result<()> {
    let lease_file_name = format!("{}.lease", id);
    let lease_file_path = config.leases_dir().join(&lease_file_name);

    if !lease_file_path.exists() {
        return Err(anyhow!("Environment lease file {:?} does not exist", lease_file_path));
    }

    let env_json = fs::read_to_string(&lease_file_path)
        .with_context(|| format!("Failed to read lease file {:?}", lease_file_path))?;
    let env_info: EnvironmentInfo =
        serde_json::from_str(&env_json).context("Failed to parse EnvironmentInfo JSON")?;

    free_environment_internal(&env_info)?;

    // Delete lease file
    fs::remove_file(&lease_file_path)
        .with_context(|| format!("Failed to delete lease file {:?}", lease_file_path))?;
    log::info!("Deleted lease file {:?}", lease_file_path);

    Ok(())
}

pub fn free_environment_internal(env_info: &EnvironmentInfo) -> Result<()> {
    log::info!("Freeing environment {}", env_info.environment_id);

    // Record index mtimes before clean
    let worktrees = find_worktrees(&env_info.path)?;
    let mut index_mtimes = Vec::new();
    for wt in &worktrees {
        let index_path = wt.join(".git/index");
        if index_path.exists() {
            if let Ok(mtime) = get_file_mtime(&index_path) {
                index_mtimes.push((index_path, mtime));
            }
        }
    }

    // 1. Clean the workspace (do not delete or remove worktrees, preserve out/ cache)
    clean_workspace(&env_info.path)?;

    // Restore index mtimes after clean
    for (path, mtime) in index_mtimes {
        if let Err(e) = set_file_mtime(&path, mtime) {
            log::warn!("Failed to restore index mtime for {:?}: {:?}", path, e);
        }
    }

    // 2. Restore args.gn in the build directory
    let out_dir = env_info.path.join("out/default");
    if out_dir.exists() {
        let args_gn_ref = out_dir.join("args.gn.ref");
        let args_gn = out_dir.join("args.gn");
        if args_gn_ref.exists() {
            log::info!("Restoring args.gn from args.gn.ref");
            fs::copy(&args_gn_ref, &args_gn).with_context(|| {
                format!("Failed to copy {:?} to {:?}", args_gn_ref, args_gn)
            })?;
        }
    }

    Ok(())
}

fn clean_workspace(workspace_path: &Path) -> Result<()> {
    if !workspace_path.exists() {
        return Ok(());
    }
    log::info!("Cleaning up workspace at {:?}", workspace_path);

    let worktrees = find_worktrees(workspace_path)?;
    worktrees.par_iter().try_for_each(|worktree_path| -> Result<()> {
        let is_root = worktree_path == workspace_path;
        if let Err(e) = clean_worktree(worktree_path, is_root) {
            log::error!("Failed to clean worktree {:?}: {:?}", worktree_path, e);
        }
        Ok(())
    })?;

    Ok(())
}
