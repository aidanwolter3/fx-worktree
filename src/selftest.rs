use anyhow::{Context, Result, anyhow};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::allocate::allocate_environment;
use crate::config::Config;
use crate::delete::delete_environment;
use crate::free::free_environment_by_id;
use crate::utils::run_command;

pub fn run_self_test(config: &Config, env_id: String) -> Result<()> {
    println!("=== Starting fxenv self-test ===");

    // 1. Initialize temporary self-test environment root inside the user's fxenv_root
    // to ensure it is on the same physical filesystem (SSD) so Jiri hardlinking works.
    let rand_id = uuid::Uuid::new_v4().to_string();
    let selftest_root = config
        .leases_dir()
        .parent()
        .unwrap()
        .join(format!("self-test-{}", &rand_id[0..8]));
    fs::create_dir_all(&selftest_root)?;

    // Share the prebuilt cache to speed up the self-test
    let base_shared_prebuilts = config.fxenv_root.join("shared-prebuilts");
    fs::create_dir_all(&base_shared_prebuilts)?;
    let test_shared_prebuilts = selftest_root.join("shared-prebuilts");
    std::os::unix::fs::symlink(&base_shared_prebuilts, &test_shared_prebuilts).with_context(
        || {
            format!(
                "Failed to symlink shared-prebuilts from {:?} to {:?}",
                base_shared_prebuilts, test_shared_prebuilts
            )
        },
    )?;

    let test_config = Config {
        fxenv_root: selftest_root.clone(),
        fuchsia_dir: config.fuchsia_dir.clone(),
    };
    test_config.init_topology()?;

    // Self-test cleanup helper to delete the temporary self-test root
    let root_cleanup = || {
        let _ = fs::remove_dir_all(&selftest_root);
    };

    if let Err(e) = run_self_test_lifecycle(&test_config, env_id) {
        root_cleanup();
        return Err(e);
    }

    root_cleanup();
    println!("=== Self-test completed successfully! ===");
    Ok(())
}

