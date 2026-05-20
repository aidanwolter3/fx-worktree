use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use tempfile::TempDir;

use fxenv::config::Config;
use fxenv::create::create_environment;
use fxenv::delete::delete_environment;
use fxenv::allocate::allocate_environment;
use fxenv::free::free_environment_by_id;
use fxenv::gc::garbage_collect;
use fxenv::list::list_environments;
use fxenv::selftest::run_self_test;

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
    run_setup_cmd("git", &["config", "user.email", "test@example.com"], fuchsia_path);

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
    run_setup_cmd("git", &["config", "user.email", "test@example.com"], &sub_path);
    fs::write(sub_path.join("sub_dummy.txt"), "sub hello").unwrap();
    run_setup_cmd("git", &["add", "sub_dummy.txt"], &sub_path);
    run_setup_cmd("git", &["commit", "-m", "sub initial commit"], &sub_path);

    // Create mock Jiri update history and config
    let update_history_dir = fuchsia_path.join(".jiri_root/update_history");
    fs::create_dir_all(&update_history_dir).unwrap();
    fs::write(update_history_dir.join("latest"), "<manifest></manifest>").unwrap();
    fs::write(fuchsia_path.join(".jiri_root/config"), "<config></config>").unwrap();

    // 2. Create mock jiri
    let jiri_dir = fuchsia_path.join(".jiri_root/bin");
    fs::create_dir_all(&jiri_dir).unwrap();
    let jiri_path = jiri_dir.join("jiri");

    let jiri_script = format!(
        r#"#!/bin/bash
cwd=$(pwd)
commit_msg=$(git -C "$cwd" log -n 1 --format=%s 2>/dev/null || echo "")
version="version:1"
if [ "$commit_msg" = "bump root" ]; then
  version="version:2"
fi

if [ "$1" = "project" ] && [ "$2" = "-json-output" ]; then
  output_file=$3
  revision=$(git -C "{}" rev-parse HEAD)
  sub_revision=$(git -C "{}/third_party/sub" rev-parse HEAD)
  cat <<EOF > "$output_file"
[
  {{
    "name": "mock_project",
    "path": "{}",
    "revision": "$revision"
  }},
  {{
    "name": "sub_project",
    "path": "{}/third_party/sub",
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
fi
"#,
        fuchsia_path.to_str().unwrap(),
        fuchsia_path.to_str().unwrap(),
        fuchsia_path.to_str().unwrap(),
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
  obj_dir="$build_dir/obj/sdk/ctf/tests/fidl/fuchsia.diagnostics"
  mkdir -p "$obj_dir"
  touch "$obj_dir/inspect-publisher.inspect_publisher.cc.o"
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
    run_setup_cmd("git", &[
        "add", 
        "dummy.txt", 
        "scripts/fx", 
        "tools/build/scripts/extract_pydantic_core_wheel.sh", 
        "tools/build/scripts/extract_protobuf_py3_wheel.sh"
    ], fuchsia_path);
    run_setup_cmd("git", &["commit", "-m", "initial commit with mocks"], fuchsia_path);

    unsafe {
        std::env::set_var("FXENV_ROOT", fenv_root_dir.path());
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
    let env_id = create_environment(config, "mock_config").unwrap();
    let env_path = config.environments_dir().join(&env_id);
    assert!(env_path.exists());
    assert!(env_path.join("out/default/args.gn").exists());
    assert!(env_path.join("out/default/args.gn.ref").exists());

    println!("--- List Environments after create ---");
    list_environments(config, false).unwrap();

    // 2. Test Environment Allocate (reuses the created slot)
    let env_info = allocate_environment(config, "mock_config", "test_agent", true).unwrap();
    assert_eq!(env_info.agent_id, "test_agent");
    assert_eq!(env_info.config, "mock_config");

    let lease_file = config.leases_dir().join(format!("{}.lease", env_id));
    assert!(lease_file.exists());

    assert!(env_info.path.join(".git").exists());
    assert!(env_info.path.join(".jiri_root").exists());
    assert!(env_info.path.join("out/default").exists());
    assert!(env_info.path.join(".fx-build-dir").exists());

    println!("--- List Environments after allocate ---");
    list_environments(config, false).unwrap();

    // Test that we cannot delete the environment while leased
    let delete_res = delete_environment(config, &env_id);
    assert!(delete_res.is_err());
    assert!(delete_res.unwrap_err().to_string().contains("Cannot delete environment"));

    // 3. Test Environment Free
    free_environment_by_id(config, &env_info.environment_id).unwrap();
    assert!(env_info.path.exists()); // Path must remain!
    assert!(env_info.path.join(".fxenv-completed").exists());
    assert!(!lease_file.exists()); // Lease must be deleted

    println!("--- List Environments after free ---");
    list_environments(config, false).unwrap();

    // 4. Test Environment Delete (fully cleans up)
    delete_environment(config, &env_id).unwrap();
    assert!(!env_path.exists()); // Directory is gone now

    println!("--- List Environments after delete ---");
    list_environments(config, false).unwrap();
}

#[test]
fn test_gc() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;

    // Create and allocate
    let env_id = create_environment(config, "mock_config").unwrap();
    let env_info = allocate_environment(config, "mock_config", "test_agent", true).unwrap();

    let lease_file = config.leases_dir().join(format!("{}.lease", env_id));
    assert!(lease_file.exists());

    // Run GC with 0 timeout
    garbage_collect(config, 0).unwrap();

    assert!(!lease_file.exists());
    assert!(env_info.path.exists()); // Workspace remains!
    assert!(env_info.path.join(".fxenv-completed").exists());
}

#[test]
fn test_locate_path() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;
    use fxenv::locate::locate_path;

    // Test last created fallback (errors initially)
    assert!(locate_path(config, None).is_err());

    // Create environment
    let env_id = create_environment(config, "mock_config").unwrap();
    let env_path = config.environments_dir().join(&env_id);

    // Locate by ID
    let path = locate_path(config, Some(env_id.clone())).unwrap();
    assert_eq!(path, env_path);

    // Locate last created (returns same path)
    let path = locate_path(config, None).unwrap();
    assert_eq!(path, env_path);

    // Allocate
    let env_info = allocate_environment(config, "mock_config", "test_agent", true).unwrap();

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
    let env_id = create_environment(config, "mock_config").unwrap();
    let env_path = config.environments_dir().join(&env_id);

    let git_file_path = env_path.join(".git");
    assert!(git_file_path.exists());
    let metadata = fs::symlink_metadata(&git_file_path).unwrap();
    assert!(metadata.file_type().is_symlink());

    // 2. Allocate (keeps symlink)
    let env_info = allocate_environment(config, "mock_config", "test_agent", true).unwrap();
    let metadata = fs::symlink_metadata(&git_file_path).unwrap();
    assert!(metadata.file_type().is_symlink());

    // 3. Free (keeps symlink)
    free_environment_by_id(config, &env_info.environment_id).unwrap();
    let metadata = fs::symlink_metadata(&git_file_path).unwrap();
    assert!(metadata.file_type().is_symlink());

    // 4. Delete (converts it back to file, and removes it)
    delete_environment(config, &env_id).unwrap();
    assert!(!env_path.exists());
}

#[test]
fn test_self_test_command() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;

    let src_dir = config
        .fuchsia_dir
        .join("sdk/ctf/tests/fidl/fuchsia.diagnostics");
    fs::create_dir_all(&src_dir).unwrap();
    let src_file = src_dir.join("inspect_publisher.cc");
    fs::write(&src_file, "numeric_properties.RecordInt(\"int\", -1);").unwrap();

    run_setup_cmd(
        "git",
        &["add", "sdk/ctf/tests/fidl/fuchsia.diagnostics/inspect_publisher.cc"],
        &config.fuchsia_dir,
    );
    run_setup_cmd(
        "git",
        &["commit", "-m", "add inspect_publisher.cc"],
        &config.fuchsia_dir,
    );

    // Run self-test
    run_self_test(config, None).unwrap();
}

#[test]
fn test_mtime_and_metadata_preservation() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Create environment
    let env_id = create_environment(config, "mock_config").unwrap();
    let env_path = config.environments_dir().join(&env_id);

    // Verify metadata files were copied during create
    assert!(env_path.join("sdk/ctf/build/internal/ctf_releases.gni").exists());
    assert!(env_path.join("build/info/jiri_generated/commit_info").exists());
    assert!(env_path.join("build/cipd.gni").exists());

    // 2. Allocate
    let env_info = allocate_environment(config, "mock_config", "test_agent", true).unwrap();
    
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
    free_environment_by_id(config, &env_info.environment_id).unwrap();

    // Verify metadata files STILL exist (not deleted by clean)
    assert!(env_path.join("sdk/ctf/build/internal/ctf_releases.gni").exists());
    assert!(env_path.join("build/info/jiri_generated/commit_info").exists());
    assert!(env_path.join("build/cipd.gni").exists());

    // Verify index mtime is preserved
    let index_mtime_after_free = fs::metadata(&index_path).unwrap().modified().unwrap();
    assert_eq!(index_mtime_before, index_mtime_after_free, "Index mtime should be preserved after free");

    // Verify dummy.txt mtime is preserved
    let dummy_mtime_after_free = fs::metadata(&dummy_path).unwrap().modified().unwrap();
    assert_eq!(dummy_mtime_before, dummy_mtime_after_free, "dummy.txt mtime should be preserved after free");

    // Sleep again
    std::thread::sleep(std::time::Duration::from_millis(100));

    // 4. Allocate again (no-op case)
    let env_info_2 = allocate_environment(config, "mock_config", "test_agent_2", true).unwrap();

    // Verify index mtime is preserved after no-op allocate
    let index_mtime_after_alloc = fs::metadata(&index_path).unwrap().modified().unwrap();
    assert_eq!(index_mtime_before, index_mtime_after_alloc, "Index mtime should be preserved after no-op allocate");

    // Verify dummy.txt mtime is preserved
    let dummy_mtime_after_alloc = fs::metadata(&dummy_path).unwrap().modified().unwrap();
    assert_eq!(dummy_mtime_before, dummy_mtime_after_alloc, "dummy.txt mtime should be preserved after no-op allocate");

    // Clean up
    free_environment_by_id(config, &env_info_2.environment_id).unwrap();
    delete_environment(config, &env_id).unwrap();
}

