// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Scanning and pretty printing of Jiri worktrees.

use anyhow::{Context, Result};
use std::fs;

use crate::colors::Colors;
use crate::config::Config;
use crate::worktree::WorktreeInfo;

#[derive(serde::Serialize)]
struct WorktreeListEntry {
    name: String,
    path: String,
    status: String,
    outdirs: Vec<String>,
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
            for op in outdir_paths {
                let rel_path = op
                    .strip_prefix(&path)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| op.to_string_lossy().into_owned());

                let config_info = if let Ok(cfg) = crate::fuchsia::get_config_name_for_outdir(&op) {
                    format!("{} ({})", rel_path, cfg)
                } else {
                    rel_path
                };
                outdirs_info.push(config_info);
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

            wt_entries.push(WorktreeListEntry {
                name: dir_name.to_string(),
                path: path.to_string_lossy().into_owned(),
                status,
                outdirs: outdirs_info,
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

    // Print pretty layout
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::new());
    let mut formatted_paths = Vec::new();
    let mut max_total_len = 0;
    for entry in &wt_entries {
        let path = std::path::Path::new(&entry.path);
        let parent = path.parent().unwrap_or(std::path::Path::new(""));
        let shortened_parent = crate::utils::shorten_path(parent, &cwd);
        let sp_str = shortened_parent.to_string_lossy().into_owned();
        let prefix = if sp_str.is_empty() || sp_str == "." {
            "".to_string()
        } else {
            format!("{}/", sp_str)
        };
        max_total_len = max_total_len.max(prefix.len() + entry.name.len());
        formatted_paths.push((prefix, entry.name.clone()));
    }
    let align_width = max_total_len + 4;

    for (idx, entry) in wt_entries.iter().enumerate() {
        let status_str = if entry.status.starts_with("In Use") {
            colors.yellow(&entry.status)
        } else if entry.status == "Reserved" {
            colors.magenta("Reserved")
        } else {
            colors.green("Free")
        };

        let (prefix, name) = &formatted_paths[idx];
        let total_len = prefix.len() + name.len();
        let spaces_to_add = align_width.saturating_sub(total_len);
        let spaces = " ".repeat(spaces_to_add);

        let colored_name = colors.bold(&colors.blue(name));
        let colored_prefix = colors.blue(prefix);

        println!("{}{}{}{}", colored_prefix, colored_name, spaces, status_str);
        let num_outdirs = entry.outdirs.len();
        for (i, outdir) in entry.outdirs.iter().enumerate() {
            let marker = if i == num_outdirs - 1 {
                "└── "
            } else {
                "├── "
            };
            println!("{}{}", marker, outdir);
        }
    }

    Ok(())
}
