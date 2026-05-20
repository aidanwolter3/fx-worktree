use crate::config::Config;
use crate::utils::{copy_toolchain_metadata, run_command, clean_worktree, find_worktrees, get_file_mtime, set_file_mtime};
use crate::environment::EnvironmentInfo;
use anyhow::{Context, Result, anyhow};
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use uuid::Uuid;

#[derive(serde::Deserialize, Debug, Clone)]
struct JiriProject {
    path: String,
    revision: String,
}

pub fn allocate_environment(
    config: &Config,
    config_name: &str,
    agent_id: &str,
    quiet: bool,
) -> Result<EnvironmentInfo> {
    let mut acquired_lease = None;
    let mut env_path = PathBuf::new();
    let mut env_id = String::new();

    // 1. Find a free environment of the config type in the pool
    let envs_dir = config.environments_dir();
    if !envs_dir.exists() {
        return Err(anyhow!(
            "No environments found. Create one first using 'fxenv create {}'",
            config_name
        ));
    }

    let entries = fs::read_dir(&envs_dir)
        .with_context(|| format!("Failed to read environments directory {:?}", envs_dir))?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
                // Environment folder name is: <config>_<uuid>
                // We check if it starts with config_name followed by underscore
                let prefix = format!("{}_", config_name);
                if dir_name.starts_with(&prefix) {
                    let id = dir_name;
                    let is_complete = path.join(".fxenv-completed").exists();
                    if !is_complete {
                        log::warn!("Skipping incomplete environment {:?}", path);
                        continue;
                    }

                    let lease_file_name = format!("{}.lease", id);
                    let lease_file_path = config.leases_dir().join(&lease_file_name);

                    // Attempt to create lease file atomically
                    match fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&lease_file_path)
                    {
                        Ok(_) => {
                            log::info!("Acquired lease lock: {:?}", lease_file_path);
                            acquired_lease = Some(lease_file_path);
                            env_path = path.clone();
                            env_id = id.to_string();
                            break;
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                            // Lease busy, try next
                            continue;
                        }
                        Err(e) => {
                            return Err(e).context(format!(
                                "Failed to create lease file {:?}",
                                lease_file_path
                            ));
                        }
                    }
                }
            }
        }
    }

    if acquired_lease.is_none() {
        return Err(anyhow!("No free environments available for config {}.", config_name));
    }

    let lease_file_path = acquired_lease.unwrap();
    let current_time = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let env_info = EnvironmentInfo {
        environment_id: env_id.clone(),
        agent_id: agent_id.to_string(),
        config: config_name.to_string(),
        pid: std::process::id(),
        timestamp_sec: current_time,
        path: env_path.clone(),
    };

    // Write lease info
    let env_json = serde_json::to_string(&env_info).context("Failed to serialize EnvironmentInfo")?;
    fs::write(&lease_file_path, env_json).context("Failed to write lease JSON")?;

    // Rollback helper on failure
    let rollback = || {
        log::warn!("Allocation failed, releasing lease {}", env_id);
        let _ = fs::remove_file(&lease_file_path);
    };

    // 2. Reuse the environment (clean and checkout target revisions)
    if let Err(e) = reuse_environment(config, &env_info, quiet) {
        rollback();
        return Err(e);
    }

    config.record_last_created(&env_info.path)?;

    Ok(env_info)
}

