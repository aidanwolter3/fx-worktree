use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use tempfile::TempDir;

use fx_worktree::add::add_environment;
use fx_worktree::config::Config;
use fx_worktree::lease::lease_environment;
use fx_worktree::list::list_environments;
use fx_worktree::release::release_worktree;
use fx_worktree::remove::remove_environment;
use fx_worktree::selftest::run_self_test;

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
commit_msg=$(git -C "$cwd" log -n 1 --format=%s 2>/dev/null || echo "")
version="version:1"
if [ "$commit_msg" = "bump root" ]; then
  version="version:2"
fi

if [ "$1" = "project" ] && [ "$2" = "-json-output" ]; then
  output_file=$3
  revision=$(git -C "$base_dir" rev-parse HEAD)
  sub_revision=$(git -C "$base_dir/third_party/sub" rev-parse HEAD)
  cat <<EOF > "$output_file"
[
  {{
    "name": "mock_project",
    "path": "$base_dir",
    "revision": "$revision"
  }},
  {{
    "name": "sub_project",
    "path": "$base_dir/third_party/sub",
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
  root_rev=$(git -C "$base_dir" rev-parse HEAD)
  git -C "$base_dir" worktree add -f --detach "$target_path" "$root_rev"
  
  git_file="$target_path/.git"
  if [ -f "$git_file" ]; then
    gitdir_line=$(head -n 1 "$git_file")
    gitdir_path=${{gitdir_line#gitdir: }}
    rm "$git_file"
    ln -s "$gitdir_path" "$git_file"
    
    if [ -d "$base_dir/.git/jiri" ]; then
      ln -s "$base_dir/.git/jiri" "$gitdir_path/jiri"
    fi
  fi
  
  sub_rev=$(git -C "$base_dir/third_party/sub" rev-parse HEAD)
  mkdir -p "$target_path/third_party/sub"
  git -C "$base_dir/third_party/sub" worktree add -f --detach "$target_path/third_party/sub" "$sub_rev"
  
  sub_git_file="$target_path/third_party/sub/.git"
  if [ -f "$sub_git_file" ]; then
    gitdir_line=$(head -n 1 "$sub_git_file")
    gitdir_path=${{gitdir_line#gitdir: }}
    rm "$sub_git_file"
    ln -s "$gitdir_path" "$sub_git_file"
    
    if [ -d "$base_dir/third_party/sub/.git/jiri" ]; then
      ln -s "$base_dir/third_party/sub/.git/jiri" "$gitdir_path/jiri"
    fi
  fi
elif [ "$1" = "worktree" ] && [ "$2" = "sync" ]; then
  root_rev=$(git -C "$base_dir" rev-parse HEAD)
  git checkout -q "$root_rev"
  
  sub_rev=$(git -C "$base_dir/third_party/sub" rev-parse HEAD)
  if [ -d "third_party/sub" ]; then
    git -C "third_party/sub" checkout -q "$sub_rev"
  fi
  
  ensure_file=$(mktemp)
  cat <<EOF > "$ensure_file"
@Subdir prebuilt/tools/mock_tool
fuchsia/tools/mock_tool/linux-amd64 $version

@Subdir prebuilt/third_party/bazel/linux-x64
infra/3pp/tools/bazel/linux-amd64 $version
EOF
  "$base_dir/.jiri_root/bin/cipd" -root . -ensure-file "$ensure_file"
  rm "$ensure_file"
  
  if [ -f "tools/build/scripts/extract_pydantic_core_wheel.sh" ]; then
    ./tools/build/scripts/extract_pydantic_core_wheel.sh
  fi
  if [ -f "tools/build/scripts/extract_protobuf_py3_wheel.sh" ]; then
    ./tools/build/scripts/extract_protobuf_py3_wheel.sh
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
        target_dir="$root_dir/$current_subdir"
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
  mkdir -p "$build_dir"
  echo "mock_args = true" > "$build_dir/args.gn"
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

    unsafe {
        std::env::set_var("FX_WORKTREE_ROOT", fenv_root_dir.path());
    }

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
    let env_id = add_environment(config, "mock_config", false).unwrap();
    let env_path = config.environments_dir().join(&env_id);
    assert!(env_path.exists());
    assert!(env_path.join("out/default/args.gn").exists());
    assert!(env_path.join("out/default/args.gn.ref").exists());

    println!("--- List Worktrees after add ---");
    list_environments(config, false).unwrap();

    // 2. Test Worktree Lease (reuses the created slot)
    let env_info = lease_environment(config, "mock_config", "test_agent", true, true).unwrap();
    assert_eq!(env_info.agent_id, "test_agent");
    assert_eq!(env_info.config, "mock_config");

    let lease_file = config.leases_dir().join(format!("{}.lease", env_id));
    assert!(lease_file.exists());

    assert!(env_info.path.join(".git").exists());
    assert!(env_info.path.join(".jiri_root").exists());
    assert!(env_info.path.join("out/default").exists());
    assert!(env_info.path.join(".fx-build-dir").exists());

    println!("--- List Worktrees after lease ---");
    list_environments(config, false).unwrap();

    // Test that we cannot delete the environment while leased
    let delete_res = remove_environment(config, &env_id, false);
    assert!(delete_res.is_err());
    assert!(
        delete_res
            .unwrap_err()
            .to_string()
            .contains("Cannot remove worktree")
    );

    // 3. Test Worktree Release
    release_worktree(config, &env_info.environment_id).unwrap();
    assert!(env_info.path.exists()); // Path must remain!
    assert!(env_info.path.join(".fx-worktree-completed").exists());
    assert!(!lease_file.exists()); // Lease must be deleted

    println!("--- List Worktrees after release ---");
    list_environments(config, false).unwrap();

    // 4. Test Worktree Remove (fully cleans up)
    remove_environment(config, &env_id, false).unwrap();
    assert!(!env_path.exists()); // Directory is gone now

    println!("--- List Worktrees after remove ---");
    list_environments(config, false).unwrap();
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
    let env_id = add_environment(config, "mock_config", false).unwrap();
    let env_path = config.environments_dir().join(&env_id);

    // Locate by ID
    let path = locate_path(config, Some(env_id.clone())).unwrap();
    assert_eq!(path, env_path);

    // Locate last created (returns same path)
    let path = locate_path(config, None).unwrap();
    assert_eq!(path, env_path);

    // Allocate
    let env_info = lease_environment(config, "mock_config", "test_agent", true, true).unwrap();

    // Locate by ID (resolves to same path)
    let path = locate_path(config, Some(env_id.clone())).unwrap();
    assert_eq!(path, env_info.path);

    // Locate last created
    let path = locate_path(config, None).unwrap();
    assert_eq!(path, env_info.path);
}

#[test]
fn test_git_symlink_conversion() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Create environment (runs worktree add and converts .git to symlink)
    let env_id = add_environment(config, "mock_config", false).unwrap();
    let env_path = config.environments_dir().join(&env_id);

    let git_file_path = env_path.join(".git");
    assert!(git_file_path.exists());
    let metadata = fs::symlink_metadata(&git_file_path).unwrap();
    assert!(metadata.file_type().is_symlink());

    // 2. Allocate (keeps symlink)
    let env_info = lease_environment(config, "mock_config", "test_agent", true, true).unwrap();
    let metadata = fs::symlink_metadata(&git_file_path).unwrap();
    assert!(metadata.file_type().is_symlink());

    // 3. Free (keeps symlink)
    release_worktree(config, &env_info.environment_id).unwrap();
    let metadata = fs::symlink_metadata(&git_file_path).unwrap();
    assert!(metadata.file_type().is_symlink());

    // 4. Delete (converts it back to file, and removes it)
    remove_environment(config, &env_id, false).unwrap();
    assert!(!env_path.exists());
}

#[test]
fn test_self_test_command() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;

    // Create environment first to get an ID
    let env_id = add_environment(config, "mock_config", false).unwrap();

    let build_gn = config.fuchsia_dir.join("BUILD.gn");
    fs::write(&build_gn, "# mock root BUILD.gn").unwrap();

    run_setup_cmd("git", &["add", "BUILD.gn"], &config.fuchsia_dir);
    run_setup_cmd(
        "git",
        &["commit", "-m", "add BUILD.gn"],
        &config.fuchsia_dir,
    );

    // Run self-test
    run_self_test(config, env_id).unwrap();
}

#[test]
fn test_mtime_and_metadata_preservation() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Create environment
    let env_id = add_environment(config, "mock_config", false).unwrap();
    let env_path = config.environments_dir().join(&env_id);

    // Verify metadata files were copied during create
    assert!(
        env_path
            .join("sdk/ctf/build/internal/ctf_releases.gni")
            .exists()
    );
    assert!(
        env_path
            .join("build/info/jiri_generated/commit_info")
            .exists()
    );
    assert!(env_path.join("build/cipd.gni").exists());

    // 2. Allocate
    let env_info = lease_environment(config, "mock_config", "test_agent", true, true).unwrap();

    // Verify index exists
    let index_path = env_info.path.join(".git/index");
    assert!(index_path.exists());

    // Record mtime of index and a source file
    let index_mtime_before = fs::metadata(&index_path).unwrap().modified().unwrap();

    // We need a tracked file to check. dummy.txt is tracked.
    let dummy_path = env_info.path.join("dummy.txt");
    let dummy_mtime_before = fs::metadata(&dummy_path).unwrap().modified().unwrap();

    // Sleep a bit to ensure time moves forward
    std::thread::sleep(std::time::Duration::from_millis(100));

    // 3. Free
    release_worktree(config, &env_info.environment_id).unwrap();

    // Verify metadata files STILL exist (not deleted by clean)
    assert!(
        env_path
            .join("sdk/ctf/build/internal/ctf_releases.gni")
            .exists()
    );
    assert!(
        env_path
            .join("build/info/jiri_generated/commit_info")
            .exists()
    );
    assert!(env_path.join("build/cipd.gni").exists());

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

    // Sleep again
    std::thread::sleep(std::time::Duration::from_millis(100));

    // 4. Allocate again (no-op case)
    let env_info_2 = lease_environment(config, "mock_config", "test_agent_2", true, true).unwrap();

    // Verify index mtime is preserved after no-op allocate
    let index_mtime_after_alloc = fs::metadata(&index_path).unwrap().modified().unwrap();
    assert_eq!(
        index_mtime_before, index_mtime_after_alloc,
        "Index mtime should be preserved after no-op allocate"
    );

    // Verify dummy.txt mtime is preserved
    let dummy_mtime_after_alloc = fs::metadata(&dummy_path).unwrap().modified().unwrap();
    assert_eq!(
        dummy_mtime_before, dummy_mtime_after_alloc,
        "dummy.txt mtime should be preserved after no-op allocate"
    );

    // Clean up
    release_worktree(config, &env_info_2.environment_id).unwrap();
    remove_environment(config, &env_id, false).unwrap();
}

#[test]
fn test_parent_jiri_update() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Create environment
    let env_id = add_environment(config, "mock_config", false).unwrap();

    // 2. Allocate (first time, gets initial revisions)
    let env_info = lease_environment(config, "mock_config", "test_agent", true, true).unwrap();

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
    release_worktree(config, &env_info.environment_id).unwrap();

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
    let env_info_2 = lease_environment(config, "mock_config", "test_agent_2", true, true).unwrap();
    assert_eq!(
        env_info_2.environment_id, env_id,
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
    release_worktree(config, &env_info_2.environment_id).unwrap();
    remove_environment(config, &env_id, false).unwrap();
}

#[test]
fn test_prebuilt_isolation() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Create environment
    let env_id = add_environment(config, "mock_config", false).unwrap();

    // 2. Allocate Workspace 1 (revision A)
    let env_info = lease_environment(config, "mock_config", "test_agent", true, true).unwrap();

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
    release_worktree(config, &env_info.environment_id).unwrap();

    // 5. Allocate Workspace 2 (uses same slot, now at revision B ➔ version 2)
    let env_info_2 = lease_environment(config, "mock_config", "test_agent_2", true, true).unwrap();
    assert_eq!(
        env_info_2.environment_id, env_id,
        "Should reuse the same environment slot"
    );

    // Verify Workspace 2 gets version 2 (from mock cipd)
    let ws2_prebuilt_file = env_info_2.path.join("prebuilt/tools/mock_tool/file.txt");
    assert_eq!(
        fs::read_to_string(&ws2_prebuilt_file).unwrap(),
        "mock_content for fuchsia/tools/mock_tool/linux-amd64 version:2\n"
    );

    // Clean up
    release_worktree(config, &env_info_2.environment_id).unwrap();
    remove_environment(config, &env_id, false).unwrap();
}

#[test]
fn test_wheel_extraction_mtime_preservation() {
    let _lock = TEST_LOCK.lock().unwrap();
    use fx_worktree::utils::{get_file_mtime, set_file_mtime};
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Create a persistent environment
    let env_id = add_environment(config, "mock_config", false).unwrap();

    // 2. Allocate Workspace (first run)
    let env_info = lease_environment(config, "mock_config", "test_agent_1", true, true).unwrap();
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
        "out/default/host_x64/gen/prebuilt/third_party/pydantic-core/pydantic_core/__init__.py",
    );
    fs::create_dir_all(output_file.parent().unwrap()).unwrap();
    fs::write(&output_file, "mock_output").unwrap();

    // Ensure output mtime is strictly newer than t1
    let t2 = t1 + std::time::Duration::from_secs(2);
    set_file_mtime(&output_file, t2).unwrap();

    // 3. Free Workspace
    release_worktree(config, &env_info.environment_id).unwrap();

    // Wait a bit to ensure 'now' (if the script runs again) would be newer than t2
    std::thread::sleep(std::time::Duration::from_millis(500));

    // 4. Allocate Workspace again (re-use)
    let env_info_2 = lease_environment(config, "mock_config", "test_agent_1", true, true).unwrap();
    assert_eq!(env_info_2.environment_id, env_id);

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
    release_worktree(config, &env_info_2.environment_id).unwrap();
    remove_environment(config, &env_id, false).unwrap();
}

#[test]
fn test_jiri_latest_snapshot_isolation() {
    let _lock = TEST_LOCK.lock().unwrap();
    use fx_worktree::utils::{get_file_mtime, set_file_mtime};
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Create and allocate workspace
    let env_id = add_environment(config, "mock_config", false).unwrap();
    let env_info = lease_environment(config, "mock_config", "test_agent_1", true, true).unwrap();
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
    release_worktree(config, &env_info.environment_id).unwrap();
    remove_environment(config, &env_id, false).unwrap();
}



#[test]
fn test_args_gn_mtime_preservation_on_free() {
    let _lock = TEST_LOCK.lock().unwrap();
    use fx_worktree::utils::{get_file_mtime, set_file_mtime};
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Allocate workspace
    let env_id = add_environment(config, "mock_config", false).unwrap();
    let env_info = lease_environment(config, "mock_config", "test_agent_1", true, true).unwrap();
    let workspace_path = env_info.path;

    let args_gn = workspace_path.join("out/default/args.gn");
    assert!(args_gn.exists());

    // Set a known mtime on args.gn and args.gn.ref
    let t1 = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1000);
    set_file_mtime(&args_gn, t1).unwrap();

    let args_gn_ref = workspace_path.join("out/default/args.gn.ref");
    fs::write(&args_gn_ref, fs::read_to_string(&args_gn).unwrap()).unwrap(); // ensure contents are identical
    set_file_mtime(&args_gn_ref, t1).unwrap();

    // 2. Free workspace
    release_worktree(config, &env_info.environment_id).unwrap();

    // 3. Verify args.gn mtime did NOT change
    let t2 = get_file_mtime(&args_gn).unwrap();
    assert_eq!(
        t2, t1,
        "args.gn mtime should be preserved on free if not modified"
    );

    // Clean up
    remove_environment(config, &env_id, false).unwrap();
}

