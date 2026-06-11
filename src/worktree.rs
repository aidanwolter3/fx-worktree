// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Jiri Worktree representation, state persistence, and sync logic.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use crate::config::Config;
use anyhow::{Context, Result};

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
    let registry_path = config.fuchsia_dir.join(".jiri_root").join("worktrees_registry");
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
