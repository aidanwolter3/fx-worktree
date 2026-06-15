#!/bin/bash
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style
# license that can be found in the LICENSE file.

# Comprehensive E2E test for fx-worktree and Jiri prebuilt cache/worktree integration.
# Supports both Mock Mode (default) and Real Mode (if fuchsia_dir is passed).

set -e

ORIG_FUCHSIA_DIR="${FUCHSIA_DIR:-}"

# Colors for output
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m' # No Color

# Setup temp directory
TEST_DIR=$(mktemp -d -t fx-worktree-uber-e2e-XXXXXX)
LOG_FILE="${FX_WORKTREE_E2E_LOG:-/tmp/uber_e2e.log}"
rm -f "$LOG_FILE"

# Redirect stdout and stderr to both console and log file
exec > >(tee -a "$LOG_FILE") 2>&1

echo "========================================================================"
echo "Starting Uber E2E Test"
echo "Log file: $LOG_FILE"
echo "Temp test directory: $TEST_DIR"
echo "========================================================================"

# Find fx-worktree source root
FX_WORKTREE_SRC=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
FX_WORKTREE_BIN="$FX_WORKTREE_SRC/target/debug/fx-worktree"

# Parse arguments
INSTALL_BASE_COMMIT=""
REAL_MODE=false
if [ -n "$1" ]; then
    REAL_MODE=true
    REAL_FUCHSIA_DIR=$(realpath "$1")
    if [ ! -d "$REAL_FUCHSIA_DIR/.jiri_root" ]; then
        echo -e "${RED}Error: $REAL_FUCHSIA_DIR is not a Jiri root repository.${NC}"
        exit 1
    fi
    TEST_ROOT="$REAL_FUCHSIA_DIR"
    
    # Auto-detect config name if not passed
    CONFIG_NAME="$2"
    if [ -z "$CONFIG_NAME" ]; then
        echo "[Progress] Attempting to auto-detect config name from parent..."
        if [ -f "$TEST_ROOT/.fx-build-dir" ]; then
            BUILD_DIR=$(cat "$TEST_ROOT/.fx-build-dir")
            BUILD_DIR_ABS="$TEST_ROOT/$BUILD_DIR"
            if [ -f "$BUILD_DIR_ABS/args.gn" ]; then
                PRODUCT=$(grep "build_info_product" "$BUILD_DIR_ABS/args.gn" | cut -d'"' -f2 || true)
                BOARD=$(grep "build_info_board" "$BUILD_DIR_ABS/args.gn" | cut -d'"' -f2 || true)
                if [ -n "$PRODUCT" ] && [ -n "$BOARD" ]; then
                    CONFIG_NAME="$PRODUCT.$BOARD"
                    echo "Detected config: $CONFIG_NAME"
                fi
            fi
        fi
    fi
    if [ -z "$CONFIG_NAME" ]; then
        echo -e "${RED}Error: Could not auto-detect config name. Please pass it as the second argument:${NC}"
        echo "  $0 <fuchsia_dir> <config_name>"
        exit 1
    fi
    echo "Running in REAL MODE on $REAL_FUCHSIA_DIR with config $CONFIG_NAME"
else
    TEST_ROOT="$TEST_DIR/test_root"
    CONFIG_NAME="mock_config"
    echo "Running in MOCK MODE"
fi

if [ "$REAL_MODE" = "true" ]; then
    echo "========================================================================"
    echo "[Safety Pre-check] Running E2E Test in MOCK MODE first..."
    echo "========================================================================"
    
    if ! FX_WORKTREE_E2E_LOG="/tmp/uber_e2e_mock.log" "$0"; then
        echo -e "${RED}Error: Safety pre-check (Mock Mode E2E) failed!${NC}"
        echo -e "${RED}Aborting Real Mode execution to prevent damage to real workspace.${NC}"
        echo -e "${RED}Review mock log at: /tmp/uber_e2e_mock.log${NC}"
        exit 1
    fi
    
    echo "========================================================================"
    echo "[Safety Pre-check] Mock Mode E2E PASSED. Proceeding to Real Mode..."
    echo "========================================================================"
fi

# Helper to run Jiri update
run_jiri_update() {
    local jiri_bin="$1"
    shift
    echo "[Progress] Running Jiri update using $jiri_bin..."
    "$jiri_bin" update "$@"

    echo "DEBUG TIMESTAMPS after run_jiri_update:"
    stat -c "  %y %n" build/regenerator.py || true
    if [ -d "out" ]; then
        local build_dir=$(scripts/fx get-build-dir 2>/dev/null || true)
        if [ -n "$build_dir" ] && [ -f "$build_dir/build.ninja.stamp" ]; then
            stat -c "  %y %n" "$build_dir/build.ninja.stamp" || true
        fi
    fi
}

