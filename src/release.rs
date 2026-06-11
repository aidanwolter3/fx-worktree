// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Management logic for releasing/resetting worktrees.

use crate::config::Config;
use crate::worktree::WorktreeInfo;
use crate::utils::{copy_file_if_different, run_command};
use anyhow::{Context, Result, anyhow};
use std::fs;

/// Releases a leased worktree and resets its state back to before the lease.
///
/// Releasing a worktree performs the following cleanup operations:
/// 1. Finds the leased worktree matching `id` (exact name or suffix/UUID).
/// 2. Restores the main git branch state (deletes the agent git branch, if any).
/// 3. Runs `jiri clean` inside the worktree directory to restore untracked/dirty states.
/// 4. Restores backed up `args.gn` files in the output build directories.
/// 5. Removes the `lease.json` metadata file, freeing it for subsequent leases.
///
/// Returns the folder name of the released worktree.
pub fn release_worktree(config: &Config, id: &str) -> Result<String> {
    if std::path::Path::new(id).components().count() > 1 {
        return Err(anyhow!("Invalid ID: {}", id));
    }

    let worktree_paths = crate::worktree::read_jiri_worktrees(config)?;
    let mut matches = Vec::new();

    for path in worktree_paths {
        let lease_file_path = path.join(".jiri_root").join("lease.json");
        if lease_file_path.is_file() {
            if let Ok(wt_json) = fs::read_to_string(&lease_file_path) {
                if let Ok(wt_info) = serde_json::from_str::<WorktreeInfo>(&wt_json) {
                    if wt_info.worktree_id == id || wt_info.agent_id.as_deref() == Some(id) {
                        matches.push((lease_file_path, wt_info));
                    }
                } else {
                    log::warn!("Failed to parse lease file {:?}", lease_file_path);
                }
            }
        }
    }

    if matches.is_empty() {
        return Err(anyhow!("No active lease found matching '{}'", id));
    }

    if matches.len() > 1 {
        let agent_matches: Vec<Option<&str>> = matches
            .iter()
            .map(|(_, info)| info.agent_id.as_deref())
            .collect();
        if agent_matches.iter().all(|&a| a == Some(id)) {
            let wt_ids: Vec<String> = matches
                .iter()
                .map(|(_, info)| info.worktree_id.clone())
                .collect();
            return Err(anyhow!(
                "Agent '{}' has leased multiple worktrees: {}. Please release by worktree ID instead.",
                id,
                wt_ids.join(", ")
            ));
        } else {
            return Err(anyhow!("Ambiguous ID '{}': matches multiple leases.", id));
        }
    }

    let (lease_file_path, wt_info) = &matches[0];
    let released_id = wt_info.worktree_id.clone();
    release_worktree_internal(config, wt_info)?;

    // Delete lease file
    fs::remove_file(lease_file_path)
        .with_context(|| format!("Failed to delete lease file {:?}", lease_file_path))?;
    log::info!("Deleted lease file {:?}", lease_file_path);

    Ok(released_id)
}

pub fn release_worktree_internal(config: &Config, wt_info: &WorktreeInfo) -> Result<()> {
    log::info!("Releasing worktree {}", wt_info.worktree_id);

    // 1. Clean the workspace by calling 'jiri clean'.
    // Note: This relies on 'jiri' being optimized to clean repositories in parallel
    // to meet performance requirements (less than 5 seconds).
    let jiri_bin = config.fuchsia_dir.join(".jiri_root/bin/jiri");
    let jiri_cmd = if jiri_bin.exists() {
        jiri_bin.to_str().unwrap()
    } else {
        "jiri"
    };

    run_command(jiri_cmd, &["clean"], &wt_info.path, &[]).context("Failed to run jiri clean")?;

    // 2. Restore args.gn in all outdirs
    let outdirs = crate::fuchsia::find_outdirs(&wt_info.path)?;
    for outdir in outdirs {
        let args_gn_ref = outdir.join("args.gn.ref");
        let args_gn = outdir.join("args.gn");
        if args_gn_ref.exists() {
            log::info!("Restoring args.gn from args.gn.ref in {:?}", outdir);
            copy_file_if_different(&args_gn_ref, &args_gn)
                .with_context(|| format!("Failed to copy {:?} to {:?}", args_gn_ref, args_gn))?;
        }
    }

    Ok(())
}