#[test]
fn test_parent_jiri_update() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Create environment
    let env_id = create_environment(config, "mock_config").unwrap();

    // 2. Allocate (first time, gets initial revisions)
    let env_info = allocate_environment(config, "mock_config", "test_agent", true).unwrap();

    // Verify initial content
    assert_eq!(fs::read_to_string(env_info.path.join("dummy.txt")).unwrap(), "hello");
    assert_eq!(fs::read_to_string(env_info.path.join("third_party/sub/sub_dummy.txt")).unwrap(), "sub hello");

    // 3. Free environment
    free_environment_by_id(config, &env_info.environment_id).unwrap();

    // 4. Update parent repo (simulate jiri update)
    let parent_dummy = env.config.fuchsia_dir.join("dummy.txt");
    fs::write(&parent_dummy, "hello v2").unwrap();
    run_setup_cmd("git", &["add", "dummy.txt"], &env.config.fuchsia_dir);
    run_setup_cmd("git", &["commit", "-m", "bump root"], &env.config.fuchsia_dir);

    let parent_sub_path = env.config.fuchsia_dir.join("third_party/sub");
    let parent_sub_dummy = parent_sub_path.join("sub_dummy.txt");
    fs::write(&parent_sub_dummy, "sub hello v2").unwrap();
    run_setup_cmd("git", &["add", "sub_dummy.txt"], &parent_sub_path);
    run_setup_cmd("git", &["commit", "-m", "bump sub"], &parent_sub_path);

    // 5. Allocate again (should reuse same slot but update revisions)
    let env_info_2 = allocate_environment(config, "mock_config", "test_agent_2", true).unwrap();
    assert_eq!(env_info_2.environment_id, env_id, "Should reuse the same environment slot");

    // Verify updated content in workspace
    assert_eq!(fs::read_to_string(env_info_2.path.join("dummy.txt")).unwrap(), "hello v2");
    assert_eq!(fs::read_to_string(env_info_2.path.join("third_party/sub/sub_dummy.txt")).unwrap(), "sub hello v2");

    // Clean up
    free_environment_by_id(config, &env_info_2.environment_id).unwrap();
    delete_environment(config, &env_id).unwrap();
}