fn reuse_environment(config: &Config, env_info: &EnvironmentInfo, quiet: bool) -> Result<()> {
    let workspace_path = &env_info.path;

    // Record index mtimes unconditionally at the very start, before any git commands run
    let worktrees = find_worktrees(workspace_path)?;
    let mut index_mtimes = Vec::new();
    for wt in &worktrees {
        let index_path = wt.join(".git/index");
        if index_path.exists() {
            if let Ok(mtime) = get_file_mtime(&index_path) {
                index_mtimes.push((index_path, mtime));
            }
        }
    }

    if !quiet {
        println!("Reusing existing environment {}...", env_info.environment_id);
    }

    // 1. Query target Jiri state from base repo
    if !quiet {
        println!("Querying Fuchsia project structure...");
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
    let projects: Vec<JiriProject> = serde_json::from_str(&jiri_json).context("Failed to parse Jiri JSON")?;
    let _ = fs::remove_file(&temp_jiri_json);

    let mut root_project = None;
    let mut sub_projects = Vec::new();
    for project in &projects {
        let rel_path = Path::new(&project.path)
            .strip_prefix(&config.fuchsia_dir)
            .context("Failed to strip prefix")?;
        if rel_path.as_os_str().is_empty() {
            root_project = Some(project);
        } else {
            sub_projects.push(project);
        }
    }

    // Check if it is a no-op (same revision and clean)
    let is_noop = (|| -> Result<bool> {
        if let Some(root) = &root_project {
            // Check revision
            let current_head = run_command("git", &["rev-parse", "HEAD"], workspace_path, &[])?;
            let current_head_sha = String::from_utf8_lossy(&current_head.stdout).trim().to_string();
            if current_head_sha != root.revision {
                return Ok(false);
            }

            // Check clean
            let status = run_command("git", &["status", "--porcelain", "-uno"], workspace_path, &[])?;
            if !status.stdout.is_empty() {
                return Ok(false);
            }

            Ok(true)
        } else {
            Ok(false)
        }
    })().unwrap_or(false);




    // 2. Clean and checkout root project (exclude out/ and markers)
    if let Some(root) = root_project {
        if !quiet {
            println!("Updating Git worktrees to target revisions...");
        }
        clean_worktree(workspace_path, true)?;
        run_command("git", &["checkout", &root.revision], workspace_path, &[])?;
        crate::utils::convert_gitdir_to_symlink(workspace_path)?;
    } else {
        return Err(anyhow!("Root project not found in Jiri projects"));
    }

    // 3. Clean and checkout sub-projects in parallel (exclude out/ if any, though subprojects usually don't have out/)
    sub_projects.par_iter().try_for_each(|project| -> Result<()> {
        let rel_path = Path::new(&project.path)
            .strip_prefix(&config.fuchsia_dir)
            .context("Failed to strip prefix")?;
        let target_path = workspace_path.join(rel_path);

        if target_path.exists() {
            clean_worktree(&target_path, false)?;
            run_command("git", &["checkout", &project.revision], &target_path, &[])?;
            crate::utils::convert_gitdir_to_symlink(&target_path)?;
        } else {
            // New sub-project added to manifest: clone it fresh!
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent)?;
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
            )?;
            // Convert to symlink
            crate::utils::convert_gitdir_to_symlink(&target_path)?;
        }
        Ok(())
    })?;

    // Copy/restore toolchain metadata files deleted by git clean
    copy_toolchain_metadata(config, workspace_path)?;

    // Resolve and download prebuilts (individual symlinking)
    resolve_and_download_prebuilts(config, workspace_path, quiet)?;

    // Restore index mtimes if no-op
    if is_noop {
        for (path, mtime) in index_mtimes {
            if let Err(e) = set_file_mtime(&path, mtime) {
                log::warn!("Failed to restore index mtime for {:?}: {:?}", path, e);
            }
        }
    }

    Ok(())
}

#[derive(serde::Deserialize, Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JiriPackage {
    pub name: String,
    pub path: String,
    pub version: String,
    pub platforms: Option<Vec<String>>,
}

fn get_cipd_platform() -> &'static str {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "linux-amd64"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "linux-arm64"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "mac-amd64"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "mac-arm64"
    } else if cfg!(target_os = "windows") {
        "windows-amd64"
    } else {
        panic!("Unsupported platform");
    }
}

fn group_packages_by_path(
    packages: Vec<JiriPackage>,
    host_platform: &str,
) -> std::collections::BTreeMap<PathBuf, Vec<JiriPackage>> {
    let mut groups = std::collections::BTreeMap::new();
    for mut pkg in packages {
        if let Some(platforms) = &pkg.platforms {
            if !platforms.contains(&host_platform.to_string()) {
                continue;
            }
        }
        pkg.name = pkg.name.replace("${platform}", host_platform);
        
        let path = PathBuf::from(&pkg.path);
        groups.entry(path).or_insert_with(Vec::new).push(pkg);
    }
    groups
}

pub fn calculate_group_hash(packages: &[JiriPackage]) -> String {
    use std::hash::{Hash, Hasher};
    use std::collections::hash_map::DefaultHasher;
    
    let mut specs = packages
        .iter()
        .map(|pkg| format!("{}:{}", pkg.name, pkg.version))
        .collect::<Vec<String>>();
    specs.sort();
    
    let mut s = DefaultHasher::new();
    specs.hash(&mut s);
    format!("{:016x}", s.finish())
}

