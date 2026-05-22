use crate::config::Config;
use crate::utils::run_command;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use uuid::Uuid;

pub fn add_environment(config: &Config, config_name: &str, quiet: bool) -> Result<String> {
    let uuid = Uuid::new_v4().to_string();
    let env_id = format!("{}_{}", config_name, &uuid[0..8]);
    let env_path = config.environments_dir().join(&env_id);

    if !quiet {
        eprintln!("Adding worktree {}...", env_id);
    }

    // 1. Create directory structure
    fs::create_dir_all(&env_path)
        .with_context(|| format!("Failed to create worktree directory {:?}", env_path))?;

    // Implement cleanup helper in case creation fails in the middle
    let cleanup = || {
        log::warn!("Environment creation failed, cleaning up {:?}", env_path);
        let _ = fs::remove_dir_all(&env_path);
    };

    if let Err(e) = provision_workspace(config, &env_path, quiet) {
        cleanup();
        return Err(e);
    }

    // Run sync to get prebuilts and ensure correct revisions (required for fx set)
    if let Err(e) = crate::sync::sync_environment(config, &env_id, &env_path, quiet) {
        cleanup();
        return Err(e);
    }

    // 2. Create physical out/default
    let out_dir = env_path.join("out/default");
    fs::create_dir_all(&out_dir)
        .with_context(|| format!("Failed to create build directory {:?}", out_dir))?;

    // 3. Write .fx-build-dir
    let fx_build_dir_file = env_path.join(".fx-build-dir");
    fs::write(&fx_build_dir_file, "out/default\n")
        .with_context(|| format!("Failed to write {:?}", fx_build_dir_file))?;

    // 4. Run fx set in the workspace
    if !quiet {
        eprintln!("Running fx set {}...", config_name);
    }
    let fx_bin = env_path.join("scripts/fx");
    let fx_cmd = if fx_bin.exists() {
        fx_bin.to_str().unwrap()
    } else {
        "fx"
    };

    run_command(
        fx_cmd,
        &["--dir", "out/default", "set", config_name],
        &env_path,
        &[],
    )
    .context("Failed to run fx set in environment")?;

    // 5. Snapshot args.gn
    let args_gn = out_dir.join("args.gn");
    let args_gn_ref = out_dir.join("args.gn.ref");
    if args_gn.exists() {
        fs::copy(&args_gn, &args_gn_ref)
            .with_context(|| format!("Failed to snapshot {:?} to {:?}", args_gn, args_gn_ref))?;
        log::info!("Created args.gn.ref");
    } else {
        log::warn!("args.gn not found after fx set. This might happen if fx set was mocked.");
    }

    // 6. Write completion marker
    fs::write(env_path.join(".fx-worktree-completed"), "")
        .with_context(|| format!("Failed to write completion marker"))?;

    config.record_last_active(&env_path)?;

    Ok(env_id)
}

fn provision_workspace(config: &Config, workspace_path: &Path, quiet: bool) -> Result<()> {
    if !quiet {
        eprintln!("Provisioning worktree workspace...");
    }
    let jiri_bin = config.fuchsia_dir.join(".jiri_root/bin/jiri");
    let jiri_cmd = if jiri_bin.exists() {
        jiri_bin.to_str().unwrap()
    } else {
        "jiri"
    };

    run_command(
        jiri_cmd,
        &["worktree", "add", workspace_path.to_str().unwrap()],
        &config.fuchsia_dir,
        &[],
    )
    .context("Failed to run jiri worktree add")?;

    Ok(())
}