#[test]
fn test_prebuilt_isolation() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Create environment
    let env_id = create_environment(config, "mock_config").unwrap();

    // 2. Allocate Workspace 1 (revision A)
    let env_info = allocate_environment(config, "mock_config", "test_agent", true).unwrap();

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
    run_setup_cmd("git", &["commit", "-m", "bump root"], &env.config.fuchsia_dir);

    // Also manually write to parent's prebuilt to simulate that parent's jiri update
    // would have updated it on disk.
    let parent_prebuilt_dir = env.config.fuchsia_dir.join("prebuilt/tools/mock_tool");
    fs::create_dir_all(&parent_prebuilt_dir).unwrap();
    fs::write(parent_prebuilt_dir.join("file.txt"), "mock_content parent version:2").unwrap();

    // Verify Workspace 1 STILL sees version 1 (isolation check)
    let content = fs::read_to_string(&ws_prebuilt_file).unwrap();
    assert_eq!(
        content,
        "mock_content for fuchsia/tools/mock_tool/linux-amd64 version:1\n",
        "Workspace 1 should be isolated from parent prebuilt updates"
    );

    // 4. Free Workspace 1
    free_environment_by_id(config, &env_info.environment_id).unwrap();

    // 5. Allocate Workspace 2 (uses same slot, now at revision B ➔ version 2)
    let env_info_2 = allocate_environment(config, "mock_config", "test_agent_2", true).unwrap();
    assert_eq!(env_info_2.environment_id, env_id, "Should reuse the same environment slot");

    // Verify Workspace 2 gets version 2 (from mock cipd)
    let ws2_prebuilt_file = env_info_2.path.join("prebuilt/tools/mock_tool/file.txt");
    assert_eq!(
        fs::read_to_string(&ws2_prebuilt_file).unwrap(),
        "mock_content for fuchsia/tools/mock_tool/linux-amd64 version:2\n"
    );

    // Clean up
    free_environment_by_id(config, &env_info_2.environment_id).unwrap();
    delete_environment(config, &env_id).unwrap();
}