fn generate_ensure_file_content(
    groups_to_download: &std::collections::BTreeMap<String, Vec<JiriPackage>>,
) -> String {
    let mut content = String::new();
    for (subdir, pkgs) in groups_to_download {
        content.push_str(&format!("@Subdir {}\n", subdir));
        for pkg in pkgs {
            content.push_str(&format!("{} {}\n", pkg.name, pkg.version));
        }
        content.push_str("\n");
    }
    content
}

fn resolve_and_download_prebuilts(
    config: &Config,
    workspace_path: &Path,
    quiet: bool,
) -> Result<()> {
    // Isolated Prebuilts Logic:
    // Fuchsia checkouts contain hundreds of prebuilt packages (toolchains, SDKs, firmware)
    // which are managed by Jiri/CIPD. Sharing a single `prebuilt/` directory causes workspaces
    // to dirty each other's builds when the parent updates.
    //
    // To isolate them while retaining cache sharing:
    // 1. We query the package list using `jiri package` in the workspace.
    // 2. We group packages by their target path in the workspace. This is critical because
    //    multiple Jiri packages can map to the same directory (e.g., Rust host compiler
    //    and target libraries both install to `prebuilt/third_party/rust/linux-x64`), and
    //    CIPD must merge them in the same directory to write all `.versions/` files correctly.
    // 3. We calculate a deterministic hash of the packages/versions in the group to use
    //    as the cache directory name under `shared-prebuilts/merged/`.
    // 4. We resolve ambiguous tags to concrete instance IDs using `jiri.lock` files.
    // 5. We run `cipd ensure` to download/merge missing groups.
    // 6. We symlink the individual package target paths in the workspace to the shared cache.

    if !quiet {
        println!("Resolving and isolating prebuilts...");
    }

    // Migrate old workspaces: if workspace/prebuilt is a symlink (pointing to parent/prebuilt),
    // convert it to a real directory so we can symlink individual packages inside it.
    let ws_prebuilt = workspace_path.join("prebuilt");
    if ws_prebuilt.is_symlink() {
        log::info!("Converting workspace prebuilt symlink to directory...");
        fs::remove_file(&ws_prebuilt)?;
        fs::create_dir_all(&ws_prebuilt)?;
    } else if !ws_prebuilt.exists() {
        fs::create_dir_all(&ws_prebuilt)?;
    }

    // 1. Run jiri package to get all packages
    if !quiet {
        println!("  Querying package list (running jiri package)...");
    }
    let temp_json = std::env::temp_dir().join(format!("packages_{}.json", Uuid::new_v4()));
    let jiri_bin = config.fuchsia_dir.join(".jiri_root/bin/jiri");
    let jiri_cmd = if jiri_bin.exists() {
        jiri_bin.to_str().unwrap()
    } else {
        "jiri"
    };

    run_command(
        jiri_cmd,
        &["package", "-json-output", temp_json.to_str().unwrap()],
        workspace_path,
        &[],
    )
    .context("Failed to run jiri package in workspace")?;

    let json_content = fs::read_to_string(&temp_json).context("Failed to read packages JSON")?;
    let _ = fs::remove_file(&temp_json);

    let packages: Vec<JiriPackage> = serde_json::from_str(&json_content)
        .context("Failed to parse Jiri packages JSON")?;

    if !quiet {
        println!("  Loading lockfiles...");
    }
    let lock_map = load_lockfiles(workspace_path)?;
    let host_platform = get_cipd_platform();
    let shared_prebuilts_dir = config.fxenv_root.join("shared-prebuilts");
    fs::create_dir_all(&shared_prebuilts_dir)?;
    let marker_dir = config.fxenv_root.join("clamped-markers");
    fs::create_dir_all(&marker_dir)?;

    // 2. Group packages by target path and resolve platforms
    if !quiet {
        println!("  Analyzing packages and checking cache...");
    }
    let grouped_packages = group_packages_by_path(packages, host_platform);

    let mut groups_to_ensure = std::collections::BTreeMap::new();
    let mut missing_subdirs = Vec::new();
    let mut symlinks_to_create = Vec::new();
    let mut bazel_to_copy = Vec::new();

    for (abs_target_path, mut pkgs) in grouped_packages {
        // Resolve versions using lockfile and calculate group hash
        for pkg in &mut pkgs {
            let resolved_version = lock_map
                .get(&(pkg.name.clone(), pkg.version.clone()))
                .cloned()
                .unwrap_or_else(|| {
                    // Downgraded to debug as many packages (with explicit instance IDs in manifests)
                    // are legitimately missing from lockfiles, and fallback to tag is expected.
                    log::debug!("Package {} (version {}) not found in lockfiles, using tag", pkg.name, pkg.version);
                    pkg.version.clone()
                });
            pkg.version = resolved_version;
        }

        let hash = calculate_group_hash(&pkgs);
        
        // Escape target path for cache subdir
        let rel_target_path = abs_target_path
            .strip_prefix(workspace_path)
            .with_context(|| format!("Package path {:?} is not inside workspace_path {:?}", abs_target_path, workspace_path))?;
        
        let escaped_path = rel_target_path.to_str().unwrap().replace('/', "_");
        let cache_subdir = format!("merged/{}/{}", escaped_path, hash);
        let shared_pkg_dir = shared_prebuilts_dir.join(&cache_subdir);

        let ws_target_path = workspace_path.join(rel_target_path);

        // Check if already in cache (directory exists and is not empty)
        let marker_file = marker_dir.join(format!("{}_{}.clamped", escaped_path, hash));
        if !is_dir_not_empty(&shared_pkg_dir) {
            missing_subdirs.push((cache_subdir.clone(), marker_file));
        } else if !marker_file.exists() {
            // Cache hit, but not clamped yet (e.g. from an older run or parent checkout).
            // We clamp it once to migrate it, then write the marker.
            log::info!("Clamping existing cache directory {:?}", shared_pkg_dir);
            crate::utils::clamp_mtimes_to_past(&shared_pkg_dir)?;
            fs::write(&marker_file, "")?;
        }

        let is_bazel = rel_target_path.to_str().unwrap().contains("prebuilt/third_party/bazel/");
        if is_bazel {
            bazel_to_copy.push((ws_target_path, shared_pkg_dir.clone()));
        } else {
            symlinks_to_create.push((ws_target_path, shared_pkg_dir.clone()));
        }

        // We ALWAYS add all packages to groups_to_ensure.
        // CIPD ensure must be run with the FULL list of packages to prevent CIPD
        // from uninstalling/deleting packages that are missing from a partial ensure file.
        groups_to_ensure.insert(cache_subdir, pkgs);
    }

    // 3. Download missing packages in bulk if any
    if !missing_subdirs.is_empty() {
        if !quiet {
            println!("  Downloading missing prebuilt packages (cache miss for {} paths)...", missing_subdirs.len());
        }
        let ensure_content = generate_ensure_file_content(&groups_to_ensure);
        
        let temp_ensure = std::env::temp_dir().join(format!("ensure_{}", Uuid::new_v4()));
        fs::write(&temp_ensure, ensure_content)?;

        let cipd_bin = config.fuchsia_dir.join(".jiri_root/bin/cipd");
        let cipd_cmd = if cipd_bin.exists() {
            cipd_bin.to_str().unwrap()
        } else {
            "cipd"
        };

        run_command(
            cipd_cmd,
            &[
                "ensure",
                "-root",
                shared_prebuilts_dir.to_str().unwrap(),
                "-ensure-file",
                temp_ensure.to_str().unwrap(),
            ],
            workspace_path,
            &[],
        )
        .context("Failed to run cipd ensure")?;

        let _ = fs::remove_file(&temp_ensure);

        // Clamp mtimes of all downloaded files to prevent future mtimes (like Bazel's 2042 date)
        // from constantly dirtying the build.
        for (cache_subdir, marker_file) in &missing_subdirs {
            let pkg_dir = shared_prebuilts_dir.join(cache_subdir);
            crate::utils::clamp_mtimes_to_past(&pkg_dir)?;
            fs::write(marker_file, "")?;
        }
    }

    // 4. Create symlinks in workspace
    if !quiet {
        println!("  Creating package symlinks in workspace...");
    }
    let mut updated_symlinks = std::collections::HashSet::new();
    for (ws_path, cache_path) in symlinks_to_create {
        if let Some(parent) = ws_path.parent() {
            fs::create_dir_all(parent)?;
        }

        if ws_path.exists() || ws_path.is_symlink() {
            if let Ok(target) = fs::read_link(&ws_path) {
                if target == cache_path {
                    continue;
                }
            }
            if ws_path.is_dir() && !ws_path.is_symlink() {
                fs::remove_dir_all(&ws_path)?;
            } else {
                fs::remove_file(&ws_path)?;
            }
        }

        std::os::unix::fs::symlink(&cache_path, &ws_path)
            .with_context(|| format!("Failed to create symlink from {:?} to {:?}", cache_path, ws_path))?;
        updated_symlinks.insert(ws_path.clone());
    }

    // 4b. Copy Bazel packages to workspace (instead of symlinking)
    //
    // WHY WE COPY BAZEL INSTEAD OF SYMLINKING:
    // 1. Bazel's prebuilt package in Fuchsia contains a wrapper script ('bazel') that executes on every
    //    build invocation. This wrapper script explicitly runs `touch` to reset the modification times
    //    of all files in its 'install_base/' directory to a future date (2042-07-29). This is a Bazel
    //    requirement to verify JRE/tools integrity on startup.
    // 2. To prevent these future (2042) timestamps from dirtying the build (since they are newer than
    //    any compiled outputs), the Fuchsia build system uses a depfile path mapper script
    //    ('bazel_source_path_mapper.py') that intercepts Ninja depfiles and filters out any inputs
    //    located inside "prebuilt/third_party/bazel/".
    // 3. This path mapper script resolves all paths using `os.path.realpath()` before checking.
    // 4. If we symlink the Bazel package to the shared cache, `realpath` resolves the path to the
    //    central shared cache path (e.g., `~/.fuchsia-agents/shared-prebuilts/merged/...`), which
    //    does NOT contain the hardcoded "prebuilt/third_party/bazel/" substring.
    // 5. As a result, the path mapper filter fails to ignore them, these 2042 files are added to Ninja's
    //    dependency tree, and Ninja constantly triggers rebuilds because the input (2042) is always
    //    newer than the output (2026).
    //
    // By physically copying the Bazel package into the workspace directory, `realpath` resolves it
    // to `workspace/prebuilt/third_party/bazel/...`, which matches the filter and keeps the build clean.
    // We use a `.fxenv_source_cache` marker file inside the copy to avoid copying it on subsequent
    // allocations if the cache version remains identical.
    for (ws_path, cache_path) in bazel_to_copy {
        let version_marker = ws_path.join(".fxenv_source_cache");
        let needs_copy = if !ws_path.exists() {
            true
        } else if !version_marker.exists() {
            true
        } else {
            let current_source = fs::read_to_string(&version_marker)?;
            current_source != cache_path.to_str().unwrap()
        };

        if needs_copy {
            if !quiet {
                println!("  Copying isolated Bazel package to workspace...");
            }
            if ws_path.exists() {
                if ws_path.is_symlink() {
                    fs::remove_file(&ws_path)?;
                } else {
                    fs::remove_dir_all(&ws_path)?;
                }
            }
            fs::create_dir_all(&ws_path)?;
            crate::utils::copy_dir_all(&cache_path, &ws_path)?;
            crate::utils::clamp_mtimes_to_past(&ws_path)?;
            fs::write(&version_marker, cache_path.to_str().unwrap())?;
        }
    }

    // 5. Run local wheel extraction scripts (normally run by Jiri hooks)
    // We run them manually to avoid Jiri's run-hooks command triggering
    // slow network downloads of other packages in the workspace.
    // We only run them if the source wheel package was updated in this run,
    // or if the extracted directory is missing, to preserve file mtimes
    // and keep builds clean (no-op).
    let pydantic_wheel = workspace_path.join("prebuilt/third_party/pydantic-core-wheel");
    let pydantic_dest = workspace_path.join("prebuilt/third_party/pydantic-core");
    let pydantic_script = workspace_path.join("tools/build/scripts/extract_pydantic_core_wheel.sh");
    if pydantic_script.exists() && (updated_symlinks.contains(&pydantic_wheel) || !pydantic_dest.exists()) {
        if !quiet {
            println!("  Extracting pydantic-core wheel...");
        }
        run_command(pydantic_script.to_str().unwrap(), &[], workspace_path, &[])
            .context("Failed to run extract_pydantic_core_wheel.sh")?;
    }

    let protobuf_wheel = workspace_path.join("prebuilt/third_party/protobuf-py3-wheel");
    let protobuf_dest = workspace_path.join("prebuilt/third_party/protobuf-py3");
    let protobuf_script = workspace_path.join("tools/build/scripts/extract_protobuf_py3_wheel.sh");
    if protobuf_script.exists() && (updated_symlinks.contains(&protobuf_wheel) || !protobuf_dest.exists()) {
        if !quiet {
            println!("  Extracting protobuf-py3 wheel...");
        }
        run_command(protobuf_script.to_str().unwrap(), &[], workspace_path, &[])
            .context("Failed to run extract_protobuf_py3_wheel.sh")?;
    }

    Ok(())
}