#[test]
fn test_bazel_package_copying() {
    let _lock = TEST_LOCK.lock().unwrap();
    use fx_worktree::utils::get_file_mtime;
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Allocate workspace
    let env_id = add_environment(config, "mock_config", false).unwrap();
    let env_info = lease_environment(config, "mock_config", "test_agent_1", true, true).unwrap();
    let workspace_path = env_info.path;

    // 2. Verify mock_tool is symlinked
    let ws_mock_tool = workspace_path.join("prebuilt/tools/mock_tool");
    assert!(ws_mock_tool.exists());
    assert!(ws_mock_tool.is_symlink(), "mock_tool should be symlinked");

    // 3. Verify Bazel package is copied (not symlinked)
    let ws_bazel = workspace_path.join("prebuilt/third_party/bazel/linux-x64");
    assert!(ws_bazel.exists());
    assert!(
        !ws_bazel.is_symlink(),
        "Bazel package should be a real directory (copied)"
    );
    assert!(
        ws_bazel.join("file.txt").exists(),
        "Bazel package contents should be copied"
    );

    let version_marker = ws_bazel.join(".fxenv_source_cache");
    assert!(
        version_marker.exists(),
        "Version marker should be written in workspace copy"
    );

    let t1 = get_file_mtime(&ws_bazel.join("file.txt")).unwrap();

    // 4. Free workspace
    release_worktree(config, &env_info.environment_id).unwrap();

    // Wait a bit
    std::thread::sleep(std::time::Duration::from_millis(500));

    // 5. Allocate again (re-use)
    let env_info_2 = lease_environment(config, "mock_config", "test_agent_1", true, true).unwrap();
    assert_eq!(env_info_2.environment_id, env_id);

    // Verify Bazel was NOT re-copied (mtime preserved)
    let ws_bazel2 = env_info_2.path.join("prebuilt/third_party/bazel/linux-x64");
    let t2 = get_file_mtime(&ws_bazel2.join("file.txt")).unwrap();
    assert_eq!(
        t2, t1,
        "Bazel package should not be re-copied if version is unchanged"
    );

    // Clean up
    release_worktree(config, &env_info_2.environment_id).unwrap();
    remove_environment(config, &env_id, false).unwrap();
}

