// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Management logic for leasing worktrees.

use crate::config::Config;
use crate::worktree::WorktreeInfo;
use anyhow::{Context, Result, anyhow};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

/// Leases a free worktree from the pool and sets up the lease metadata.
///
/// If `name` is specified, it attempts to lease that specific worktree. If `any` is true,
/// it scans the pool and leases the first available free worktree.
///
/// Under the hood, this function:
/// 1. Finds a candidate worktree path.
/// 2. Performs an atomic lock file creation (`lease.json`) to prevent concurrent lease race conditions.
/// 3. Backs up the current `args.gn` files in the worktree outdirs.
/// 4. Synchronizes the worktree if `sync` is true.
/// 5. Automatically creates and switches to a git branch named after the `agent_id` (if specified).
///
/// Returns the [`WorktreeInfo`] containing the details of the acquired lease.
pub fn lease_worktree(
    config: &Config,
    name: Option<&str>,
    any: bool,
    agent_id: Option<&str>,
    sync: bool,
    quiet: bool,
) -> Result<WorktreeInfo> {
    let mut acquired_lease = None;
    let mut wt_path = PathBuf::new();
    let mut wt_id = String::new();

    // 1. Find a free worktree in the pool
    let worktree_paths = crate::worktree::read_jiri_worktrees(config)?;
    if worktree_paths.is_empty() {
        return Err(anyhow!(
            "No worktrees found. Add one first using 'jiri worktree add'"
        ));
    }

    if let Some(target_name) = name {
        // Lease specific worktree
        let path = crate::locate::locate_path(config, Some(target_name.to_string()))?;
        if crate::worktree::get_worktree_state(config, &path)
            != crate::worktree::WorktreeState::Free
        {
            return Err(anyhow!("Worktree '{}' is not free", target_name));
        }

        let lease_file_path = path.join(".jiri_root").join("lease.json");

        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lease_file_path)
        {
            Ok(_) => {
                log::info!("Acquired lease lock: {:?}", lease_file_path);
                acquired_lease = Some(lease_file_path);
                wt_path = path;
                wt_id = target_name.to_string();
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(anyhow!("Worktree '{}' is already leased", target_name));
            }
            Err(e) => {
                return Err(e)
                    .context(format!("Failed to create lease file {:?}", lease_file_path));
            }
        }
    } else if any {
        // Lease any free worktree
        for path in worktree_paths {
            if crate::worktree::get_worktree_state(config, &path)
                != crate::worktree::WorktreeState::Free
            {
                continue;
            }
            let lease_file_path = path.join(".jiri_root").join("lease.json");
            if lease_file_path.exists() {
                continue;
            }
            if path.is_dir() {
                if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
                    let id = dir_name;

                    match fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&lease_file_path)
                    {
                        Ok(_) => {
                            log::info!("Acquired lease lock: {:?}", lease_file_path);
                            acquired_lease = Some(lease_file_path);
                            wt_path = path.clone();
                            wt_id = id.to_string();
                            break;
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                            // Lease busy, try next
                            continue;
                        }
                        Err(e) => {
                            return Err(e).context(format!(
                                "Failed to create lease file {:?}",
                                lease_file_path
                            ));
                        }
                    }
                }
            }
        }
    } else {
        return Err(anyhow!("Either worktree name or --any must be specified"));
    }

    if acquired_lease.is_none() {
        return Err(anyhow!("No free worktrees available in the pool."));
    }

    let lease_file_path = acquired_lease.unwrap();
    let current_time = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let wt_info = WorktreeInfo {
        worktree_id: wt_id.clone(),
        agent_id: agent_id.map(|s| s.to_string()),
        pid: std::process::id(),
        timestamp_sec: current_time,
        path: wt_path.clone(),
    };

    // Find outdirs first
    let outdirs = crate::fuchsia::find_outdirs(&wt_path)
        .context("Failed to find outdirs for lease backup")?;

    // Backup args.gn
    for outdir in &outdirs {
        let args_gn = outdir.join("args.gn");
        let args_gn_ref = outdir.join("args.gn.ref");
        if args_gn.exists() {
            log::info!("Backing up args.gn to args.gn.ref in {:?}", outdir);
            fs::copy(&args_gn, &args_gn_ref)
                .with_context(|| format!("Failed to backup {:?} to {:?}", args_gn, args_gn_ref))?;
        }
    }

    // Write lease info
    let wt_json = serde_json::to_string(&wt_info).context("Failed to serialize WorktreeInfo")?;
    fs::write(&lease_file_path, wt_json).context("Failed to write lease JSON")?;

    // Rollback helper on failure
    let rollback = || {
        log::warn!("Lease failed, releasing lease {}", wt_id);
        let _ = fs::remove_file(&lease_file_path);
        for outdir in &outdirs {
            let args_gn = outdir.join("args.gn");
            let args_gn_ref = outdir.join("args.gn.ref");
            if args_gn_ref.exists() {
                let _ = fs::copy(&args_gn_ref, &args_gn);
            }
        }
    };

    // 2. Reuse the worktree (clean and checkout target revisions)
    if sync {
        if let Err(e) = crate::worktree::sync_worktree(config, &wt_id, &wt_path, quiet) {
            rollback();
            return Err(e);
        }
    }

    if let Some(agent) = agent_id {
        if let Err(e) = setup_branch(&wt_path, agent) {
            rollback();
            return Err(e);
        }
    }

    config.record_last_worktree(&wt_info.path)?;

    Ok(wt_info)
}

