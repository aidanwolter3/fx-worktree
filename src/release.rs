use crate::config::Config;
use crate::environment::EnvironmentInfo;
use crate::utils::{
    copy_file_if_different, run_command,
};
use anyhow::{Context, Result, anyhow};
use std::fs;

pub fn release_worktree(config: &Config, id: &str) -> Result<String> {
    if std::path::Path::new(id).components().count() > 1 {
        return Err(anyhow!("Invalid ID: {}", id));
    }

    let leases_dir = config.leases_dir();
    if !leases_dir.exists() {
        return Err(anyhow!("No active leases found."));
    }

    let mut matches = Vec::new();

    for entry in fs::read_dir(&leases_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("lease") {
            if let Ok(env_json) = fs::read_to_string(&path) {
                if let Ok(env_info) = serde_json::from_str::<EnvironmentInfo>(&env_json) {
                    if env_info.environment_id == id || env_info.agent_id == id {
                        matches.push((path, env_info));
                    }
                } else {
                    log::warn!("Failed to parse lease file {:?}", path);
                }
            }
        }
    }

    if matches.is_empty() {
        return Err(anyhow!("No active lease found matching '{}'", id));
    }

    if matches.len() > 1 {
        let agent_matches: Vec<&str> = matches
            .iter()
            .map(|(_, info)| info.agent_id.as_str())
            .collect();
        if agent_matches.iter().all(|&a| a == id) {
            let env_ids: Vec<String> = matches
                .iter()
                .map(|(_, info)| info.environment_id.clone())
                .collect();
            return Err(anyhow!(
                "Agent '{}' has leased multiple worktrees: {}. Please release by worktree ID instead.",
                id,
                env_ids.join(", ")
            ));
        } else {
            return Err(anyhow!("Ambiguous ID '{}': matches multiple leases.", id));
        }
    }

    let (lease_file_path, env_info) = &matches[0];
    let released_id = env_info.environment_id.clone();
    release_worktree_internal(config, env_info)?;

    // Delete lease file
    fs::remove_file(lease_file_path)
        .with_context(|| format!("Failed to delete lease file {:?}", lease_file_path))?;
    log::info!("Deleted lease file {:?}", lease_file_path);

    Ok(released_id)
}

pub fn release_worktree_internal(config: &Config, env_info: &EnvironmentInfo) -> Result<()> {
    log::info!("Releasing worktree {}", env_info.environment_id);


    // 1. Clean the workspace by calling 'jiri worktree clean'.
    // Note: This relies on 'jiri' being optimized to clean repositories in parallel
    // to meet performance requirements (less than 5 seconds).
    let jiri_bin = config.fuchsia_dir.join(".jiri_root/bin/jiri");
    let jiri_cmd = if jiri_bin.exists() {
        jiri_bin.to_str().unwrap()
    } else {
        "jiri"
    };

    run_command(jiri_cmd, &["worktree", "clean"], &env_info.path, &[])
        .context("Failed to run jiri worktree clean")?;


    // 2. Restore args.gn in the build directory
    let out_dir = env_info.path.join("out/default");
    if out_dir.exists() {
        let args_gn_ref = out_dir.join("args.gn.ref");
        let args_gn = out_dir.join("args.gn");
        if args_gn_ref.exists() {
            log::info!("Restoring args.gn from args.gn.ref");
            copy_file_if_different(&args_gn_ref, &args_gn)
                .with_context(|| format!("Failed to copy {:?} to {:?}", args_gn_ref, args_gn))?;
        }
    }

    Ok(())
}
