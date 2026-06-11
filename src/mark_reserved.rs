// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Management logic for reserving worktrees.

use crate::config::Config;
use crate::worktree::{set_worktree_state, WorktreeState};
use anyhow::{Result, anyhow};

/// Marks a Jiri worktree as reserved (unavailable for leasing).
///
/// Returns an error if:
/// - The worktree does not exist on disk or in Jiri registry.
/// - The worktree is already marked reserved.
/// - The worktree is currently leased (has an active `lease.json` file).
pub fn mark_reserved_worktree(
    config: &Config,
    name: &str,
    quiet: bool,
) -> Result<()> {
    // 1. Resolve name to current path
    let path = crate::locate::locate_path(config, Some(name.to_string()))?;

    let state = crate::worktree::get_worktree_state(config, &path);
    if state == WorktreeState::Reserved {
        return Err(anyhow!("Worktree '{}' is already reserved", name));
    }

    // 2. Check if leased
    let lease_file = path.join(".jiri_root").join("lease.json");
    if lease_file.exists() {
        return Err(anyhow!(
            "Cannot reserve worktree '{}' because it is currently leased",
            name
        ));
    }

    if !quiet {
        eprintln!("Marking worktree {} as reserved...", name);
    }

    // 3. Mark it as Reserved
    set_worktree_state(&path, WorktreeState::Reserved)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn setup_mock_registry(fuchsia_dir: &std::path::Path, name: &str) -> PathBuf {
        let registry_dir = fuchsia_dir.join(".jiri_root");
        fs::create_dir_all(&registry_dir).unwrap();

        let path = registry_dir.join("worktrees").join(name);
        fs::create_dir_all(&path).unwrap();
        let canonical_path = path.canonicalize().unwrap();

        fs::write(
            registry_dir.join("worktrees_registry"),
            format!("{}\n", canonical_path.to_string_lossy()),
        )
        .unwrap();

        canonical_path
    }

    #[test]
    fn test_mark_reserved_success() {
        let dir = tempdir().unwrap();
        let config = Config::new(Some(dir.path().to_path_buf())).unwrap();
        let wt_path = setup_mock_registry(dir.path(), "wt-one");

        // Set to Free first
        set_worktree_state(&wt_path, WorktreeState::Free).unwrap();
        assert_eq!(
            crate::worktree::get_worktree_state(&config, &wt_path),
            WorktreeState::Free
        );

        // Mark reserved
        mark_reserved_worktree(&config, "wt-one", true).unwrap();
        assert_eq!(
            crate::worktree::get_worktree_state(&config, &wt_path),
            WorktreeState::Reserved
        );
    }

    #[test]
    fn test_mark_reserved_already_reserved_error() {
        let dir = tempdir().unwrap();
        let config = Config::new(Some(dir.path().to_path_buf())).unwrap();
        let wt_path = setup_mock_registry(dir.path(), "wt-one");

        // By default, it is Reserved (no state file is treated as Reserved)
        assert_eq!(
            crate::worktree::get_worktree_state(&config, &wt_path),
            WorktreeState::Reserved
        );

        // Mark reserved again -> should error
        let res = mark_reserved_worktree(&config, "wt-one", true);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("already reserved"));
    }

    #[test]
    fn test_mark_reserved_leased_error() {
        let dir = tempdir().unwrap();
        let config = Config::new(Some(dir.path().to_path_buf())).unwrap();
        let wt_path = setup_mock_registry(dir.path(), "wt-one");

        // Set state to Free
        set_worktree_state(&wt_path, WorktreeState::Free).unwrap();

        // Create mock lease file
        let lease_file = wt_path.join(".jiri_root").join("lease.json");
        fs::create_dir_all(lease_file.parent().unwrap()).unwrap();
        fs::write(lease_file, "{}").unwrap();

        // Try mark reserved -> should fail because it is leased
        let res = mark_reserved_worktree(&config, "wt-one", true);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("currently leased"));
    }
}
