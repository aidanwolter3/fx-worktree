use crate::config::Config;
use anyhow::{Context, Result, anyhow};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn run_command(
    cmd: &str,
    args: &[&str],
    cwd: &Path,
    env: &[(&str, &str)],
) -> Result<std::process::Output> {
    let mut command = Command::new(cmd);
    command.args(args);
    command.current_dir(cwd);
    for (key, val) in env {
        command.env(key, val);
    }

    log::debug!("Running command: {} {} in {:?}", cmd, args.join(" "), cwd);

    let output = command
        .output()
        .with_context(|| format!("Failed to execute command: {} {}", cmd, args.join(" ")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(anyhow!(
            "Command failed: {} {}\nExit Status: {}\nStdout: {}\nStderr: {}",
            cmd,
            args.join(" "),
            output.status,
            stdout,
            stderr
        ));
    }

    Ok(output)
}

pub fn copy_file_if_different(src: &Path, dst: &Path) -> Result<()> {
    if !dst.exists() {
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(src, dst).with_context(|| format!("Failed to copy {:?} to {:?}", src, dst))?;
        return Ok(());
    }

    let src_meta = fs::metadata(src)?;
    let dst_meta = fs::metadata(dst)?;

    if src_meta.len() != dst_meta.len() {
        fs::copy(src, dst).with_context(|| format!("Failed to copy {:?} to {:?}", src, dst))?;
        return Ok(());
    }

    let src_content = fs::read(src)?;
    let dst_content = fs::read(dst)?;

    if src_content != dst_content {
        fs::copy(src, dst).with_context(|| format!("Failed to copy {:?} to {:?}", src, dst))?;
    }

    Ok(())
}

pub fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<()> {
    fs::create_dir_all(&dst)
        .with_context(|| format!("Failed to create directory {:?}", dst.as_ref()))?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()))?;
        } else {
            let src_path = entry.path();
            let dst_path = dst.as_ref().join(entry.file_name());
            copy_file_if_different(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

pub fn find_worktrees(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut worktrees = Vec::new();
    find_worktrees_recursive(dir, &mut worktrees)?;
    // Sort in reverse depth order so nested repos are processed first
    worktrees.sort_by_key(|p| p.components().count());
    worktrees.reverse();
    Ok(worktrees)
}

fn find_worktrees_recursive(dir: &Path, worktrees: &mut Vec<PathBuf>) -> Result<()> {
    if dir.is_dir() {
        let git_file = dir.join(".git");
        if git_file.exists() {
            worktrees.push(dir.to_path_buf());
        }

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                let path = entry.path();
                if path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|name| {
                        name == ".git"
                            || name == "prebuilt"
                            || name == ".jiri_root"
                            || name == "out"
                    })
                {
                    continue;
                }
                find_worktrees_recursive(&path, worktrees)?;
            }
        }
    }
    Ok(())
}

pub fn copy_toolchain_metadata(config: &Config, workspace_path: &Path) -> Result<()> {
    let base_ctf_releases = config
        .fuchsia_dir
        .join("sdk/ctf/build/internal/ctf_releases.gni");
    let workspace_ctf_releases = workspace_path.join("sdk/ctf/build/internal/ctf_releases.gni");
    if base_ctf_releases.exists() {
        copy_file_if_different(&base_ctf_releases, &workspace_ctf_releases)?;
    }

    let base_jiri_gen = config.fuchsia_dir.join("build/info/jiri_generated");
    let workspace_jiri_gen = workspace_path.join("build/info/jiri_generated");
    if base_jiri_gen.exists() {
        copy_dir_all(&base_jiri_gen, &workspace_jiri_gen)?;
    }

    let base_cipd_gni = config.fuchsia_dir.join("build/cipd.gni");
    let workspace_cipd_gni = workspace_path.join("build/cipd.gni");
    if base_cipd_gni.exists() {
        copy_file_if_different(&base_cipd_gni, &workspace_cipd_gni)?;
    }

    let base_manifest = config.fuchsia_dir.join(".jiri_manifest");
    let workspace_manifest = workspace_path.join(".jiri_manifest");
    if base_manifest.exists() {
        copy_file_if_different(&base_manifest, &workspace_manifest)?;
    }

    let base_jiri_root = config.fuchsia_dir.join(".jiri_root");
    let ws_jiri_root = workspace_path.join(".jiri_root");

    if ws_jiri_root.is_symlink() {
        fs::remove_file(&ws_jiri_root)?;
    }
    fs::create_dir_all(&ws_jiri_root)?;

    let base_bin = base_jiri_root.join("bin");
    let ws_bin = ws_jiri_root.join("bin");
    if base_bin.exists() {
        if ws_bin.is_symlink() || ws_bin.exists() {
            let _ = fs::remove_file(&ws_bin);
            let _ = fs::remove_dir_all(&ws_bin);
        }
        let home = std::env::var("HOME").context("Failed to get HOME environment variable")?;
        let local_jiri = Path::new(&home).join("src/jiri/jiri");
        if local_jiri.exists() {
            log::info!("Injecting local jiri from {:?}", local_jiri);
            fs::create_dir_all(&ws_bin)?;
            for entry in fs::read_dir(&base_bin)? {
                let entry = entry?;
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                let dest = ws_bin.join(&name);
                if name_str == "jiri" {
                    std::os::unix::fs::symlink(&local_jiri, &dest)?;
                } else {
                    std::os::unix::fs::symlink(entry.path(), &dest)?;
                }
            }
        } else {
            std::os::unix::fs::symlink(&base_bin, &ws_bin)?;
        }
    }

    for file_name in &["config", "prebuilt.json", "prebuilt_versions.json"] {
        if *file_name == "config" && ws_jiri_root.join(file_name).exists() {
            continue;
        }
        let base_file = base_jiri_root.join(file_name);
        if base_file.exists() {
            copy_file_if_different(&base_file, &ws_jiri_root.join(file_name))?;
        }
    }

    let base_latest = base_jiri_root.join("update_history/latest");
    let ws_latest = ws_jiri_root.join("update_history/latest");
    if base_latest.exists() {
        fs::create_dir_all(ws_latest.parent().unwrap())?;
        copy_file_if_different(&base_latest, &ws_latest)?;
    }

    Ok(())
}

