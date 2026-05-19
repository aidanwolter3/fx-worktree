use crate::config::Config;
use crate::worktree::WorktreeInfo;
use anyhow::{Context, Result, anyhow};
use std::fs;
use std::path::PathBuf;

pub fn locate_path(config: &Config, id: Option<String>) -> Result<PathBuf> {
    // If id is None or empty, read and return the last created path
    let id = match id {
        Some(ref val) if !val.trim().is_empty() => val.clone(),
        _ => return config.read_last_created(),
    };

    let uuid = id.strip_prefix("out_").unwrap_or(&id);

    // 1. Check if it is a worktree ID (format: <config>_out_<uuid>)
    let lease_file_name = id.replace("_out_", "_") + ".lease";
    let lease_file_path = config.leases_dir().join(&lease_file_name);
    if lease_file_path.exists() {
        let worktree_json = fs::read_to_string(&lease_file_path)
            .with_context(|| format!("Failed to read lease file {:?}", lease_file_path))?;
        let worktree_info: WorktreeInfo = serde_json::from_str(&worktree_json)
            .context("Failed to parse worktree JSON")?;
        return Ok(worktree_info.workspace_path);
    }

    // 2. Scan leases just in case they passed raw UUID or short ID for worktree
    if config.leases_dir().exists() {
        for entry in fs::read_dir(config.leases_dir())? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("lease") {
                if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
                    let suffix = format!("_{}.lease", uuid);
                    if file_name.ends_with(&suffix) {
                        let worktree_json = fs::read_to_string(&path)
                            .with_context(|| format!("Failed to read lease file {:?}", path))?;
                        let worktree_info: WorktreeInfo = serde_json::from_str(&worktree_json)
                            .context("Failed to parse worktree JSON")?;
                        return Ok(worktree_info.workspace_path);
                    }
                }
            }
        }
    }

    // 3. Check if it is an outdir ID in the pool
    let outdirs_dir = config.outdirs_dir();
    let mut resolved_config_name = None;
    if outdirs_dir.exists() {
        for config_entry in fs::read_dir(&outdirs_dir)? {
            let config_entry = config_entry?;
            let config_path = config_entry.path();
            if config_path.is_dir() {
                let target_path = config_path.join(&id);
                let target_path_with_prefix = config_path.join(format!("out_{}", uuid));
                if target_path.exists() || target_path_with_prefix.exists() {
                    resolved_config_name = Some(config_path.file_name().unwrap().to_str().unwrap().to_string());
                    break;
                }
            }
        }
    }

    if let Some(config_name) = resolved_config_name {
        let outdir_path = config.outdirs_dir().join(&config_name).join(format!("out_{}", uuid));
        if outdir_path.exists() {
            return Ok(outdir_path);
        }
        let outdir_path_no_prefix = config.outdirs_dir().join(&config_name).join(uuid);
        if outdir_path_no_prefix.exists() {
            return Ok(outdir_path_no_prefix);
        }
    }

    Err(anyhow!("Outdir or Worktree ID {} not found", id))
}
