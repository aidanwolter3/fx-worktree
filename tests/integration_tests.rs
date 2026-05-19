use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use tempfile::TempDir;

use fxenv::alloc::allocate;
use fxenv::config::Config;
use fxenv::free::free_worktree_by_id;
use fxenv::gc::garbage_collect;
use fxenv::list::{list_outdirs, list_worktrees};
use fxenv::outdir::{create_outdir, delete_outdir};
use fxenv::selftest::run_self_test;

// Global lock to serialize tests that modify env vars
static TEST_LOCK: Mutex<()> = Mutex::new(());

// Helper to run commands during test setup
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
    // Config git user for commits in test
    run_setup_cmd("git", &["config", "user.name", "Test User"], fuchsia_path);
    run_setup_cmd(
        "git",
        &["config", "user.email", "test@example.com"],
        fuchsia_path,
    );

    let dummy_file = fuchsia_path.join("dummy.txt");
    fs::write(&dummy_file, "hello").unwrap();

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

    // Commit only dummy.txt and scripts/fx. .jiri_root is not tracked in real Fuchsia.
    run_setup_cmd("git", &["add", "dummy.txt", "scripts/fx"], fuchsia_path);
    run_setup_cmd(
        "git",
        &["commit", "-m", "initial commit with mocks"],
        fuchsia_path,
    );

    // Set env var for FXENV_ROOT so Config::new picks it up
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

    // 1. Test Outdir Create
    let outdir_id = create_outdir(config, "mock_config", &["--some-arg".to_string()]).unwrap();

    let outdirs_dir = config.outdirs_dir().join("mock_config");
    assert!(outdirs_dir.exists());
    let out_dir = outdirs_dir.join(&outdir_id);
    assert!(out_dir.exists());
    assert!(outdir_id.starts_with("out_"));
    assert!(out_dir.join("args.gn").exists());
    assert!(out_dir.join("args.gn.ref").exists());

    println!("--- List Outdirs after create ---");
    list_outdirs(config).unwrap();
    println!("--- List Worktrees after create ---");
    list_worktrees(config).unwrap();

    // 2. Test Worktree Create
    let worktree_info = allocate(config, "mock_config", "test_agent", None, None).unwrap();
    assert_eq!(worktree_info.agent_id, "test_agent");
    assert_eq!(worktree_info.config, "mock_config");

    let lease_file = config.leases_dir().join(format!(
        "mock_config_{}.lease",
        worktree_info.worktree_id.split('_').next_back().unwrap()
    ));
    assert!(lease_file.exists());

    let workspace_path = &worktree_info.workspace_path;
    assert!(workspace_path.exists());

    assert!(workspace_path.join(".git").exists());
    assert!(workspace_path.join(".jiri_root").exists()); // Symlink
    assert!(workspace_path.join("out/default").exists()); // Symlink
    assert!(workspace_path.join(".fx-build-dir").exists());

    println!("--- List Outdirs after alloc ---");
    list_outdirs(config).unwrap();
    println!("--- List Worktrees after alloc ---");
    list_worktrees(config).unwrap();

    // Test that we cannot delete the outdir while it is in use
    let delete_res = delete_outdir(config, "mock_config", &outdir_id);
    assert!(delete_res.is_err());
    assert!(
        delete_res
            .unwrap_err()
            .to_string()
            .contains("Cannot delete outdir")
    );

    // 3. Test Worktree Delete
    free_worktree_by_id(config, &worktree_info.worktree_id).unwrap();
    assert!(!workspace_path.exists());
    assert!(!lease_file.exists());

    println!("--- List Outdirs after free ---");
    list_outdirs(config).unwrap();
    println!("--- List Worktrees after free ---");
    list_worktrees(config).unwrap();

    // Verify git worktree was removed from base repo
    let output = Command::new("git")
        .args(["worktree", "list"])
        .current_dir(&config.fuchsia_dir)
        .output()
        .unwrap();
    let worktree_list = String::from_utf8(output.stdout).unwrap();
    assert!(!worktree_list.contains(workspace_path.to_str().unwrap()));

    // Now we should be able to delete the outdir
    delete_outdir(config, "mock_config", &outdir_id).unwrap();
    assert!(!out_dir.exists());

    println!("--- List Outdirs after outdir delete ---");
    list_outdirs(config).unwrap();
}

#[test]
fn test_gc() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;

    // Create outdir and worktree
    create_outdir(config, "mock_config", &[]).unwrap();
    let worktree_info = allocate(config, "mock_config", "test_agent", None, None).unwrap();

    let lease_file = config.leases_dir().join(format!(
        "mock_config_{}.lease",
        worktree_info.worktree_id.split('_').next_back().unwrap()
    ));
    assert!(lease_file.exists());

    // Run GC with 0 timeout (force expiry)
    garbage_collect(config, 0).unwrap();

    assert!(!lease_file.exists());
    assert!(!worktree_info.workspace_path.exists());
}

#[test]
fn test_self_test_command() {
    let _lock = TEST_LOCK.lock().unwrap();
    let env = setup_mock_env();
    let config = &env.config;

    // Create the mock source file that self-test expects to modify
    let src_dir = config
        .fuchsia_dir
        .join("sdk/ctf/tests/fidl/fuchsia.diagnostics");
    fs::create_dir_all(&src_dir).unwrap();
    let src_file = src_dir.join("inspect_publisher.cc");
    fs::write(&src_file, "numeric_properties.RecordInt(\"int\", -1);").unwrap();

    // Commit it so it is tracked and checked out in the temp workspace
    run_setup_cmd(
        "git",
        &[
            "add",
            "sdk/ctf/tests/fidl/fuchsia.diagnostics/inspect_publisher.cc",
        ],
        &config.fuchsia_dir,
    );
    run_setup_cmd(
        "git",
        &["commit", "-m", "add inspect_publisher.cc"],
        &config.fuchsia_dir,
    );

    // Run self-test. It should use the mock fx and jiri we committed.
    run_self_test(config, None).unwrap();
}