# Cleanup handler
cleanup() {
    local exit_code=$?
    echo "========================================================================"
    if [ $exit_code -eq 0 ]; then
        echo -e "${GREEN}Uber E2E Test PASSED!${NC}"
        echo "Cleaning up $TEST_DIR..."
        rm -rf "$TEST_DIR"
    else
        echo -e "${RED}Uber E2E Test FAILED!${NC}"
        echo "Keeping $TEST_DIR for debugging."
        echo "Review the log file at: $LOG_FILE"
    fi
    echo "========================================================================"
}
trap cleanup EXIT

# Export env vars for fx-worktree
export FUCHSIA_DIR="$TEST_ROOT"


# Safety checks for Real Mode
if [ "$REAL_MODE" = "true" ]; then
    cd "$TEST_ROOT"
    echo "[Progress] Checking if parent repository is clean..."
    if [ -n "$(git status --porcelain)" ]; then
        echo -e "${RED}Error: Parent repository has local changes. Please stash or commit them.${NC}"
        exit 1
    fi
    JIRI_STATUS=$(./.jiri_root/bin/jiri status)
    if echo "$JIRI_STATUS" | grep -q "^[MAD] "; then
        echo -e "${RED}Error: Jiri projects have modified files. Please stash or clean them.${NC}"
        echo "$JIRI_STATUS"
        exit 1
    fi
fi

# Build fx-worktree
echo "[Progress] Building fx-worktree..."
cargo build --manifest-path "$FX_WORKTREE_SRC/Cargo.toml"

# ==============================================================================
# 1. Prepare Mock Repositories (Mock Mode only)
# ==============================================================================
if [ "$REAL_MODE" = "false" ]; then
    echo "[Progress] Preparing mock repositories..."
    MANIFEST_REPO="$TEST_DIR/manifest_repo"
    SOURCE_REPO1="$TEST_DIR/source_repo1"
    SOURCE_REPO2="$TEST_DIR/source_repo2"

    # --- source_repo1 ---
    mkdir -p "$SOURCE_REPO1"
    cd "$SOURCE_REPO1"
    git init -q
    git config user.name "Test User"
    git config user.email "test@example.com"

    cat << 'EOF' > BUILD.gn
resolved_git_files = exec_script(
    "//build/git/resolve_git_path.py",
    [
      rebase_path(".", root_build_dir),
      "index",
    ],
    "list lines"
)
git_index = resolved_git_files[0]

action("sim_link") {
  script = "link_tool.py"
  sources = [
    "source.txt",
    "//prebuilt/tools/gsutil/gsutil",
    git_index,
  ]
  outputs = [ "$root_out_dir/gen/link.txt" ]
  args = [
    rebase_path("source.txt", root_build_dir),
    rebase_path(outputs[0], root_build_dir),
  ]
}
EOF

    cat << 'EOF' > link_tool.py
import os
import sys

src = sys.argv[1]
dst = sys.argv[2]

if os.path.exists(dst):
    os.remove(dst)

os.makedirs(os.path.dirname(dst), exist_ok=True)
os.link(src, dst)
EOF
    chmod +x link_tool.py

    cat << 'EOF' > verify_jiri_manifest.sh
#!/bin/bash
_SCRIPT_DIR="$(dirname "${BASH_SOURCE[0]}")"
JIRI_ROOT="$(cd "${_SCRIPT_DIR}/.." && pwd -P)"
if [ ! -f "${JIRI_ROOT}/.jiri_manifest" ]; then
    echo "FATAL: Cannot locate .jiri_manifest in Jiri root: ${JIRI_ROOT}"
    exit 1
fi
echo "✔ verify_jiri_manifest hook: found .jiri_manifest"
EOF
    chmod +x verify_jiri_manifest.sh

    echo "hello" > source.txt
    git add .
    git commit -m "initial commit" -q

    # --- source_repo2 ---
    mkdir -p "$SOURCE_REPO2"
    cd "$SOURCE_REPO2"
    git init -q
    git config user.name "Test User"
    git config user.email "test@example.com"
    echo "dummy" > dummy.txt
    git add .
    git commit -m "initial commit" -q

    # --- manifest_repo ---
    mkdir -p "$MANIFEST_REPO"
    cd "$MANIFEST_REPO"
    git init -q
    git config user.name "Test User"
    git config user.email "test@example.com"

    cat << 'EOF' > .gitignore