fn setup_branch(workspace_path: &Path, agent_id: &str) -> Result<()> {
    if agent_id.is_empty() {
        return Ok(());
    }

    let branch_name = if agent_id.starts_with("feat/") || agent_id.starts_with("bug/") {
        agent_id.to_string()
    } else if agent_id.to_uppercase().starts_with("T-") {
        format!("feat/{}", agent_id.to_lowercase())
    } else {
        format!("feat/{}", agent_id)
    };

    log::info!("Setting up branch {} in {:?}", branch_name, workspace_path);

    // Check if we are already on this branch
    let current_branch_out = Command::new("git")
        .args(&["symbolic-ref", "--short", "HEAD"])
        .current_dir(workspace_path)
        .output();
    if let Ok(out) = current_branch_out {
        if out.status.success() {
            let current_branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if current_branch == branch_name {
                log::info!("Already on branch {}, skipping checkout", branch_name);
                return Ok(());
            }
        }
    }

    // Check if branch already exists
    let branch_exists = Command::new("git")
        .args(&[
            "show-ref",
            "--verify",
            &format!("refs/heads/{}", branch_name),
        ])
        .current_dir(workspace_path)
        .output()
        .context("Failed to run git show-ref")?;

    if branch_exists.status.success() {
        // Branch exists, check it out
        let output = Command::new("git")
            .args(&["checkout", &branch_name])
            .current_dir(workspace_path)
            .output()
            .context("Failed to run git checkout")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("already used by worktree") {
                log::warn!(
                    "Branch {} is already checked out in another worktree. Staying on detached HEAD.",
                    branch_name
                );
                eprintln!(
                    "⚠ Warning: Branch {} is already checked out in another worktree. Staying on detached HEAD.",
                    branch_name
                );
            } else {
                return Err(anyhow!(
                    "Failed to checkout existing branch {}: {}",
                    branch_name,
                    stderr
                ));
            }
        }
    } else {
        // Branch does not exist, create and checkout
        let output = Command::new("git")
            .args(&["checkout", "-b", &branch_name])
            .current_dir(workspace_path)
            .output()
            .context("Failed to run git checkout -b")?;
        if !output.status.success() {
            return Err(anyhow!(
                "Failed to create and checkout branch {}: {}",
                branch_name,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }

    Ok(())
}
