use crate::config::Config;
use crate::utils::run_command;
use anyhow::{Context, Result, anyhow};
use std::fs;
use uuid::Uuid;

pub fn create_outdir(config: &Config, config_name: &str) -> Result<String> {
    let uuid = Uuid::new_v4().to_string();
    let outdir_config_dir = config.outdirs_dir().join(config_name);
    let outdir_name = format!("out_{}", uuid);
    let outdir_path = outdir_config_dir.join(&outdir_name);

    fs::create_dir_all(&outdir_path)
        .with_context(|| format!("Failed to create outdir {:?}", outdir_path))?;

    log::info!("Created outdir {:?}", outdir_path);

    // Run fx set
    let fx_abs_path = config.fuchsia_dir.join("scripts/fx");
    let fx_cmd = if fx_abs_path.exists() {
        fx_abs_path.to_str().unwrap()
    } else {
        "fx"
    };

    let args = vec!["--dir", outdir_path.to_str().unwrap(), "set", config_name];

    log::info!("Running fx set...");
    run_command(fx_cmd, &args, &config.fuchsia_dir, &[]).context("Failed to run fx set")?;

    // Copy args.gn to args.gn.ref
    let args_gn = outdir_path.join("args.gn");
    let args_gn_ref = outdir_path.join("args.gn.ref");

    if args_gn.exists() {
        fs::copy(&args_gn, &args_gn_ref)
            .with_context(|| format!("Failed to copy {:?} to {:?}", args_gn, args_gn_ref))?;
        log::info!("Created args.gn.ref");
    } else {
        log::warn!(
            "args.gn not found in outdir after fx set. This might happen if fx set was mocked or failed silently."
        );
    }

    Ok(outdir_name)
}

pub fn delete_outdir(config: &Config, outdir_id: &str) -> Result<()> {
    let uuid = outdir_id.strip_prefix("out_").unwrap_or(outdir_id);

    // 1. Check if the outdir is in use by scanning leases (needed because outdirs are moved to workspaces when leased)
    let leases_dir = config.leases_dir();
    if leases_dir.exists() {
        for entry in fs::read_dir(&leases_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("lease") {
                if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
                    let suffix = format!("_{}.lease", uuid);
                    if file_name.ends_with(&suffix) {
                        return Err(anyhow!(
                            "Cannot delete outdir {} because it is currently in use (lease file {:?} exists)",
                            outdir_id,
                            path
                        ));
                    }
                }
            }
        }
    }

    // 2. Scan pool to resolve config name (since it is not in use, it must be in the pool)
    let outdirs_dir = config.outdirs_dir();
    let mut resolved_config_name = None;

    if outdirs_dir.exists() {
        for config_entry in fs::read_dir(&outdirs_dir)? {
            let config_entry = config_entry?;
            let config_path = config_entry.path();
            if config_path.is_dir() {
                let target_path = config_path.join(outdir_id);
                let target_path_with_prefix = config_path.join(format!("out_{}", uuid));
                if target_path.exists() || target_path_with_prefix.exists() {
                    resolved_config_name = Some(config_path.file_name().unwrap().to_str().unwrap().to_string());
                    break;
                }
            }
        }
    }

    let config_name = match resolved_config_name {
        Some(name) => name,
        None => return Err(anyhow!("Outdir {} not found in any configuration pool", outdir_id)),
    };

    // 3. Delete from pool
    let outdir_path = config.outdirs_dir().join(&config_name).join(format!("out_{}", uuid));
    if !outdir_path.exists() {
        let outdir_path_no_prefix = config.outdirs_dir().join(&config_name).join(uuid);
        if outdir_path_no_prefix.exists() {
            log::info!("Deleting outdir {:?}", outdir_path_no_prefix);
            fs::remove_dir_all(&outdir_path_no_prefix)
                .with_context(|| format!("Failed to delete outdir {:?}", outdir_path_no_prefix))?;
            return Ok(());
        }
        return Err(anyhow!("Outdir {:?} does not exist", outdir_path));
    }

    log::info!("Deleting outdir {:?}", outdir_path);
    fs::remove_dir_all(&outdir_path)
        .with_context(|| format!("Failed to delete outdir {:?}", outdir_path))?;

    Ok(())
}
