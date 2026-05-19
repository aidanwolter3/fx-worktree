use crate::config::Config;
use crate::utils::run_command;
use anyhow::{Context, Result, anyhow};
use std::fs;
use uuid::Uuid;

pub fn create_outdir(config: &Config, config_name: &str, fx_args: &[String]) -> Result<String> {
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

    let mut args = vec!["--dir", outdir_path.to_str().unwrap(), "set", config_name];
    for arg in fx_args {
        args.push(arg);
    }

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

pub fn delete_outdir(config: &Config, config_name: &str, outdir_id: &str) -> Result<()> {
    let uuid = outdir_id.strip_prefix("out_").unwrap_or(outdir_id);

    let lease_file_name = format!("{}_{}.lease", config_name, uuid);
    let lease_file_path = config.leases_dir().join(&lease_file_name);

    if lease_file_path.exists() {
        return Err(anyhow!(
            "Cannot delete outdir {} because it is currently in use (lease file {:?} exists)",
            outdir_id,
            lease_file_path
        ));
    }

    let outdir_path = config.outdirs_dir().join(config_name).join(outdir_id);
    if !outdir_path.exists() {
        return Err(anyhow!("Outdir {:?} does not exist", outdir_path));
    }

    log::info!("Deleting outdir {:?}", outdir_path);
    fs::remove_dir_all(&outdir_path)
        .with_context(|| format!("Failed to delete outdir {:?}", outdir_path))?;

    Ok(())
}
