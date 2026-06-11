use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use tempfile::TempDir;

fn add_worktree(config: &Config, config_name: &str, _quiet: bool) -> anyhow::Result<String> {
    let name = format!(
        "{}-{}",
        config_name,
        &uuid::Uuid::new_v4().to_string()[0..8]
    );
    let wt_path = config.worktrees_dir().join(&name);

    fx_worktree::worktree::add_worktree(config, &name)?;

    // Write default build directory config
    let out_dir = wt_path.join("out").join(config_name);
    fs::create_dir_all(&out_dir)?;
    fs::write(out_dir.join("args.gn"), "mock_arg = true\n")?;

    // Also write .fx-build-dir
    fs::write(
        wt_path.join(".fx-build-dir"),
        format!("out/{}\n", config_name),
    )?;

    Ok(name)
}
use fx_worktree::config::Config;
use fx_worktree::lease::lease_worktree as lease_worktree_raw;
use fx_worktree::list::list_worktrees;
use fx_worktree::release::release_worktree;
fn remove_worktree(config: &Config, id: &str, force: bool, _quiet: bool) -> anyhow::Result<()> {
    fx_worktree::worktree::remove_worktree(config, id, force)
}

fn lease_worktree(
    config: &Config,
    _config_name: &str,
    agent_id: &str,
    sync: bool,
    quiet: bool,
) -> anyhow::Result<fx_worktree::worktree::WorktreeInfo> {
    lease_worktree_raw(config, None, true, Some(agent_id), sync, quiet)
}

// Global lock to serialize tests
static TEST_LOCK: Mutex<()> = Mutex::new(());

fn run_setup_cmd(cmd: &str, args: &[&str], cwd: &Path) {
    let status = Command::new(cmd)
        .args(args)
        .current_dir(cwd)
        .status()
        .expect("failed to execute setup command");
    assert!(
        status.success(),
        "setup command failed: {} {}",
        cmd,
        args.join(" ")
    );
}

fn resolve_git_dir(repo_path: &Path) -> PathBuf {
    let git_path = repo_path.join(".git");
    if git_path.is_dir() {
        git_path
    } else if git_path.is_file() {
        let content = fs::read_to_string(&git_path).unwrap();
        let content = content.trim();
        if content.starts_with("gitdir: ") {
            let gitdir = content.trim_start_matches("gitdir: ").trim();
            let p = PathBuf::from(gitdir);
            if p.is_absolute() {
                p
            } else {
                repo_path.join(p).canonicalize().unwrap()
            }
        } else {
            panic!("Invalid .git file: {}", content);
        }
    } else {
        panic!(".git does not exist in {}", repo_path.display());
    }
}

struct TestEnv {
    _fenv_root_dir: TempDir,
    _fuchsia_dir_dir: TempDir,
    config: Config,
}

