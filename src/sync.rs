use crate::config::Config;
use crate::utils::{copy_toolchain_metadata, find_worktrees, get_file_mtime, run_command, set_file_mtime};
use anyhow::{Context, Result};
use std::path::Path;

pub fn sync_environment_by_id(config: &Config, id: &str, quiet: bool) -> Result<()> {
    let path = crate::locate::locate_path(config, Some(id.to_string()))?;
    sync_environment(config, id, &path, quiet)
}

pub fn sync_environment(
    config: &Config,
    env_id: &str,
    workspace_path: &Path,
    quiet: bool,
) -> Result<()> {
    let workspace_path_buf = workspace_path
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize workspace path {:?}", workspace_path))?;
    let workspace_path = &workspace_path_buf;

    let worktrees = find_worktrees(workspace_path)?;

    // Record initial mtimes in case we exit early (git status in no-op check might touch them)
    let mut initial_mtimes = Vec::new();
    for wt in &worktrees {
        let index_path = wt.join(".git/index");
        if index_path.exists() {
            if let Ok(mtime) = get_file_mtime(&index_path) {
                initial_mtimes.push((index_path, mtime));
            }
        }
    }

    // 1. Optimize no-op check: Compare HEAD of parent repo with HEAD of workspace root
    // and check if all worktrees are clean.
    let is_noop = (|| -> Result<bool> {
        let parent_head = run_command("git", &["rev-parse", "HEAD"], &config.fuchsia_dir, &[])?;
        let parent_head_sha = String::from_utf8_lossy(&parent_head.stdout)
            .trim()
            .to_string();

        let workspace_head = run_command("git", &["rev-parse", "HEAD"], workspace_path, &[])?;
        let workspace_head_sha = String::from_utf8_lossy(&workspace_head.stdout)
            .trim()
            .to_string();

        if parent_head_sha != workspace_head_sha {
            return Ok(false);
        }

        if !workspace_path.join(".jiri_root").exists() {
            return Ok(false);
        }

        // Check if all worktrees are clean
        for wt in &worktrees {
            let status = run_command(
                "git",
                &["status", "--porcelain", "-uno"],
                wt,
                &[],
            )?;
            if !status.stdout.is_empty() {
                return Ok(false);
            }
        }

        Ok(true)
    })()
    .unwrap_or(false);

    if is_noop {
        log::info!("Environment {} is already synced (no-op).", env_id);
        // Restore mtimes because git status might have touched them
        for (path, mtime) in initial_mtimes {
            if let Err(e) = set_file_mtime(&path, mtime) {
                log::warn!("Failed to restore index mtime for {:?}: {:?}", path, e);
            }
        }
        return Ok(());
    }

    if !quiet {
        eprintln!("Syncing environment {}...", env_id);
    }

    // Record index mtimes and HEADs before sync (for partial restoration)
    let mut wt_states = Vec::new();
    for wt in &worktrees {
        let index_path = wt.join(".git/index");
        if index_path.exists() {
            let mtime = get_file_mtime(&index_path).ok();
            let head = run_command("git", &["rev-parse", "HEAD"], wt, &[])
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .ok();
            wt_states.push((wt.clone(), index_path, mtime, head));
        }
    }

    // Copy/restore toolchain metadata (must be before sync for hooks)
    copy_toolchain_metadata(config, workspace_path)?;

    // Call 'jiri worktree sync'
    let jiri_bin = config.fuchsia_dir.join(".jiri_root/bin/jiri");
    let jiri_cmd = if jiri_bin.exists() {
        jiri_bin.to_str().unwrap()
    } else {
        "jiri"
    };

    run_command(
        jiri_cmd,
        &["worktree", "sync"],
        workspace_path,
        &[],
    )
    .context("Failed to run jiri worktree sync")?;

    // Restore index mtimes for unchanged and clean worktrees
    for (wt, index_path, mtime_opt, old_head_opt) in wt_states {
        if let (Some(mtime), Some(old_head)) = (mtime_opt, old_head_opt) {
            let new_head = run_command("git", &["rev-parse", "HEAD"], &wt, &[])
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();
            if new_head == old_head {
                let status = run_command("git", &["status", "--porcelain", "-uno"], &wt, &[])?;
                if status.stdout.is_empty() {
                    if let Err(e) = set_file_mtime(&index_path, mtime) {
                        log::warn!("Failed to restore index mtime for {:?}: {:?}", index_path, e);
                    }
                }
            }
        }
    }

    Ok(())
}