#[derive(serde::Deserialize, Debug)]
struct LockEntry {
    package: String,
    version: String,
    #[serde(rename = "instance_id")]
    instance_id: String,
}

fn load_lockfiles(workspace_path: &Path) -> Result<std::collections::HashMap<(String, String), String>> {
    let mut lock_map = std::collections::HashMap::new();
    let lockfiles = find_lockfiles(workspace_path)?;
    log::info!("Found {} lockfiles: {:?}", lockfiles.len(), lockfiles);
    for path in &lockfiles {
        log::info!("Parsing lockfile {:?}", path);
        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read lockfile {:?}", path))?;
        let entries: Vec<LockEntry> = match serde_json::from_str(&content) {
            Ok(e) => e,
            Err(e) => {
                log::warn!("Failed to parse lockfile {:?}: {:?}", path, e);
                continue;
            }
        };
        for entry in entries {
            lock_map.insert((entry.package, entry.version), entry.instance_id);
        }
    }
    Ok(lock_map)
}

fn find_lockfiles(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut lockfiles = Vec::new();
    find_lockfiles_recursive(dir, &mut lockfiles)?;
    Ok(lockfiles)
}

fn find_lockfiles_recursive(dir: &Path, lockfiles: &mut Vec<PathBuf>) -> Result<()> {
    if dir.is_dir() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let path = entry.path();
            if file_type.is_dir() {
                if path.file_name().and_then(|n| n.to_str()).is_some_and(|name| {
                    name == ".git" || name == "prebuilt" || name == "out"
                }) {
                    continue;
                }
                find_lockfiles_recursive(&path, lockfiles)?;
            } else if file_type.is_file() {
                if path.file_name().and_then(|n| n.to_str()) == Some("jiri.lock") {
                    lockfiles.push(path);
                }
            }
        }
    }
    Ok(())
}