#[test]
fn test_wheel_extraction_mtime_preservation() {
    let _lock = TEST_LOCK.lock().unwrap();
    use fxenv::utils::{get_file_mtime, set_file_mtime};
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Create a persistent environment
    let env_id = create_environment(config, "mock_config").unwrap();

    // 2. Allocate Workspace (first run)
    let env_info = allocate_environment(config, "mock_config", "test_agent_1", true).unwrap();
    let workspace_path = env_info.path;

    let pydantic_init = workspace_path.join("prebuilt/third_party/pydantic-core/pydantic_core/__init__.py");
    assert!(pydantic_init.exists(), "pydantic_core/__init__.py should be extracted");
    let t1 = get_file_mtime(&pydantic_init).unwrap();

    // Simulate a build by creating the output file with a newer mtime
    let output_file = workspace_path.join("out/default/host_x64/gen/prebuilt/third_party/pydantic-core/pydantic_core/__init__.py");
    fs::create_dir_all(output_file.parent().unwrap()).unwrap();
    fs::write(&output_file, "mock_output").unwrap();

    // Ensure output mtime is strictly newer than t1
    let t2 = t1 + std::time::Duration::from_secs(2);
    set_file_mtime(&output_file, t2).unwrap();

    // 3. Free Workspace
    free_environment_by_id(config, &env_info.environment_id).unwrap();

    // Wait a bit to ensure 'now' (if the script runs again) would be newer than t2
    std::thread::sleep(std::time::Duration::from_millis(500));

    // 4. Allocate Workspace again (re-use)
    let env_info_2 = allocate_environment(config, "mock_config", "test_agent_1", true).unwrap();
    assert_eq!(env_info_2.environment_id, env_id);

    // Check mtime of the input file after re-use
    let t3 = get_file_mtime(&pydantic_init).unwrap();

    // With the fix, t3 should be equal to t1 (not updated to now).
    // So the output (t2) is still newer than the input (t3).
    // If it failed (re-extracted), t3 would be 'now' (> t2), dirtying the build.
    assert_eq!(t3, t1, "pydantic_core/__init__.py mtime should be preserved on reuse");
    assert!(t2 > t3, "Build output should remain newer than input");

    // Clean up
    free_environment_by_id(config, &env_info_2.environment_id).unwrap();
    delete_environment(config, &env_id).unwrap();
}

