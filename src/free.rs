use crate::config::Config;
use crate::utils::{find_worktrees, run_command};
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

    // 1. Clean the workspace (do not delete or remove worktrees, preserve out/ cache)
    clean_workspace(&env_info.path)?;

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
        if let Err(e) = run_command("git", &["reset", "--hard"], worktree_path, &[]) {
            log::error!("Failed to run git reset in {:?}: {:?}", worktree_path, e);
        }

        let clean_args = if worktree_path == workspace_path {
            vec![
                "clean",
                "-fdx",
                "-e", ".fxenv-completed",
                "-e", "prebuilt",
                "-e", ".jiri_root",
                "-e", ".fx-build-dir",
                "-e", "out", // Preserves build cache!
            ]
        } else {
            vec!["clean", "-fdx"]
        };

        if let Err(e) = run_command("git", &clean_args, worktree_path, &[]) {
            log::error!("Failed to run git clean in {:?}: {:?}", worktree_path, e);
        }
        Ok(())
    })?;

    Ok(())
}