fn run_self_test_lifecycle(test_config: &Config, env_id_or_path: String) -> Result<()> {
    let should_delete_env = false;

    // 1. Resolve and verify environment
    let env_id = if env_id_or_path.contains('/') || env_id_or_path.contains('\\') {
        // It's a path
        let path = PathBuf::from(&env_id_or_path);
        if !path.exists() {
            return Err(anyhow!(
                "Specified environment path {:?} does not exist",
                path
            ));
        }
        let id = path.file_name().unwrap().to_str().unwrap().to_string();
        let temp_env_dir = test_config.environments_dir().join(&id);
        std::os::unix::fs::symlink(&path, &temp_env_dir)
            .with_context(|| format!("Failed to symlink {:?} to {:?}", path, temp_env_dir))?;
        id
    } else {
        // It's an ID, verify it exists in the user's config root
        let base_config = Config {
            fxenv_root: test_config
                .leases_dir()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .to_path_buf(),
            fuchsia_dir: test_config.fuchsia_dir.clone(),
        };
        let env_path = base_config.environments_dir().join(&env_id_or_path);
        if !env_path.exists() {
            return Err(anyhow!(
                "Specified environment ID {} does not exist in pool",
                env_id_or_path
            ));
        }
        let temp_env_dir = test_config.environments_dir().join(&env_id_or_path);
        std::os::unix::fs::symlink(&env_path, &temp_env_dir)
            .with_context(|| format!("Failed to symlink {:?} to {:?}", env_path, temp_env_dir))?;
        env_id_or_path
    };

    // Extract config name from env_id (format is <config>_<uuid>)
    let last_underscore = env_id
        .rfind('_')
        .context("Invalid environment ID format. Expected <config>_<uuid>")?;
    let config_name = &env_id[0..last_underscore];

    let env_path = test_config.environments_dir().join(&env_id);
    println!("Test environment path: {:?}", env_path);

    let target_label = "//:default";
    let watch_relative_path = "build.ninja";
    let watch_file_path = env_path.join("out/default").join(watch_relative_path);

    let fx_bin = env_path.join("scripts/fx");
    let fx_cmd = if fx_bin.exists() {
        fx_bin.to_str().unwrap()
    } else {
        "fx"
    };

    let mut allocated = false;

    let test_res = (|| -> Result<()> {
        // 2. Warm environment (build in the environment slot)
        if should_delete_env {
            println!("Warming environment (building target)...");
            run_command(fx_cmd, &["build", target_label], &env_path, &[])
                .context("Failed to build target to warm cache")?;
        } else {
            println!("Verifying build configuration exists in the specified environment...");
            if !watch_file_path.exists() {
                return Err(anyhow!(
                    "Expected build file {:?} was not found. You must run generator in the environment first.",
                    watch_file_path
                ));
            }
        }

        let t1 = get_modify_time(&watch_file_path)?;
        println!("Build file modify time: {:?}", t1);

        // 3. Allocate the environment (leases it and updates revisions)
        println!("Allocating environment (updating Git worktrees)...");
        let env_info =
            allocate_environment(test_config, config_name, "self_test_agent", true, false)
                .context("Failed to allocate environment")?;
        println!("Allocated workspace: {:?}", env_info.path);
        allocated = true;

        // 4. Verify build in workspace is a no-op (or at least succeeds)
        println!("Verifying build in workspace (no changes)...");
        run_command(fx_cmd, &["build", target_label], &env_info.path, &[])
            .context("Failed to build in workspace (no changes)")?;

        let t2 = get_modify_time(&watch_file_path)?;
        println!("Workspace build completed successfully.");

        // Wait to ensure timestamp resolution changes before modification
        std::thread::sleep(Duration::from_secs(1));

        // 5. Modify file in workspace
        println!("Modifying file in workspace...");

        let file_to_mod = env_info.path.join("BUILD.gn");
        if !file_to_mod.exists() {
            return Err(anyhow!(
                "Source file to modify does not exist: {:?}",
                file_to_mod
            ));
        }

        let content = fs::read_to_string(&file_to_mod)
            .with_context(|| format!("Failed to read {:?}", file_to_mod))?;

        let new_content = format!("{}\n# fxenv-self-test-marker\n", content);
        fs::write(&file_to_mod, new_content)
            .with_context(|| format!("Failed to write modified content to {:?}", file_to_mod))?;

        // 6. Verify build detects changes
        println!("Building in workspace with changes (should trigger regen)...");
        run_command(fx_cmd, &["build", target_label], &env_info.path, &[])
            .context("Failed to build in workspace with changes")?;

        let t3 = get_modify_time(&watch_file_path)?;
        if t3 <= t2 {
            return Err(anyhow!(
                "Build in workspace with changes did not trigger regeneration! Build file was not updated: t2={:?}, t3={:?}",
                t2,
                t3
            ));
        }
        println!("Workspace build triggered regeneration successfully (confirmed by timestamp).");

        // Restore file
        fs::write(&file_to_mod, content)
            .context("Failed to restore source file to original content")?;

        // Re-warm/rebuild to restore cache
        if !should_delete_env {
            println!("Restoring build cache in the reused environment...");
            run_command(fx_cmd, &["build", target_label], &env_info.path, &[])
                .context("Failed to rebuild after restoring source file")?;
        }

        Ok(())
    })();

    // 7. Cleanup
    println!("Cleaning up environment lease...");
    if allocated {
        if let Err(e) = free_environment_by_id(test_config, &env_id) {
            log::error!("Failed to free environment during cleanup: {:?}", e);
        }
    }

    if should_delete_env {
        delete_environment(test_config, &env_id).context("Failed to delete environment")?;
    } else {
        println!("Skipping environment deletion (reused environment)");
    }

    test_res
}

fn get_modify_time(path: &Path) -> Result<SystemTime> {
    let metadata =
        fs::metadata(path).with_context(|| format!("Failed to get metadata for {:?}", path))?;
    metadata
        .modified()
        .with_context(|| format!("Failed to get modification time for {:?}", path))
}
