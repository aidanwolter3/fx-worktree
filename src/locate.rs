use crate::config::Config;
use anyhow::{Result, anyhow};
use std::fs;
use std::path::PathBuf;

pub fn locate_path(config: &Config, id: Option<String>) -> Result<PathBuf> {
    let id = match id {
        Some(ref val) if !val.trim().is_empty() => val.clone(),
        _ => return config.read_last_active(),
    };

    if std::path::Path::new(&id).components().count() > 1 {
        return Err(anyhow!("Invalid worktree ID: {}", id));
    }

    // 1. Check if the exact directory exists
    let env_path = config.environments_dir().join(&id);
    if env_path.exists() {
        return Ok(env_path);
    }

    // 2. Scan to see if it matches a suffix (like out_uuid or uuid)
    let uuid = id.strip_prefix("out_").unwrap_or(&id);
    let suffix = format!("_{}", uuid);
    if config.environments_dir().exists() {
        for entry in fs::read_dir(config.environments_dir())? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
                    if dir_name.ends_with(&suffix) || dir_name.ends_with(&id) {
                        return Ok(path);
                    }
                }
            }
        }
    }

    Err(anyhow!("Worktree ID {} not found", id))
}
