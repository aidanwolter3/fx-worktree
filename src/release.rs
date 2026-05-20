use crate::config::Config;
use crate::environment::EnvironmentInfo;
use crate::utils::{
    clean_worktree, copy_file_if_different, find_worktrees, get_file_mtime, set_file_mtime,
};
use anyhow::{Context, Result, anyhow};
use rayon::prelude::*;
use std::fs;
use std::path::Path;

pub fn release_worktree(config: &Config, id: &str) -> Result<String> {
    if std::path::Path::new(id).components().count() > 1 {
        return Err(anyhow!("Invalid ID: {}", id));
    }

    let leases_dir = config.leases_dir();
    if !leases_dir.exists() {
        return Err(anyhow!("No active leases found."));
    }

    let mut matches = Vec::new();

    for entry in fs::read_dir(&leases_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("lease") {
            if let Ok(env_json) = fs::read_to_string(&path) {
                if let Ok(env_info) = serde_json::from_str::<EnvironmentInfo>(&env_json) {
                    if env_info.environment_id == id || env_info.agent_id == id {
                        matches.push((path, env_info));
                    }
                } else {
                    log::warn!("Failed to parse lease file {:?}", path);
                }
            }
        }
    }

    if matches.is_empty() {
        return Err(anyhow!("No active lease found matching '{}'", id));
    }

    if matches.len() > 1 {
        let agent_matches: Vec<&str> = matches
            .iter()
            .map(|(_, info)| info.agent_id.as_str())
            .collect();
        if agent_matches.iter().all(|&a| a == id) {
            let env_ids: Vec<String> = matches
                .iter()
                .map(|(_, info)| info.environment_id.clone())
                .collect();
            return Err(anyhow!(
                "Agent '{}' has leased multiple worktrees: {}. Please release by worktree ID instead.",
                id,
                env_ids.join(", ")
            ));
        } else {
            return Err(anyhow!("Ambiguous ID '{}': matches multiple leases.", id));
        }
    }

    let (lease_file_path, env_info) = &matches[0];
    let released_id = env_info.environment_id.clone();
    release_worktree_internal(env_info)?;

    // Delete lease file
    fs::remove_file(lease_file_path)
        .with_context(|| format!("Failed to delete lease file {:?}", lease_file_path))?;
    log::info!("Deleted lease file {:?}", lease_file_path);

    Ok(released_id)
}

pub fn release_worktree_internal(env_info: &EnvironmentInfo) -> Result<()> {
    log::info!("Releasing worktree {}", env_info.environment_id);

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
    // We use copy_file_if_different instead of a standard copy to ensure that if the args.gn
    // contents haven't changed, we do not update its modification time.
    //
    // If we update its mtime on every free/use cycle, Ninja will detect args.gn as newer than
    // the build.ninja.stamp (which was generated during the previous compile), causing GN
    // to regenerate build files on the next compile, which in turn dirties generated Bazel
    // workspace files (e.g., BUILD.bazel mappings) and breaks no-op builds.
    let out_dir = env_info.path.join("out/default");
    if out_dir.exists() {
        let args_gn_ref = out_dir.join("args.gn.ref");
        let args_gn = out_dir.join("args.gn");
        if args_gn_ref.exists() {
            log::info!("Restoring args.gn from args.gn.ref");
            copy_file_if_different(&args_gn_ref, &args_gn)
                .with_context(|| format!("Failed to copy {:?} to {:?}", args_gn_ref, args_gn))?;
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
    worktrees
        .par_iter()
        .try_for_each(|worktree_path| -> Result<()> {
            let is_root = worktree_path == workspace_path;
            if let Err(e) = clean_worktree(worktree_path, is_root) {
                log::error!("Failed to clean worktree {:?}: {:?}", worktree_path, e);
            }
            Ok(())
        })?;

    Ok(())
}
