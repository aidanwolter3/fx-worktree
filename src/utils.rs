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

pub fn clean_worktree(worktree_path: &Path, is_root: bool) -> Result<()> {
    // Refresh index to prevent git from touching clean files during reset
    let _ = run_command("git", &["update-index", "--refresh", "-q"], worktree_path, &[]);

    run_command("git", &["reset", "--hard"], worktree_path, &[])?;

    let mut clean_args = vec!["clean", "-fdx"];
    if is_root {
        clean_args.extend_from_slice(&[
            "-e", ".fx-worktree-completed",
            "-e", "prebuilt",
            "-e", ".jiri_root",
            "-e", ".fx-build-dir",
            "-e", "out", // Preserves build cache!
            "-e", "sdk/ctf/build/internal/ctf_releases.gni",
            "-e", "build/info/jiri_generated",
            "-e", "build/cipd.gni",
            "-e", ".jiri_manifest",
        ]);
    }

    run_command("git", &clean_args, worktree_path, &[])?;
    Ok(())
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

    // Quick check: size
    if src_meta.len() != dst_meta.len() {
        fs::copy(src, dst).with_context(|| format!("Failed to copy {:?} to {:?}", src, dst))?;
        return Ok(());
    }

    // Byte comparison
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
    // Copy CTF releases
    let base_ctf_releases = config.fuchsia_dir.join("sdk/ctf/build/internal/ctf_releases.gni");
    let workspace_ctf_releases = workspace_path.join("sdk/ctf/build/internal/ctf_releases.gni");
    if base_ctf_releases.exists() {
        copy_file_if_different(&base_ctf_releases, &workspace_ctf_releases)?;
    }

    // Copy jiri generated commits info
    let base_jiri_gen = config.fuchsia_dir.join("build/info/jiri_generated");
    let workspace_jiri_gen = workspace_path.join("build/info/jiri_generated");
    if base_jiri_gen.exists() {
        copy_dir_all(&base_jiri_gen, &workspace_jiri_gen)?;
    }

    // Copy cipd.gni
    let base_cipd_gni = config.fuchsia_dir.join("build/cipd.gni");
    let workspace_cipd_gni = workspace_path.join("build/cipd.gni");
    if base_cipd_gni.exists() {
        copy_file_if_different(&base_cipd_gni, &workspace_cipd_gni)?;
    }

    // Copy .jiri_manifest
    let base_manifest = config.fuchsia_dir.join(".jiri_manifest");
    let workspace_manifest = workspace_path.join(".jiri_manifest");
    if base_manifest.exists() {
        copy_file_if_different(&base_manifest, &workspace_manifest)?;
    }

    // Setup and copy .jiri_root metadata
    // We cannot symlink the entire .jiri_root directory because it contains the `update_history/latest`
    // snapshot. If we symlink the directory, any `jiri update` in the parent will update the snapshot
    // mtime, dirtying the workspace's build.ninja.stamp (which depends on it).
    //
    // Instead, we create a real .jiri_root directory in the workspace, symlink only the binaries (bin/),
    // and copy the config and latest snapshot as static files.
    let base_jiri_root = config.fuchsia_dir.join(".jiri_root");
    let ws_jiri_root = workspace_path.join(".jiri_root");
    
    if ws_jiri_root.is_symlink() {
        fs::remove_file(&ws_jiri_root)?;
    }
    fs::create_dir_all(&ws_jiri_root)?;

    // Symlink bin/ (contains jiri and cipd executables)
    let base_bin = base_jiri_root.join("bin");
    let ws_bin = ws_jiri_root.join("bin");
    if base_bin.exists() && !ws_bin.exists() {
        std::os::unix::fs::symlink(&base_bin, &ws_bin)?;
    }

    // Copy Jiri configuration files
    for file_name in &["config", "prebuilt.json", "prebuilt_versions.json"] {
        let base_file = base_jiri_root.join(file_name);
        if base_file.exists() {
            copy_file_if_different(&base_file, &ws_jiri_root.join(file_name))?;
        }
    }

    // Copy the latest update history snapshot
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
    let metadata = fs::metadata(path)
        .with_context(|| format!("Failed to get metadata for {:?}", path))?;
    metadata.modified().with_context(|| format!("Failed to get mtime for {:?}", path))
}