use std::fs::{File, FileTimes};
use std::time::SystemTime;

pub fn get_file_mtime(path: &Path) -> Result<SystemTime> {
    let metadata =
        fs::metadata(path).with_context(|| format!("Failed to get metadata for {:?}", path))?;
    metadata
        .modified()
        .with_context(|| format!("Failed to get mtime for {:?}", path))
}

pub fn set_file_mtime(path: &Path, mtime: SystemTime) -> Result<()> {
    let file = File::open(path)
        .with_context(|| format!("Failed to open file for setting times: {:?}", path))?;
    let times = FileTimes::new().set_modified(mtime);
    file.set_times(times)
        .with_context(|| format!("Failed to set times for {:?}", path))?;
    Ok(())
}

pub fn get_config_name(workspace_path: &Path) -> Result<String> {
    let args_gn_path = workspace_path.join("out/default/args.gn");
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
    fn test_get_config_name_success() {
        let dir = tempdir().unwrap();
        let out_dir = dir.path().join("out/default");
        fs::create_dir_all(&out_dir).unwrap();
        let args_gn_path = out_dir.join("args.gn");
        let mut file = File::create(args_gn_path).unwrap();
        writeln!(file, "build_info_product = \"core\"").unwrap();
        writeln!(file, "build_info_board = \"x64\"").unwrap();

        let config = get_config_name(dir.path()).unwrap();
        assert_eq!(config, "core.x64");
    }

    #[test]
    fn test_get_config_name_product_only() {
        let dir = tempdir().unwrap();
        let out_dir = dir.path().join("out/default");
        fs::create_dir_all(&out_dir).unwrap();
        let args_gn_path = out_dir.join("args.gn");
        let mut file = File::create(args_gn_path).unwrap();
        writeln!(file, "build_info_product = \"core\"").unwrap();

        let config = get_config_name(dir.path()).unwrap();
        assert_eq!(config, "core");
    }

    #[test]
    fn test_get_config_name_empty_board() {
        let dir = tempdir().unwrap();
        let out_dir = dir.path().join("out/default");
        fs::create_dir_all(&out_dir).unwrap();
        let args_gn_path = out_dir.join("args.gn");
        let mut file = File::create(args_gn_path).unwrap();
        writeln!(file, "build_info_product = \"core\"").unwrap();
        writeln!(file, "build_info_board = \"\"").unwrap();

        let config = get_config_name(dir.path()).unwrap();
        assert_eq!(config, "core");
    }

    #[test]
    fn test_get_config_name_missing_product() {
        let dir = tempdir().unwrap();
        let out_dir = dir.path().join("out/default");
        fs::create_dir_all(&out_dir).unwrap();
        let args_gn_path = out_dir.join("args.gn");
        let mut file = File::create(args_gn_path).unwrap();
        writeln!(file, "build_info_board = \"x64\"").unwrap();

        let res = get_config_name(dir.path());
        assert!(res.is_err());
    }

    #[test]
    fn test_get_config_name_no_args_gn() {
        let dir = tempdir().unwrap();
        let res = get_config_name(dir.path());
        assert!(res.is_err());
    }

    #[test]
    fn test_get_config_name_with_comments_and_spaces() {
        let dir = tempdir().unwrap();
        let out_dir = dir.path().join("out/default");
        fs::create_dir_all(&out_dir).unwrap();
        let args_gn_path = out_dir.join("args.gn");
        let mut file = File::create(args_gn_path).unwrap();
        writeln!(file, "# Some comment").unwrap();
        writeln!(file, "  build_info_product   =   \"core-nested\"  ").unwrap();
        writeln!(file, "other_var = \"value\"").unwrap();
        writeln!(file, "build_info_board = \"arm64\"").unwrap();

        let config = get_config_name(dir.path()).unwrap();
        assert_eq!(config, "core-nested.arm64");
    }
}