out/
.fx-build-dir
.fx-worktree-completed
prebuilt/
.jiri_root/
.jiri_manifest
.cipd/
source_repo1/
source_repo2/
EOF

    mkdir -p build
    echo "# CIPD settings" > build/cipd.gni

    cat << EOF > minimal.xml
<?xml version="1.0" encoding="UTF-8"?>
<manifest>
  <projects>
    <project name="manifest"
             path="."
             remote="$MANIFEST_REPO"
             revision="HEAD"/>
    <project name="source_repo1"
             path="source_repo1"
             remote="$SOURCE_REPO1"
             revision="HEAD"/>
    <project name="source_repo2"
             path="source_repo2"
             remote="$SOURCE_REPO2"
             revision="HEAD"/>
  </projects>
  <packages>
    <package name="infra/tools/luci/gsutil/\${platform}"
             version="git_revision:e7191d1ea4af5d5bfa6e243f7d6a5697e3d7b600"
             path="prebuilt/tools/gsutil"/>
    <package name="chromium/fuchsia/web_engine/amd64/tests"
             version="version:149.0.7826.0"
             path="prebuilt/third_party/web_engine_tests_latest/arch/x64"/>
  </packages>
  <hooks>
    <hook name="verify-jiri-manifest"
          project="source_repo1"
          action="verify_jiri_manifest.sh"/>
  </hooks>
</manifest>
EOF

    echo 'buildconfig = "//build/BUILD.gn"' > .gn
    mkdir -p build/toolchain
    echo 'set_default_toolchain("//build/toolchain:dummy")' > build/BUILD.gn

    cat << 'EOF' > build/toolchain/BUILD.gn
toolchain("dummy") {
  tool("stamp") {
    command = "touch {{output}}"
    description = "STAMP {{output}}"
  }
  tool("copy") {
    command = "cp {{source}} {{output}}"
    description = "COPY {{source}} {{output}}"
  }
}
EOF

    mkdir -p build/git
    cat << 'EOF' > build/git/resolve_git_path.py
#!/usr/bin/env python3
import sys
from pathlib import Path

def main():
    repo_dir = Path(sys.argv[1])
    file_path = sys.argv[2]
    
    git_path = repo_dir / ".git"
    if git_path.is_dir():
        git_dir = git_path
    elif git_path.is_file():
        content = git_path.read_text().strip()
        if content.startswith("gitdir: "):
            git_dir = Path(content[8:])
            if not git_dir.is_absolute():
                git_dir = (repo_dir / git_dir).resolve()
        else:
            print(f"Invalid .git file: {content}", file=sys.stderr)
            return 1
    else:
        print(f".git not found in {repo_dir}", file=sys.stderr)
        return 1
        
    worktree_specific = ["HEAD", "index", "ORIG_HEAD"]
    is_shared = file_path not in worktree_specific
    
    if is_shared:
        commondir_file = git_dir / "commondir"
        if commondir_file.exists():
            commondir_path = commondir_file.read_text().strip()
            target_dir = (git_dir / commondir_path).resolve()
        else:
            target_dir = git_dir
    else:
        target_dir = git_dir
        
    print(target_dir / file_path)
    return 0

if __name__ == "__main__":
    sys.exit(main())
EOF
    chmod +x build/git/resolve_git_path.py

    cat << 'EOF' > BUILD.gn
import("//build/cipd.gni")
copy("check_jiri") {
  sources = [ "//.jiri_root/update_history/latest" ]
  outputs = [ "$target_gen_dir/jiri_latest_copy" ]
}
group("all") {
  deps = [
    ":check_jiri",
    "//source_repo1:sim_link",
  ]
}
EOF

    # Mock fx script
    GN_BIN=""
    if [ -n "$ORIG_FUCHSIA_DIR" -a -f "$ORIG_FUCHSIA_DIR/prebuilt/third_party/gn/linux-x64/gn" ]; then
        GN_BIN="$ORIG_FUCHSIA_DIR/prebuilt/third_party/gn/linux-x64/gn"
    elif which gn >/dev/null 2>&1; then
        GN_BIN=$(which gn)
    fi

    if [ -z "$GN_BIN" ]; then
        echo -e "${RED}Error: gn binary not found. Please set FUCHSIA_DIR or add gn to your PATH.${NC}"
        exit 1
    fi

    NINJA_BIN=""
    if [ -n "$ORIG_FUCHSIA_DIR" -a -f "$ORIG_FUCHSIA_DIR/prebuilt/third_party/ninja/linux-x64/ninja" ]; then
        NINJA_BIN="$ORIG_FUCHSIA_DIR/prebuilt/third_party/ninja/linux-x64/ninja"
    elif which ninja >/dev/null 2>&1; then
        NINJA_BIN=$(which ninja)
    fi

    if [ -z "$NINJA_BIN" ]; then
        echo -e "${RED}Error: ninja binary not found. Please set FUCHSIA_DIR or add ninja to your PATH.${NC}"
        exit 1
    fi

    mkdir -p scripts
    cat << EOF > scripts/fx
