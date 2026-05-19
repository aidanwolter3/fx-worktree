use anyhow::{Context, Result};
use std::collections::HashSet;
use std::fs;
use std::time::SystemTime;

use crate::config::Config;
use crate::worktree::WorktreeInfo;

pub fn list_worktrees(config: &Config) -> Result<()> {
    let leases_dir = config.leases_dir();

    println!("Active Worktrees:");
    if leases_dir.exists() {
        let entries = fs::read_dir(&leases_dir)
            .with_context(|| format!("Failed to read leases directory {:?}", leases_dir))?;

        let current_time = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let mut found_active = false;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("lease") {
                let worktree_json = fs::read_to_string(&path)
                    .with_context(|| format!("Failed to read worktree file {:?}", path))?;
                if let Ok(worktree_info) = serde_json::from_str::<WorktreeInfo>(&worktree_json) {
                    found_active = true;
                    let age_secs = current_time.saturating_sub(worktree_info.timestamp_sec);
                    let age_str = format_duration(age_secs);

                    println!("  Worktree ID: {}", worktree_info.worktree_id);
                    println!("    Agent: {}", worktree_info.agent_id);
                    println!("    Age: {}", age_str);
                    println!("    Workspace: {:?}", worktree_info.workspace_path);
                    println!("    Outdir: {:?}", worktree_info.outdir_path);
                    println!();
                }
            }
        }
        if !found_active {
            println!("  No active worktrees.");
        }
    } else {
        println!("  No active worktrees (leases directory missing).");
    }

    Ok(())
}

pub fn list_outdirs(config: &Config) -> Result<()> {
    let leases_dir = config.leases_dir();
    let outdirs_dir = config.outdirs_dir();

    // First, collect all leased outdirs
    let mut active_outdir_paths = HashSet::new();
    if leases_dir.exists() {
        let entries = fs::read_dir(&leases_dir)?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("lease") {
                if let Some(worktree_info) = fs::read_to_string(&path).ok().and_then(|content| {
                    serde_json::from_str::<WorktreeInfo>(&content).ok()
                }) {
                    active_outdir_paths.insert(worktree_info.outdir_path);
                }
            }
        }
    }

    println!("Outdirs:");
    if outdirs_dir.exists() {
        let config_entries = fs::read_dir(&outdirs_dir)
            .with_context(|| format!("Failed to read outdirs directory {:?}", outdirs_dir))?;

        let mut found_any = false;
        for config_entry in config_entries {
            let config_entry = config_entry?;
            let config_path = config_entry.path();
            if config_path.is_dir() {
                let config_name = config_path.file_name().unwrap().to_str().unwrap();
                let outdir_entries = fs::read_dir(&config_path).with_context(|| {
                    format!("Failed to read config directory {:?}", config_path)
                })?;

                let mut config_printed = false;
                for outdir_entry in outdir_entries {
                    let outdir_entry = outdir_entry?;
                    let outdir_path = outdir_entry.path();
                    if outdir_path.is_dir() {
                        if let Some(dir_name) = outdir_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .filter(|s| s.starts_with("out_"))
                        {
                            if !config_printed {
                                println!("  Config: {}", config_name);
                                config_printed = true;
                            }
                            let status = if active_outdir_paths.contains(&outdir_path) {
                                "In Use"
                            } else {
                                "Free"
                            };
                            println!("    - {} ({})", dir_name, status);
                            found_any = true;
                        }
                    }
                }
            }
        }
        if !found_any {
            println!("  No outdirs found.");
        }
    } else {
        println!("  No outdirs found (outdirs directory missing).");
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
