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

    // 2. Create mock jiri
    let jiri_dir = fuchsia_path.join(".jiri_root/bin");
    fs::create_dir_all(&jiri_dir).unwrap();
    let jiri_path = jiri_dir.join("jiri");

    let jiri_script = format!(
        r#"#!/bin/bash
if [ "$1" = "project" ] && [ "$2" = "-json-output" ]; then
  output_file=$3
  revision=$(git -C "{}" rev-parse HEAD)
  cat <<EOF > "$output_file"
[
  {{
    "name": "mock_project",
    "path": "{}",
    "revision": "$revision"
  }}
]
EOF
fi
"#,
        fuchsia_path.to_str().unwrap(),
        fuchsia_path.to_str().unwrap()
    );
    fs::write(&jiri_path, jiri_script).unwrap();
    make_executable(&jiri_path);

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

    // Commit only dummy.txt and scripts/fx
    run_setup_cmd("git", &["add", "dummy.txt", "scripts/fx"], fuchsia_path);
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

