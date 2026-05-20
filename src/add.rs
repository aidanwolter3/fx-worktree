use crate::config::Config;
use crate::utils::run_command;
use anyhow::{Context, Result, anyhow};
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use uuid::Uuid;

#[derive(serde::Deserialize, Debug, Clone)]
struct JiriProject {
    name: String,
    path: String,
    revision: String,
}

pub fn add_environment(config: &Config, config_name: &str, quiet: bool) -> Result<String> {
    let uuid = Uuid::new_v4().to_string();
    let env_id = format!("{}_{}", config_name, &uuid[0..8]);
    let env_path = config.environments_dir().join(&env_id);

    if !quiet {
        eprintln!("Adding worktree {}...", env_id);
    }

    // 1. Create directory structure
    fs::create_dir_all(&env_path)
        .with_context(|| format!("Failed to create worktree directory {:?}", env_path))?;

    // Implement cleanup helper in case creation fails in the middle
    let cleanup = || {
        log::warn!("Environment creation failed, cleaning up {:?}", env_path);
        let _ = fs::remove_dir_all(&env_path);
    };

    if let Err(e) = provision_workspace(config, &env_path, quiet) {
        cleanup();
        return Err(e);
    }

    // Run sync to get prebuilts and ensure correct revisions (required for fx set)
    if let Err(e) = crate::sync::sync_environment(config, &env_id, &env_path, quiet) {
        cleanup();
        return Err(e);
    }

    // 2. Create physical out/default
    let out_dir = env_path.join("out/default");
    fs::create_dir_all(&out_dir)
        .with_context(|| format!("Failed to create build directory {:?}", out_dir))?;

    // 3. Write .fx-build-dir
    let fx_build_dir_file = env_path.join(".fx-build-dir");
    fs::write(&fx_build_dir_file, "out/default\n")
        .with_context(|| format!("Failed to write {:?}", fx_build_dir_file))?;

    // 4. Run fx set in the workspace
    if !quiet {
        eprintln!("Running fx set {}...", config_name);
    }
    let fx_bin = env_path.join("scripts/fx");
    let fx_cmd = if fx_bin.exists() {
        fx_bin.to_str().unwrap()
    } else {
        "fx"
    };

    run_command(
        fx_cmd,
        &["--dir", "out/default", "set", config_name],
        &env_path,
        &[],
    )
    .context("Failed to run fx set in environment")?;

    // 5. Snapshot args.gn
    let args_gn = out_dir.join("args.gn");
    let args_gn_ref = out_dir.join("args.gn.ref");
    if args_gn.exists() {
        fs::copy(&args_gn, &args_gn_ref)
            .with_context(|| format!("Failed to snapshot {:?} to {:?}", args_gn, args_gn_ref))?;
        log::info!("Created args.gn.ref");
    } else {
        log::warn!("args.gn not found after fx set. This might happen if fx set was mocked.");
    }

    // 6. Write completion marker
    fs::write(env_path.join(".fx-worktree-completed"), "")
        .with_context(|| format!("Failed to write completion marker"))?;

    config.record_last_active(&env_path)?;

    Ok(env_id)
}

