use anyhow::{Context, Result, anyhow};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::config::Config;
use crate::create::create_environment;
use crate::delete::delete_environment;
use crate::allocate::allocate_environment;
use crate::free::free_environment_by_id;
use crate::utils::run_command;

pub fn run_self_test(config: &Config, use_env_id: Option<String>) -> Result<()> {
    println!("=== Starting fxenv self-test ===");

    // 1. Initialize temporary self-test environment root inside the user's fxenv_root
    // to ensure it is on the same physical filesystem (SSD) so Jiri hardlinking works.
    let rand_id = uuid::Uuid::new_v4().to_string();
    let selftest_root = config.leases_dir().parent().unwrap().join(format!("self-test-{}", &rand_id[0..8]));
    fs::create_dir_all(&selftest_root)?;

    let test_config = Config {
        fxenv_root: selftest_root.clone(),
        fuchsia_dir: config.fuchsia_dir.clone(),
    };
    test_config.init_topology()?;

    // Self-test cleanup helper to delete the temporary self-test root
    let root_cleanup = || {
        let _ = fs::remove_dir_all(&selftest_root);
    };

    if let Err(e) = run_self_test_lifecycle(&test_config, use_env_id) {
        root_cleanup();
        return Err(e);
    }

    root_cleanup();
    println!("=== Self-test completed successfully! ===");
    Ok(())
}

fn run_self_test_lifecycle(test_config: &Config, use_env_id: Option<String>) -> Result<()> {
    let config_name = "fuchsia_internal.x64"; // hardcoded target configuration for mock verification

    let mut should_delete_env = true;

    // 1. Create or reuse environment
    let env_id = if let Some(id_or_path) = use_env_id {
        should_delete_env = false;
        if id_or_path.contains('/') || id_or_path.contains('\\') {
            // It's a path
            let path = PathBuf::from(&id_or_path);
            if !path.exists() {
                return Err(anyhow!("Specified environment path {:?} does not exist", path));
            }
            let id = path.file_name().unwrap().to_str().unwrap().to_string();
            // Force copy the completed marker to the temp config root if it was in the base
            let temp_env_dir = test_config.environments_dir().join(&id);
            fs::create_dir_all(&temp_env_dir)?;
            fs::write(temp_env_dir.join(".fxenv-completed"), "")?;
            id
        } else {
            // It's an ID, verify it exists in the user's config root
            let base_config = Config {
                fxenv_root: test_config.leases_dir().parent().unwrap().parent().unwrap().to_path_buf(),
                fuchsia_dir: test_config.fuchsia_dir.clone(),
            };
            let env_path = base_config.environments_dir().join(&id_or_path);
            if !env_path.exists() {
                return Err(anyhow!("Specified environment ID {} does not exist in pool", id_or_path));
            }
            // Force copy marker to temp config root
            let temp_env_dir = test_config.environments_dir().join(&id_or_path);
            fs::create_dir_all(&temp_env_dir)?;
            fs::write(temp_env_dir.join(".fxenv-completed"), "")?;
            id_or_path
        }
    } else {
        println!("Creating test environment...");
        create_environment(test_config, config_name)
            .context("Failed to create test environment")?
    };

    let env_path = test_config.environments_dir().join(&env_id);
    println!("Test environment path: {:?}", env_path);

    let target_label = "//sdk/ctf/tests/fidl/fuchsia.diagnostics:inspect-publisher";
    let obj_relative_path =
        "obj/sdk/ctf/tests/fidl/fuchsia.diagnostics/inspect-publisher.inspect_publisher.cc.o";
    let obj_file_path = env_path.join("out/default").join(obj_relative_path);

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
            run_command(
                fx_cmd,
                &["build", target_label],
                &env_path,
                &[],
            )
            .context("Failed to build target to warm cache")?;
        } else {
            println!("Verifying target is pre-built in the specified environment...");
            if !obj_file_path.exists() {
                return Err(anyhow!(
                    "Expected object file {:?} was not found. You must build the target {} in the environment first.",
                    obj_file_path,
                    target_label
                ));
            }
        }

        let t1 = get_modify_time(&obj_file_path)?;
        println!("Target object file modify time: {:?}", t1);

        // 3. Allocate the environment (leases it and updates revisions)
        println!("Allocating environment (updating Git worktrees)...");
        let env_info = allocate_environment(test_config, config_name, "self_test_agent", false)
            .context("Failed to allocate environment")?;
        println!("Allocated workspace: {:?}", env_info.path);
        allocated = true;

        // 4. Verify build in workspace is a no-op (or at least succeeds)
        println!("Verifying build in workspace (no changes)...");
        run_command(
            fx_cmd,
            &["build", target_label],
            &env_info.path,
            &[],
        )
        .context("Failed to build in workspace (no changes)")?;

        let t2 = get_modify_time(&obj_file_path)?;
        println!("Workspace build completed successfully.");

        // 5. Modify file in workspace
        println!("Modifying file in workspace...");
        let file_to_mod = env_info
            .path
            .join("sdk/ctf/tests/fidl/fuchsia.diagnostics/inspect_publisher.cc");
        if !file_to_mod.exists() {
            return Err(anyhow!("Source file to modify does not exist: {:?}", file_to_mod));
        }

        let content = fs::read_to_string(&file_to_mod)
            .with_context(|| format!("Failed to read {:?}", file_to_mod))?;

        let target_str = "numeric_properties.RecordInt(\"int\", -1);";
        let replacement_str = "numeric_properties.RecordInt(\"int\", -2);";

        if !content.contains(target_str) {
            return Err(anyhow!("Target string not found in source file: {}", target_str));
        }
        let new_content = content.replace(target_str, replacement_str);
        fs::write(&file_to_mod, new_content)
            .with_context(|| format!("Failed to write modified content to {:?}", file_to_mod))?;

        // Wait to ensure timestamp resolution changes
        std::thread::sleep(Duration::from_secs(1));

        // 6. Verify build compiles changes
        println!("Building in workspace with changes (should compile)...");
        run_command(
            fx_cmd,
            &["build", target_label],
            &env_info.path,
            &[],
        )
        .context("Failed to build in workspace with changes")?;

        let t3 = get_modify_time(&obj_file_path)?;
        if t3 <= t2 {
            return Err(anyhow!(
                "Build in workspace with changes did not compile the file! Object file was not updated: t2={:?}, t3={:?}",
                t2,
                t3
            ));
        }
        println!("Workspace build compiled changes successfully (confirmed by timestamp).");

        // Restore file
        fs::write(&file_to_mod, content)
            .context("Failed to restore source file to original content")?;

        // Re-warm/rebuild to restore cache
        if !should_delete_env {
            println!("Restoring build cache in the reused environment...");
            run_command(
                fx_cmd,
                &["build", target_label],
                &env_info.path,
                &[],
            )
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
