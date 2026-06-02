use crate::config::Config;
use crate::environment::EnvironmentInfo;
use anyhow::{Context, Result, anyhow};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

pub fn lease_environment(
    config: &Config,
    config_name: &str,
    agent_id: &str,
    sync: bool,
    quiet: bool,
) -> Result<EnvironmentInfo> {
    let mut acquired_lease = None;
    let mut env_path = PathBuf::new();
    let mut env_id = String::new();

    // 1. Find a free environment of the config type in the pool
    let envs_dir = config.environments_dir();
    if !envs_dir.exists() {
        return Err(anyhow!(
            "No worktrees found. Add one first using 'fx-worktree add {}'",
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
                let id = dir_name;
                let current_config_name = match crate::utils::get_config_name(&path) {
                    Ok(name) => name,
                    Err(e) => {
                        log::warn!(
                            "Skipping worktree {:?} due to error reading config: {:?}",
                            path,
                            e
                        );
                        continue;
                    }
                };

                if current_config_name == config_name {
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
        return Err(anyhow!(
            "No free environments available for config {}.",
            config_name
        ));
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
    let env_json =
        serde_json::to_string(&env_info).context("Failed to serialize EnvironmentInfo")?;
    fs::write(&lease_file_path, env_json).context("Failed to write lease JSON")?;

    // Rollback helper on failure
    let rollback = || {
        log::warn!("Lease failed, releasing lease {}", env_id);
        let _ = fs::remove_file(&lease_file_path);
    };

    // 2. Reuse the worktree (clean and checkout target revisions)
    if sync {
        if let Err(e) = crate::sync::sync_environment(config, &env_id, &env_path, quiet) {
            rollback();
            return Err(e);
        }
    }

    if let Err(e) = setup_branch(&env_path, agent_id) {
        rollback();
        return Err(e);
    }

    config.record_last_active(&env_info.path)?;

    Ok(env_info)
}

fn setup_branch(workspace_path: &Path, agent_id: &str) -> Result<()> {
    if agent_id.is_empty() {
        return Ok(());
    }

    let branch_name = if agent_id.starts_with("feat/") || agent_id.starts_with("bug/") {
        agent_id.to_string()
    } else if agent_id.to_uppercase().starts_with("T-") {
        format!("feat/{}", agent_id.to_lowercase())
    } else {
        format!("feat/{}", agent_id)
    };

    log::info!("Setting up branch {} in {:?}", branch_name, workspace_path);

    // Check if branch already exists
    let branch_exists = Command::new("git")
        .args(&["show-ref", "--verify", &format!("refs/heads/{}", branch_name)])
        .current_dir(workspace_path)
        .output()
        .context("Failed to run git show-ref")?;

    if branch_exists.status.success() {
        // Branch exists, check it out
        let output = Command::new("git")
            .args(&["checkout", &branch_name])
            .current_dir(workspace_path)
            .output()
            .context("Failed to run git checkout")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("already used by worktree") {
                log::warn!(
                    "Branch {} is already checked out in another worktree. Staying on detached HEAD.",
                    branch_name
                );
                eprintln!(
                    "⚠ Warning: Branch {} is already checked out in another worktree. Staying on detached HEAD.",
                    branch_name
                );
            } else {
                return Err(anyhow!(
                    "Failed to checkout existing branch {}: {}",
                    branch_name,
                    stderr
                ));
            }
        }
    } else {
        // Branch does not exist, create and checkout
        let output = Command::new("git")
            .args(&["checkout", "-b", &branch_name])
            .current_dir(workspace_path)
            .output()
            .context("Failed to run git checkout -b")?;
        if !output.status.success() {
            return Err(anyhow!(
                "Failed to create and checkout branch {}: {}",
                branch_name,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }

    Ok(())
}