#[test]
fn test_nosync_and_sync() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Create environment
    let env_id = add_environment(config, "mock_config", false).unwrap();

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
    let env_info = lease_environment(config, "mock_config", "test_agent", false, true).unwrap();

    // Verify that workspace dummy.txt is STILL "hello" (not updated to "hello v2")
    let ws_dummy = env_info.path.join("dummy.txt");
    assert_eq!(fs::read_to_string(&ws_dummy).unwrap(), "hello");

    // 4. Run sync
    fx_worktree::sync::sync_environment_by_id(config, &env_info.environment_id, true).unwrap();

    // Verify that workspace dummy.txt is now "hello v2" (updated)
    assert_eq!(fs::read_to_string(&ws_dummy).unwrap(), "hello v2");

    // Clean up
    release_worktree(config, &env_info.environment_id).unwrap();
    remove_environment(config, &env_id, false).unwrap();
}

#[test]
fn test_invalid_id_validation() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;

    // Test locate_path with invalid IDs
    assert!(fx_worktree::locate::locate_path(config, Some("../invalid".to_string())).is_err());
    assert!(fx_worktree::locate::locate_path(config, Some("/absolute/path".to_string())).is_err());

    // Test remove_environment with invalid IDs
    assert!(remove_environment(config, "../invalid", false).is_err());
    assert!(remove_environment(config, "/absolute/path", false).is_err());

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
    let env_id1 = add_environment(config, "mock_config", false).unwrap();
    let env_id2 = add_environment(config, "mock_config", false).unwrap();

    // 2. Lease first one with agent_id "agent_unique"
    let env_info1 = lease_environment(config, "mock_config", "agent_unique", true, true).unwrap();
    let leased_id = env_info1.environment_id.clone();

    // Verify we can release it using agent_id "agent_unique"
    release_worktree(config, "agent_unique").unwrap();

    let lease_file1 = config.leases_dir().join(format!("{}.lease", leased_id));
    assert!(!lease_file1.exists());

    // 3. Lease both with same agent_id "agent_multiple"
    let env_info1 = lease_environment(config, "mock_config", "agent_multiple", true, true).unwrap();
    let env_info2 = lease_environment(config, "mock_config", "agent_multiple", true, true).unwrap();

    // Verify releasing by "agent_multiple" fails because it's ambiguous (multiple leases)
    let release_res = release_worktree(config, "agent_multiple");
    assert!(release_res.is_err());
    let err_msg = release_res.unwrap_err().to_string();
    assert!(err_msg.contains("has leased multiple worktrees"));

    // Verify they are still leased
    let lease_file1 = config
        .leases_dir()
        .join(format!("{}.lease", env_info1.environment_id));
    let lease_file2 = config
        .leases_dir()
        .join(format!("{}.lease", env_info2.environment_id));
    assert!(lease_file1.exists());
    assert!(lease_file2.exists());

    // Clean up individually by worktree ID
    release_worktree(config, &env_info1.environment_id).unwrap();
    release_worktree(config, &env_info2.environment_id).unwrap();

    remove_environment(config, &env_id1, false).unwrap();
    remove_environment(config, &env_id2, false).unwrap();
}
