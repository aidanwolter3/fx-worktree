use anyhow::{Context, Result, anyhow};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use crate::config::Config;

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
            fs::copy(&src_path, &dst_path)
                .with_context(|| format!("Failed to copy {:?} to {:?}", src_path, dst_path))?;
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
    // Copy CTF releases
    let base_ctf_releases = config.fuchsia_dir.join("sdk/ctf/build/internal/ctf_releases.gni");
    let workspace_ctf_releases = workspace_path.join("sdk/ctf/build/internal/ctf_releases.gni");
    if base_ctf_releases.exists() {
        if let Some(parent) = workspace_ctf_releases.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory {:?}", parent))?;
        }
        fs::copy(&base_ctf_releases, &workspace_ctf_releases)
            .with_context(|| format!("Failed to copy {:?} to {:?}", base_ctf_releases, workspace_ctf_releases))?;
    }

    // Copy jiri generated commits info
    let base_jiri_gen = config.fuchsia_dir.join("build/info/jiri_generated");
    let workspace_jiri_gen = workspace_path.join("build/info/jiri_generated");
    if base_jiri_gen.exists() {
        if let Some(parent) = workspace_jiri_gen.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory {:?}", parent))?;
        }
        copy_dir_all(&base_jiri_gen, &workspace_jiri_gen)
            .with_context(|| format!("Failed to copy build/info/jiri_generated to {:?}", workspace_jiri_gen))?;
    }

    // Copy cipd.gni
    let base_cipd_gni = config.fuchsia_dir.join("build/cipd.gni");
    let workspace_cipd_gni = workspace_path.join("build/cipd.gni");
    if base_cipd_gni.exists() {
        if let Some(parent) = workspace_cipd_gni.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory {:?}", parent))?;
        }
        fs::copy(&base_cipd_gni, &workspace_cipd_gni)
            .with_context(|| format!("Failed to copy build/cipd.gni to {:?}", workspace_cipd_gni))?;
    }

    Ok(())
}
