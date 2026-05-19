use anyhow::{Context, Result, anyhow};
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use uuid::Uuid;

use crate::config::Config;
use crate::free::free_worktree_internal;
use crate::utils::{copy_dir_all, run_command};
use crate::worktree::WorktreeInfo;

#[derive(serde::Deserialize, Debug, Clone)]
struct JiriProject {
    name: String,
    path: String,     // Absolute path in the base tree
    revision: String, // Git SHA
}

pub fn allocate(
    config: &Config,
    config_name: &str,
    agent_id: &str,
    preferred_outdir_id: Option<&str>,
    forced_outdir_path: Option<PathBuf>,
) -> Result<WorktreeInfo> {
    let mut acquired_lease = None;
    let mut outdir_path = PathBuf::new();
    let mut out_id = String::new();

    if let Some(path) = forced_outdir_path {
        if !path.exists() {
            return Err(anyhow!("Forced outdir path {:?} does not exist", path));
        }
        let uuid = Uuid::new_v4().to_string();
        let lease_file_name = format!("{}_{}.lease", config_name, uuid);
        let lease_file_path = config.leases_dir().join(&lease_file_name);

        // Attempt to create lease file atomically
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lease_file_path)
            .with_context(|| format!("Failed to create lease file {:?}", lease_file_path))?;

        log::info!("Acquired lease lock for forced outdir: {:?}", lease_file_path);
        acquired_lease = Some(lease_file_path);
        outdir_path = path;
        out_id = uuid;
    } else {
        // 1. Acquire Atomic Lock from Pool
        let outdir_config_dir = config.outdirs_dir().join(config_name);
        if !outdir_config_dir.exists() {
            return Err(anyhow!(
                "Outdir config {:?} does not exist. Add it first using 'outdir create'.",
                config_name
            ));
        }

        let entries = fs::read_dir(&outdir_config_dir)
            .with_context(|| format!("Failed to read outdirs directory {:?}", outdir_config_dir))?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
                    if let Some(id) = dir_name.strip_prefix("out_") {
                        if let Some(pref_id) = preferred_outdir_id {
                            if id != pref_id {
                                continue;
                            }
                        }
                        let args_gn_ref = path.join("args.gn.ref");
                        if !args_gn_ref.exists() {
                            log::warn!("Skipping invalid/corrupted outdir {:?}", path);
                            continue;
                        }
                        let lease_file_name = format!("{}_{}.lease", config_name, id);
                        let lease_file_path = config.leases_dir().join(&lease_file_name);
                        log::info!(
                            "DEBUG: leases_dir={:?}, exists={}",
                            config.leases_dir(),
                            config.leases_dir().exists()
                        );

                        // Attempt to create lease file atomically
                        match fs::OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(&lease_file_path)
                        {
                            Ok(_) => {
                                log::info!("Acquired lease lock: {:?}", lease_file_path);
                                acquired_lease = Some(lease_file_path);
                                outdir_path = path.clone();
                                out_id = id.to_string();
                                break;
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                                // Lease busy, try next
                                continue;
                            }
                            Err(e) => {
                                return Err(e).context(format!(
                                    "Failed to create lease file {:?}",
                                    lease_file_path
                                ));
                            }
                        }
                    }
                }
            }
        }
    }

    let lease_file_path = match acquired_lease {
        Some(path) => path,
        None => {
            // OUTDIR_EXHAUSTED
            println!("{}", serde_json::json!({"error": "OUTDIR_EXHAUSTED"}));
            return Err(anyhow!("Outdirs exhausted for config {}", config_name));
        }
    };

    let worktree_id = format!("{}_out_{}", config_name, out_id);
    let workspace_path = config.workspaces_dir().join(&worktree_id);

    let worktree_info = WorktreeInfo {
        worktree_id: worktree_id.clone(),
        agent_id: agent_id.to_string(),
        config: config_name.to_string(),
        pid: std::process::id(),
        timestamp_sec: SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        workspace_path: workspace_path.clone(),
        outdir_path: outdir_path.clone(),
    };

    // Write Worktree State (using lease file internally)
    let worktree_json = serde_json::to_string_pretty(&worktree_info)
        .context("Failed to serialize worktree info")?;
    fs::write(&lease_file_path, worktree_json).context("Failed to write worktree JSON")?;

    // Implement rollback helper
    let rollback = || {
        log::warn!("Allocation failed, rolling back worktree {}", worktree_id);
        if let Err(e) = free_worktree_internal(config, &worktree_info) {
            log::error!("Rollback failed for worktree {}: {:?}", worktree_id, e);
        }
        if let Err(e) = fs::remove_file(&lease_file_path) {
            log::error!(
                "Failed to delete lease file {:?} during rollback: {:?}",
                lease_file_path,
                e
            );
        }
    };

    // Run the rest of provisioning, rollback on failure
    if let Err(e) = provision_workspace(config, &worktree_info) {
        rollback();
        return Err(e);
    }

    Ok(worktree_info)
}

