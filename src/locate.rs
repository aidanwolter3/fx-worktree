// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Worktree path resolution.
//!
//! This module provides the [`locate_path`] function to resolve a worktree name
//! or path into a canonicalized directory path.

use crate::config::Config;
use anyhow::{Result, anyhow};
use std::path::PathBuf;

/// Locates the absolute path of a worktree by its name or path.
///
/// If `name` is `None`, it falls back to the last active leased worktree path.
///
/// The resolution checks in the following order:
/// 1. If `name` is a path containing directory components or pointing to an existing file/folder,
///    it canonicalizes it and verifies it is a registered Jiri worktree.
/// 2. If it is a string name, it performs an exact match against the folder names
///    of all registered Jiri worktrees.
/// 3. If no exact match is found, it performs a prefix match on the folder names.
/// 4. If still not found, it checks if the name matches a suffix/UUID part of a worktree
///    (e.g., matching the `e2a3f019` part of `fuchsia.x64_e2a3f019`).
///
/// Returns an error if the name is ambiguous (matches multiple worktrees) or is not found.
pub fn locate_path(config: &Config, name: Option<String>) -> Result<PathBuf> {
    let name = match name {
        Some(ref val) if !val.trim().is_empty() => val.clone(),
        _ => return config.read_last_worktree(),
    };

    let paths = crate::worktree::read_jiri_worktrees(config)?;

    let name_path = std::path::Path::new(&name);
    if name_path.components().count() > 1 || name_path.exists() {
        if let Ok(abs_path) = std::fs::canonicalize(name_path) {
            if paths.contains(&abs_path) {
                return Ok(abs_path);
            }
        }
        return Err(anyhow!(
            "Path {:?} is not a registered Jiri worktree",
            name_path
        ));
    }

    // Try exact match on directory name first
    for path in &paths {
        if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
            if dir_name == name {
                return Ok(path.clone());
            }
        }
    }

    // Try prefix match on directory name or UUID part
    let mut matches = Vec::new();
    for path in &paths {
        if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
            if dir_name.starts_with(&name) {
                matches.push((dir_name.to_string(), path.clone()));
                continue;
            }
            if let Some((_cfg, uuid_part)) = dir_name.rsplit_once('_') {
                if uuid_part.starts_with(&name) {
                    matches.push((dir_name.to_string(), path.clone()));
                }
            }
        }
    }

    if matches.is_empty() {
        return Err(anyhow!(
            "Worktree name '{}' not found in Jiri registry",
            name
        ));
    }

    if matches.len() > 1 {
        let match_names: Vec<String> = matches.iter().map(|(n, _)| n.clone()).collect();
        return Err(anyhow!(
            "Ambiguous worktree name '{}'. Matches: {}",
            name,
            match_names.join(", ")
        ));
    }

    Ok(matches.remove(0).1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn setup_mock_registry(fuchsia_dir: &std::path::Path, worktree_names: &[&str]) -> Vec<PathBuf> {
        let registry_dir = fuchsia_dir.join(".jiri_root");
        fs::create_dir_all(&registry_dir).unwrap();

        let mut paths = Vec::new();
        let mut registry_content = String::new();

        for name in worktree_names {
            // Jiri worktrees reside under .jiri_root/worktrees/
            let path = registry_dir.join("worktrees").join(name);
            fs::create_dir_all(&path).unwrap();

            // canonicalize is required because locate_path uses canonicalized paths from read_jiri_worktrees
            let canonical_path = path.canonicalize().unwrap();
            registry_content.push_str(&format!("{}\n", canonical_path.to_string_lossy()));
            paths.push(canonical_path);
        }

        fs::write(registry_dir.join("worktrees_registry"), registry_content).unwrap();
        paths
    }

    #[test]
    fn test_locate_exact_match() {
        let dir = tempdir().unwrap();
        let config = Config::new(Some(dir.path().to_path_buf())).unwrap();
        let paths = setup_mock_registry(dir.path(), &["wt-one", "wt-two"]);

        let res = locate_path(&config, Some("wt-one".to_string())).unwrap();
        assert_eq!(res, paths[0]);
    }

    #[test]
    fn test_locate_prefix_match() {
        let dir = tempdir().unwrap();
        let config = Config::new(Some(dir.path().to_path_buf())).unwrap();
        let paths = setup_mock_registry(dir.path(), &["wt-one", "wt-two"]);

        // "wt-o" prefix matches "wt-one"
        let res = locate_path(&config, Some("wt-o".to_string())).unwrap();
        assert_eq!(res, paths[0]);
    }

    #[test]
    fn test_locate_uuid_match() {
        let dir = tempdir().unwrap();
        let config = Config::new(Some(dir.path().to_path_buf())).unwrap();
        // Setup Jiri worktrees with config_uuid style naming
        let paths = setup_mock_registry(
            dir.path(),
            &["minimal.x64_e2a3f019", "minimal.arm64_98f41bc3"],
        );

        // Match suffix/UUID exactly
        let res = locate_path(&config, Some("e2a3f019".to_string())).unwrap();
        assert_eq!(res, paths[0]);

        // Match prefix of suffix/UUID
        let res2 = locate_path(&config, Some("98f4".to_string())).unwrap();
        assert_eq!(res2, paths[1]);
    }

    #[test]
    fn test_locate_ambiguous_match() {
        let dir = tempdir().unwrap();
        let config = Config::new(Some(dir.path().to_path_buf())).unwrap();
        setup_mock_registry(dir.path(), &["worktree-one", "worktree-two"]);

        // "worktree" prefix matches both
        let res = locate_path(&config, Some("worktree".to_string()));
        assert!(res.is_err());
        let err = res.unwrap_err().to_string();
        assert!(err.contains("Ambiguous worktree name 'worktree'"));
        assert!(err.contains("worktree-one"));
        assert!(err.contains("worktree-two"));
    }

    #[test]
    fn test_locate_not_found() {
        let dir = tempdir().unwrap();
        let config = Config::new(Some(dir.path().to_path_buf())).unwrap();
        setup_mock_registry(dir.path(), &["wt-one"]);

        let res = locate_path(&config, Some("nonexistent".to_string()));
        assert!(res.is_err());
        assert!(
            res.unwrap_err()
                .to_string()
                .contains("Worktree name 'nonexistent' not found")
        );
    }

    #[test]
    fn test_locate_last_worktree() {
        let dir = tempdir().unwrap();
        let config = Config::new(Some(dir.path().to_path_buf())).unwrap();
        config.init_topology().unwrap();
        let paths = setup_mock_registry(dir.path(), &["wt-one"]);

        // Record last active worktree
        config.record_last_worktree(&paths[0]).unwrap();

        // Pass None to locate
        let res = locate_path(&config, None).unwrap();
        assert_eq!(res, paths[0]);

        // Pass empty/whitespace to locate
        let res2 = locate_path(&config, Some("  ".to_string())).unwrap();
        assert_eq!(res2, paths[0]);
    }
}
