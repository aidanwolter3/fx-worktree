use anyhow::{Context, Result, anyhow};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::alloc::allocate;
use crate::config::Config;
use crate::free::free_worktree_by_id;
use crate::outdir::{create_outdir, delete_outdir};
use crate::utils::run_command;

pub fn run_self_test(config: &Config, use_outdir_id: Option<String>) -> Result<()> {
    log::info!("Starting self-test...");

    // 1. Create temporary FXENV_ROOT
    log::info!("Creating temporary FXENV_ROOT...");
    fs::create_dir_all(&config.fxenv_root).context("Failed to create fxenv_root directory")?;
    let temp_fxenv_root = tempfile::Builder::new()
        .prefix("self-test-")
        .tempdir_in(&config.fxenv_root)
        .context("Failed to create temporary FXENV_ROOT directory")?;
    let test_config = Config {
        fxenv_root: temp_fxenv_root.path().to_path_buf(),
        fuchsia_dir: config.fuchsia_dir.clone(),
    };
    test_config.init_topology()?;

    // 2. Create or reuse test outdir
    let config_name = "fuchsia.x64";
    let mut forced_outdir_path = None;
    let mut preferred_outdir_id = None;
    let mut should_delete_outdir = true;
    let outdir_id: String;

    if let Some(id_or_path) = use_outdir_id {
        should_delete_outdir = false;
        if id_or_path.starts_with('/') || id_or_path.starts_with('~') || id_or_path.starts_with('.') {
            // Resolve path
            let resolved_path = if id_or_path.starts_with('~') {
                if let Ok(home) = std::env::var("HOME") {
                    PathBuf::from(home).join(&id_or_path[2..])
                } else {
                    return Err(anyhow!("Failed to resolve HOME directory for path {}", id_or_path));
                }
            } else {
                fs::canonicalize(&id_or_path)
                    .with_context(|| format!("Failed to canonicalize path {}", id_or_path))?
            };

            if !resolved_path.exists() {
                return Err(anyhow!("Specified outdir path {:?} does not exist", resolved_path));
            }
            log::info!("Reusing existing outdir path: {:?}", resolved_path);
            forced_outdir_path = Some(resolved_path.clone());
            outdir_id = resolved_path.file_name().and_then(|n| n.to_str()).unwrap_or("reused_outdir").to_string();
        } else {
            // Resolve ID
            let outdir_path = config.fuchsia_dir.join("out/fxenv").join(config_name).join(&id_or_path);
            if !outdir_path.exists() {
                return Err(anyhow!("Specified outdir {:?} does not exist in pool", outdir_path));
            }
            let args_gn_ref = outdir_path.join("args.gn.ref");
            if !args_gn_ref.exists() {
                return Err(anyhow!(
                    "Specified outdir {:?} is missing args.gn.ref. It might be corrupted.",
                    outdir_path
                ));
            }
            println!("Reusing existing outdir from pool: {:?}", outdir_path);
            preferred_outdir_id = Some(id_or_path.clone());
            outdir_id = id_or_path;
        }
    } else {
        println!("Creating test outdir...");
        let id = create_outdir(&test_config, config_name)
            .context("Failed to create test outdir")?;
        outdir_id = id.clone();
        preferred_outdir_id = Some(id);
    };

    let outdir_path = if let Some(ref path) = forced_outdir_path {
        path.clone()
    } else {
        test_config.outdirs_dir().join(config_name).join(&outdir_id)
    };
    println!("Test outdir path: {:?}", outdir_path);

    // Resolve fx path in base repo
    let fx_abs_path = config.fuchsia_dir.join("scripts/fx");
    let fx_cmd = if fx_abs_path.exists() {
        fx_abs_path.to_str().unwrap()
    } else {
        "fx"
    };

    let target_label = "//sdk/ctf/tests/fidl/fuchsia.diagnostics:inspect-publisher";
    let obj_relative_path =
        "obj/sdk/ctf/tests/fidl/fuchsia.diagnostics/inspect-publisher.inspect_publisher.cc.o";
    let obj_file_path = outdir_path.join(obj_relative_path);

    let mut allocated = false;
    let mut allocated_worktree_id = String::new();

    let test_res = (|| -> Result<()> {
        // 3. Warm outdir (build in base repo)
        if should_delete_outdir {
            println!("Warming outdir (building in base repo)...");
            run_command(
                fx_cmd,
                &[
                    "--dir",
                    outdir_path.to_str().unwrap(),
                    "build",
                    target_label,
                ],
                &config.fuchsia_dir,
                &[],
            )
            .context("Failed to build target in base repo to warm cache")?;
        } else {
            println!("Verifying target is pre-built in the specified outdir...");
            if !obj_file_path.exists() {
                return Err(anyhow!(
                    "Expected object file {:?} was not found. You must build the target {} in the outdir first.",
                    obj_file_path,
                    target_label
                ));
            }
        }

        let t1 = get_modify_time(&obj_file_path)?;
        println!("Target object file modify time: {:?}", t1);

        // 4. Allocate worktree
        println!("Allocating test worktree (this will run fx gen)...");
        let pref_id = preferred_outdir_id.as_deref().and_then(|id| id.strip_prefix("out_").or(Some(id)));
        let worktree_info = allocate(&test_config, config_name, "self_test_agent", pref_id, forced_outdir_path.clone(), false)
            .context("Failed to allocate worktree")?;
        println!("Allocated workspace: {:?}", worktree_info.workspace_path);
        allocated_worktree_id = worktree_info.worktree_id.clone();
        allocated = true;

        let ws_obj_file_path = worktree_info.workspace_path.join("out/default").join(obj_relative_path);

        // Resolve fx path in workspace
        let ws_fx_bin = worktree_info.workspace_path.join("scripts/fx");
        let ws_fx_cmd = if ws_fx_bin.exists() {
            ws_fx_bin.to_str().unwrap()
        } else {
            "fx"
        };

        // 5. Verify build is no-op (or at least succeeds)
        println!("Verifying build in workspace (no changes)...");
        run_command(
            ws_fx_cmd,
            &["build", target_label],
            &worktree_info.workspace_path,
            &[],
        )
        .context("Failed to build in workspace (no changes)")?;

        let t2 = get_modify_time(&ws_obj_file_path)?;
        println!("Workspace build completed successfully.");

        // 6. Modify file in workspace
        println!("Modifying file in workspace...");
        let file_to_mod = worktree_info
            .workspace_path
            .join("sdk/ctf/tests/fidl/fuchsia.diagnostics/inspect_publisher.cc");
        if !file_to_mod.exists() {
            return Err(anyhow!(
                "Source file to modify does not exist: {:?}",
                file_to_mod
            ));
        }

        let content = fs::read_to_string(&file_to_mod)
            .with_context(|| format!("Failed to read {:?}", file_to_mod))?;

        let target_str = "numeric_properties.RecordInt(\"int\", -1);";
        let replacement_str = "numeric_properties.RecordInt(\"int\", -2);";

        if !content.contains(target_str) {
            return Err(anyhow!(
                "Target string not found in source file: {}",
                target_str
            ));
        }
        let new_content = content.replace(target_str, replacement_str);
        fs::write(&file_to_mod, new_content)
            .with_context(|| format!("Failed to write modified content to {:?}", file_to_mod))?;

        // Wait a bit to ensure filesystem timestamp resolution doesn't merge compilation time with previous
        // (POSIX timestamps can have 1s resolution on some filesystems, though usually sub-second on modern ones)
        std::thread::sleep(Duration::from_secs(1));

        // 7. Verify build compiles change
        println!("Building in workspace with changes (should compile)...");
        run_command(
            ws_fx_cmd,
            &["build", target_label],
            &worktree_info.workspace_path,
            &[],
        )
        .context("Failed to build in workspace with changes")?;

        let t3 = get_modify_time(&ws_obj_file_path)?;
        if t3 <= t2 {
            return Err(anyhow!(
                "Build in workspace with changes did not compile the file! Object file was not updated: t2={:?}, t3={:?}",
                t2,
                t3
            ));
        }
        println!(
            "Workspace build compiled changes successfully (confirmed by timestamp: t2={:?}, t3={:?}).",
            t2,
            t3
        );

        // Restore file just to be clean
        fs::write(&file_to_mod, content)
            .context("Failed to restore source file to original content")?;

        // If we reused the outdir, compile it again to restore the .o file to original state
        if !should_delete_outdir {
            println!("Restoring build cache in the reused outdir...");
            run_command(
                ws_fx_cmd,
                &["build", target_label],
                &worktree_info.workspace_path,
                &[],
            )
            .context("Failed to rebuild after restoring source file")?;

            let t4 = get_modify_time(&ws_obj_file_path)?;
            println!("Build cache restored. Object file modify time: {:?}", t4);
        }

        Ok(())
    })();

    // 8. Cleanup (Always runs)
    println!("Cleaning up worktree and outdir...");
    if allocated {
        if let Err(e) = free_worktree_by_id(&test_config, &allocated_worktree_id) {
            log::error!("Failed to free worktree during cleanup: {:?}", e);
        }
    }

    // Also restore build.ninja in base repo (must do after outdir is moved back by free_worktree)
    if !should_delete_outdir {
        println!("Restoring build.ninja in the reused outdir...");
        if let Err(e) = run_command(
            fx_cmd,
            &["--dir", outdir_path.to_str().unwrap(), "gen"],
            &config.fuchsia_dir,
            &[],
        ) {
            log::error!("Failed to run fx gen in base repo to restore build.ninja: {:?}", e);
        }
    }

    if should_delete_outdir {
        delete_outdir(&test_config, &outdir_id).context("Failed to delete outdir")?;
    } else {
        println!("Skipping outdir deletion (reused outdir)");
    }

    if test_res.is_ok() {
        println!("Self-test completed successfully!");
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
