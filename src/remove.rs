use crate::config::Config;
use crate::utils::run_command;
use anyhow::{anyhow, Context, Result};
use std::fs;

pub fn remove_environment(config: &Config, id: &str, force: bool, quiet: bool) -> Result<()> {
    if std::path::Path::new(id).components().count() > 1 {
        return Err(anyhow!("Invalid worktree ID: {}", id));
    }

    let resolved_path = match crate::locate::locate_path(config, Some(id.to_string())) {
        Ok(path) => Some(path),
        Err(e) => {
            if force {
                None
            } else {
                return Err(e);
            }
        }
    };

    let env_path = match &resolved_path {
        Some(path) => path.clone(),
        None => config.environments_dir().join(id),
    };

    let resolved_id = match &resolved_path {
        Some(path) => path.file_name().unwrap().to_str().unwrap().to_string(),
        None => id.to_string(),
    };

    // 1. Find and handle lease
    let leases_dir = config.leases_dir();
    let mut lease_file = None;
    if leases_dir.exists() {
        for entry in fs::read_dir(&leases_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("lease") {
                if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
                    let lease_id = file_name.strip_suffix(".lease").unwrap_or(file_name);
                    if lease_id == resolved_id
                        || lease_id.starts_with(&resolved_id)
                        || lease_id.ends_with(&format!("_{}", resolved_id))
                    {
                        lease_file = Some(path);
                        break;
                    }
                }
            }
        }
    }

    if lease_file.is_some() && !force {
        return Err(anyhow!(
            "Cannot remove worktree {} because it is currently in use (leased).",
            resolved_id
        ));
    }

    if !force && !env_path.exists() {
        return Err(anyhow!(
            "Environment {} does not exist at {:?}",
            resolved_id,
            env_path
        ));
    }

    if !quiet {
        eprintln!("Deleting environment {}...", resolved_id);
    }

    if force {
        // 1. Delete directory from disk
        if env_path.exists() {
            fs::remove_dir_all(&env_path).with_context(|| {
                format!("Failed to delete environment directory {:?}", env_path)
            })?;
        }

        // Find jiri bin
        let jiri_bin = config.fuchsia_dir.join(".jiri_root/bin/jiri");
        let jiri_cmd = if jiri_bin.exists() {
            jiri_bin.to_str().unwrap()
        } else {
            "jiri"
        };

        // 2. Run git worktree prune in root repo
        if !quiet {
            eprintln!("Pruning git worktrees in root repo...");
        }
        run_command("git", &["worktree", "prune"], &config.fuchsia_dir, &[])
            .context("Failed to run git worktree prune in root repo")?;

        // 3. Run jiri runp git worktree prune in all subprojects
        if !quiet {
            eprintln!("Pruning git worktrees in all subprojects...");
        }
        run_command(
            jiri_cmd,
            &["runp", "git", "worktree", "prune"],
            &config.fuchsia_dir,
            &[],
        )
        .context("Failed to run jiri runp git worktree prune")?;

        // 4. Delete lease file
        if let Some(path) = lease_file {
            if !quiet {
                eprintln!("Deleting lease file...");
            }
            fs::remove_file(&path)
                .with_context(|| format!("Failed to delete lease file {:?}", path))?;
        }
    } else {
        // Normal removal logic

        // 2. Call 'jiri worktree remove'
        let jiri_bin = config.fuchsia_dir.join(".jiri_root/bin/jiri");
        let jiri_cmd = if jiri_bin.exists() {
            jiri_bin.to_str().unwrap()
        } else {
            "jiri"
        };

        run_command(
            jiri_cmd,
            &["worktree", "remove", env_path.to_str().unwrap()],
            &config.fuchsia_dir,
            &[],
        )
        .context("Failed to run jiri worktree remove")?;

        // 3. Delete directory from disk
        if env_path.exists() {
            fs::remove_dir_all(&env_path).with_context(|| {
                format!("Failed to delete environment directory {:?}", env_path)
            })?;
        }
    }

    Ok(())
}
