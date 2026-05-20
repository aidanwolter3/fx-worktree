use crate::config::Config;
use crate::utils::{find_worktrees, run_command};
use anyhow::{Context, Result, anyhow};
use std::fs;
use std::path::Path;

pub fn remove_environment(config: &Config, id: &str, quiet: bool) -> Result<()> {
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

    // 2. Scan and restore .git files to regular files so git worktree remove can validate them
    let worktrees = find_worktrees(&env_path)?;
    for worktree_path in &worktrees {
        if let Err(e) = restore_git_file(worktree_path) {
            log::warn!(
                "Failed to restore .git file at {:?}: {:?}",
                worktree_path,
                e
            );
        }
    }

    // 3. Remove git worktrees in reverse depth order
    for worktree_path in &worktrees {
        if let Err(e) = remove_worktree(config, &env_path, worktree_path) {
            log::warn!(
                "Failed to remove git worktree at {:?}: {:?}",
                worktree_path,
                e
            );
        }
    }

    // 4. Delete directory from disk
    if env_path.exists() {
        fs::remove_dir_all(&env_path)
            .with_context(|| format!("Failed to delete environment directory {:?}", env_path))?;
    }

    Ok(())
}

fn remove_worktree(config: &Config, env_root: &Path, worktree_path: &Path) -> Result<()> {
    let rel_path = worktree_path
        .strip_prefix(env_root)
        .context("Failed to strip prefix from worktree path")?;

    let base_repo_path = config.fuchsia_dir.join(rel_path);

    if !base_repo_path.exists() {
        return Err(anyhow!(
            "Base repository {:?} does not exist",
            base_repo_path
        ));
    }

    log::info!("Removing git worktree at {:?}", worktree_path);

    run_command(
        "git",
        &[
            "worktree",
            "remove",
            "--force",
            worktree_path.to_str().unwrap(),
        ],
        &base_repo_path,
        &[],
    )
    .with_context(|| format!("Failed to remove git worktree at {:?}", worktree_path))?;

    Ok(())
}

fn restore_git_file(repo_path: &Path) -> Result<()> {
    let git_file_path = repo_path.join(".git");
    if git_file_path.exists() {
        let metadata = fs::symlink_metadata(&git_file_path)
            .with_context(|| format!("Failed to get metadata for {:?}", git_file_path))?;
        if metadata.file_type().is_symlink() {
            let gitdir_path = fs::read_link(&git_file_path)
                .with_context(|| format!("Failed to read link {:?}", git_file_path))?;
            fs::remove_file(&git_file_path)
                .with_context(|| format!("Failed to delete symlink {:?}", git_file_path))?;
            fs::write(
                &git_file_path,
                format!("gitdir: {}\n", gitdir_path.to_string_lossy()),
            )
            .with_context(|| format!("Failed to write .git file at {:?}", git_file_path))?;
        }
    }
    Ok(())
}
