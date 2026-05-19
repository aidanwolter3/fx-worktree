use anyhow::{Context, Result, anyhow};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::utils::run_command;
use crate::worktree::WorktreeInfo;

pub fn free_worktree_by_id(config: &Config, worktree_id: &str) -> Result<()> {
    // The worktree_id is <config>_out_<uuid>.
    // The lease file is in config.leases_dir() / <config>_<uuid>.lease
    // We can convert worktree_id to lease_file_name by replacing "_out_" with "_".
    let lease_file_name = worktree_id.replace("_out_", "_") + ".lease";
    let lease_file_path = config.leases_dir().join(lease_file_name);

    if !lease_file_path.exists() {
        return Err(anyhow!(
            "Worktree lease file {:?} does not exist",
            lease_file_path
        ));
    }

    let worktree_json = fs::read_to_string(&lease_file_path)
        .with_context(|| format!("Failed to read worktree file {:?}", lease_file_path))?;
    let worktree_info: WorktreeInfo =
        serde_json::from_str(&worktree_json).context("Failed to parse worktree JSON")?;

    free_worktree_internal(config, &worktree_info)?;

    // Delete lease file
    fs::remove_file(&lease_file_path)
        .with_context(|| format!("Failed to delete lease file {:?}", lease_file_path))?;
    log::info!("Deleted lease file {:?}", lease_file_path);

    Ok(())
}

pub fn free_worktree_internal(config: &Config, worktree_info: &WorktreeInfo) -> Result<()> {
    log::info!("Freeing worktree {}", worktree_info.worktree_id);

    // Move outdir back to pool if it exists in workspace (before git worktree remove deletes it)
    let workspace_outdir = worktree_info.workspace_path.join("out/default");
    if workspace_outdir.exists() {
        log::info!("Moving outdir back to pool: {:?}", worktree_info.outdir_path);
        if worktree_info.outdir_path.exists() {
            log::warn!("Destination pool outdir {:?} already exists (likely recreated). Deleting it to overwrite with workspace outdir.", worktree_info.outdir_path);
            fs::remove_dir_all(&worktree_info.outdir_path)
                .with_context(|| format!("Failed to delete existing pool outdir {:?}", worktree_info.outdir_path))?;
        }
        if let Some(parent) = worktree_info.outdir_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory {:?}", parent))?;
        }
        fs::rename(&workspace_outdir, &worktree_info.outdir_path)
            .with_context(|| format!("Failed to move outdir back from {:?} to {:?}", workspace_outdir, worktree_info.outdir_path))?;
    }

    // 2. Remove Git Worktrees
    if worktree_info.workspace_path.exists() {
        let worktrees = find_worktrees(&worktree_info.workspace_path)?;
        for worktree_path in worktrees {
            if let Err(e) = restore_git_file(&worktree_path) {
                log::error!("Failed to restore .git file at {:?}: {:?}", worktree_path, e);
            }
            if let Err(e) = remove_worktree(config, &worktree_info.workspace_path, &worktree_path) {
                log::error!("Failed to remove worktree at {:?}: {:?}", worktree_path, e);
                // Continue cleaning up others
            }
        }

        // 3. Destroy Workspace Remnants
        if worktree_info.workspace_path.exists() {
            log::info!(
                "Deleting workspace directory {:?}",
                worktree_info.workspace_path
            );
            fs::remove_dir_all(&worktree_info.workspace_path).with_context(|| {
                format!(
                    "Failed to delete workspace dir {:?}",
                    worktree_info.workspace_path
                )
            })?;
        } else {
            log::info!(
                "Workspace directory {:?} was already removed (likely by git worktree remove)",
                worktree_info.workspace_path
            );
        }
    } else {
        log::warn!(
            "Workspace directory {:?} does not exist, skipping worktree removal",
            worktree_info.workspace_path
        );
    }

    // 4. Scrub Outdir
    if worktree_info.outdir_path.exists() {
        let args_gn = worktree_info.outdir_path.join("args.gn");
        let args_gn_ref = worktree_info.outdir_path.join("args.gn.ref");

        if args_gn_ref.exists() {
            log::info!("Restoring args.gn from args.gn.ref");
            fs::copy(&args_gn_ref, &args_gn)
                .with_context(|| format!("Failed to restore {:?} to {:?}", args_gn_ref, args_gn))?;
        } else {
            log::warn!(
                "args.gn.ref not found in outdir {:?}",
                worktree_info.outdir_path
            );
        }
    } else {
        log::warn!(
            "Outdir {:?} does not exist, skipping scrub",
            worktree_info.outdir_path
        );
    }

    Ok(())
}

fn find_worktrees(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut worktrees = Vec::new();
    find_worktrees_recursive(dir, &mut worktrees)?;
    // Reverse to ensure children are removed before parents
    worktrees.reverse();
    Ok(worktrees)
}

fn find_worktrees_recursive(dir: &Path, worktrees: &mut Vec<PathBuf>) -> Result<()> {
    if dir.is_dir() {
        let git_file = dir.join(".git");
        if git_file.exists() {
            // It's a worktree (or git repo)
            worktrees.push(dir.to_path_buf());
            // Do not return early, continue to find nested repos
        }

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                let path = entry.path();
                // Skip prebuilt and .jiri_root to avoid searching them (optimization)
                if path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|name| {
                        name == ".git"
                            || name == "prebuilt"
                            || name == ".jiri_root"
                            || name == "out"
                    })
                {
                    continue;
                }
                find_worktrees_recursive(&path, worktrees)?;
            }
        }
    }
    Ok(())
}

fn remove_worktree(config: &Config, workspace_root: &Path, worktree_path: &Path) -> Result<()> {
    let rel_path = worktree_path
        .strip_prefix(workspace_root)
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
            fs::write(&git_file_path, format!("gitdir: {}\n", gitdir_path.to_string_lossy()))
                .with_context(|| format!("Failed to write .git file at {:?}", git_file_path))?;
            log::debug!("Restored .git file from symlink for {:?}", repo_path);
        }
    }
    Ok(())
}