#!/bin/bash
dir=""
if [ -f ".fx-build-dir" ]; then
  dir=\$(cat .fx-build-dir)
fi
if [ -z "\$dir" ]; then
  dir="out/default"
fi

while [[ "\$#" -gt 0 ]]; do
    case \$1 in
        --dir) dir="\$2"; shift ;;
        *) break ;;
    esac
    shift
done

cmd=\$1
shift

if [ "\$cmd" = "set" ]; then
  mkdir -p "\$dir"
  config_name="\${@: -1}"
  echo "build_info_product = \\"\$config_name\\"" > "\$dir/args.gn"
  "$GN_BIN" gen "\$dir"
elif [ "\$cmd" = "build" ] || [ "\$cmd" = "ninja" ]; then
  "$NINJA_BIN" -C "\$dir" "\$@"
elif [ "\$cmd" = "get-build-dir" ]; then
  echo "\$dir"
fi
EOF
    chmod +x scripts/fx

    git add .
    git commit -m "initial commit" -q
fi

# Helper for no-op verification
check_noop() {
    local dir="$1"
    echo "[Progress] Verifying no-op build in $dir..."
    cd "$dir"
    local build_dir=$(scripts/fx get-build-dir)
    echo "DEBUG TIMESTAMPS in check_noop:"
    stat -c "  %y %n" build/regenerator.py || true
    stat -c "  %y %n" "$build_dir/build.ninja.stamp" || true
    local explain_output
    explain_output=$(scripts/fx ninja -C "$build_dir" -d explain -n -v 2>&1)
    local ninja_status=${PIPESTATUS[0]}
    if [ $ninja_status -ne 0 ]; then
        echo -e "${RED}FAIL: Ninja command failed with exit code $ninja_status in $dir${NC}"
        echo "Output:"
        echo "$explain_output"
        exit 1
    fi
    local explain_line=$(echo "$explain_output" | grep "ninja explain:" | head -n 1 || true)
    if [ -n "$explain_line" ]; then
        echo -e "${RED}FAIL: Build was not a no-op in $dir${NC}"
        echo "Ninja explain output:"
        echo "$explain_line"
        exit 1
    fi
    echo "✔ Build is no-op."
}

print_git_mtimes() {
    local label="$1"
    local wt_path="$2"
    echo "--- GIT MTIMES: $label ---"
    stat -c "  Parent index: %y" "$TEST_ROOT/.git/index" || true
    stat -c "  Parent HEAD:  %y" "$TEST_ROOT/.git/HEAD" || true
    if [ -n "$wt_path" -a -d "$wt_path" ]; then
        if [ -f "$wt_path/.git" ]; then
            local gitdir=$(git -C "$wt_path" rev-parse --git-dir 2>/dev/null)
            if [ -n "$gitdir" ]; then
                stat -c "  WT index:     %y" "$gitdir/index" || true
                stat -c "  WT HEAD:      %y" "$gitdir/HEAD" || true
            fi
        fi
    fi
    echo "--------------------------"
}


# ==============================================================================
# ==============================================================================
# 2. Setup Jiri and Initial Build
# ==============================================================================
if [ "$REAL_MODE" = "false" ]; then
    echo "[Progress] Bootstrapping official Jiri..."
    mkdir -p "$TEST_ROOT"
    cd "$TEST_ROOT"
    curl -s "https://fuchsia.googlesource.com/jiri/+/HEAD/scripts/bootstrap_jiri?format=TEXT" | base64 --decode | bash -s .
    JIRI_BIN="$TEST_ROOT/.jiri_root/bin/jiri"

    echo "[Progress] Initializing Jiri root..."
    "$JIRI_BIN" init -shared=true -enable-lockfile=false -analytics-opt=false .
    "$JIRI_BIN" import minimal.xml "$MANIFEST_REPO"
    run_jiri_update "$JIRI_BIN"

    # Initial build
    echo "[Progress] Running initial build..."
    scripts/fx set "$CONFIG_NAME"
    scripts/fx build
