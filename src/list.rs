// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Scanning and pretty printing of Jiri worktrees.

use anyhow::{Context, Result};
use std::fs;

use crate::colors::Colors;
use crate::config::Config;
use crate::worktree::WorktreeInfo;

struct OutdirInfo {
    path: String,
    config: Option<String>,
    last_built: Option<std::time::SystemTime>,
}

#[derive(serde::Serialize)]
struct WorktreeListEntry {
    name: String,
    path: String,
    status: String,
    sync_status: String,
    outdirs: Vec<String>,
    #[serde(skip)]
    outdirs_raw: Vec<OutdirInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_id: Option<String>,
}

/// Scans the Jiri worktrees registry and prints a tabular tree representation of all
/// worktrees, their lease statuses, and their configured GN outdirs.
///
/// If `json` is true, prints structured JSON representing the worktrees instead.
pub fn list_worktrees(config: &Config, json: bool) -> Result<()> {
    let colors = Colors::new();

    // Scan Jiri worktrees
    let mut wt_entries = Vec::new();
    let worktree_paths = crate::worktree::read_jiri_worktrees(config)?;
    for path in worktree_paths {
        if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
            // Find all outdirs in this worktree
            let outdir_paths = crate::fuchsia::find_outdirs(&path).unwrap_or_else(|e| {
                log::warn!("Failed to find outdirs for {:?}: {:?}", path, e);
                Vec::new()
            });

            let mut outdirs_info = Vec::new();
            let mut outdirs_raw = Vec::new();
            for op in outdir_paths {
                let rel_path = op
                    .strip_prefix(&path)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| op.to_string_lossy().into_owned());

                let config = crate::fuchsia::get_config_name_for_outdir(&op).ok();
                let config_info = if let Some(ref cfg) = config {
                    format!("{} ({})", rel_path, cfg)
                } else {
                    rel_path.clone()
                };
                outdirs_info.push(config_info);

                let ninja_log_path = op.join(".ninja_log");
                let last_built = crate::utils::get_file_mtime(&ninja_log_path).ok();

                outdirs_raw.push(OutdirInfo {
                    path: rel_path,
                    config,
                    last_built,
                });
            }

            // Check lease
            let lease_file_path = path.join(".jiri_root").join("lease.json");
            let mut agent_id = None;
            let mut is_leased = false;
            if lease_file_path.is_file() {
                if let Ok(content) = fs::read_to_string(&lease_file_path) {
                    if let Ok(wt_info) = serde_json::from_str::<WorktreeInfo>(&content) {
                        agent_id = wt_info.agent_id;
                        is_leased = true;
                    }
                }
            }

            let state = crate::worktree::get_worktree_state(config, &path);
            let status = if is_leased {
                if let Some(agent) = &agent_id {
                    format!("In Use ({})", agent)
                } else {
                    "In Use".to_string()
                }
            } else {
                match state {
                    crate::worktree::WorktreeState::Free => "Free".to_string(),
                    crate::worktree::WorktreeState::Reserved => "Reserved".to_string(),
                }
            };

            let sync_status = crate::utils::get_git_sync_status(&path, &config.fuchsia_dir)
                .unwrap_or_else(|e| {
                    log::warn!("Failed to check sync status for {:?}: {:?}", path, e);
                    "Unknown".to_string()
                });

            wt_entries.push(WorktreeListEntry {
                name: dir_name.to_string(),
                path: path.to_string_lossy().into_owned(),
                status,
                sync_status,
                outdirs: outdirs_info,
                outdirs_raw,
                agent_id,
            });
        }
    }

    if json {
        let json_str = serde_json::to_string_pretty(&wt_entries)
            .context("Failed to serialize worktrees list to JSON")?;
        println!("{}", json_str);
        return Ok(());
    }

    if wt_entries.is_empty() {
        println!("No worktrees found.");
        return Ok(());
    }

    // Find global max path length for alignment
    let mut max_outdir_path_len = 0;
    for entry in &wt_entries {
        for outdir in &entry.outdirs_raw {
            max_outdir_path_len = max_outdir_path_len.max(outdir.path.len());
        }
    }

    // Print pretty layout
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::new());
    let mut formatted_paths = Vec::new();
    for entry in &wt_entries {
        let path = std::path::Path::new(&entry.path);
        let parent = path.parent().unwrap_or(std::path::Path::new(""));
        let is_default_worktrees_dir = if let (Ok(p_canon), Ok(wt_canon)) =
            (parent.canonicalize(), config.worktrees_dir().canonicalize())
        {
            p_canon == wt_canon
        } else {
            false
        };

        let prefix = if is_default_worktrees_dir {
            "".to_string()
        } else {
            let shortened_parent = crate::utils::shorten_path(parent, &cwd);
            let sp_str = shortened_parent.to_string_lossy().into_owned();
            if sp_str.is_empty() || sp_str == "." {
                "".to_string()
            } else {
                format!("{}/", sp_str)
            }
        };
        formatted_paths.push((prefix, entry.name.clone()));
    }

    for (idx, entry) in wt_entries.iter().enumerate() {
        let status_str = if entry.status.starts_with("In Use") {
            colors.yellow(&entry.status)
        } else if entry.status == "Reserved" {
            colors.magenta("Reserved")
        } else {
            colors.green("Free")
        };

        let sync_str = if entry.sync_status == "Synced" {
            colors.bold("Synced")
        } else if entry.sync_status.contains("behind") && entry.sync_status.contains("new") {
            colors.magenta(&entry.sync_status)
        } else if entry.sync_status.contains("behind") {
            colors.yellow(&entry.sync_status)
        } else if entry.sync_status.contains("new") {
            colors.blue(&entry.sync_status)
        } else {
            colors.bold(&entry.sync_status)
        };
        let meta_str = format!("({}, {})", status_str, sync_str);

        let (prefix, name) = &formatted_paths[idx];
        let colored_name = colors.bold(&colors.blue(name));
        let colored_prefix = colors.blue(prefix);

        println!("{}{} {}", colored_prefix, colored_name, meta_str);
        let num_outdirs = entry.outdirs_raw.len();
        for (i, outdir) in entry.outdirs_raw.iter().enumerate() {
            let marker = if i == num_outdirs - 1 {
                "└── "
            } else {
                "├── "
            };
            let built_str = if let Some(t) = outdir.last_built {
                format!(" ({})", crate::utils::format_relative_time(t))
            } else {
                " (never built)".to_string()
            };

            if let Some(cfg) = &outdir.config {
                let target_col = max_outdir_path_len + 4;
                let current_len = outdir.path.len() + 1; // path + ":"
                let pad_len = target_col.saturating_sub(current_len);
                let padding = " ".repeat(pad_len);
                let formatted_cfg = format_config_name(&colors, cfg);
                println!("{}{}:{}{}{}", marker, outdir.path, padding, formatted_cfg, built_str);
            } else {
                println!("{}{}{}", marker, outdir.path, built_str);
            }
        }
        if idx < wt_entries.len() - 1 {
            println!();
        }
    }

    Ok(())
}

fn format_config_name(colors: &Colors, config: &str) -> String {
    let parts: Vec<String> = config
        .split('.')
        .map(|part| colors.yellow(part))
        .collect();
    parts.join(".")
}
