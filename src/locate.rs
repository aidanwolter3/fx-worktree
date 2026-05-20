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

    // 2. Scan to find matches (prefix of full ID, or prefix of UUID part)
    let mut matches = Vec::new();
    if config.environments_dir().exists() {
        for entry in fs::read_dir(config.environments_dir())? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
                    if dir_name == id {
                        // Double check exact match just in case
                        return Ok(path);
                    }

                    if dir_name.starts_with(&id) {
                        matches.push((dir_name.to_string(), path.clone()));
                        continue;
                    }

                    if let Some((_cfg, uuid_part)) = dir_name.rsplit_once('_') {
                        if uuid_part.starts_with(&id) {
                            matches.push((dir_name.to_string(), path));
                        }
                    }
                }
            }
        }
    }

    if matches.is_empty() {
        return Err(anyhow!("Worktree ID {} not found", id));
    }

    if matches.len() > 1 {
        let match_ids: Vec<String> = matches.iter().map(|(id, _)| id.clone()).collect();
        return Err(anyhow!(
            "Ambiguous worktree ID '{}'. Matches: {}",
            id,
            match_ids.join(", ")
        ));
    }

    Ok(matches.remove(0).1)
}
