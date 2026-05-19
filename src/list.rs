use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::time::SystemTime;

use crate::config::Config;
use crate::worktree::WorktreeInfo;

#[derive(serde::Serialize)]
struct OutdirListEntry {
    config: String,
    outdir_id: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_id: Option<String>,
}

pub fn list_worktrees(config: &Config, json: bool) -> Result<()> {
    let leases_dir = config.leases_dir();
    let mut active_worktrees = Vec::new();

    if leases_dir.exists() {
        let entries = fs::read_dir(&leases_dir)
            .with_context(|| format!("Failed to read leases directory {:?}", leases_dir))?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("lease") {
                let worktree_json = fs::read_to_string(&path)
                    .with_context(|| format!("Failed to read worktree file {:?}", path))?;
                if let Ok(worktree_info) = serde_json::from_str::<WorktreeInfo>(&worktree_json) {
                    active_worktrees.push(worktree_info);
                }
            }
        }
    }

    if json {
        let json_str = serde_json::to_string_pretty(&active_worktrees)
            .context("Failed to serialize worktree list to JSON")?;
        println!("{}", json_str);
        return Ok(());
    }

    if active_worktrees.is_empty() {
        println!("No active worktrees.");
        return Ok(());
    }

    let current_time = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    for worktree in active_worktrees {
        let age_secs = current_time.saturating_sub(worktree.timestamp_sec);
        let age_str = format_duration(age_secs);

        println!("■ {}", worktree.worktree_id);
        println!("  Agent     : {}", worktree.agent_id);
        println!("  Created   : {} ago", age_str);
        println!("  Workspace : {}", worktree.workspace_path.to_string_lossy());
        println!("  Outdir    : {}", worktree.outdir_path.to_string_lossy());
        println!();
    }

    Ok(())
}

pub fn list_outdirs(config: &Config, json: bool) -> Result<()> {
    let leases_dir = config.leases_dir();
    let outdirs_dir = config.outdirs_dir();

    // 1. Collect all leased outdirs and their leasing agents
    let mut active_outdirs = BTreeMap::new(); // outdir_path -> agent_id
    if leases_dir.exists() {
        let entries = fs::read_dir(&leases_dir)?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("lease") {
                if let Some(worktree_info) = fs::read_to_string(&path)
                    .ok()
                    .and_then(|content| serde_json::from_str::<WorktreeInfo>(&content).ok())
                {
                    active_outdirs.insert(worktree_info.outdir_path, worktree_info.agent_id);
                }
            }
        }
    }

    // 2. Scan pool to collect all outdirs
    let mut outdir_entries = Vec::new();
    if outdirs_dir.exists() {
        let config_entries = fs::read_dir(&outdirs_dir)
            .with_context(|| format!("Failed to read outdirs directory {:?}", outdirs_dir))?;

        for config_entry in config_entries {
            let config_entry = config_entry?;
            let config_path = config_entry.path();
            if config_path.is_dir() {
                let config_name = config_path.file_name().unwrap().to_str().unwrap().to_string();
                let sub_entries = fs::read_dir(&config_path).with_context(|| {
                    format!("Failed to read config directory {:?}", config_path)
                })?;

                for outdir_entry in sub_entries {
                    let outdir_entry = outdir_entry?;
                    let outdir_path = outdir_entry.path();
                    if outdir_path.is_dir() {
                        if let Some(dir_name) = outdir_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .filter(|s| s.starts_with("out_"))
                        {
                            let agent_id = active_outdirs.get(&outdir_path).cloned();
                            let status = if agent_id.is_some() {
                                "In Use".to_string()
                            } else {
                                "Free".to_string()
                            };

                            outdir_entries.push(OutdirListEntry {
                                config: config_name.clone(),
                                outdir_id: dir_name.to_string(),
                                status,
                                agent_id,
                            });
                        }
                    }
                }
            }
        }
    }

    if json {
        let json_str = serde_json::to_string_pretty(&outdir_entries)
            .context("Failed to serialize outdir list to JSON")?;
        println!("{}", json_str);
        return Ok(());
    }

    if outdir_entries.is_empty() {
        println!("No outdirs found in pool.");
        return Ok(());
    }

    // 3. Print as a pretty table
    // Calculate column widths
    let mut max_config = 6; // "CONFIG".len()
    let mut max_id = 9;     // "OUTDIR ID".len()
    for entry in &outdir_entries {
        max_config = max_config.max(entry.config.len());
        max_id = max_id.max(entry.outdir_id.len());
    }

    println!(
        "{:<cfg_width$}   {:<id_width$}   STATUS",
        "CONFIG",
        "OUTDIR ID",
        cfg_width = max_config,
        id_width = max_id
    );

    for entry in &outdir_entries {
        let status_str = match &entry.agent_id {
            Some(agent) => format!("In Use ({})", agent),
            None => "Free".to_string(),
        };
        println!(
            "{:<cfg_width$}   {:<id_width$}   {}",
            entry.config,
            entry.outdir_id,
            status_str,
            cfg_width = max_config,
            id_width = max_id
        );
    }

    Ok(())
}

fn format_duration(secs: u64) -> String {
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;
    if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, seconds)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    }
}
