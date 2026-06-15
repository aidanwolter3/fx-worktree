// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Jiri Worktree representation, state persistence, and sync logic.

use crate::config::Config;
use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Represents information stored in the active `lease.json` metadata file
/// of a leased worktree.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct WorktreeInfo {
    /// Unique identifier for this worktree (folder name).
    pub worktree_id: String,
    /// Optional identifier of the agent leasing the worktree.
    pub agent_id: Option<String>,
    /// Process ID of the leasing agent or parent process.
    pub pid: u32,
    /// POSIX timestamp indicating when the lease was acquired.
    pub timestamp_sec: u64,
    /// Absolute path to the worktree directory.
    pub path: PathBuf,
}

/// The state of availability of a worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeState {
    /// Available to be leased.
    Free,
    /// Unavailable for leasing (manual lock or active reservation).
    Reserved,
}

/// Reads the Jiri worktrees registry file (`.jiri_root/worktrees_registry`) and returns
/// canonical paths to all existing worktrees.
pub fn read_jiri_worktrees(config: &Config) -> Result<Vec<PathBuf>> {
    let registry_path = config
        .fuchsia_dir
        .join(".jiri_root")
        .join("worktrees_registry");
    if !registry_path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&registry_path)
        .with_context(|| format!("Failed to read registry {:?}", registry_path))?;
    let paths = content
        .lines()
        .map(|line| PathBuf::from(line.trim()))
        .filter(|path| path.exists())
        .collect();
    Ok(paths)
}

/// Retrieves the current state of a worktree from its local metadata file.
///
/// Falls back to [`WorktreeState::Reserved`] if the metadata file is missing or corrupted.
pub fn get_worktree_state(_config: &Config, path: &Path) -> WorktreeState {
    let state_file = path.join(".jiri_root").join("worktree-state");
    if state_file.is_file() {
        if let Ok(content) = fs::read_to_string(&state_file) {
            match content.trim() {
                "free" | "pool" => return WorktreeState::Free,
                "reserved" | "not_in_pool" => return WorktreeState::Reserved,
                _ => {}
            }
        }
    }
    WorktreeState::Reserved
}

/// Sets the state of a worktree by writing to its local metadata file.
pub fn set_worktree_state(path: &Path, state: WorktreeState) -> Result<()> {
    let state_file = path.join(".jiri_root").join("worktree-state");
    if let Some(parent) = state_file.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory {:?}", parent))?;
    }
    let content = match state {
        WorktreeState::Free => "free",
        WorktreeState::Reserved => "reserved",
    };
    fs::write(&state_file, content)
        .with_context(|| format!("Failed to write state to {:?}", state_file))
}

/// Invokes the `jiri worktree sync` command on the specified worktree to update
/// it to the configurations declared in the Jiri manifests.
pub fn sync_worktree(
    config: &Config,
    wt_id: &str,
    workspace_path: &Path,
    quiet: bool,
) -> Result<()> {
    let workspace_path_buf = workspace_path
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize workspace path {:?}", workspace_path))?;
    let workspace_path = &workspace_path_buf;

    if !quiet {
        eprintln!("Syncing worktree {}...", wt_id);
    }

    // Call 'jiri worktree sync'
    let jiri_bin = config.fuchsia_dir.join(".jiri_root/bin/jiri");
    let jiri_cmd = if jiri_bin.exists() {
        jiri_bin.to_str().unwrap()
    } else {
        "jiri"
    };

    crate::utils::run_command(jiri_cmd, &["worktree", "sync"], workspace_path, &[])
        .context("Failed to run jiri worktree sync")?;

    Ok(())
}

/// Adds a new Jiri worktree inside the configured worktrees directory
/// and marks its state as `Free`.
pub fn add_worktree(config: &Config, name: &str, set_configs: Vec<String>) -> Result<()> {
    let target_path = config.worktrees_dir().join(name);
    if target_path.exists() {
        return Err(anyhow!(
            "Directory already exists at target path {:?}",
            target_path
        ));
    }

    if !crate::fuchsia::is_package_cache_enabled(&config.fuchsia_dir) {
        println!(
            "Warning: package-cache is not enabled. Enabling it makes 'jiri worktree add' faster by sharing packages with the main tree."
        );
        println!("To enable it, run:");
        println!("  jiri init -package-cache=true");
        println!("After enabling, run the following to migrate:");
        println!("  jiri fetch-packages -local-manifest");
        println!();
    }

    let jiri_bin = config.fuchsia_dir.join(".jiri_root/bin/jiri");
    let jiri_cmd = if jiri_bin.exists() {
        jiri_bin.to_str().unwrap()
    } else {
        "jiri"
    };

    println!("Creating Jiri worktree '{}'...", name);
    crate::utils::run_command(
        jiri_cmd,
        &["worktree", "add", target_path.to_str().unwrap()],
        &config.fuchsia_dir,
        &[],
    )
    .context("Failed to run jiri worktree add")?;

    // Mark the new worktree as Free by default
    set_worktree_state(&target_path, WorktreeState::Free)?;
    config.record_last_worktree(&target_path)?;
    println!("Worktree '{}' successfully added and marked as free.", name);

    // Set up configurations if requested
    for cfg in set_configs {
        println!("Configuring outdir for '{}' in worktree '{}'...", cfg, name);
        crate::utils::run_command(
            "scripts/fx",
            &["--dir", &format!("out/{}", cfg), "set", &cfg],
            &target_path,
            &[],
        )
        .with_context(|| format!("Failed to run fx set for config '{}'", cfg))?;
    }

    Ok(())
}

/// Removes a Jiri worktree after verifying it is not leased (unless forced).
pub fn remove_worktree(config: &Config, name: &str, force: bool) -> Result<()> {
    let path = crate::locate::locate_path(config, Some(name.to_string()))?;

    // Check if leased
    let lease_file_path = path.join(".jiri_root").join("lease.json");
    if lease_file_path.exists() && !force {
        return Err(anyhow!(
            "Worktree '{}' is currently leased. Use --force to override.",
            name
        ));
    }

    let jiri_bin = config.fuchsia_dir.join(".jiri_root/bin/jiri");
    let jiri_cmd = if jiri_bin.exists() {
        jiri_bin.to_str().unwrap()
    } else {
        "jiri"
    };

    println!("Removing Jiri worktree '{}'...", name);
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("-f");
    }
    args.push(path.to_str().unwrap());

    crate::utils::run_command(jiri_cmd, &args, &config.fuchsia_dir, &[])
        .context("Failed to run jiri worktree remove")?;

    Ok(())
}