fn setup_mock_env() -> TestEnv {
    let _ = env_logger::builder().is_test(true).try_init();
    let fenv_root_dir = TempDir::new().unwrap();
    let fuchsia_dir_dir = TempDir::new().unwrap();
    let fuchsia_path = fuchsia_dir_dir.path();

    // 1. Initialize git repo in fuchsia_dir
    run_setup_cmd("git", &["init", "--initial-branch=main"], fuchsia_path);
    run_setup_cmd("git", &["config", "user.name", "Test User"], fuchsia_path);
    run_setup_cmd(
        "git",
        &["config", "user.email", "test@example.com"],
        fuchsia_path,
    );

    let dummy_file = fuchsia_path.join("dummy.txt");
    fs::write(&dummy_file, "hello").unwrap();

    // Create untracked metadata files in mock fuchsia repo
    let ctf_dir = fuchsia_path.join("sdk/ctf/build/internal");
    fs::create_dir_all(&ctf_dir).unwrap();
    fs::write(ctf_dir.join("ctf_releases.gni"), "ctf_releases = []").unwrap();

    let build_info_dir = fuchsia_path.join("build/info/jiri_generated");
    fs::create_dir_all(&build_info_dir).unwrap();
    fs::write(build_info_dir.join("commit_info"), "some info").unwrap();

    fs::write(fuchsia_path.join("build/cipd.gni"), "cipd = []").unwrap();
    fs::write(fuchsia_path.join(".jiri_manifest"), "manifest").unwrap();

    // Create sub-project repo in fuchsia_dir
    let sub_path = fuchsia_path.join("third_party/sub");
    fs::create_dir_all(&sub_path).unwrap();
    run_setup_cmd("git", &["init", "--initial-branch=main"], &sub_path);
    run_setup_cmd("git", &["config", "user.name", "Test User"], &sub_path);
    run_setup_cmd(
        "git",
        &["config", "user.email", "test@example.com"],
        &sub_path,
    );
    fs::write(sub_path.join("sub_dummy.txt"), "sub hello").unwrap();
    run_setup_cmd("git", &["add", "sub_dummy.txt"], &sub_path);
    run_setup_cmd("git", &["commit", "-m", "sub initial commit"], &sub_path);

    // Create mock Jiri update history and config
    let update_history_dir = fuchsia_path.join(".jiri_root/update_history");
    fs::create_dir_all(&update_history_dir).unwrap();
    fs::write(update_history_dir.join("latest"), "<manifest></manifest>").unwrap();
    fs::write(fuchsia_path.join(".jiri_root/config"), "<config></config>").unwrap();
    fs::create_dir_all(fuchsia_path.join(".git/jiri")).unwrap();
    fs::create_dir_all(sub_path.join(".git/jiri")).unwrap();

    // 2. Create mock jiri
    let jiri_dir = fuchsia_path.join(".jiri_root/bin");
    fs::create_dir_all(&jiri_dir).unwrap();
    let jiri_path = jiri_dir.join("jiri");

    let jiri_script = format!(
        r#"#!/bin/bash
base_dir="{0}"
cwd=$(pwd)

copy_if_different() {{
  local src="$1"
  local dest="$2"
  if [ -f "$src" ]; then
    if [ -f "$dest" ]; then
      if cmp -s "$src" "$dest"; then
        return 0
      fi
    fi
    mkdir -p "$(dirname "$dest")"
    cp "$src" "$dest"
  fi
}}
commit_msg=$(git -C "$base_dir" log -n 1 --format=%s 2>/dev/null || echo "")
version="version:1"
if [ "$commit_msg" = "bump root" ] || [ "$commit_msg" = "bump sub" ]; then
  if git -C "$base_dir" log --format=%s | grep -q "bump root"; then
    version="version:2"
  fi
fi

if [ "$1" = "project" ] && [ "$2" = "-json-output" ]; then
  output_file=$3
  if [[ "$cwd" == *"/worktrees/"* ]]; then
    project_base="$cwd"
  else
    project_base="$base_dir"
  fi
  revision=$(git -C "$project_base" rev-parse HEAD 2>/dev/null || echo "unknown")
  sub_revision=$(git -C "$project_base/third_party/sub" rev-parse HEAD 2>/dev/null || echo "unknown")
  cat <<EOF > "$output_file"
[
  {{
    "name": "mock_project",
    "path": "$project_base",
    "revision": "$revision"
  }},
  {{
    "name": "sub_project",
    "path": "$project_base/third_party/sub",
    "revision": "$sub_revision"
  }}
]
EOF
elif [ "$1" = "package" ] && [ "$2" = "-json-output" ]; then
  output_file=$3
  cat <<EOF > "$output_file"
[
  {{
    "name": "fuchsia/tools/mock_tool/\${{platform}}",
    "path": "$cwd/prebuilt/tools/mock_tool",
    "version": "$version",
    "platforms": [
      "linux-amd64"
    ]
  }},
  {{
    "name": "infra/3pp/tools/bazel/\${{platform}}",
    "path": "$cwd/prebuilt/third_party/bazel/linux-x64",
    "version": "$version",
    "platforms": [
      "linux-amd64"
    ]
  }}
]
EOF
elif [ "$1" = "worktree" ] && [ "$2" = "add" ]; then
  target_path=$3
  if [ -z "$target_path" ]; then
    id=$(od -An -tx1 -N8 /dev/urandom | tr -d ' \n')
    target_path="$base_dir/.jiri_root/worktrees/$id"
    echo "$target_path"
  fi
  root_rev=$(git -C "$base_dir" rev-parse HEAD)
  mkdir -p "$(dirname "$target_path")"
  git -C "$base_dir" worktree add -f --detach "$target_path" "$root_rev" >/dev/null
  
  # Append to registry
  mkdir -p "$base_dir/.jiri_root"
  echo "$target_path" >> "$base_dir/.jiri_root/worktrees_registry"
  
  sub_rev=$(git -C "$base_dir/third_party/sub" rev-parse HEAD)
  mkdir -p "$target_path/third_party/sub"
  git -C "$base_dir/third_party/sub" worktree add -f --detach "$target_path/third_party/sub" "$sub_rev" >/dev/null

  # Simulate Jiri WorktreeAdd metadata setup
  mkdir -p "$target_path/.jiri_root"
  ln -sf "$base_dir/.jiri_root/bin" "$target_path/.jiri_root/bin"
  if [ -f "$base_dir/.jiri_root/config" ]; then
    cp "$base_dir/.jiri_root/config" "$target_path/.jiri_root/config"
  fi
  if [ -f "$base_dir/.jiri_manifest" ]; then
    cp "$base_dir/.jiri_manifest" "$target_path/.jiri_manifest"
  fi
  if [ -f "$base_dir/.jiri_root/update_history/latest" ]; then
    mkdir -p "$target_path/.jiri_root/update_history"
    cp "$base_dir/.jiri_root/update_history/latest" "$target_path/.jiri_root/update_history/latest"
  fi

  # Run sync to simulate Jiri's internal sync during add
  (
    cd "$target_path"
    "$base_dir/.jiri_root/bin/jiri" worktree sync
  )
elif [ "$1" = "worktree" ] && [ "$2" = "sync" ]; then
  parent_rev=$(git -C "$base_dir" rev-parse HEAD)
  current_rev=$(git rev-parse HEAD)
  if [ "$current_rev" != "$parent_rev" ]; then
    git checkout -q "$parent_rev"
  fi
  if [ -d "third_party/sub" ]; then
    parent_sub_rev=$(git -C "$base_dir/third_party/sub" rev-parse HEAD)
    current_sub_rev=$(git -C "third_party/sub" rev-parse HEAD)
    if [ "$current_sub_rev" != "$parent_sub_rev" ]; then
      git -C "third_party/sub" checkout -q "$parent_sub_rev"
    fi
  fi
  ensure_file=$(mktemp)
  cat <<EOF > "$ensure_file"
@Subdir prebuilt/tools/mock_tool
fuchsia/tools/mock_tool/linux-amd64 $version

@Subdir prebuilt/third_party/bazel/linux-x64
infra/3pp/tools/bazel/linux-amd64 $version

@Subdir prebuilt/third_party/gn/linux-amd64
fuchsia/tools/gn/linux-amd64 $version

@Subdir prebuilt/tools/shac
fuchsia/tools/shac/linux-amd64 $version
EOF
  cache_dir="$base_dir/.jiri_root/packages"
  "$base_dir/.jiri_root/bin/cipd" -root "$cache_dir" -ensure-file "$ensure_file"
  
  while read -r line || [ -n "$line" ]; do
    [[ "$line" =~ ^# ]] && continue
    [[ -z "$line" ]] && continue
    if [[ "$line" =~ ^@Subdir[[:space:]]+(.*) ]]; then
      current_subdir="${{BASH_REMATCH[1]}}"
    else
      read -r pkg ver <<< "$line"
      if [ -n "$current_subdir" ]; then
        mkdir -p "$(dirname "$current_subdir")"
        rm -rf "$current_subdir"
        ln -sf "$cache_dir/$current_subdir/$ver" "$current_subdir"
      fi
    fi
  done < "$ensure_file"
  
  rm "$ensure_file"
  
  # Simulate Jiri generating metadata files (preserving mtimes if unchanged)
  copy_if_different "$base_dir/build/info/jiri_generated/commit_info" "build/info/jiri_generated/commit_info"
  copy_if_different "$base_dir/build/cipd.gni" "build/cipd.gni"
  copy_if_different "$base_dir/sdk/ctf/build/internal/ctf_releases.gni" "sdk/ctf/build/internal/ctf_releases.gni"

  if [ "$current_rev" != "$parent_rev" ] || [ ! -d "prebuilt/third_party/pydantic-core" ] || [ ! -d "prebuilt/third_party/protobuf-py3" ]; then
    if [ -f "tools/build/scripts/extract_pydantic_core_wheel.sh" ]; then
      ./tools/build/scripts/extract_pydantic_core_wheel.sh
    fi
    if [ -f "tools/build/scripts/extract_protobuf_py3_wheel.sh" ]; then
      ./tools/build/scripts/extract_protobuf_py3_wheel.sh
    fi
  fi
elif [ "$1" = "worktree" -a "$2" = "clean" ] || [ "$1" = "clean" ]; then
  git clean -fdx \
    -e prebuilt \
    -e .jiri_root \
    -e .fx-build-dir \
    -e out \
    -e sdk/ctf/build/internal/ctf_releases.gni \
    -e build/info/jiri_generated \
    -e build/cipd.gni \
    -e .jiri_manifest
  if [ -d "third_party/sub" ]; then
    git -C "third_party/sub" clean -fdx
  fi

elif [ "$1" = "worktree" ] && [ "$2" = "remove" ]; then
  target_path=$3
  if [ -d "$target_path/third_party/sub" ]; then
    git -C "$target_path/third_party/sub" worktree remove -f "$target_path/third_party/sub"
  fi
  git worktree remove -f "$target_path"
  rm -rf "$target_path"
  
  # Remove from registry
  if [ -f "$base_dir/.jiri_root/worktrees_registry" ]; then
    grep -v "^$target_path$" "$base_dir/.jiri_root/worktrees_registry" > "$base_dir/.jiri_root/worktrees_registry.tmp" || true
    mv "$base_dir/.jiri_root/worktrees_registry.tmp" "$base_dir/.jiri_root/worktrees_registry"
  fi
  
  # Run GC
  commit_msg=$(git -C "$base_dir" log -n 1 --format=%s 2>/dev/null || echo "")
  parent_version="version:1"
  if [ "$commit_msg" = "bump root" ] || [ "$commit_msg" = "bump sub" ]; then
    if git -C "$base_dir" log --format=%s | grep -q "bump root"; then
      parent_version="version:2"
    fi
  fi
  
  used_versions=""
  if [ -d "$base_dir/.jiri_root/worktrees" ]; then
    for wt in "$base_dir/.jiri_root/worktrees"/*; do
      if [ -d "$wt" ] && [ "$wt" != "$target_path" ]; then
        if [ -L "$wt/prebuilt/tools/mock_tool" ]; then
          target=$(readlink "$wt/prebuilt/tools/mock_tool")
          ver=$(basename "$target")
          used_versions="$used_versions $ver"
        fi
      fi
    done
  fi
  
  cache_dir="$base_dir/.jiri_root/packages"
  if [ -d "$cache_dir" ]; then
    cleanup_package_cache() {{
      local pkg_cache_dir=$1
      if [ -d "$pkg_cache_dir" ]; then
        for ver_dir in "$pkg_cache_dir"/*; do
          if [ -d "$ver_dir" ]; then
            local ver=$(basename "$ver_dir")
            local keep=false
            if [ "$ver" = "$parent_version" ]; then
              keep=true
            fi
            for u in $used_versions; do
              if [ "$ver" = "$u" ]; then
                keep=true
              fi
            done
            if [ "$keep" = false ]; then
              rm -rf "$ver_dir"
            fi
          fi
        done
      fi
    }}
    cleanup_package_cache "$cache_dir/prebuilt/tools/mock_tool"
    cleanup_package_cache "$cache_dir/prebuilt/third_party/bazel/linux-x64"
    cleanup_package_cache "$cache_dir/prebuilt/third_party/gn/linux-amd64"
    cleanup_package_cache "$cache_dir/prebuilt/tools/shac"
  fi
fi
"#,
        fuchsia_path.to_str().unwrap()
    );
    fs::write(&jiri_path, jiri_script).unwrap();
    make_executable(&jiri_path);

    // Create mock cipd
    let cipd_path = jiri_dir.join("cipd");
    let cipd_script = r#"#!/bin/bash
# Mock cipd ensure

root_dir=""
ensure_file=""

while [ $# -gt 0 ]; do
  case "$1" in
    -root)
      root_dir="$2"
      shift 2
      ;;
    -ensure-file)
      ensure_file="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done

if [ -n "$root_dir" ] && [ -n "$ensure_file" ]; then
  current_subdir=""
  while read -r line || [ -n "$line" ]; do
    [[ "$line" =~ ^# ]] && continue
    [[ -z "$line" ]] && continue
    
    if [[ "$line" =~ ^@Subdir[[:space:]]+(.*) ]]; then
      current_subdir="${BASH_REMATCH[1]}"
    else
      read -r pkg ver <<< "$line"
      if [ -n "$current_subdir" ]; then
        if [[ "$root_dir" =~ \.jiri_root/packages$ ]]; then
          target_dir="$root_dir/$current_subdir/$ver"
        else
          target_dir="$root_dir/$current_subdir"
        fi
        mkdir -p "$target_dir"
        echo "mock_content for $pkg $ver" > "$target_dir/file.txt"
      fi
    fi
  done < "$ensure_file"
fi
"#;
    fs::write(&cipd_path, cipd_script).unwrap();
    make_executable(&cipd_path);

    // 3. Create mock fx
    let scripts_dir = fuchsia_path.join("scripts");
    fs::create_dir_all(&scripts_dir).unwrap();
    let fx_path = scripts_dir.join("fx");
    let fx_script = r#"#!/bin/bash
build_dir=""
if [ "$1" = "--dir" ]; then
  build_dir=$2
  shift 2
fi

if [ "$1" = "set" ]; then
  config=$2
  if [ "$config" = "fail_config" ]; then
    echo "mock fx set failure" >&2
    exit 1
  fi
  mkdir -p "$build_dir"
  if [[ "$config" == *.* ]]; then
    product=${config%.*}
    board=${config#*.}
  else
    product=$config
    board=""
  fi
  echo "build_info_product = \"$product\"" > "$build_dir/args.gn"
  if [ -n "$board" ]; then
    echo "build_info_board = \"$board\"" >> "$build_dir/args.gn"
  fi
  touch "$build_dir/build.ninja"
elif [ "$1" = "gen" ]; then
  echo "mock gen success"
elif [ "$1" = "build" ]; then
  if [ -z "$build_dir" ]; then
    if [ -f ".fx-build-dir" ]; then
      build_dir=$(cat .fx-build-dir)
    else
      build_dir="out/default"
    fi
  fi
  build_ninja="$build_dir/build.ninja"
  build_gn="BUILD.gn"
  if [ -f "$build_gn" ] && [ -f "$build_ninja" ]; then
    if [ "$build_gn" -nt "$build_ninja" ]; then
      touch "$build_ninja"
    fi
  fi
  echo "mock build success"
fi
"#;
    fs::write(&fx_path, fx_script).unwrap();
    make_executable(&fx_path);

    // Create mock wheel extraction scripts
    let tools_scripts_dir = fuchsia_path.join("tools/build/scripts");
    fs::create_dir_all(&tools_scripts_dir).unwrap();

    let mock_extract_script = r#"#!/bin/bash
mkdir -p prebuilt/third_party/pydantic-core/pydantic_core
touch prebuilt/third_party/pydantic-core/pydantic_core/__init__.py
mkdir -p prebuilt/third_party/protobuf-py3/protobuf
touch prebuilt/third_party/protobuf-py3/protobuf/__init__.py
echo "mock extraction done"
"#;

    let pydantic_script = tools_scripts_dir.join("extract_pydantic_core_wheel.sh");
    fs::write(&pydantic_script, mock_extract_script).unwrap();
    make_executable(&pydantic_script);

    let protobuf_script = tools_scripts_dir.join("extract_protobuf_py3_wheel.sh");
    fs::write(&protobuf_script, mock_extract_script).unwrap();
    make_executable(&protobuf_script);

    // Commit only dummy.txt, scripts/fx and tools/build/scripts/extract_*.sh
    run_setup_cmd(
        "git",
        &[
            "add",
            "dummy.txt",
            "scripts/fx",
            "tools/build/scripts/extract_pydantic_core_wheel.sh",
            "tools/build/scripts/extract_protobuf_py3_wheel.sh",
        ],
        fuchsia_path,
    );
    run_setup_cmd(
        "git",
        &["commit", "-m", "initial commit with mocks"],
        fuchsia_path,
    );

    let config = Config::new(Some(fuchsia_path.to_path_buf())).unwrap();
    config.init_topology().unwrap();

    TestEnv {
        _fenv_root_dir: fenv_root_dir,
        _fuchsia_dir_dir: fuchsia_dir_dir,
        config,
    }
}

fn make_executable(path: &Path) {
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

#[test]
fn test_full_lifecycle() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Test Environment Create
    let env_id = add_worktree(config, "mock_config", false).unwrap();
    let env_path = config.worktrees_dir().join(&env_id);
    assert!(env_path.exists());
    assert!(env_path.join("out/mock_config/args.gn").exists());
    // 2. Test Worktree Lease (reuses the created slot)
    let env_info = lease_worktree(config, "mock_config", "test_agent", true, true).unwrap();
    assert_eq!(env_info.agent_id.as_deref(), Some("test_agent"));
    assert!(env_path.join("out/mock_config/args.gn.ref").exists());

    let lease_file = env_path.join(".jiri_root").join("lease.json");
    assert!(lease_file.exists());

    assert!(env_info.path.join(".git").exists());
    assert!(env_info.path.join(".jiri_root").exists());
    assert!(env_info.path.join("out/mock_config").exists());
    assert!(env_info.path.join(".fx-build-dir").exists());

    println!("--- List Worktrees after lease ---");
    list_worktrees(config, false).unwrap();

    // 3. Test Worktree Release
    release_worktree(config, &env_info.worktree_id).unwrap();
    assert!(env_info.path.exists()); // Path must remain!
    assert!(!lease_file.exists()); // Lease must be deleted

    println!("--- List Worktrees after release ---");
    list_worktrees(config, false).unwrap();

    // 4. Test Worktree Remove (fully cleans up)
    remove_worktree(config, &env_id, false, false).unwrap();
    assert!(!env_path.exists()); // Directory is gone now

    println!("--- List Worktrees after remove ---");
    list_worktrees(config, false).unwrap();
}

#[test]
fn test_locate_path() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;
    use fx_worktree::locate::locate_path;

    // Test last created fallback (errors initially)
    assert!(locate_path(config, None).is_err());

    // Create environment
    let env_id = add_worktree(config, "mock_config", false).unwrap();
    let env_path = config.worktrees_dir().join(&env_id);

    // Locate by ID
    let path = locate_path(config, Some(env_id.clone())).unwrap();
    assert_eq!(path, env_path);

    // Allocate
    let env_info = lease_worktree(config, "mock_config", "test_agent", true, true).unwrap();

    // Locate by ID (resolves to same path)
    let path = locate_path(config, Some(env_id.clone())).unwrap();
    assert_eq!(path, env_info.path);

    // Locate last created
    let path = locate_path(config, None).unwrap();
    assert_eq!(path, env_info.path);
}

#[test]
fn test_manual_worktree() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;
    use fx_worktree::locate::locate_path;

    // 1. Manually add a worktree (simulating Jiri worktree add outside of fx-worktree)
    let manual_path = env._fuchsia_dir_dir.path().join("my-manual-wt");

    let jiri_bin = config.fuchsia_dir.join(".jiri_root/bin/jiri");
    let jiri_cmd = if jiri_bin.exists() {
        jiri_bin.to_str().unwrap()
    } else {
        "jiri"
    };

    // Run mock jiri worktree add
    fx_worktree::utils::run_command(
        jiri_cmd,
        &["worktree", "add", manual_path.to_str().unwrap()],
        &config.fuchsia_dir,
        &[],
    )
    .unwrap();

    // Create fake args.gn inside it so config name is resolvable
    let out_dir = manual_path.join("out/default");
    std::fs::create_dir_all(&out_dir).unwrap();
    std::fs::write(
        out_dir.join("args.gn"),
        "build_info_product = \"manual_config\"\nbuild_info_board = \"x64\"\n",
    )
    .unwrap();

    // 2. Locate should find it
    let path = locate_path(config, Some("my-manual-wt".to_string())).unwrap();
    assert_eq!(path, manual_path);

    // 3. List should show it as "NotInPool" (verify it doesn't panic)
    println!("--- List Worktrees with manual worktree ---");
    list_worktrees(config, false).unwrap();
}

#[test]
fn test_git_symlink_conversion() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Create environment (runs worktree add)
    let env_id = add_worktree(config, "mock_config", false).unwrap();
    let env_path = config.worktrees_dir().join(&env_id);

    let git_file_path = env_path.join(".git");
    assert!(git_file_path.exists());
    let metadata = fs::symlink_metadata(&git_file_path).unwrap();
    assert!(metadata.file_type().is_file());

    // 2. Allocate (keeps file)
    let env_info = lease_worktree(config, "mock_config", "test_agent", true, true).unwrap();
    let metadata = fs::symlink_metadata(&git_file_path).unwrap();
    assert!(metadata.file_type().is_file());

    // 3. Free (keeps file)
    release_worktree(config, &env_info.worktree_id).unwrap();
    let metadata = fs::symlink_metadata(&git_file_path).unwrap();
    assert!(metadata.file_type().is_file());

    // 4. Delete (removes it)
    remove_worktree(config, &env_id, false, false).unwrap();
    assert!(!env_path.exists());
}

#[test]
fn test_mtime_and_metadata_preservation() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Create environment
    let env_id = add_worktree(config, "mock_config", false).unwrap();
    let env_path = config.worktrees_dir().join(&env_id);

    // Verify metadata files were copied during create
    let ctf_gni = env_path.join("sdk/ctf/build/internal/ctf_releases.gni");
    let commit_info = env_path.join("build/info/jiri_generated/commit_info");
    let cipd_gni = env_path.join("build/cipd.gni");
    let jiri_manifest = env_path.join(".jiri_manifest");

    assert!(ctf_gni.exists());
    assert!(commit_info.exists());
    assert!(cipd_gni.exists());
    assert!(jiri_manifest.exists());

    // 2. Allocate
    let env_info = lease_worktree(config, "mock_config", "test_agent", true, true).unwrap();

    // Verify index exists
    let git_dir = resolve_git_dir(&env_info.path);
    let index_path = git_dir.join("index");
    assert!(index_path.exists());

    // Record mtime of index, dummy.txt and metadata files before free
    let index_mtime_before = fs::metadata(&index_path).unwrap().modified().unwrap();
    let dummy_path = env_info.path.join("dummy.txt");
    let dummy_mtime_before = fs::metadata(&dummy_path).unwrap().modified().unwrap();

    let ctf_mtime_before = fs::metadata(&ctf_gni).unwrap().modified().unwrap();
    let commit_info_mtime_before = fs::metadata(&commit_info).unwrap().modified().unwrap();
    let cipd_mtime_before = fs::metadata(&cipd_gni).unwrap().modified().unwrap();
    let jiri_manifest_mtime_before = fs::metadata(&jiri_manifest).unwrap().modified().unwrap();

    // Sleep a bit to ensure time moves forward
    std::thread::sleep(std::time::Duration::from_millis(100));

    // 3. Free (runs jiri clean, which we mocked to touch files)
    release_worktree(config, &env_info.worktree_id).unwrap();

    // Verify metadata files STILL exist
    assert!(ctf_gni.exists());
    assert!(commit_info.exists());
    assert!(cipd_gni.exists());
    assert!(jiri_manifest.exists());

    // Verify index mtime is preserved
    let index_mtime_after_free = fs::metadata(&index_path).unwrap().modified().unwrap();
    assert_eq!(
        index_mtime_before, index_mtime_after_free,
        "Index mtime should be preserved after free"
    );

    // Verify dummy.txt mtime is preserved
    let dummy_mtime_after_free = fs::metadata(&dummy_path).unwrap().modified().unwrap();
    assert_eq!(
        dummy_mtime_before, dummy_mtime_after_free,
        "dummy.txt mtime should be preserved after free"
    );

    // Verify metadata mtimes are preserved after free
    let ctf_mtime_after_free = fs::metadata(&ctf_gni).unwrap().modified().unwrap();
    let commit_info_mtime_after_free = fs::metadata(&commit_info).unwrap().modified().unwrap();
    let cipd_mtime_after_free = fs::metadata(&cipd_gni).unwrap().modified().unwrap();
    let jiri_manifest_mtime_after_free = fs::metadata(&jiri_manifest).unwrap().modified().unwrap();

    assert_eq!(
        ctf_mtime_before, ctf_mtime_after_free,
        "ctf_releases.gni mtime should be preserved after free"
    );
    assert_eq!(
        commit_info_mtime_before, commit_info_mtime_after_free,
        "commit_info mtime should be preserved after free"
    );
    assert_eq!(
        cipd_mtime_before, cipd_mtime_after_free,
        "cipd.gni mtime should be preserved after free"
    );
    assert_eq!(
        jiri_manifest_mtime_before, jiri_manifest_mtime_after_free,
        "jiri_manifest mtime should be preserved after free"
    );

    // Record mtimes before sync
    let ctf_mtime_before_sync = fs::metadata(&ctf_gni).unwrap().modified().unwrap();
    let commit_info_mtime_before_sync = fs::metadata(&commit_info).unwrap().modified().unwrap();
    let cipd_mtime_before_sync = fs::metadata(&cipd_gni).unwrap().modified().unwrap();
    let jiri_manifest_mtime_before_sync = fs::metadata(&jiri_manifest).unwrap().modified().unwrap();

    // Sleep again
    std::thread::sleep(std::time::Duration::from_millis(100));

    // 4. Allocate again (triggers sync, which we mocked to touch files)
    let env_info_2 = lease_worktree(config, "mock_config", "test_agent_2", true, true).unwrap();

    // Verify index mtime is preserved
    let index_mtime_after_alloc = fs::metadata(&index_path).unwrap().modified().unwrap();
    assert_eq!(
        index_mtime_before, index_mtime_after_alloc,
        "Index mtime should be preserved after allocate"
    );

    // Verify dummy.txt mtime is preserved
    let dummy_mtime_after_alloc = fs::metadata(&dummy_path).unwrap().modified().unwrap();
    assert_eq!(
        dummy_mtime_before, dummy_mtime_after_alloc,
        "dummy.txt mtime should be preserved after allocate"
    );

    // Verify metadata mtimes are preserved after sync
    let ctf_mtime_after_sync = fs::metadata(&ctf_gni).unwrap().modified().unwrap();
    let commit_info_mtime_after_sync = fs::metadata(&commit_info).unwrap().modified().unwrap();
    let cipd_mtime_after_sync = fs::metadata(&cipd_gni).unwrap().modified().unwrap();
    let jiri_manifest_mtime_after_sync = fs::metadata(&jiri_manifest).unwrap().modified().unwrap();

    assert_eq!(
        ctf_mtime_before_sync, ctf_mtime_after_sync,
        "ctf_releases.gni mtime should be preserved after sync"
    );
    assert_eq!(
        commit_info_mtime_before_sync, commit_info_mtime_after_sync,
        "commit_info mtime should be preserved after sync"
    );
    assert_eq!(
        cipd_mtime_before_sync, cipd_mtime_after_sync,
        "cipd.gni mtime should be preserved after sync"
    );
    assert_eq!(
        jiri_manifest_mtime_before_sync, jiri_manifest_mtime_after_sync,
        "jiri_manifest mtime should be preserved after sync"
    );

    // 5. Test that mtime is NOT preserved if content changes
    // Modify a file in parent
    let parent_cipd = env.config.fuchsia_dir.join("build/cipd.gni");
    fs::write(&parent_cipd, "cipd = [different]").unwrap();

    // Make a dummy commit in parent to change HEAD and force sync
    let dummy_parent_file = env.config.fuchsia_dir.join("dummy.txt");
    fs::write(&dummy_parent_file, "force sync change").unwrap();
    run_setup_cmd("git", &["add", "dummy.txt"], &env.config.fuchsia_dir);
    run_setup_cmd(
        "git",
        &["commit", "-m", "force sync"],
        &env.config.fuchsia_dir,
    );

    // Release env_info_2 first
    release_worktree(config, &env_info_2.worktree_id).unwrap();

    // Record mtime before sync that changes content
    let cipd_mtime_before_change_sync = fs::metadata(&cipd_gni).unwrap().modified().unwrap();

    // Sleep
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Allocate again with sync, it should copy the new cipd.gni
    let env_info_3 = lease_worktree(config, "mock_config", "test_agent_3", true, true).unwrap();

    let cipd_mtime_after_change_sync = fs::metadata(&cipd_gni).unwrap().modified().unwrap();
    assert_ne!(
        cipd_mtime_before_change_sync, cipd_mtime_after_change_sync,
        "cipd.gni mtime should NOT be preserved if content changed"
    );

    // Clean up
    release_worktree(config, &env_info_3.worktree_id).unwrap();
    remove_worktree(config, &env_id, false, false).unwrap();
}

#[test]
fn test_parent_jiri_update() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Create environment
    let env_id = add_worktree(config, "mock_config", false).unwrap();

    // 2. Allocate (first time, gets initial revisions)
    let env_info = lease_worktree(config, "mock_config", "test_agent", true, true).unwrap();

    // Verify initial content
    assert_eq!(
        fs::read_to_string(env_info.path.join("dummy.txt")).unwrap(),
        "hello"
    );
    assert_eq!(
        fs::read_to_string(env_info.path.join("third_party/sub/sub_dummy.txt")).unwrap(),
        "sub hello"
    );

    // 3. Free environment
    release_worktree(config, &env_info.worktree_id).unwrap();

    // 4. Update parent repo (simulate jiri update)
    let parent_dummy = env.config.fuchsia_dir.join("dummy.txt");
    fs::write(&parent_dummy, "hello v2").unwrap();
    run_setup_cmd("git", &["add", "dummy.txt"], &env.config.fuchsia_dir);
    run_setup_cmd(
        "git",
        &["commit", "-m", "bump root"],
        &env.config.fuchsia_dir,
    );

    let parent_sub_path = env.config.fuchsia_dir.join("third_party/sub");
    let parent_sub_dummy = parent_sub_path.join("sub_dummy.txt");
    fs::write(&parent_sub_dummy, "sub hello v2").unwrap();
    run_setup_cmd("git", &["add", "sub_dummy.txt"], &parent_sub_path);
    run_setup_cmd("git", &["commit", "-m", "bump sub"], &parent_sub_path);

    // 5. Allocate again (should reuse same slot but update revisions)
    let env_info_2 = lease_worktree(config, "mock_config", "test_agent_2", true, true).unwrap();
    assert_eq!(
        env_info_2.worktree_id, env_id,
        "Should reuse the same environment slot"
    );

    // Verify updated content in workspace
    assert_eq!(
        fs::read_to_string(env_info_2.path.join("dummy.txt")).unwrap(),
        "hello v2"
    );
    assert_eq!(
        fs::read_to_string(env_info_2.path.join("third_party/sub/sub_dummy.txt")).unwrap(),
        "sub hello v2"
    );

    // Clean up
    release_worktree(config, &env_info_2.worktree_id).unwrap();
    remove_worktree(config, &env_id, false, false).unwrap();
}

#[test]
fn test_prebuilt_isolation() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Create environment
    let env_id = add_worktree(config, "mock_config", false).unwrap();

    // 2. Allocate Workspace 1 (revision A)
    let env_info = lease_worktree(config, "mock_config", "test_agent", true, true).unwrap();

    // Verify Workspace 1 sees version 1 (from mock cipd)
    let ws_prebuilt_file = env_info.path.join("prebuilt/tools/mock_tool/file.txt");
    assert_eq!(
        fs::read_to_string(&ws_prebuilt_file).unwrap(),
        "mock_content for fuchsia/tools/mock_tool/linux-amd64 version:1\n"
    );

    // 3. Simulate parent update to revision B (which updates parent's prebuilts to version 2)
    // In our mock, revision B is triggered by committing a change with message "bump root"
    // We modify dummy.txt in parent and commit it.
    let parent_dummy = env.config.fuchsia_dir.join("dummy.txt");
    fs::write(&parent_dummy, "hello v2").unwrap();
    run_setup_cmd("git", &["add", "dummy.txt"], &env.config.fuchsia_dir);
    run_setup_cmd(
        "git",
        &["commit", "-m", "bump root"],
        &env.config.fuchsia_dir,
    );

    // Also manually write to parent's prebuilt to simulate that parent's jiri update
    // would have updated it on disk.
    let parent_prebuilt_dir = env.config.fuchsia_dir.join("prebuilt/tools/mock_tool");
    fs::create_dir_all(&parent_prebuilt_dir).unwrap();
    fs::write(
        parent_prebuilt_dir.join("file.txt"),
        "mock_content parent version:2",
    )
    .unwrap();

    // Verify Workspace 1 STILL sees version 1 (isolation check)
    let content = fs::read_to_string(&ws_prebuilt_file).unwrap();
    assert_eq!(
        content, "mock_content for fuchsia/tools/mock_tool/linux-amd64 version:1\n",
        "Workspace 1 should be isolated from parent prebuilt updates"
    );

    // 4. Free Workspace 1
    release_worktree(config, &env_info.worktree_id).unwrap();

    // 5. Allocate Workspace 2 (uses same slot, now at revision B ➔ version 2)
    let env_info_2 = lease_worktree(config, "mock_config", "test_agent_2", true, true).unwrap();
    assert_eq!(
        env_info_2.worktree_id, env_id,
        "Should reuse the same environment slot"
    );

    // Verify Workspace 2 gets version 2 (from mock cipd)
    let ws2_prebuilt_file = env_info_2.path.join("prebuilt/tools/mock_tool/file.txt");
    assert_eq!(
        fs::read_to_string(&ws2_prebuilt_file).unwrap(),
        "mock_content for fuchsia/tools/mock_tool/linux-amd64 version:2\n"
    );

    // Verify cache before remove
    let cache_dir = env.config.fuchsia_dir.join(".jiri_root/packages");
    let mock_tool_cache = cache_dir.join("prebuilt/tools/mock_tool");
    assert!(mock_tool_cache.join("version:1").exists());
    assert!(mock_tool_cache.join("version:2").exists());

    // Clean up
    release_worktree(config, &env_info_2.worktree_id).unwrap();
    remove_worktree(config, &env_id, false, false).unwrap();

    // Verify GC cleaned up unused versions from cache
    assert!(
        !mock_tool_cache.join("version:1").exists(),
        "version:1 should be GC'ed"
    );
    assert!(
        mock_tool_cache.join("version:2").exists(),
        "version:2 should be kept (parent uses it)"
    );
}

#[test]
fn test_wheel_extraction_mtime_preservation() {
    let _lock = TEST_LOCK.lock().unwrap();
    use fx_worktree::utils::{get_file_mtime, set_file_mtime};
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Create a persistent environment
    let env_id = add_worktree(config, "mock_config", false).unwrap();

    // 2. Allocate Workspace (first run)
    let env_info = lease_worktree(config, "mock_config", "test_agent_1", true, true).unwrap();
    let workspace_path = env_info.path;

    let pydantic_init =
        workspace_path.join("prebuilt/third_party/pydantic-core/pydantic_core/__init__.py");
    assert!(
        pydantic_init.exists(),
        "pydantic_core/__init__.py should be extracted"
    );
    let t1 = get_file_mtime(&pydantic_init).unwrap();

    // Simulate a build by creating the output file with a newer mtime
    let output_file = workspace_path.join(
        "out/mock_config/host_x64/gen/prebuilt/third_party/pydantic-core/pydantic_core/__init__.py",
    );
    fs::create_dir_all(output_file.parent().unwrap()).unwrap();
    fs::write(&output_file, "mock_output").unwrap();

    // Ensure output mtime is strictly newer than t1
    let t2 = t1 + std::time::Duration::from_secs(2);
    set_file_mtime(&output_file, t2).unwrap();

    // 3. Free Workspace
    release_worktree(config, &env_info.worktree_id).unwrap();

    // Wait a bit to ensure 'now' (if the script runs again) would be newer than t2
    std::thread::sleep(std::time::Duration::from_millis(500));

    // 4. Allocate Workspace again (re-use)
    let env_info_2 = lease_worktree(config, "mock_config", "test_agent_1", true, true).unwrap();
    assert_eq!(env_info_2.worktree_id, env_id);

    // Check mtime of the input file after re-use
    let t3 = get_file_mtime(&pydantic_init).unwrap();

    // With the fix, t3 should be equal to t1 (not updated to now).
    // So the output (t2) is still newer than the input (t3).
    // If it failed (re-extracted), t3 would be 'now' (> t2), dirtying the build.
    assert_eq!(
        t3, t1,
        "pydantic_core/__init__.py mtime should be preserved on reuse"
    );
    assert!(t2 > t3, "Build output should remain newer than input");

    // Clean up
    release_worktree(config, &env_info_2.worktree_id).unwrap();
    remove_worktree(config, &env_id, false, false).unwrap();
}

#[test]
fn test_jiri_latest_snapshot_isolation() {
    let _lock = TEST_LOCK.lock().unwrap();
    use fx_worktree::utils::{get_file_mtime, set_file_mtime};
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Create and allocate workspace
    let env_id = add_worktree(config, "mock_config", false).unwrap();
    let env_info = lease_worktree(config, "mock_config", "test_agent_1", true, true).unwrap();
    let workspace_path = env_info.path;

    let ws_latest = workspace_path.join(".jiri_root/update_history/latest");
    assert!(
        ws_latest.exists(),
        "latest snapshot should be copied to workspace"
    );
    let t1 = get_file_mtime(&ws_latest).unwrap();

    // 2. Simulate a parent jiri update by touching the parent's latest snapshot file
    let parent_latest = config.fuchsia_dir.join(".jiri_root/update_history/latest");

    // Wait a bit to ensure the new mtime is different
    std::thread::sleep(std::time::Duration::from_millis(500));

    let new_time = std::time::SystemTime::now();
    set_file_mtime(&parent_latest, new_time).unwrap();

    // 3. Verify workspace latest snapshot mtime did NOT change (isolated)
    let t2 = get_file_mtime(&ws_latest).unwrap();
    assert_eq!(
        t2, t1,
        "Workspace latest snapshot mtime should be isolated from parent updates"
    );

    // Clean up
    release_worktree(config, &env_info.worktree_id).unwrap();
    remove_worktree(config, &env_id, false, false).unwrap();
}

#[test]
fn test_args_gn_mtime_preservation_on_free() {
    let _lock = TEST_LOCK.lock().unwrap();
    use fx_worktree::utils::{get_file_mtime, set_file_mtime};
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Allocate workspace
    let env_id = add_worktree(config, "mock_config", false).unwrap();
    let env_info = lease_worktree(config, "mock_config", "test_agent_1", true, true).unwrap();
    let workspace_path = env_info.path;

    let args_gn = workspace_path.join("out/mock_config/args.gn");
    assert!(args_gn.exists());

    // Set a known mtime on args.gn and args.gn.ref
    let t1 = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1000);
    set_file_mtime(&args_gn, t1).unwrap();

    let args_gn_ref = workspace_path.join("out/mock_config/args.gn.ref");
    fs::write(&args_gn_ref, fs::read_to_string(&args_gn).unwrap()).unwrap(); // ensure contents are identical
    set_file_mtime(&args_gn_ref, t1).unwrap();

    // 2. Free workspace
    release_worktree(config, &env_info.worktree_id).unwrap();

    // 3. Verify args.gn mtime did NOT change
    let t2 = get_file_mtime(&args_gn).unwrap();
    assert_eq!(
        t2, t1,
        "args.gn mtime should be preserved on free if not modified"
    );

    // Clean up
    remove_worktree(config, &env_id, false, false).unwrap();
}

#[test]
fn test_nosync_and_sync() {
    // This test verifies that the `sync` command correctly aligns the worktree's projects
    // to match the parent repository's local git revisions.
    // The mock `jiri worktree sync` is simulated to be buggy (doing nothing to update revisions).
    // Thus, any revision update is driven by our manual project alignment logic.
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Create environment
    let env_id = add_worktree(config, "mock_config", false).unwrap();

    // 2. Update parent repo (simulate changes that would be pulled during sync)
    let parent_dummy = env.config.fuchsia_dir.join("dummy.txt");
    fs::write(&parent_dummy, "hello v2").unwrap();
    run_setup_cmd("git", &["add", "dummy.txt"], &env.config.fuchsia_dir);
    run_setup_cmd(
        "git",
        &["commit", "-m", "bump root"],
        &env.config.fuchsia_dir,
    );

    // 3. Allocate with nosync = true
    let env_info = lease_worktree(config, "mock_config", "test_agent", false, true).unwrap();

    // Verify that workspace dummy.txt is STILL "hello" (not updated to "hello v2")
    let ws_dummy = env_info.path.join("dummy.txt");
    assert_eq!(fs::read_to_string(&ws_dummy).unwrap(), "hello");

    // 4. Release and re-lease with sync = true
    release_worktree(config, &env_info.worktree_id).unwrap();
    let env_info = lease_worktree(config, "mock_config", "test_agent2", true, true).unwrap();

    // Verify that workspace dummy.txt is now "hello v2" (updated)
    assert_eq!(fs::read_to_string(&ws_dummy).unwrap(), "hello v2");

    // Clean up
    release_worktree(config, &env_info.worktree_id).unwrap();
    remove_worktree(config, &env_id, false, false).unwrap();
}

#[test]
fn test_sync_on_subproject_change() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Create environment
    let env_id = add_worktree(config, "mock_config", false).unwrap();

    // 2. Allocate with sync = false
    let env_info = lease_worktree(config, "mock_config", "test_agent", false, true).unwrap();

    // Verify initial state of sub-project
    let ws_sub_dummy = env_info.path.join("third_party/sub/sub_dummy.txt");
    assert_eq!(fs::read_to_string(&ws_sub_dummy).unwrap(), "sub hello");

    // 3. Update sub-project in parent repo
    let parent_sub = env.config.fuchsia_dir.join("third_party/sub");
    let sub_dummy = parent_sub.join("sub_dummy.txt");
    fs::write(&sub_dummy, "sub hello v2").unwrap();
    run_setup_cmd("git", &["add", "sub_dummy.txt"], &parent_sub);
    run_setup_cmd("git", &["commit", "-m", "bump sub"], &parent_sub);

    // Verify that workspace sub-project is STILL "sub hello" (not updated yet)
    assert_eq!(fs::read_to_string(&ws_sub_dummy).unwrap(), "sub hello");

    // 4. Release and re-lease with sync = true
    release_worktree(config, &env_info.worktree_id).unwrap();
    let env_info = lease_worktree(config, "mock_config", "test_agent2", true, true).unwrap();

    // Verify that workspace sub-project is now "sub hello v2" (updated)
    assert_eq!(fs::read_to_string(&ws_sub_dummy).unwrap(), "sub hello v2");

    // Clean up
    release_worktree(config, &env_info.worktree_id).unwrap();
    remove_worktree(config, &env_id, false, false).unwrap();
}

#[test]
fn test_invalid_id_validation() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;

    // Test locate_path with invalid IDs
    assert!(fx_worktree::locate::locate_path(config, Some("../invalid".to_string())).is_err());
    assert!(fx_worktree::locate::locate_path(config, Some("/absolute/path".to_string())).is_err());

    // Test remove_worktree with invalid IDs
    assert!(remove_worktree(config, "../invalid", false, false).is_err());
    assert!(remove_worktree(config, "/absolute/path", false, false).is_err());

    // Test release_worktree with invalid IDs
    assert!(release_worktree(config, "../invalid").is_err());
    assert!(release_worktree(config, "/absolute/path").is_err());
}

#[test]
fn test_release_by_agent_id() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Create two environments
    let env_id1 = add_worktree(config, "mock_config", false).unwrap();
    let env_id2 = add_worktree(config, "mock_config", false).unwrap();

    // 2. Lease first one with agent_id "agent_unique"
    let env_info1 = lease_worktree(config, "mock_config", "agent_unique", true, true).unwrap();
    let leased_id = env_info1.worktree_id.clone();

    // Verify we can release it using agent_id "agent_unique"
    release_worktree(config, "agent_unique").unwrap();

    let lease_file1 = config
        .worktrees_dir()
        .join(&leased_id)
        .join(".jiri_root")
        .join("lease.json");
    assert!(!lease_file1.exists());

    // 3. Lease both with same agent_id "agent_multiple"
    let env_info1 = lease_worktree(config, "mock_config", "agent_multiple", true, true).unwrap();
    let env_info2 = lease_worktree(config, "mock_config", "agent_multiple", true, true).unwrap();

    // Verify releasing by "agent_multiple" fails because it's ambiguous (multiple leases)
    let release_res = release_worktree(config, "agent_multiple");
    assert!(release_res.is_err());
    let err_msg = release_res.unwrap_err().to_string();
    assert!(err_msg.contains("has leased multiple worktrees"));

    // Verify they are still leased
    let lease_file1 = config
        .worktrees_dir()
        .join(&env_info1.worktree_id)
        .join(".jiri_root")
        .join("lease.json");
    let lease_file2 = config
        .worktrees_dir()
        .join(&env_info2.worktree_id)
        .join(".jiri_root")
        .join("lease.json");
    assert!(lease_file1.exists());
    assert!(lease_file2.exists());

    // Clean up individually by worktree ID
    release_worktree(config, &env_info1.worktree_id).unwrap();
    release_worktree(config, &env_info2.worktree_id).unwrap();

    remove_worktree(config, &env_id1, false, false).unwrap();
    remove_worktree(config, &env_id2, false, false).unwrap();
}

#[test]
fn test_mark_reserved_worktree() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;
    use fx_worktree::list::list_worktrees;
    use fx_worktree::locate::locate_path;
    use fx_worktree::mark_reserved::mark_reserved_worktree;
    use fx_worktree::worktree::{WorktreeState, get_worktree_state};

    // 1. Create a worktree (will be Free because add_worktree helper marks it Free)
    let env_id = add_worktree(config, "mock_config", false).unwrap();
    let env_path = config.worktrees_dir().join(&env_id);
    assert!(env_path.exists());
    assert_eq!(get_worktree_state(config, &env_path), WorktreeState::Free);

    // 2. Mark it reserved (should NOT move it, just mark it)
    mark_reserved_worktree(config, &env_id, false).unwrap();

    // 3. Verify it did NOT move
    assert!(env_path.exists());
    assert_eq!(
        get_worktree_state(config, &env_path),
        WorktreeState::Reserved
    );

    // 4. Locate should still find it
    let path = locate_path(config, Some(env_id.clone())).unwrap();
    assert_eq!(path, env_path);

    // 5. List should show it as "Reserved"
    println!("--- List Worktrees after marking reserved ---");
    list_worktrees(config, false).unwrap();
}

#[test]
fn test_mark_free_worktree() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;
    use fx_worktree::list::list_worktrees;
    use fx_worktree::locate::locate_path;
    use fx_worktree::mark_free::mark_free_worktree;
    use fx_worktree::worktree::{WorktreeState, get_worktree_state};

    // 1. Manually add a worktree (it will be Reserved by default because it has no state file)
    let manual_path = config.worktrees_dir().join("my-manual-wt");
    let jiri_bin = config.fuchsia_dir.join(".jiri_root/bin/jiri");
    let jiri_cmd = if jiri_bin.exists() {
        jiri_bin.to_str().unwrap()
    } else {
        "jiri"
    };

    fx_worktree::utils::run_command(
        jiri_cmd,
        &["worktree", "add", manual_path.to_str().unwrap()],
        &config.fuchsia_dir,
        &[],
    )
    .unwrap();

    // Create fake args.gn inside it
    let out_dir = manual_path.join("out/default");
    fs::create_dir_all(&out_dir).unwrap();
    fs::write(
        out_dir.join("args.gn"),
        "build_info_product = \"manual_config\"\nbuild_info_board = \"x64\"\n",
    )
    .unwrap();

    // Verify it is Reserved initially
    assert_eq!(
        get_worktree_state(config, &manual_path),
        WorktreeState::Reserved
    );

    // 2. Mark it free (should NOT move it, just mark it)
    let free_id = mark_free_worktree(config, "my-manual-wt", false).unwrap();
    assert_eq!(free_id, "my-manual-wt");

    // 3. Verify it did NOT move but state is Free
    assert!(manual_path.exists());
    assert_eq!(
        get_worktree_state(config, &manual_path),
        WorktreeState::Free
    );

    // 4. Locate should still find it
    let path = locate_path(config, Some("my-manual-wt".to_string())).unwrap();
    assert_eq!(path, manual_path);

    // 5. List should show it as "Free" (in pool)
    println!("--- List Worktrees after import ---");
    list_worktrees(config, false).unwrap();
}

#[test]
fn test_multiple_configs() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Create a worktree with two configs
    let configs = vec!["config_one".to_string(), "config_two".to_string()];
    let env_id = "multi_config".to_string();
    let env_path = config.worktrees_dir().join(&env_id);

    // Call mock jiri directly to add worktree
    let jiri_bin = config.fuchsia_dir.join(".jiri_root/bin/jiri");
    let status = Command::new(&jiri_bin)
        .args(&["worktree", "add", env_path.to_str().unwrap()])
        .current_dir(&config.fuchsia_dir)
        .status()
        .unwrap();
    assert!(status.success());

    // Write two outdirs
    for cfg in &configs {
        let out_dir = env_path.join("out").join(cfg);
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("args.gn"), "mock_arg = true\n").unwrap();
    }

    // Write .fx-build-dir pointing to the first one
    fs::write(
        env_path.join(".fx-build-dir"),
        format!("out/{}\n", configs[0]),
    )
    .unwrap();

    fx_worktree::worktree::set_worktree_state(
        &env_path,
        fx_worktree::worktree::WorktreeState::Free,
    )
    .unwrap();

    // Verify both outdirs exist
    assert!(env_path.join("out/config_one/args.gn").exists());
    assert!(env_path.join("out/config_two/args.gn").exists());

    // Verify .fx-build-dir points to the first one
    let active_dir = fs::read_to_string(env_path.join(".fx-build-dir")).unwrap();
    assert_eq!(active_dir.trim(), "out/config_one");

    // 2. Verify we can lease it using EITHER config
    let lease_info1 = lease_worktree(config, "config_one", "agent_1", true, true).unwrap();
    assert_eq!(lease_info1.worktree_id, env_id);
    release_worktree(config, &env_id).unwrap();

    let lease_info2 = lease_worktree(config, "config_two", "agent_2", true, true).unwrap();
    assert_eq!(lease_info2.worktree_id, env_id);
    release_worktree(config, &env_id).unwrap();

    // 3. List should show both configs in the OUTDIRS column
    println!("--- List Worktrees with multiple configs ---");
    list_worktrees(config, false).unwrap();

    // Clean up
    remove_worktree(config, &env_id, false, false).unwrap();
}