pub fn set_file_mtime(path: &Path, mtime: SystemTime) -> Result<()> {
    let file = File::open(path)
        .with_context(|| format!("Failed to open file for setting times: {:?}", path))?;
    let times = FileTimes::new().set_modified(mtime);
    file.set_times(times)
        .with_context(|| format!("Failed to set times for {:?}", path))?;
    Ok(())
}

pub fn convert_gitdir_to_symlink(target_path: &Path) -> Result<()> {
    // Git Worktrees and Jiri Metadata:
    // When we run `git worktree add`, Git creates a `.git` file in the worktree pointing
    // to a dedicated Git directory under the parent repository's `.git/worktrees/<name>`.
    //
    // However, Jiri stores its project metadata (like remote URL and branch) inside `.git/jiri/`.
    // Since `git worktree add` creates a fresh Git directory, it lacks this Jiri metadata.
    // Consequently, `jiri` commands run in the workspace will fail to recognize the projects
    // as local and will attempt to download/clone them from the network, causing severe delays.
    //
    // To resolve this, this function:
    // 1. Ensures the `.git` file is converted to a symlink pointing to the worktree's Git directory.
    // 2. Automatically symlinks the parent project's Jiri metadata directory (`.git/jiri`)
    //    into the worktree's Git directory (`.git/worktrees/<name>/jiri`), making `jiri` happy
    //    and offline-friendly.
    let git_file_path = target_path.join(".git");
    let mut gitdir_path = None;

    if git_file_path.exists() {
        if git_file_path.is_file() {
            let contents = fs::read_to_string(&git_file_path)?;
            if let Some(gitdir_line) = contents.lines().next() {
                if let Some(gitdir_path_str) = gitdir_line.strip_prefix("gitdir: ") {
                    let path = PathBuf::from(gitdir_path_str.trim());
                    fs::remove_file(&git_file_path)?;
                    std::os::unix::fs::symlink(&path, &git_file_path)?;
                    gitdir_path = Some(path);
                }
            }
        } else {
            // It's already a symlink or directory
            if let Ok(target) = fs::read_link(&git_file_path) {
                gitdir_path = Some(target);
            } else if git_file_path.is_dir() {
                gitdir_path = Some(git_file_path.clone());
            }
        }
    }

    if let Some(gitdir) = gitdir_path {
        let abs_gitdir = if gitdir.is_absolute() {
            gitdir
        } else {
            target_path.join(gitdir)
        };

        if let Some(parent_git_dir) = abs_gitdir.parent().and_then(|p| p.parent()) {
            let parent_jiri = parent_git_dir.join("jiri");
            if parent_jiri.exists() {
                let ws_jiri = abs_gitdir.join("jiri");
                if !ws_jiri.exists() {
                    std::os::unix::fs::symlink(&parent_jiri, &ws_jiri)
                        .with_context(|| format!("Failed to symlink Jiri metadata from {:?} to {:?}", parent_jiri, ws_jiri))?;
                }
            }
        }
    }

    Ok(())
}

pub fn clamp_mtimes_to_past(dir: &Path) -> Result<()> {
    // WHY WE CLAMP MTIMES TO 2020-01-01:
    // Some prebuilt packages (like Bazel) contain files with artificial future timestamps (e.g.,
    // 2042-07-28 00:00:00 UTC) for integrity checking or determinism.
    //
    // Since Ninja tracks these as dynamic inputs (recorded in .ninja_deps) during the build, it compares
    // their modify times with the build outputs (compiled in the present, e.g. 2026). Because the
    // inputs (2042) are always newer than the outputs (2026), Ninja constantly triggers rebuilds on
    // subsequent invocations, breaking no-op builds.
    //
    // By recursively clamping all files in newly downloaded/copied prebuilt directories to a fixed
    // past date (2020-01-01), we ensure that the inputs are always older than the build outputs,
    // preserving Ninja's no-op build cache logic.
    let past_time = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1577836800);
    clamp_mtimes_recursive(dir, past_time)
}

fn clamp_mtimes_recursive(dir: &Path, time: SystemTime) -> Result<()> {
    if dir.is_dir() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                clamp_mtimes_recursive(&path, time)?;
            } else if path.is_file() {
                set_file_mtime(&path, time)?;
            }
        }
    }
    Ok(())
}