#[test]
fn test_jiri_latest_snapshot_isolation() {
    let _lock = TEST_LOCK.lock().unwrap();
    use fxenv::utils::{get_file_mtime, set_file_mtime};
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Create and allocate workspace
    let env_id = create_environment(config, "mock_config").unwrap();
    let env_info = allocate_environment(config, "mock_config", "test_agent_1", true).unwrap();
    let workspace_path = env_info.path;

    let ws_latest = workspace_path.join(".jiri_root/update_history/latest");
    assert!(ws_latest.exists(), "latest snapshot should be copied to workspace");
    let t1 = get_file_mtime(&ws_latest).unwrap();

    // 2. Simulate a parent jiri update by touching the parent's latest snapshot file
    let parent_latest = config.fuchsia_dir.join(".jiri_root/update_history/latest");
    
    // Wait a bit to ensure the new mtime is different
    std::thread::sleep(std::time::Duration::from_millis(500));
    
    let new_time = std::time::SystemTime::now();
    set_file_mtime(&parent_latest, new_time).unwrap();

    // 3. Verify workspace latest snapshot mtime did NOT change (isolated)
    let t2 = get_file_mtime(&ws_latest).unwrap();
    assert_eq!(t2, t1, "Workspace latest snapshot mtime should be isolated from parent updates");

    // Clean up
    free_environment_by_id(config, &env_info.environment_id).unwrap();
    delete_environment(config, &env_id).unwrap();
}

#[test]
fn test_existing_cache_clamping_migration() {
    let _lock = TEST_LOCK.lock().unwrap();
    use fxenv::utils::{get_file_mtime, set_file_mtime};
    use fxenv::allocate::{JiriPackage, calculate_group_hash};
    
    let env = setup_mock_env();
    let config = &env.config;

    let shared_prebuilts_dir = config.fxenv_root.join("shared-prebuilts");
    
    let pkgs = vec![
        JiriPackage {
            name: "fuchsia/tools/mock_tool/linux-amd64".to_string(),
            path: "prebuilt/tools/mock_tool".to_string(),
            version: "version:1".to_string(),
            platforms: Some(vec!["linux-amd64".to_string()]),
        }
    ];
    let hash = calculate_group_hash(&pkgs);
    let escaped_path = "prebuilt_tools_mock_tool";
    let cache_subdir = format!("merged/{}/{}", escaped_path, hash);
    let shared_pkg_dir = shared_prebuilts_dir.join(&cache_subdir);
    
    fs::create_dir_all(&shared_pkg_dir).unwrap();
    
    // Create .versions directory to simulate successful CIPD installation (cache hit)
    fs::create_dir_all(shared_pkg_dir.join(".versions")).unwrap();
    
    // Create a file with 2042 mtime
    let file_2042 = shared_pkg_dir.join("file_2042.txt");
    fs::write(&file_2042, "mock content").unwrap();
    
    let future_time = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(2290204800); // 2042-07-28
    set_file_mtime(&file_2042, future_time).unwrap();
    
    // 2. Allocate workspace (this will be a cache hit)
    let env_id = create_environment(config, "mock_config").unwrap();
    let env_info = allocate_environment(config, "mock_config", "test_agent_1", true).unwrap();
    let workspace_path = env_info.path;
    
    let ws_file = workspace_path.join("prebuilt/tools/mock_tool/file_2042.txt");
    assert!(ws_file.exists());
    
    let mtime = get_file_mtime(&ws_file).unwrap();
    
    // With the old code, mtime will still be 2042.
    // With the fix, mtime will be clamped to 2020-01-01.
    let expected_clamp_time = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1577836800); // 2020-01-01
    assert_eq!(mtime, expected_clamp_time, "Existing cache files should be clamped on allocation");
    
    // Clean up
    free_environment_by_id(config, &env_info.environment_id).unwrap();
    delete_environment(config, &env_id).unwrap();
}