fn provision_workspace(config: &Config, workspace_path: &Path, quiet: bool) -> Result<()> {
    // 1. Parse Jiri State from base repository
    if !quiet {
        eprintln!("Querying Fuchsia project structure...");
    }
    let temp_jiri_json = std::env::temp_dir().join(format!("jiri_{}.json", Uuid::new_v4()));
    let jiri_bin = config.fuchsia_dir.join(".jiri_root/bin/jiri");
    let jiri_cmd = if jiri_bin.exists() {
        jiri_bin.to_str().unwrap()
    } else {
        "jiri"
    };

    run_command(
        jiri_cmd,
        &["project", "-json-output", temp_jiri_json.to_str().unwrap()],
        &config.fuchsia_dir,
        &[],
    )
    .context("Failed to run jiri project in base repo")?;

    let jiri_json = fs::read_to_string(&temp_jiri_json).context("Failed to read jiri json")?;
    let projects: Vec<JiriProject> =
        serde_json::from_str(&jiri_json).context("Failed to parse Jiri JSON")?;
    let _ = fs::remove_file(&temp_jiri_json);

    let mut root_project = None;
    let mut sub_projects = Vec::new();

    for project in &projects {
        let rel_path = Path::new(&project.path)
            .strip_prefix(&config.fuchsia_dir)
            .context("Failed to strip prefix from project path")?;
        if rel_path.as_os_str().is_empty() {
            root_project = Some(project);
        } else {
            sub_projects.push(project);
        }
    }

    // 2. Provision Root Git Worktree
    if let Some(root) = root_project {
        log::info!("Provisioning root git worktree at {:?}", workspace_path);
        run_command(
            "git",
            &[
                "worktree",
                "add",
                "-f",
                "--detach",
                workspace_path.to_str().unwrap(),
                &root.revision,
            ],
            Path::new(&root.path),
            &[],
        )
        .with_context(|| "Failed to add root git worktree")?;
        crate::utils::convert_gitdir_to_symlink(workspace_path)?;
        let common_git = config.fuchsia_dir.join(".git");
        crate::utils::exclude_from_git(&common_git, ".fx-worktree-completed")?;
        crate::utils::exclude_from_git(&common_git, ".fx-build-dir")?;
        crate::utils::exclude_from_git(&common_git, ".fx-root")?;
    } else {
        return Err(anyhow!("Root project not found in Jiri projects"));
    }

    // Copy .jiri_manifest if it exists in base checkout
    let base_manifest = config.fuchsia_dir.join(".jiri_manifest");
    let workspace_manifest = workspace_path.join(".jiri_manifest");
    if base_manifest.exists() {
        log::info!("Copying .jiri_manifest to workspace...");
        fs::copy(&base_manifest, &workspace_manifest).with_context(|| {
            format!("Failed to copy .jiri_manifest to {:?}", workspace_manifest)
        })?;
    }

    // Group sub-projects by path depth
    let mut groups: BTreeMap<usize, Vec<&JiriProject>> = BTreeMap::new();
    for project in &sub_projects {
        let rel_path = Path::new(&project.path)
            .strip_prefix(&config.fuchsia_dir)
            .context("Failed to strip prefix from project path")?;
        let depth = rel_path.components().count();
        groups.entry(depth).or_default().push(project);
    }

    // 3. Provision sub-projects group by group in parallel
    for (depth, group) in groups {
        log::info!(
            "Provisioning sub-projects at depth {} (count: {})...",
            depth,
            group.len()
        );
        if !quiet {
            eprintln!("Provisioning Git worktrees at depth {}...", depth);
        }
        group.par_iter().try_for_each(|project| -> Result<()> {
            let rel_path = Path::new(&project.path)
                .strip_prefix(&config.fuchsia_dir)
                .context("Failed to strip prefix from project path")?;

            let target_path = workspace_path.join(rel_path);

            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create directory {:?}", parent))?;
            }

            run_command(
                "git",
                &[
                    "worktree",
                    "add",
                    "-f",
                    "--detach",
                    target_path.to_str().unwrap(),
                    &project.revision,
                ],
                Path::new(&project.path),
                &[],
            )
            .with_context(|| format!("Failed to add git worktree for {}", project.name))?;
            crate::utils::convert_gitdir_to_symlink(&target_path)?;

            Ok(())
        })?;
    }

    // 4. Isolate Toolchains (Symlink prebuilts & copy generated files)
    if !quiet {
        eprintln!("Isolating toolchains...");
    }

    let workspace_prebuilt = workspace_path.join("prebuilt");
    fs::create_dir_all(&workspace_prebuilt).with_context(|| {
        format!(
            "Failed to create prebuilt directory {:?}",
            workspace_prebuilt
        )
    })?;

    // Copy Jiri generated files
    crate::utils::copy_toolchain_metadata(config, workspace_path)?;

    Ok(())
}
