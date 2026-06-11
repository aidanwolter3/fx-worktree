// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Fuchsia GN configuration and build directory utilities.

use anyhow::{Context, Result, anyhow};
use std::fs;
use std::path::{Path, PathBuf};

/// Scans the `out/` directory inside `workspace_path` and returns all subdirectories
/// containing an `args.gn` file.
pub fn find_outdirs(workspace_path: &Path) -> Result<Vec<PathBuf>> {
    let out_dir = workspace_path.join("out");
    if !out_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut outdirs = Vec::new();
    for entry in fs::read_dir(&out_dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            let path = entry.path();
            if path.join("args.gn").is_file() {
                outdirs.push(path);
            }
        }
    }
    Ok(outdirs)
}

/// Parses the `args.gn` file of a build output directory to determine the configured product
/// and board names.
///
/// Returns a string formatting like `{product}.{board}` (or just `{product}` if board is empty).
pub fn get_config_name_for_outdir(outdir_path: &Path) -> Result<String> {
    let args_gn_path = outdir_path.join("args.gn");
    if !args_gn_path.exists() {
        return Err(anyhow!("args.gn not found at {:?}", args_gn_path));
    }
    let content = fs::read_to_string(&args_gn_path)
        .with_context(|| format!("Failed to read {:?}", args_gn_path))?;
    let mut product = None;
    let mut board = None;
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("build_info_product") {
            if let Some(val) = line.split('=').nth(1) {
                product = Some(val.trim().trim_matches('"').to_string());
            }
        } else if line.starts_with("build_info_board") {
            if let Some(val) = line.split('=').nth(1) {
                board = Some(val.trim().trim_matches('"').to_string());
            }
        }
    }
    match (product, board) {
        (Some(p), Some(b)) => {
            if b.is_empty() {
                Ok(p)
            } else {
                Ok(format!("{}.{}", p, b))
            }
        }
        (Some(p), None) => Ok(p),
        _ => Err(anyhow!(
            "Failed to find build_info_product in {:?}",
            args_gn_path
        )),
    }
}

/// Returns the configuration names for all build output directories found in `workspace_path`.
pub fn get_configs(workspace_path: &Path) -> Result<Vec<String>> {
    let outdirs = find_outdirs(workspace_path)?;
    let mut configs = Vec::new();
    for outdir in outdirs {
        if let Ok(config) = get_config_name_for_outdir(&outdir) {
            configs.push(config);
        }
    }
    Ok(configs)
}

/// Inspects `.jiri_root/config` to verify if Jiri's `<package_cache>` is enabled.
pub fn is_package_cache_enabled(fuchsia_dir: &Path) -> bool {
    let config_path = fuchsia_dir.join(".jiri_root").join("config");
    if !config_path.exists() {
        return false;
    }
    let content = match fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(_) => return false,
    };

    if let Some(start) = content.find("<package_cache>") {
        if let Some(end) = content[start..].find("</package_cache>") {
            let section = &content[start..start + end];
            let normalized: String = section.chars().filter(|c| !c.is_whitespace()).collect();
            return normalized.contains("<enabled>true</enabled>");
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_get_configs_success() {
        let dir = tempdir().unwrap();
        let out_dir = dir.path().join("out/default");
        fs::create_dir_all(&out_dir).unwrap();
        let args_gn_path = out_dir.join("args.gn");
        let mut file = File::create(args_gn_path).unwrap();
        writeln!(file, "build_info_product = \"core\"").unwrap();
        writeln!(file, "build_info_board = \"x64\"").unwrap();

        let configs = get_configs(dir.path()).unwrap();
        assert_eq!(configs, vec!["core.x64"]);
    }

    #[test]
    fn test_get_configs_multiple() {
        let dir = tempdir().unwrap();
        let out_dir1 = dir.path().join("out/config1");
        fs::create_dir_all(&out_dir1).unwrap();
        let mut file1 = File::create(out_dir1.join("args.gn")).unwrap();
        writeln!(file1, "build_info_product = \"core\"").unwrap();
        writeln!(file1, "build_info_board = \"x64\"").unwrap();

        let out_dir2 = dir.path().join("out/config2");
        fs::create_dir_all(&out_dir2).unwrap();
        let mut file2 = File::create(out_dir2.join("args.gn")).unwrap();
        writeln!(file2, "build_info_product = \"workbench\"").unwrap();

        let mut configs = get_configs(dir.path()).unwrap();
        configs.sort();
        assert_eq!(configs, vec!["core.x64", "workbench"]);
    }

    #[test]
    fn test_get_configs_product_only() {
        let dir = tempdir().unwrap();
        let out_dir = dir.path().join("out/default");
        fs::create_dir_all(&out_dir).unwrap();
        let args_gn_path = out_dir.join("args.gn");
        let mut file = File::create(args_gn_path).unwrap();
        writeln!(file, "build_info_product = \"core\"").unwrap();

        let configs = get_configs(dir.path()).unwrap();
        assert_eq!(configs, vec!["core"]);
    }

    #[test]
    fn test_get_configs_empty_board() {
        let dir = tempdir().unwrap();
        let out_dir = dir.path().join("out/default");
        fs::create_dir_all(&out_dir).unwrap();
        let args_gn_path = out_dir.join("args.gn");
        let mut file = File::create(args_gn_path).unwrap();
        writeln!(file, "build_info_product = \"core\"").unwrap();
        writeln!(file, "build_info_board = \"\"").unwrap();

        let configs = get_configs(dir.path()).unwrap();
        assert_eq!(configs, vec!["core"]);
    }

    #[test]
    fn test_get_configs_missing_product() {
        let dir = tempdir().unwrap();
        let out_dir = dir.path().join("out/default");
        fs::create_dir_all(&out_dir).unwrap();
        let args_gn_path = out_dir.join("args.gn");
        let mut file = File::create(args_gn_path).unwrap();
        writeln!(file, "build_info_board = \"x64\"").unwrap();

        let configs = get_configs(dir.path()).unwrap();
        assert!(configs.is_empty());
    }

    #[test]
    fn test_get_configs_no_outdirs() {
        let dir = tempdir().unwrap();
        let configs = get_configs(dir.path()).unwrap();
        assert!(configs.is_empty());
    }

    #[test]
    fn test_get_configs_with_comments_and_spaces() {
        let dir = tempdir().unwrap();
        let out_dir = dir.path().join("out/default");
        fs::create_dir_all(&out_dir).unwrap();
        let args_gn_path = out_dir.join("args.gn");
        let mut file = File::create(args_gn_path).unwrap();
        writeln!(file, "# Some comment").unwrap();
        writeln!(file, "  build_info_product   =   \"core-nested\"  ").unwrap();
        writeln!(file, "other_var = \"value\"").unwrap();
        writeln!(file, "build_info_board = \"arm64\"").unwrap();

        let configs = get_configs(dir.path()).unwrap();
        assert_eq!(configs, vec!["core-nested.arm64"]);
    }
}