else
    JIRI_BIN="$TEST_ROOT/.jiri_root/bin/jiri"
    echo "[Progress] Running initial build..."
    scripts/fx set "$CONFIG_NAME"
    scripts/fx build
fi

check_noop "$TEST_ROOT"

# ==============================================================================
# 4. Toggle Package Cache (Main Tree)
# ==============================================================================

echo "[Progress] Enabling package cache in main tree..."
"$JIRI_BIN" init -package-cache=true
echo "[Progress] Running jiri update to migrate to cache..."
run_jiri_update "$JIRI_BIN"

if [ "$REAL_MODE" = "false" ]; then
    echo "[Progress] Verifying symlinks in CACHE..."
    web_engine_dest="$TEST_ROOT/prebuilt/third_party/web_engine_tests_latest/arch/x64"
    cache_dir=$(readlink "$web_engine_dest")
    echo "Cache dir for web_engine tests is $cache_dir"
    check_file="$cache_dir/common_tests_manifest.json"
    
    echo "DEBUG ALL SYMLINKS IN CACHE:"
    python3 -c "import os; [print(f'  {os.path.join(r, f)} -> {os.readlink(os.path.join(r, f))}') for r, d, files in os.walk('$cache_dir') for f in files if os.path.islink(os.path.join(r, f))]"
    
    if [ -L "$check_file" ]; then
        if [ ! -e "$check_file" ]; then
            echo -e "${RED}FAIL: common_tests_manifest.json symlink in CACHE is broken!${NC}"
            exit 1
        else
            echo "✔ common_tests_manifest.json symlink in CACHE is resolved correctly: $(readlink "$check_file")"
        fi
    else
        echo -e "${RED}FAIL: common_tests_manifest.json in CACHE is not a symlink or does not exist!${NC}"
        exit 1
    fi
fi

echo "[Progress] Building after cache enablement..."
scripts/fx build
check_noop "$TEST_ROOT"

if [ "$REAL_MODE" = "true" ]; then
    echo "[Progress] Testing format-code with cache enabled..."
    echo "" >> build/bazel/scripts/bazel_source_path_mapper.py
    scripts/fx format-code
    git checkout build/bazel/scripts/bazel_source_path_mapper.py
fi

echo "[Progress] Disabling package cache in main tree..."
"$JIRI_BIN" init -package-cache=false
echo "[Progress] Running jiri update to restore from cache..."
run_jiri_update "$JIRI_BIN"

if [ "$REAL_MODE" = "false" ]; then
    echo "[Progress] Verifying symlinks after restoration..."
    check_file="$TEST_ROOT/prebuilt/third_party/web_engine_tests_latest/arch/x64/common_tests_manifest.json"
    if [ -L "$check_file" ]; then
        target=$(readlink "$check_file")
        if [[ "$target" == *".jiri_root/packages"* ]]; then
            echo -e "${RED}FAIL: Restored symlink points to cache! Target: $target${NC}"
            exit 1
        else
            echo "✔ Restored symlink is correct (does not point to cache): $target"
        fi
    else
        echo -e "${RED}FAIL: Restored file is not a symlink or does not exist!${NC}"
        exit 1
    fi
    
    if grep -q "has broken symlinks" "$LOG_FILE"; then
        echo -e "${RED}FAIL: Restoration failed and fell back to download!${NC}"
        exit 1
    fi
fi

echo "[Progress] Building after cache disablement..."
scripts/fx build
check_noop "$TEST_ROOT"

# ==============================================================================
# 5. Worktree Testing (Cache Off)
# ==============================================================================
echo "[Progress] Adding worktree (cache off)..."
cd "$TEST_ROOT"

# Get mtimes of main tree .git/config and .git/index before add
config_mtime_before=$(stat -c %Y .git/config)
index_mtime_before=$(stat -c %Y .git/index)

"$JIRI_BIN" worktree add "$TEST_ROOT/.jiri_root/worktrees/$CONFIG_NAME"
(cd "$TEST_ROOT/.jiri_root/worktrees/$CONFIG_NAME" && scripts/fx set "$CONFIG_NAME")
$FX_WORKTREE_BIN mark-free "$CONFIG_NAME"