fn is_dir_not_empty(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    if let Ok(mut entries) = fs::read_dir(path) {
        return entries.next().is_some();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_group_packages_by_path() {
        let pkgs = vec![
            JiriPackage {
                name: "pkg1/${platform}".to_string(),
                path: "/root/dir1".to_string(),
                version: "v1".to_string(),
                platforms: Some(vec!["linux-amd64".to_string()]),
            },
            JiriPackage {
                name: "pkg2".to_string(),
                path: "/root/dir1".to_string(),
                version: "v2".to_string(),
                platforms: None,
            },
            JiriPackage {
                name: "pkg3".to_string(),
                path: "/root/dir2".to_string(),
                version: "v3".to_string(),
                platforms: Some(vec!["mac-amd64".to_string()]), // Should be filtered out
            },
        ];

        let groups = group_packages_by_path(pkgs, "linux-amd64");
        assert_eq!(groups.len(), 1);
        let dir1_pkgs = groups.get(Path::new("/root/dir1")).unwrap();
        assert_eq!(dir1_pkgs.len(), 2);
        assert_eq!(dir1_pkgs[0].name, "pkg1/linux-amd64");
        assert_eq!(dir1_pkgs[1].name, "pkg2");
    }

    #[test]
    fn test_calculate_group_hash() {
        let pkgs1 = vec![
            JiriPackage {
                name: "pkg1".to_string(),
                path: "".to_string(),
                version: "v1".to_string(),
                platforms: None,
            },
            JiriPackage {
                name: "pkg2".to_string(),
                path: "".to_string(),
                version: "v2".to_string(),
                platforms: None,
            },
        ];
        let pkgs2 = vec![
            pkgs1[1].clone(),
            pkgs1[0].clone(),
        ];

        let hash1 = calculate_group_hash(&pkgs1);
        let hash2 = calculate_group_hash(&pkgs2);
        assert_eq!(hash1, hash2, "Hash must be independent of input order");

        let mut pkgs3 = pkgs1.clone();
        pkgs3[0].version = "v1-updated".to_string();
        let hash3 = calculate_group_hash(&pkgs3);
        assert_ne!(hash1, hash3);
    }
}
