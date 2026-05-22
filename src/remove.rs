use crate::config::Config;
use crate::utils::run_command;
use anyhow::{anyhow, Context, Result};
use std::fs;

pub fn remove_environment(config: &Config, id: &str, force: bool, quiet: bool) -> Result<()> {
    if std::path::Path::new(id).components().count() > 1 {
        return Err(anyhow!("Invalid worktree ID: {}", id));
    }

    // 1. Verify the environment is not leased
    let leases_dir = config.leases_dir();
    if leases_dir.exists() {
        let suffix = format!("_{}.lease", id.split('_').next_back().unwrap_or(id));
        for entry in fs::read_dir(&leases_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("lease") {
                if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
                    if file_name.ends_with(&suffix) {
                        return Err(anyhow!(
                            "Cannot remove worktree {} because it is currently in use (leased).",
                            id
                        ));
                    }
                }
            }
        }
    }

    let env_path = config.environments_dir().join(id);
    if !env_path.exists() {
        return Err(anyhow!(
            "Environment {} does not exist at {:?}",
            id,
            env_path
        ));
    }

    if !quiet {
        eprintln!("Deleting environment {}...", id);
    }

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
        fs::remove_dir_all(&env_path)
            .with_context(|| format!("Failed to delete environment directory {:?}", env_path))?;
    }

    Ok(())
}