# Check if parent .git/config or .git/index mtime changed
config_mtime_after=$(stat -c %Y .git/config)
index_mtime_after=$(stat -c %Y .git/index)

if [ "$config_mtime_before" != "$config_mtime_after" ]; then
    echo -e "${RED}FAIL: Parent .git/config was modified during worktree add!${NC}"
    exit 1
fi

if [ "$index_mtime_before" != "$index_mtime_after" ]; then
    echo -e "${RED}FAIL: Parent .git/index was modified during worktree add!${NC}"
    exit 1
fi

# Verify main tree is still no-op
check_noop "$TEST_ROOT"

echo "[Progress] Leasing worktree..."
AGENT_ID="e2e-test-agent"
WT_PATH=$($FX_WORKTREE_BIN lease "$CONFIG_NAME" --agent-id "$AGENT_ID" --print-path-only)
WT_ID=$(basename "$WT_PATH")
echo "Leased worktree $WT_ID at $WT_PATH"

cd "$WT_PATH"
echo "[Progress] Building in worktree..."
scripts/fx build
check_noop "$WT_PATH"

echo "[Progress] Verifying build regeneration in worktree..."
echo "# E2E test modification" >> BUILD.gn
explain_output_tmp=$(scripts/fx ninja -C "$(scripts/fx get-build-dir)" -d explain -n -v 2>&1)
ninja_status_tmp=${PIPESTATUS[0]}
if [ $ninja_status_tmp -ne 0 ]; then
    echo -e "${RED}FAIL: Ninja command failed with exit code $ninja_status_tmp during regeneration check${NC}"
    echo "Output:"
    echo "$explain_output_tmp"
    exit 1
fi
explain_output=$(echo "$explain_output_tmp" | grep "ninja explain:" | head -n 1 || true)
if [ -z "$explain_output" ]; then
    echo -e "${RED}FAIL: Build was still a no-op after modifying BUILD.gn in $WT_PATH${NC}"
    exit 1
fi
echo "✔ Build correctly detects modification: $explain_output"
scripts/fx build
check_noop "$WT_PATH"

# Restore file
git checkout BUILD.gn
scripts/fx build
check_noop "$WT_PATH"

echo "[Progress] Testing release and re-lease..."
print_git_mtimes "BEFORE RELEASE" "$WT_PATH"
$FX_WORKTREE_BIN release "$WT_ID"
print_git_mtimes "AFTER RELEASE" "$WT_PATH"

print_git_mtimes "BEFORE LEASE" "$WT_PATH"
WT_PATH2=$($FX_WORKTREE_BIN lease "$CONFIG_NAME" --agent-id "$AGENT_ID" --print-path-only)
print_git_mtimes "AFTER LEASE" "$WT_PATH"
if [ "$WT_PATH" != "$WT_PATH2" ]; then
    echo -e "${RED}FAIL: Leased a different worktree path: $WT_PATH2 (expected $WT_PATH)${NC}"
    exit 1
fi
cd "$WT_PATH"
check_noop "$WT_PATH"

echo "[Progress] Updating main tree..."
cd "$TEST_ROOT"
run_jiri_update "$JIRI_BIN"

echo "[Progress] Syncing worktree..."
(cd "$WT_PATH" && "$JIRI_BIN" update)
cd "$WT_PATH"
scripts/fx build
check_noop "$WT_PATH"

echo "[Progress] Testing lease with --base-branch..."
cd "$WT_PATH"
git checkout -b my-base-branch
echo "base" > base.txt
git add base.txt
git commit -m "base commit" -q

# Release it (cleans up agent branch, resets to JIRI_HEAD)
$FX_WORKTREE_BIN release "$WT_ID"

# Verify we are on JIRI_HEAD and base.txt is gone (because JIRI_HEAD doesn't have it)
if [ -f base.txt ]; then
    echo -e "${RED}FAIL: base.txt still exists after release!${NC}"
    exit 1
fi

# Lease again with --base-branch
WT_PATH3=$($FX_WORKTREE_BIN lease "$CONFIG_NAME" --agent-id "agent-base" --base-branch "my-base-branch" --print-path-only)
if [ "$WT_PATH" != "$WT_PATH3" ]; then
    echo -e "${RED}FAIL: Leased a different worktree path for base test${NC}"
    exit 1
fi

cd "$WT_PATH3"
# Verify base.txt exists now
if [ ! -f base.txt ]; then
    echo -e "${RED}FAIL: base.txt does not exist in leased worktree with base-branch!${NC}"
    exit 1
