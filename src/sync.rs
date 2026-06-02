use crate::config::Config;
use crate::utils::{copy_toolchain_metadata, run_command};
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

pub fn sync_environment_by_id(config: &Config, id: &str, quiet: bool) -> Result<()> {
    let path = crate::locate::locate_path(config, Some(id.to_string()))?;
    sync_environment(config, id, &path, quiet)
}

pub fn sync_environment(
    config: &Config,
    env_id: &str,
    workspace_path: &Path,
    quiet: bool,
) -> Result<()> {
    let workspace_path_buf = workspace_path
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize workspace path {:?}", workspace_path))?;
    let workspace_path = &workspace_path_buf;

    if !quiet {
        eprintln!("Syncing environment {}...", env_id);
    }

    // Copy/restore toolchain metadata (must be before sync for hooks)
    copy_toolchain_metadata(config, workspace_path)?;

    // Call 'jiri worktree sync'
    let jiri_bin = config.fuchsia_dir.join(".jiri_root/bin/jiri");
    let jiri_cmd = if jiri_bin.exists() {
        jiri_bin.to_str().unwrap()
    } else {
        "jiri"
    };

    run_command(jiri_cmd, &["worktree", "sync"], workspace_path, &[])
        .context("Failed to run jiri worktree sync")?;

    // Inject locally-built GN if it exists
    if let Err(e) = inject_local_gn(workspace_path) {
        log::warn!("Failed to inject local GN: {:?}", e);
        if !quiet {
            eprintln!("⚠ Warning: Failed to inject local GN: {:?}", e);
        }
    }

    // Inject locally-built shac if it exists
    if let Err(e) = inject_local_shac(workspace_path) {
        log::warn!("Failed to inject local shac: {:?}", e);
        if !quiet {
            eprintln!("⚠ Warning: Failed to inject local shac: {:?}", e);
        }
    }

    Ok(())
}

fn inject_local_gn(workspace_path: &Path) -> Result<()> {
    let home = std::env::var("HOME").context("Failed to get HOME environment variable")?;
    let local_gn = Path::new(&home).join("src/gn/out/gn");
    if !local_gn.exists() {
        return Ok(());
    }

    log::info!("Found locally-built GN at {:?}", local_gn);

    // Find the gn platform directory inside prebuilt/third_party/gn
    let gn_parent = workspace_path.join("prebuilt/third_party/gn");
    if !gn_parent.exists() {
        return Ok(());
    }

    let mut platform_dir = None;
    for entry in fs::read_dir(&gn_parent)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("linux-") || name.starts_with("mac-") {
                    platform_dir = Some(path);
                    break;
                }
            }
        }
    }

    let gn_dir = match platform_dir {
        Some(d) => d,
        None => return Ok(()),
    };

    let meta = fs::symlink_metadata(&gn_dir)?;
    if !meta.file_type().is_symlink() {
        // If it's already a real directory, maybe we already injected it or it's not managed by jiri.
        // Let's just override the "gn" file/symlink inside it.
        let target_gn = gn_dir.join("gn");
        if target_gn.is_symlink() || target_gn.exists() {
            let _ = fs::remove_file(&target_gn);
        }
        std::os::unix::fs::symlink(&local_gn, &target_gn)?;
        log::info!("Injected local GN symlink to {:?}", target_gn);
        return Ok(());
    }

    let target = fs::read_link(&gn_dir)?;
    let target_abs = if target.is_absolute() {
        target
    } else {
        gn_dir.parent().unwrap().join(target)
    };

    let target_abs = fs::canonicalize(target_abs)?;
    log::info!("Replacing symlink {:?} with real directory, linking other files from {:?}", gn_dir, target_abs);

    fs::remove_file(&gn_dir)?;
    fs::create_dir_all(&gn_dir)?;

    let mut gn_injected = false;
    for entry in fs::read_dir(&target_abs)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let dest = gn_dir.join(&name);
        if name_str == "gn" {
            std::os::unix::fs::symlink(&local_gn, &dest)?;
            gn_injected = true;
        } else {
            std::os::unix::fs::symlink(entry.path(), &dest)?;
        }
    }

    if !gn_injected {
        std::os::unix::fs::symlink(&local_gn, gn_dir.join("gn"))?;
    }

    log::info!("Local GN successfully injected in {:?}", gn_dir);
    Ok(())
}

fn inject_local_shac(workspace_path: &Path) -> Result<()> {
    let home = std::env::var("HOME").context("Failed to get HOME environment variable")?;
    let local_shac = Path::new(&home).join("src/shac/shac");
    if !local_shac.exists() {
        return Ok(());
    }

    log::info!("Found locally-built shac at {:?}", local_shac);

    let shac_dir = workspace_path.join("prebuilt/tools/shac");
    if !shac_dir.exists() {
        return Ok(());
    }

    let meta = fs::symlink_metadata(&shac_dir)?;
    if !meta.file_type().is_symlink() {
        let target_shac = shac_dir.join("shac");
        if target_shac.is_symlink() || target_shac.exists() {
            let _ = fs::remove_file(&target_shac);
        }
        std::os::unix::fs::symlink(&local_shac, &target_shac)?;
        log::info!("Injected local shac symlink to {:?}", target_shac);
        return Ok(());
    }

    let target = fs::read_link(&shac_dir)?;
    let target_abs = if target.is_absolute() {
        target
    } else {
        shac_dir.parent().unwrap().join(target)
    };

    let target_abs = fs::canonicalize(target_abs)?;
    log::info!("Replacing symlink {:?} with real directory, linking other files from {:?}", shac_dir, target_abs);

    fs::remove_file(&shac_dir)?;
    fs::create_dir_all(&shac_dir)?;

    let mut shac_injected = false;
    for entry in fs::read_dir(&target_abs)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let dest = shac_dir.join(&name);
        if name_str == "shac" {
            std::os::unix::fs::symlink(&local_shac, &dest)?;
            shac_injected = true;
        } else {
            std::os::unix::fs::symlink(entry.path(), &dest)?;
        }
    }

    if !shac_injected {
        std::os::unix::fs::symlink(&local_shac, shac_dir.join("shac"))?;
    }

    log::info!("Local shac successfully injected in {:?}", shac_dir);
    Ok(())
}