fn provision_workspace(config: &Config, worktree_info: &WorktreeInfo) -> Result<()> {
    let workspace_path = &worktree_info.workspace_path;
    fs::create_dir_all(workspace_path)
        .with_context(|| format!("Failed to create workspace dir {:?}", workspace_path))?;

    // 3. Parse Jiri State
    let temp_jiri_json = std::env::temp_dir().join(format!("jiri_{}.json", Uuid::new_v4()));
    let jiri_bin = config.fuchsia_dir.join(".jiri_root/bin/jiri");
    let jiri_cmd = if jiri_bin.exists() {
        jiri_bin.to_str().unwrap()
    } else {
        "jiri"
    };

    log::info!("Running jiri project to export metadata...");
    run_command(
        jiri_cmd,
        &["project", "-json-output", temp_jiri_json.to_str().unwrap()],
        &config.fuchsia_dir,
        &[],
    )
    .context("Failed to export jiri projects")?;

    let jiri_json_content =
        fs::read_to_string(&temp_jiri_json).context("Failed to read jiri projects JSON")?;
    let _ = fs::remove_file(&temp_jiri_json); // Clean up temp file

    let projects: Vec<JiriProject> =
        serde_json::from_str(&jiri_json_content).context("Failed to parse jiri projects JSON")?;

    // Find the root project (rel_path is empty)
    let mut root_project = None;
    let mut sub_projects = Vec::new();

    for project in &projects {
        let rel_path = Path::new(&project.path)
            .strip_prefix(&config.fuchsia_dir)
            .context("Failed to strip prefix from project path")?;
        if rel_path.as_os_str().is_empty() {
            root_project = Some(project);
        } else {
            sub_projects.push(project);
        }
    }

    // 4. Provision Root Git Worktree
    if let Some(root) = root_project {
        log::info!("Provisioning root git worktree at {:?}", workspace_path);
        run_command(
            "git",
            &[
                "worktree",
                "add",
                "--detach",
                workspace_path.to_str().unwrap(),
                &root.revision,
            ],
            Path::new(&root.path),
            &[],
        )
        .with_context(|| "Failed to add root git worktree")?;
    } else {
        return Err(anyhow!("Root project not found in Jiri projects"));
    }

    // Copy .jiri_manifest if it exists in base checkout
    let base_manifest = config.fuchsia_dir.join(".jiri_manifest");
    let workspace_manifest = workspace_path.join(".jiri_manifest");
    if base_manifest.exists() {
        log::info!("Copying .jiri_manifest to workspace...");
        fs::copy(&base_manifest, &workspace_manifest).with_context(|| {
            format!("Failed to copy .jiri_manifest to {:?}", workspace_manifest)
        })?;
    }

    // Group sub-projects by path depth (number of components)
    let mut groups: BTreeMap<usize, Vec<&JiriProject>> = BTreeMap::new();
    for project in &sub_projects {
        let rel_path = Path::new(&project.path)
            .strip_prefix(&config.fuchsia_dir)
            .context("Failed to strip prefix from project path")?;
        let depth = rel_path.components().count();
        groups.entry(depth).or_default().push(project);
    }

    // 5. Provision sub-projects group by group (by depth) to avoid race conditions with nested repos
    for (depth, group) in groups {
        log::info!(
            "Provisioning sub-projects at depth {} (count: {})...",
            depth,
            group.len()
        );
        group.par_iter().try_for_each(|project| -> Result<()> {
            let rel_path = Path::new(&project.path)
                .strip_prefix(&config.fuchsia_dir)
                .context("Failed to strip prefix from project path")?;

            let target_path = workspace_path.join(rel_path);

            // Ensure parent directory exists
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create directory {:?}", parent))?;
            }

            // Run git worktree add
            log::debug!("Adding worktree for {} at {:?}", project.name, target_path);

            run_command(
                "git",
                &[
                    "worktree",
                    "add",
                    "--detach",
                    target_path.to_str().unwrap(),
                    &project.revision,
                ],
                Path::new(&project.path),
                &[],
            )
            .with_context(|| format!("Failed to add git worktree for {}", project.name))?;

            Ok(())
        })?;
    }

    // 5. Isolate Toolchains (Optimized: Symlink prebuilts & copy generated files, skip run-hooks)
    log::info!("Isolating toolchains and copying generated files...");

    // Symlink .jiri_root
    let base_jiri_root = config.fuchsia_dir.join(".jiri_root");
    let workspace_jiri_root = workspace_path.join(".jiri_root");
    std::os::unix::fs::symlink(&base_jiri_root, &workspace_jiri_root).with_context(|| {
        format!(
            "Failed to symlink {:?} to {:?}",
            base_jiri_root, workspace_jiri_root
        )
    })?;

    // Symlink prebuilt directory
    let base_prebuilt = config.fuchsia_dir.join("prebuilt");
    let workspace_prebuilt = workspace_path.join("prebuilt");
    std::os::unix::fs::symlink(&base_prebuilt, &workspace_prebuilt).with_context(|| {
        format!(
            "Failed to symlink {:?} to {:?}",
            base_prebuilt, workspace_prebuilt
        )
    })?;

    // Copy ctf_releases.gni if it exists in base checkout
    let base_ctf_gni = config
        .fuchsia_dir
        .join("sdk/ctf/build/internal/ctf_releases.gni");
    let workspace_ctf_gni = workspace_path.join("sdk/ctf/build/internal/ctf_releases.gni");
    if base_ctf_gni.exists() {
        if let Some(parent) = workspace_ctf_gni.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory {:?}", parent))?;
        }
        fs::copy(&base_ctf_gni, &workspace_ctf_gni).with_context(|| {
            format!(
                "Failed to copy {:?} to {:?}",
                base_ctf_gni, workspace_ctf_gni
            )
        })?;
        log::info!("Copied ctf_releases.gni");
    }

    // Copy build/info/jiri_generated directory
    let base_info_dir = config.fuchsia_dir.join("build/info/jiri_generated");
    let workspace_info_dir = workspace_path.join("build/info/jiri_generated");
    if base_info_dir.exists() {
        copy_dir_all(&base_info_dir, &workspace_info_dir)
            .context("Failed to copy build/info/jiri_generated directory")?;
        log::info!("Copied build/info/jiri_generated");
    }

    // Copy build/cipd.gni if it exists in base checkout
    let base_cipd_gni = config.fuchsia_dir.join("build/cipd.gni");
    let workspace_cipd_gni = workspace_path.join("build/cipd.gni");
    if base_cipd_gni.exists() {
        if let Some(parent) = workspace_cipd_gni.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory {:?}", parent))?;
        }
        fs::copy(&base_cipd_gni, &workspace_cipd_gni).with_context(|| {
            format!(
                "Failed to copy {:?} to {:?}",
                base_cipd_gni, workspace_cipd_gni
            )
        })?;
        log::info!("Copied build/cipd.gni");
    }

    // 6. Wire Build Directory (Moved for RBE support)
    log::info!("Moving outdir to workspace...");
    let workspace_out = workspace_path.join("out");
    fs::create_dir_all(&workspace_out)
        .with_context(|| format!("Failed to create workspace out dir {:?}", workspace_out))?;

    let workspace_out_default = workspace_out.join("default");
    if workspace_out_default.exists() {
        log::warn!("Destination workspace outdir {:?} already exists. Deleting it before moving.", workspace_out_default);
        fs::remove_dir_all(&workspace_out_default)
            .with_context(|| format!("Failed to delete existing workspace outdir {:?}", workspace_out_default))?;
    }
    fs::rename(&worktree_info.outdir_path, &workspace_out_default)
        .with_context(|| format!("Failed to move outdir from {:?} to {:?}", worktree_info.outdir_path, workspace_out_default))?;

    let fx_build_dir_file = workspace_path.join(".fx-build-dir");
    fs::write(&fx_build_dir_file, "out/default\n")
        .with_context(|| format!("Failed to write {:?}", fx_build_dir_file))?;

    // 7. Run fx gen to update build files to point to workspace sources
    log::info!("Running fx gen to initialize workspace build files...");
    let fx_bin = workspace_path.join("scripts/fx");
    let fx_cmd = if fx_bin.exists() {
        fx_bin.to_str().unwrap()
    } else {
        "fx"
    };

    run_command(fx_cmd, &["gen"], workspace_path, &[])
        .context("Failed to run fx gen in workspace")?;

    Ok(())
}