fi

# Clean up for next tests
$FX_WORKTREE_BIN release "$WT_ID"
git branch -D my-base-branch

# Test auto-creation of base branch
# Lease with a non-existent base branch
WT_PATH4=$($FX_WORKTREE_BIN lease "$CONFIG_NAME" --agent-id "agent-base2" --base-branch "auto-created-base" --print-path-only)
if [ "$WT_PATH" != "$WT_PATH4" ]; then
    echo -e "${RED}FAIL: Leased a different worktree path for auto-create base test${NC}"
    exit 1
fi

cd "$WT_PATH4"
# Verify the base branch was created locally
if ! git show-ref --verify --quiet refs/heads/auto-created-base; then
    echo -e "${RED}FAIL: auto-created-base branch was not created!${NC}"
    exit 1
fi

# Verify we are on the agent branch
CURRENT_BRANCH=$(git branch --show-current)
if [ "$CURRENT_BRANCH" != "mock_config-agent-base2" ]; then
    echo -e "${RED}FAIL: Not on correct agent branch: $CURRENT_BRANCH${NC}"
    exit 1
fi

# Release to clean up
$FX_WORKTREE_BIN release "$WT_ID"
# Delete the auto-created branch
git branch -D auto-created-base

# Restore state for subsequent tests (re-lease as e2e-test-agent)
WT_PATH_RESTORED=$($FX_WORKTREE_BIN lease "$CONFIG_NAME" --agent-id "$AGENT_ID" --print-path-only)
if [ "$WT_PATH" != "$WT_PATH_RESTORED" ]; then
    echo -e "${RED}FAIL: Failed to restore lease on correct path${NC}"
    exit 1
fi


# ==============================================================================
# 6. Enable Cache & Migrate Both
# ==============================================================================
echo "[Progress] Enabling cache in main tree..."
cd "$TEST_ROOT"
"$JIRI_BIN" init -package-cache=true
echo "[Progress] Updating main tree (migration)..."
run_jiri_update "$JIRI_BIN"
scripts/fx build
check_noop "$TEST_ROOT"

echo "[Progress] Syncing worktree (migration)..."
(cd "$WT_PATH" && "$JIRI_BIN" update)
cd "$WT_PATH"
scripts/fx build
check_noop "$WT_PATH"

# ==============================================================================
# 7. Add New Worktree (Cache On)
# ==============================================================================
echo "[Progress] Adding second worktree (cache on)..."
cd "$TEST_ROOT"
"$JIRI_BIN" worktree add "$TEST_ROOT/.jiri_root/worktrees/${CONFIG_NAME}_2"
(cd "$TEST_ROOT/.jiri_root/worktrees/${CONFIG_NAME}_2" && scripts/fx set "$CONFIG_NAME")
$FX_WORKTREE_BIN mark-free "${CONFIG_NAME}_2"

echo "[Progress] Leasing second worktree..."
WT_PATH_NEW=$($FX_WORKTREE_BIN lease --any --print-path-only)
WT_ID_NEW=$(basename "$WT_PATH_NEW")
if [ "$WT_PATH" = "$WT_PATH_NEW" ]; then
    echo -e "${RED}FAIL: Leased the same worktree, expected a new one${NC}"
    exit 1
fi
echo "Leased second worktree $WT_ID_NEW at $WT_PATH_NEW"

cd "$WT_PATH_NEW"
echo "[Progress] Building in second worktree..."
scripts/fx build
check_noop "$WT_PATH_NEW"

# ==============================================================================
# 8. Test GC Safety with Active Worktrees
# ==============================================================================
echo "[Progress] Testing GC safety with active worktrees..."

# Ensure WT_ID is synced and clean
cd "$TEST_ROOT"
(cd "$WT_PATH" && "$JIRI_BIN" update)

# Verify source_repo2 exists in both parent and worktrees before test
if [ ! -d "$TEST_ROOT/source_repo2" ]; then
    echo -e "${RED}FAIL: source_repo2 does not exist in parent before test${NC}"
    exit 1
fi
if [ ! -d "$WT_PATH/source_repo2" ]; then
    echo -e "${RED}FAIL: source_repo2 does not exist in worktree WT_ID before test${NC}"
    exit 1
fi
if [ ! -d "$WT_PATH_NEW/source_repo2" ]; then
    echo -e "${RED}FAIL: source_repo2 does not exist in worktree WT_ID_NEW before test${NC}"
    exit 1