#[test]
fn test_args_gn_mtime_preservation_on_free() {
    let _lock = TEST_LOCK.lock().unwrap();
    use fxenv::utils::{get_file_mtime, set_file_mtime};
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Allocate workspace
    let env_id = create_environment(config, "mock_config").unwrap();
    let env_info = allocate_environment(config, "mock_config", "test_agent_1", true).unwrap();
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
    free_environment_by_id(config, &env_info.environment_id).unwrap();

    // 3. Verify args.gn mtime did NOT change
    let t2 = get_file_mtime(&args_gn).unwrap();
    assert_eq!(t2, t1, "args.gn mtime should be preserved on free if not modified");

    // Clean up
    delete_environment(config, &env_id).unwrap();
}

#[test]
fn test_bazel_package_copying() {
    let _lock = TEST_LOCK.lock().unwrap();
    use fxenv::utils::get_file_mtime;
    let env = setup_mock_env();
    let config = &env.config;

    // 1. Allocate workspace
    let env_id = create_environment(config, "mock_config").unwrap();
    let env_info = allocate_environment(config, "mock_config", "test_agent_1", true).unwrap();
    let workspace_path = env_info.path;

    // 2. Verify mock_tool is symlinked
    let ws_mock_tool = workspace_path.join("prebuilt/tools/mock_tool");
    assert!(ws_mock_tool.exists());
    assert!(ws_mock_tool.is_symlink(), "mock_tool should be symlinked");

    // 3. Verify Bazel package is copied (not symlinked)
    let ws_bazel = workspace_path.join("prebuilt/third_party/bazel/linux-x64");
    assert!(ws_bazel.exists());
    assert!(!ws_bazel.is_symlink(), "Bazel package should be a real directory (copied)");
    assert!(ws_bazel.join("file.txt").exists(), "Bazel package contents should be copied");
    
    let version_marker = ws_bazel.join(".fxenv_source_cache");
    assert!(version_marker.exists(), "Version marker should be written in workspace copy");

    let t1 = get_file_mtime(&ws_bazel.join("file.txt")).unwrap();

    // 4. Free workspace
    free_environment_by_id(config, &env_info.environment_id).unwrap();

    // Wait a bit
    std::thread::sleep(std::time::Duration::from_millis(500));

    // 5. Allocate again (re-use)
    let env_info_2 = allocate_environment(config, "mock_config", "test_agent_1", true).unwrap();
    assert_eq!(env_info_2.environment_id, env_id);

    // Verify Bazel was NOT re-copied (mtime preserved)
    let ws_bazel2 = env_info_2.path.join("prebuilt/third_party/bazel/linux-x64");
    let t2 = get_file_mtime(&ws_bazel2.join("file.txt")).unwrap();
    assert_eq!(t2, t1, "Bazel package should not be re-copied if version is unchanged");

    // Clean up
    free_environment_by_id(config, &env_info_2.environment_id).unwrap();
    delete_environment(config, &env_id).unwrap();
}
