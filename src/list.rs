use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fs;

use crate::config::Config;
use crate::environment::EnvironmentInfo;

#[derive(serde::Serialize)]
struct WorktreeListEntry {
    config: String,
    worktree_id: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_id: Option<String>,
}

pub fn list_environments(config: &Config, json: bool) -> Result<()> {
    let leases_dir = config.leases_dir();
    let envs_dir = config.environments_dir();

    // 1. Collect all active leases
    let mut active_leases = BTreeMap::new(); // env_id -> agent_id
    if leases_dir.exists() {
        let entries = fs::read_dir(&leases_dir)?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("lease") {
                if let Some(env_info) = fs::read_to_string(&path)
                    .ok()
                    .and_then(|content| serde_json::from_str::<EnvironmentInfo>(&content).ok())
                {
                    active_leases.insert(env_info.environment_id.clone(), env_info.agent_id);
                }
            }
        }
    }

    // 2. Scan environments pool
    let mut env_entries = Vec::new();
    if envs_dir.exists() {
        let entries = fs::read_dir(&envs_dir)
            .with_context(|| format!("Failed to read environments directory {:?}", envs_dir))?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
                    let completed_file = path.join(".fx-worktree-completed");
                    if !completed_file.exists() {
                        continue;
                    }
                    let config_name = fs::read_to_string(&completed_file)
                        .map(|s| s.trim().to_string())
                        .unwrap_or_else(|_| "unknown".to_string());

                    let agent_id = active_leases.get(dir_name).cloned();
                    let status = if agent_id.is_some() {
                        "In Use".to_string()
                    } else {
                        "Free".to_string()
                    };

                    env_entries.push(WorktreeListEntry {
                        config: config_name,
                        worktree_id: dir_name.to_string(),
                        status,
                        agent_id,
                    });
                }
            }
        }
    }

    if json {
        let json_str = serde_json::to_string_pretty(&env_entries)
            .context("Failed to serialize environments list to JSON")?;
        println!("{}", json_str);
        return Ok(());
    }

    if env_entries.is_empty() {
        println!("No worktrees found in pool.");
        return Ok(());
    }

    // 3. Print pretty table
    let mut max_config = 6; // "CONFIG".len()
    let mut max_id = 11; // "WORKTREE ID".len()
    for entry in &env_entries {
        max_config = max_config.max(entry.config.len());
        max_id = max_id.max(entry.worktree_id.len());
    }

    println!(
        "{:<cfg_width$}   {:<id_width$}   STATUS",
        "CONFIG",
        "WORKTREE ID",
        cfg_width = max_config,
        id_width = max_id
    );

    for entry in &env_entries {
        let status_str = match &entry.agent_id {
            Some(agent) => format!("In Use ({})", agent),
            None => "Free".to_string(),
        };
        println!(
            "{:<cfg_width$}   {:<id_width$}   {}",
            entry.config,
            entry.worktree_id,
            status_str,
            cfg_width = max_config,
            id_width = max_id
        );
    }

    Ok(())
}
