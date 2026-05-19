use crate::config::Config;
use crate::utils::{copy_toolchain_metadata, run_command};
use crate::environment::EnvironmentInfo;
use anyhow::{Context, Result, anyhow};
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use uuid::Uuid;

#[derive(serde::Deserialize, Debug, Clone)]
struct JiriProject {
    path: String,
    revision: String,
}

pub fn allocate_environment(
    config: &Config,
    config_name: &str,
    agent_id: &str,
    quiet: bool,
) -> Result<EnvironmentInfo> {
    let mut acquired_lease = None;
    let mut env_path = PathBuf::new();
    let mut env_id = String::new();

    // 1. Find a free environment of the config type in the pool
    let envs_dir = config.environments_dir();
    if !envs_dir.exists() {
        return Err(anyhow!(
            "No environments found. Create one first using 'fxenv create {}'",
            config_name
        ));
    }

    let entries = fs::read_dir(&envs_dir)
        .with_context(|| format!("Failed to read environments directory {:?}", envs_dir))?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
                // Environment folder name is: <config>_<uuid>
                // We check if it starts with config_name followed by underscore
                let prefix = format!("{}_", config_name);
                if dir_name.starts_with(&prefix) {
                    let id = dir_name;
                    let is_complete = path.join(".fxenv-completed").exists();
                    if !is_complete {
                        log::warn!("Skipping incomplete environment {:?}", path);
                        continue;
                    }

                    let lease_file_name = format!("{}.lease", id);
                    let lease_file_path = config.leases_dir().join(&lease_file_name);

                    // Attempt to create lease file atomically
                    match fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&lease_file_path)
                    {
                        Ok(_) => {
                            log::info!("Acquired lease lock: {:?}", lease_file_path);
                            acquired_lease = Some(lease_file_path);
                            env_path = path.clone();
                            env_id = id.to_string();
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

    if acquired_lease.is_none() {
        return Err(anyhow!("No free environments available for config {}.", config_name));
    }

    let lease_file_path = acquired_lease.unwrap();
    let current_time = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let env_info = EnvironmentInfo {
        environment_id: env_id.clone(),
        agent_id: agent_id.to_string(),
        config: config_name.to_string(),
        pid: std::process::id(),
        timestamp_sec: current_time,
        path: env_path.clone(),
    };

    // Write lease info
    let env_json = serde_json::to_string(&env_info).context("Failed to serialize EnvironmentInfo")?;
    fs::write(&lease_file_path, env_json).context("Failed to write lease JSON")?;

    // Rollback helper on failure
    let rollback = || {
        log::warn!("Allocation failed, releasing lease {}", env_id);
        let _ = fs::remove_file(&lease_file_path);
    };

    // 2. Reuse the environment (clean and checkout target revisions)
    if let Err(e) = reuse_environment(config, &env_info, quiet) {
        rollback();
        return Err(e);
    }

    config.record_last_created(&env_info.path)?;

    Ok(env_info)
}

fn reuse_environment(config: &Config, env_info: &EnvironmentInfo, quiet: bool) -> Result<()> {
    let workspace_path = &env_info.path;

    if !quiet {
        println!("Reusing existing environment {}...", env_info.environment_id);
    }

    // 1. Query target Jiri state from base repo
    if !quiet {
        println!("Querying Fuchsia project structure...");
    }
    let temp_jiri_json = std::env::temp_dir().join(format!("jiri_{}.json", Uuid::new_v4()));
    let jiri_bin = config.fuchsia_dir.join(".jiri_root/bin/jiri");
    let jiri_cmd = if jiri_bin.exists() {
        jiri_bin.to_str().unwrap()
    } else {
        "jiri"
    };

    run_command(
        jiri_cmd,
        &["project", "-json-output", temp_jiri_json.to_str().unwrap()],
        &config.fuchsia_dir,
        &[],
    )
    .context("Failed to run jiri project in base repo")?;

    let jiri_json = fs::read_to_string(&temp_jiri_json).context("Failed to read jiri json")?;
    let projects: Vec<JiriProject> = serde_json::from_str(&jiri_json).context("Failed to parse Jiri JSON")?;
    let _ = fs::remove_file(&temp_jiri_json);

    let mut root_project = None;
    let mut sub_projects = Vec::new();
    for project in &projects {
        let rel_path = Path::new(&project.path)
            .strip_prefix(&config.fuchsia_dir)
            .context("Failed to strip prefix")?;
        if rel_path.as_os_str().is_empty() {
            root_project = Some(project);
        } else {
            sub_projects.push(project);
        }
    }

    // 2. Clean and checkout root project (exclude out/ and markers)
    if let Some(root) = root_project {
        if !quiet {
            println!("Updating Git worktrees to target revisions...");
        }
        run_command("git", &["reset", "--hard"], workspace_path, &[])?;
        run_command(
            "git",
            &[
                "clean",
                "-fdx",
                "-e", ".fxenv-completed",
                "-e", "prebuilt",
                "-e", ".jiri_root",
                "-e", ".fx-build-dir",
                "-e", "out", // Preserves build cache!
            ],
            workspace_path,
            &[],
        )?;
        run_command("git", &["checkout", &root.revision], workspace_path, &[])?;
    } else {
        return Err(anyhow!("Root project not found in Jiri projects"));
    }

    // 3. Clean and checkout sub-projects in parallel (exclude out/ if any, though subprojects usually don't have out/)
    sub_projects.par_iter().try_for_each(|project| -> Result<()> {
        let rel_path = Path::new(&project.path)
            .strip_prefix(&config.fuchsia_dir)
            .context("Failed to strip prefix")?;
        let target_path = workspace_path.join(rel_path);

        if target_path.exists() {
            run_command("git", &["reset", "--hard"], &target_path, &[])?;
            run_command("git", &["clean", "-fdx"], &target_path, &[])?;
            run_command("git", &["checkout", &project.revision], &target_path, &[])?;
        } else {
            // New sub-project added to manifest: clone it fresh!
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent)?;
            }
            run_command(
                "git",
                &[
                    "worktree",
                    "add",
                    "-f",
                    "--detach",
                    target_path.to_str().unwrap(),
                    &project.revision,
                ],
                Path::new(&project.path),
                &[],
            )?;
            // Convert to symlink
            let git_file_path = target_path.join(".git");
            if git_file_path.exists() && git_file_path.is_file() {
                let contents = fs::read_to_string(&git_file_path)?;
                if let Some(gitdir_line) = contents.lines().next() {
                    if let Some(gitdir_path_str) = gitdir_line.strip_prefix("gitdir: ") {
                        let gitdir_path = PathBuf::from(gitdir_path_str.trim());
                        fs::remove_file(&git_file_path)?;
                        std::os::unix::fs::symlink(&gitdir_path, &git_file_path)?;
                    }
                }
            }
        }
        Ok(())
    })?;

    // Copy/restore toolchain metadata files deleted by git clean
    copy_toolchain_metadata(config, workspace_path)?;

    // 4. Run fx gen to initialize workspace build files
    if !quiet {
        println!("Initializing workspace build files (running fx gen)...");
    }
    let fx_bin = workspace_path.join("scripts/fx");
    let fx_cmd = if fx_bin.exists() {
        fx_bin.to_str().unwrap()
    } else {
        "fx"
    };
    run_command(fx_cmd, &["gen"], workspace_path, &[])?;

    Ok(())
}
