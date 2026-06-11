// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Management logic for marking worktrees as free.

use crate::config::Config;
use crate::worktree::{WorktreeState, set_worktree_state};
use anyhow::{Context, Result, anyhow};
use std::fs;

/// Marks a Jiri worktree as free (available for leasing).
///
/// Returns the name of the worktree if successful, or an error if:
/// - The worktree does not exist on disk.
/// - The worktree is not registered in Jiri's worktrees registry.
/// - The worktree is already marked free.
pub fn mark_free_worktree(config: &Config, name: &str, quiet: bool) -> Result<String> {
    let path = config.worktrees_dir().join(name);

    if !path.exists() {
        return Err(anyhow!(
            "Worktree with name '{}' does not exist in {:?}",
            name,
            config.worktrees_dir()
        ));
    }

    let path = fs::canonicalize(&path)
        .with_context(|| format!("Failed to resolve absolute path for {:?}", path))?;

    // 1. Verify it is a registered Jiri worktree
    let registered_paths = crate::worktree::read_jiri_worktrees(config)?;
    if !registered_paths.contains(&path) {
        return Err(anyhow!(
            "Path {:?} is not registered in Jiri worktrees registry. Only Jiri worktrees can be marked free.",
            path
        ));
    }

    // 2. Check if already free
    if crate::worktree::get_worktree_state(config, &path) == WorktreeState::Free {
        return Err(anyhow!("Worktree '{}' is already free", name));
    }

    if !quiet {
        eprintln!("Marking worktree {} as free...", name);
    }

    // 3. Mark it as Free
    set_worktree_state(&path, WorktreeState::Free)?;

    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn test_mark_free_success() {
        let dir = tempdir().unwrap();
        let config = Config::new(Some(dir.path().to_path_buf())).unwrap();
        let wt_path = setup_mock_registry(dir.path(), "wt-one");

        // By default (no state file), state is Reserved (since we have no state file, get_worktree_state falls back to Reserved/non-free logic)
        assert_eq!(
            crate::worktree::get_worktree_state(&config, &wt_path),
            WorktreeState::Reserved
        );

        // Mark it free
        let res = mark_free_worktree(&config, "wt-one", true).unwrap();
        assert_eq!(res, "wt-one");
        assert_eq!(
            crate::worktree::get_worktree_state(&config, &wt_path),
            WorktreeState::Free
        );
    }

    #[test]
    fn test_mark_free_already_free_error() {
        let dir = tempdir().unwrap();
        let config = Config::new(Some(dir.path().to_path_buf())).unwrap();
        let _wt_path = setup_mock_registry(dir.path(), "wt-one");

        // Mark it free once
        mark_free_worktree(&config, "wt-one", true).unwrap();

        // Mark it free again -> should fail
        let res = mark_free_worktree(&config, "wt-one", true);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("already free"));
    }

    #[test]
    fn test_mark_free_nonexistent_error() {
        let dir = tempdir().unwrap();
        let config = Config::new(Some(dir.path().to_path_buf())).unwrap();
        config.init_topology().unwrap();

        // No registry / no directory
        let res = mark_free_worktree(&config, "nonexistent", true);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("does not exist"));
    }
}
