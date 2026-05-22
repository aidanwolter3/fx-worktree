use crate::config::Config;
use crate::utils::run_command;
use anyhow::{Context, Result};
use std::fs;

pub fn add_environment(config: &Config, config_name: &str, quiet: bool) -> Result<String> {
    let env_path = provision_workspace(config, quiet)?;

    // Implement cleanup helper in case creation fails in the middle
    let cleanup = || {
        log::warn!("Environment creation failed, cleaning up {:?}", env_path);
        let jiri_bin = config.fuchsia_dir.join(".jiri_root/bin/jiri");
        let jiri_cmd = if jiri_bin.exists() {
            jiri_bin.to_str().unwrap()
        } else {
            "jiri"
        };
        let _ = run_command(
            jiri_cmd,
            &["worktree", "remove", env_path.to_str().unwrap()],
            &config.fuchsia_dir,
            &[],
        );
        let _ = fs::remove_dir_all(&env_path);
    };

    let env_id = env_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| {
            cleanup();
            anyhow::anyhow!("Failed to get env_id from path {:?}", env_path)
        })?
        .to_string();

    if !quiet {
        eprintln!("Adding worktree {}...", env_id);
    }

    // Run sync to get prebuilts and ensure correct revisions (required for fx set)
    if let Err(e) = crate::sync::sync_environment(config, &env_id, &env_path, quiet) {
        cleanup();
        return Err(e);
    }

    // 2. Create physical out/default
    let out_dir = env_path.join("out/default");
    if let Err(e) = fs::create_dir_all(&out_dir) {
        cleanup();
        return Err(e).with_context(|| format!("Failed to create build directory {:?}", out_dir));
    }

    // 3. Write .fx-build-dir
    let fx_build_dir_file = env_path.join(".fx-build-dir");
    if let Err(e) = fs::write(&fx_build_dir_file, "out/default\n") {
        cleanup();
        return Err(e).with_context(|| format!("Failed to write {:?}", fx_build_dir_file));
    }

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

    if let Err(e) = run_command(
        fx_cmd,
        &["--dir", "out/default", "set", config_name],
        &env_path,
        &[],
    ) {
        cleanup();
        return Err(e).context("Failed to run fx set in environment");
    }

    // 5. Snapshot args.gn
    let args_gn = out_dir.join("args.gn");
    let args_gn_ref = out_dir.join("args.gn.ref");
    if args_gn.exists() {
        if let Err(e) = fs::copy(&args_gn, &args_gn_ref) {
            cleanup();
            return Err(e).with_context(|| format!("Failed to snapshot {:?} to {:?}", args_gn, args_gn_ref));
        }
        log::info!("Created args.gn.ref");
    } else {
        log::warn!("args.gn not found after fx set. This might happen if fx set was mocked.");
    }

    if let Err(e) = config.record_last_active(&env_path) {
        cleanup();
        return Err(e);
    }

    Ok(env_id)
}

fn provision_workspace(config: &Config, quiet: bool) -> Result<std::path::PathBuf> {
    if !quiet {
        eprintln!("Provisioning worktree workspace...");
    }
    let jiri_bin = config.fuchsia_dir.join(".jiri_root/bin/jiri");
    let jiri_cmd = if jiri_bin.exists() {
        jiri_bin.to_str().unwrap()
    } else {
        "jiri"
    };

    let output = run_command(
        jiri_cmd,
        &["worktree", "add"],
        &config.fuchsia_dir,
        &[],
    )
    .context("Failed to run jiri worktree add")?;

    let stdout = String::from_utf8(output.stdout).context("Failed to parse jiri output as UTF-8")?;
    let path_str = stdout.trim();
    if path_str.is_empty() {
        return Err(anyhow::anyhow!("Jiri worktree add returned empty path"));
    }
    Ok(std::path::PathBuf::from(path_str))
}