fi

# 1. Modify manifest to delete source_repo2
echo "[Progress] Deleting source_repo2 from manifest..."
cd "$MANIFEST_REPO"
python3 -c "
with open('minimal.xml', 'r') as f:
    lines = f.readlines()
new_lines = []
skip = False
for line in lines:
    if 'name=\"source_repo2\"' in line:
        skip = True
        continue
    if skip:
        if '/>' in line:
            skip = False
        continue
    new_lines.append(line)
with open('minimal.xml', 'w') as f:
    f.writelines(new_lines)
"
git commit -a -m "Delete source_repo2 from manifest" -q

# 2. Run Jiri update with GC in main tree.
# It should NOT delete source_repo2 in main tree because worktrees still reference it.
echo "[Progress] Running Jiri update -gc in main tree..."
cd "$TEST_ROOT"
run_jiri_update "$JIRI_BIN" -gc

# Verify source_repo2 STILL exists in parent because of active worktrees
if [ ! -d "$TEST_ROOT/source_repo2" ]; then
    echo -e "${RED}FAIL: source_repo2 was deleted in parent despite active worktrees!${NC}"
    exit 1
else
    echo "✔ source_repo2 was preserved in parent (GC safety check worked)."
fi

# 3. Sync worktree WT_ID.
# Jiri in worktree should see source_repo2 is deleted from manifest, and should delete it in worktree.
echo "[Progress] Syncing worktree WT_ID to apply manifest deletion..."
(cd "$WT_PATH" && "$JIRI_BIN" update -gc)

# Verify source_repo2 is deleted in worktree WT_ID
if [ -d "$WT_PATH/source_repo2" ]; then
    echo -e "${RED}FAIL: source_repo2 was not deleted in worktree WT_ID after sync!${NC}"
    exit 1
else
    echo "✔ source_repo2 was deleted in worktree WT_ID."
fi

# 4. Sync second worktree WT_ID_NEW as well.
echo "[Progress] Syncing second worktree WT_ID_NEW to apply manifest deletion..."
(cd "$WT_PATH_NEW" && "$JIRI_BIN" update -gc)

# Verify source_repo2 is deleted in second worktree
if [ -d "$WT_PATH_NEW/source_repo2" ]; then
    echo -e "${RED}FAIL: source_repo2 was not deleted in worktree WT_ID_NEW after sync!${NC}"
    exit 1
else
    echo "✔ source_repo2 was deleted in worktree WT_ID_NEW."
fi

# 5. Run Jiri update -gc in parent again.
# Since both worktrees have deleted their source_repo2 directories,
# the parent project should now be deleted (if prune works).
echo "[Progress] Running Jiri update -gc in main tree again..."
cd "$TEST_ROOT"
run_jiri_update "$JIRI_BIN" -gc

# Verify source_repo2 is now deleted in parent
if [ -d "$TEST_ROOT/source_repo2" ]; then
    echo -e "${RED}WARNING: source_repo2 still exists in parent. Trying with manual prune...${NC}"
    if [ -d "$TEST_ROOT/source_repo2/.git" ]; then
        git -C "$TEST_ROOT/source_repo2" worktree prune
        run_jiri_update "$JIRI_BIN" -gc
        if [ -d "$TEST_ROOT/source_repo2" ]; then
             echo -e "${RED}FAIL: source_repo2 still exists in parent even after manual prune!${NC}"
             exit 1
        else
             echo "✔ source_repo2 was deleted in parent after manual prune."
             echo -e "${RED}FAIL: Jiri did not auto-prune worktrees, manual prune was required.${NC}"
             exit 1
        fi
    else
        echo -e "${RED}FAIL: source_repo2 still exists but has no .git?${NC}"
        exit 1
    fi
else
    echo "✔ source_repo2 was deleted in parent (GC cleanup worked)."
fi

# Restore manifest for subsequent runs (just in case)
cd "$MANIFEST_REPO"
git reset --hard HEAD~1 -q

# ==============================================================================
# 9. Cleanup
# ==============================================================================
echo "[Progress] Releasing and removing test worktrees..."
cd "$TEST_ROOT"
$FX_WORKTREE_BIN release "$WT_ID"
$FX_WORKTREE_BIN release "$WT_ID_NEW"
"$JIRI_BIN" worktree remove -force "$WT_PATH"
"$JIRI_BIN" worktree remove -force "$WT_PATH_NEW"

# The EXIT trap will handle restoring Jiri if failed, or installing new Jiri on success.
