use crate::config::Config;
use crate::utils::{copy_toolchain_metadata, run_command};
use anyhow::{Context, Result};
use std::path::Path;

pub fn sync_environment_by_id(config: &Config, id: &str, quiet: bool) -> Result<()> {
    let path = crate::locate::locate_path(config, Some(id.to_string()))?;
    sync_environment(config, id, &path, quiet)
}

pub fn sync_environment(
    config: &Config,
    env_id: &str,
    workspace_path: &Path,
    quiet: bool,
) -> Result<()> {
    let workspace_path_buf = workspace_path
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize workspace path {:?}", workspace_path))?;
    let workspace_path = &workspace_path_buf;

    if !quiet {
        eprintln!("Syncing environment {}...", env_id);
    }

    // Copy/restore toolchain metadata (must be before sync for hooks)
    copy_toolchain_metadata(config, workspace_path)?;

    // Call 'jiri worktree sync'
    let jiri_bin = config.fuchsia_dir.join(".jiri_root/bin/jiri");
    let jiri_cmd = if jiri_bin.exists() {
        jiri_bin.to_str().unwrap()
    } else {
        "jiri"
    };

    run_command(jiri_cmd, &["worktree", "sync"], workspace_path, &[])
        .context("Failed to run jiri worktree sync")?;

    Ok(())
}
