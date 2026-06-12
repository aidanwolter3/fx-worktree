// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Generic system, file, and path utilities.

use anyhow::{Context, Result, anyhow};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Spawns a shell process to execute the given command.
///
/// Log details of execution, captures outputs, and returns an error containing
/// stdout and stderr outputs if the command fails (exit code is non-zero).
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

/// Copies a file from `src` to `dst` only if the destination doesn't exist
/// or its size or bytes differ, reducing unnecessary writes and preventing mtime bumps.
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

use std::fs::{File, FileTimes};
use std::time::SystemTime;

/// Gets the last modification time of the file at `path`.
pub fn get_file_mtime(path: &Path) -> Result<SystemTime> {
    let metadata =
        fs::metadata(path).with_context(|| format!("Failed to get metadata for {:?}", path))?;
    metadata
        .modified()
        .with_context(|| format!("Failed to get mtime for {:?}", path))
}

/// Sets the last modification time of the file at `path` to `mtime`.
pub fn set_file_mtime(path: &Path, mtime: SystemTime) -> Result<()> {
    let file = File::open(path)
        .with_context(|| format!("Failed to open file for setting times: {:?}", path))?;
    let times = FileTimes::new().set_modified(mtime);
    file.set_times(times)
        .with_context(|| format!("Failed to set times for {:?}", path))?;
    Ok(())
}

/// Computes the relative path from `base` to `path`.
///
/// Returns `None` if they are on different drives (Windows) or no relative path exists.
pub fn diff_paths(path: &Path, base: &Path) -> Option<PathBuf> {
    let mut ita = path.components();
    let mut itb = base.components();
    let mut comps = vec![];
    loop {
        match (ita.next(), itb.next()) {
            (None, None) => break,
            (Some(a), None) => {
                comps.push(a);
                for c in ita {
                    comps.push(c);
                }
                break;
            }
            (None, Some(_)) => {
                comps.push(std::path::Component::ParentDir);
            }
            (Some(a), Some(b)) if a == b => (),
            (Some(a), Some(_)) => {
                comps.push(std::path::Component::ParentDir);
                for _ in itb {
                    comps.push(std::path::Component::ParentDir);
                }
                comps.push(a);
                for c in ita {
                    comps.push(c);
                }
                break;
            }
        }
    }
    if comps.is_empty() {
        Some(PathBuf::from("."))
    } else {
        let mut result = PathBuf::new();
        for c in comps {
            result.push(c);
        }
        Some(result)
    }
}

/// Finds the common directory prefix shared by two paths.
pub fn common_prefix(a: &Path, b: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for (ca, cb) in a.components().zip(b.components()) {
        if ca == cb {
            result.push(ca);
        } else {
            break;
        }
    }
    result
}

/// Returns a shortened/relative representation of `path` with respect to `cwd`,
/// if they share a common directory prefix.
pub fn shorten_path(path: &Path, cwd: &Path) -> PathBuf {
    let prefix = common_prefix(path, cwd);
    let has_shared_prefix = if cfg!(unix) {
        prefix.components().count() > 1
    } else {
        prefix.components().count() > 0
    };

    if has_shared_prefix {
        if let Some(rel) = diff_paths(path, cwd) {
            return rel;
        }
    }
    path.to_path_buf()
}

/// Formats a `SystemTime` relative to the current system time as a short human-readable string.
/// E.g. "3d ago", "2h ago", "10m ago", "5s ago", or "just now".
pub fn format_relative_time(time: SystemTime) -> String {
    let now = SystemTime::now();
    let duration = match now.duration_since(time) {
        Ok(d) => d,
        Err(_) => return "just now".to_string(),
    };

    let secs = duration.as_secs();
    if secs < 60 {
        if secs < 5 {
            "just now".to_string()
        } else {
            format!("{}s ago", secs)
        }
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

/// Helper to determine the sync status of a Git repository at `repo_path` relative
/// to the parent Git repository at `parent_path`.
pub fn get_git_sync_status(repo_path: &Path, parent_path: &Path) -> Result<String> {
    // 1. Get HEAD commit of parent
    let parent_out = run_command("git", &["rev-parse", "HEAD"], parent_path, &[])?;
    let parent_head = String::from_utf8(parent_out.stdout)?.trim().to_string();

    // 2. Get HEAD commit of repo
    let repo_out = run_command("git", &["rev-parse", "HEAD"], repo_path, &[])?;
    let repo_head = String::from_utf8(repo_out.stdout)?.trim().to_string();

    if parent_head == repo_head {
        return Ok("Synced".to_string());
    }

    // 3. Get rev-list counts: <repo_head>...<parent_head>
    let count_out = run_command(
        "git",
        &[
            "rev-list",
            "--left-right",
            "--count",
            &format!("{}...{}", repo_head, parent_head),
        ],
        repo_path,
        &[],
    )?;
    let count_str = String::from_utf8(count_out.stdout)?;
    let parts: Vec<&str> = count_str.trim().split_whitespace().collect();
    if parts.len() != 2 {
        return Err(anyhow!("Unexpected rev-list output: '{}'", count_str));
    }

    let ahead: usize = parts[0].parse()?;
    let behind: usize = parts[1].parse()?;

    match (ahead, behind) {
        (0, 0) => Ok("Synced".to_string()),
        (0, b) => Ok(format!("{} behind", b)),
        (a, 0) => Ok(format!("{} new", a)),
        (a, b) => Ok(format!("{} behind, {} new", b, a)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_diff_paths() {
        assert_eq!(
            diff_paths(Path::new("/a/b/c/d"), Path::new("/a/b")),
            Some(PathBuf::from("c/d"))
        );
        assert_eq!(
            diff_paths(Path::new("/a/b"), Path::new("/a/b/c/d")),
            Some(PathBuf::from("../.."))
        );
        assert_eq!(
            diff_paths(Path::new("/a/b/c/d"), Path::new("/a/b/e/f")),
            Some(PathBuf::from("../../c/d"))
        );
        assert_eq!(
            diff_paths(Path::new("/a/b"), Path::new("/a/b")),
            Some(PathBuf::from("."))
        );
    }

    #[test]
    fn test_shorten_path() {
        let cwd = Path::new("/home/user/fuchsia/out/default");

        // Shares prefix "/home/user/fuchsia"
        let wt = Path::new("/home/user/fuchsia/.jiri_root/worktrees/my-feature");
        assert_eq!(
            shorten_path(wt, cwd),
            PathBuf::from("../../.jiri_root/worktrees/my-feature")
        );

        // Does not share prefix other than root
        let other = Path::new("/tmp/foo");
        assert_eq!(shorten_path(other, cwd), PathBuf::from("/tmp/foo"));
    }

    #[test]
    fn test_format_relative_time() {
        let now = SystemTime::now();

        assert_eq!(format_relative_time(now), "just now");
        assert_eq!(format_relative_time(now - Duration::from_secs(3)), "just now");
        assert_eq!(format_relative_time(now - Duration::from_secs(10)), "10s ago");
        assert_eq!(format_relative_time(now - Duration::from_secs(120)), "2m ago");
        assert_eq!(format_relative_time(now - Duration::from_secs(7200)), "2h ago");
        assert_eq!(format_relative_time(now - Duration::from_secs(86400 * 3)), "3d ago");
    }
}
