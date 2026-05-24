#!/bin/bash
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style
# license that can be found in the LICENSE file.

# Comprehensive E2E test for fx-worktree and Jiri prebuilt cache/worktree integration.
# Supports both Mock Mode (default) and Real Mode (if fuchsia_dir is passed).

set -e

# Colors for output
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m' # No Color

# Setup temp directory
TEST_DIR=$(mktemp -d -t fx-worktree-uber-e2e-XXXXXX)
LOG_FILE="/tmp/uber_e2e.log"
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

# Helper to run Jiri update and preserve GN patch in Real Mode
run_jiri_update() {
    local jiri_bin="$1"
    shift
    echo "[Progress] Running Jiri update using $jiri_bin..."
    if [ "$REAL_MODE" = "true" ] && [ -n "$INSTALL_BASE_COMMIT" ]; then
        echo "[Progress] Temporarily resetting install_base commit before update..."
        (
            cd "$REAL_FUCHSIA_DIR"
            git reset --hard
            git checkout "$INSTALL_BASE_COMMIT~1"
        )
    fi
    "$jiri_bin" update "$@"
    if [ "$REAL_MODE" = "true" ]; then
        if [ -n "$INSTALL_BASE_COMMIT" ]; then
            echo "[Progress] Cherry-picking install_base commit after update..."
            (
                cd "$REAL_FUCHSIA_DIR"
                git checkout JIRI_HEAD
                git -c user.name="E2E Test" -c user.email="e2e@test.com" cherry-pick "$INSTALL_BASE_COMMIT"
            )
        fi
        echo "[Progress] Re-applying GN threads patch after update..."
        (
            cd "$REAL_FUCHSIA_DIR"
            python3 "$TEST_DIR/patch_regenerator.py"
            git -c user.name="E2E Test" -c user.email="e2e@test.com" commit -m "temp: patch GN threads for E2E" build/regenerator.py
        )
        echo "DEBUG TIMESTAMPS after run_jiri_update:"
        stat -c "  %y %n" build/regenerator.py || true
        if [ -d "out" ]; then
            local build_dir=$(scripts/fx get-build-dir 2>/dev/null || true)
            if [ -n "$build_dir" ] && [ -f "$build_dir/build.ninja.stamp" ]; then
                stat -c "  %y %n" "$build_dir/build.ninja.stamp" || true
            fi
        fi
    fi
}

# Cleanup handler
cleanup() {
    local exit_code=$?
    echo "========================================================================"
    if [ $exit_code -eq 0 ]; then
        echo -e "${GREEN}Uber E2E Test PASSED!${NC}"
        if [ "$REAL_MODE" = "true" ]; then
            echo "Installing latest compiled Jiri to host..."
            rm -f "$REAL_FUCHSIA_DIR/.jiri_root/bin/jiri"
            cp "$TEST_DIR/new_jiri" "$REAL_FUCHSIA_DIR/.jiri_root/bin/jiri"
            rm -f "$HOME/go/bin/jiri"
            mkdir -p "$HOME/go/bin"
            cp "$TEST_DIR/new_jiri" "$HOME/go/bin/jiri"
            echo "✔ New Jiri installed to $REAL_FUCHSIA_DIR/.jiri_root/bin/jiri and $HOME/go/bin/jiri"
            
            # Restore original config even on success to leave workspace clean
            if [ -f "$TEST_DIR/backup_config" ]; then
                echo "Restoring original config to $REAL_FUCHSIA_DIR..."
                cp "$TEST_DIR/backup_config" "$REAL_FUCHSIA_DIR/.jiri_root/config"
            fi
            if [ -f "$TEST_DIR/orig_head" ]; then
                ORIG_HEAD=$(cat "$TEST_DIR/orig_head")
                echo "Resetting repository to $ORIG_HEAD..."
                git -C "$REAL_FUCHSIA_DIR" reset "$ORIG_HEAD"
                git -C "$REAL_FUCHSIA_DIR" checkout build/regenerator.py
            fi
        fi
        echo "Cleaning up $TEST_DIR..."
        rm -rf "$TEST_DIR"
    else
        echo -e "${RED}Uber E2E Test FAILED!${NC}"
        if [ "$REAL_MODE" = "true" ]; then
            if [ -f "$TEST_DIR/orig_head" ]; then
                ORIG_HEAD=$(cat "$TEST_DIR/orig_head")
                echo "Resetting repository to $ORIG_HEAD..."
                git -C "$REAL_FUCHSIA_DIR" reset "$ORIG_HEAD"
                git -C "$REAL_FUCHSIA_DIR" checkout build/regenerator.py
            fi
            if [ -f "$TEST_DIR/backup_jiri" ]; then
                echo "Restoring original Jiri to $REAL_FUCHSIA_DIR..."
                rm -f "$REAL_FUCHSIA_DIR/.jiri_root/bin/jiri"
                cp "$TEST_DIR/backup_jiri" "$REAL_FUCHSIA_DIR/.jiri_root/bin/jiri"
            fi
            if [ -f "$TEST_DIR/backup_config" ]; then
                echo "Restoring original config to $REAL_FUCHSIA_DIR..."
                cp "$TEST_DIR/backup_config" "$REAL_FUCHSIA_DIR/.jiri_root/config"
            fi
        fi
        echo "Keeping $TEST_DIR for debugging."
        echo "Review the log file at: $LOG_FILE"
    fi
    echo "========================================================================"
}
trap cleanup EXIT

# Export env vars for fx-worktree
export FUCHSIA_DIR="$TEST_ROOT"
export FX_WORKTREE_ROOT="$TEST_DIR/.fx_worktree_root"
mkdir -p "$FX_WORKTREE_ROOT"

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
    # Backup original Jiri and config
    cp .jiri_root/bin/jiri "$TEST_DIR/backup_jiri"
    if [ -f .jiri_root/config ]; then
        cp .jiri_root/config "$TEST_DIR/backup_config"
    fi

    # Capture original HEAD
    ORIG_HEAD=$(git rev-parse HEAD)
    echo "$ORIG_HEAD" > "$TEST_DIR/orig_head"

    # Find the install_base commit
    INSTALL_BASE_COMMIT=$(git log --grep="\[build\]\[bazel\] Move install_base under parent outdir" --format="%H" -n 1 || true)
    if [ -n "$INSTALL_BASE_COMMIT" ]; then
        echo "Found install_base commit: $INSTALL_BASE_COMMIT"
    else
        echo "WARNING: Could not find install_base commit in log!"
    fi

    echo "[Progress] Temporarily patching build/regenerator.py to limit GN to 1 thread..."
    cat << 'EOF' > "$TEST_DIR/patch_regenerator.py"
import sys
with open('build/regenerator.py', 'r') as f:
    content = f.read()
target = '"--ninja-outputs-file=ninja_outputs.json",'
replacement = '"--ninja-outputs-file=ninja_outputs.json",\n            "--threads=1",'
if target in content:
    content = content.replace(target, replacement)
    with open('build/regenerator.py', 'w') as f:
        f.write(content)
    print('Patched build/regenerator.py successfully')
else:
    print('Failed to find target in build/regenerator.py')
    sys.exit(1)
EOF
    python3 "$TEST_DIR/patch_regenerator.py"

    # Commit it so it is checked out in worktrees
    git -c user.name="E2E Test" -c user.email="e2e@test.com" commit -m "temp: patch GN threads for E2E" build/regenerator.py
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
action("sim_link") {
  script = "link_tool.py"
  sources = [
    "source.txt",
    "//prebuilt/tools/gsutil/gsutil",
    "//.git/index",
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
    GN_BIN="/usr/local/google/home/awolter/fuchsia/prebuilt/third_party/gn/linux-x64/gn"
    NINJA_BIN="/usr/bin/ninja"
    mkdir -p scripts
    cat << EOF > scripts/fx
#!/bin/bash
dir="out/default"
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

# ==============================================================================
# 2. Setup Old Jiri
# ==============================================================================
if [ "$REAL_MODE" = "false" ]; then
    echo "[Progress] Bootstrapping official (old) Jiri..."
    mkdir -p "$TEST_ROOT"
    cd "$TEST_ROOT"
    curl -s "https://fuchsia.googlesource.com/jiri/+/HEAD/scripts/bootstrap_jiri?format=TEXT" | base64 --decode | bash -s .
    OLD_JIRI="$TEST_ROOT/.jiri_root/bin/jiri"

    echo "[Progress] Initializing Jiri root with old Jiri..."
    "$OLD_JIRI" init -shared=true -enable-lockfile=false -analytics-opt=false .
    "$OLD_JIRI" import minimal.xml "$MANIFEST_REPO"
    run_jiri_update "$OLD_JIRI"

    # Initial build with old Jiri
    echo "[Progress] Running initial build with old Jiri..."
    scripts/fx set "$CONFIG_NAME"
    scripts/fx build
else
    echo "[Progress] Compiling temporary new Jiri to restore symlinks..."
    NEW_JIRI_SRC="/usr/local/google/home/awolter/src/jiri"
    (cd "$NEW_JIRI_SRC" && go build -o "$TEST_DIR/pre_cleanup_jiri" ./cmd/jiri)
    
    echo "[Progress] Restoring cache symlinks to real directories..."
    "$TEST_DIR/pre_cleanup_jiri" init -prebuilt-cache=false
    "$TEST_DIR/pre_cleanup_jiri" update

    echo "[Progress] Restoring official (old) Jiri to real repo..."
    cd "$TEST_ROOT"
    # Download and overwrite with official Jiri
    curl -s "https://fuchsia.googlesource.com/jiri/+/HEAD/scripts/bootstrap_jiri?format=TEXT" | base64 --decode | bash -s .
    # Make sure cache is disabled initially
    if [ -f .jiri_root/config ]; then
        sed -i 's/<enabled>true<\/enabled>/<enabled>false<\/enabled>/g' .jiri_root/config
    fi
    run_jiri_update ./.jiri_root/bin/jiri
    
    echo "[Progress] Running initial build with old Jiri..."
    scripts/fx set "$CONFIG_NAME"
    scripts/fx build
fi

# ==============================================================================
# 3. Inject New Jiri
# ==============================================================================
echo "[Progress] Compiling and injecting new Jiri..."
NEW_JIRI_SRC="/usr/local/google/home/awolter/src/jiri"
(cd "$NEW_JIRI_SRC" && go build -o "$TEST_DIR/new_jiri" ./cmd/jiri)
cp "$TEST_DIR/new_jiri" "$TEST_ROOT/.jiri_root/bin/jiri"
JIRI_BIN="./.jiri_root/bin/jiri"

echo "[Progress] Verifying Jiri update with new Jiri..."
    run_jiri_update "$JIRI_BIN"

# Verify build still works and is no-op
scripts/fx build
check_noop "$TEST_ROOT"

# ==============================================================================
# 4. Toggle Prebuilt Cache (Main Tree)
# ==============================================================================

echo "[Progress] Enabling prebuilt cache in main tree..."
"$JIRI_BIN" init -prebuilt-cache=true
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

echo "[Progress] Disabling prebuilt cache in main tree..."
"$JIRI_BIN" init -prebuilt-cache=false
echo "[Progress] Running jiri update to restore from cache..."
run_jiri_update "$JIRI_BIN"

if [ "$REAL_MODE" = "false" ]; then
    echo "[Progress] Verifying symlinks after restoration..."
    check_file="$TEST_ROOT/prebuilt/third_party/web_engine_tests_latest/arch/x64/common_tests_manifest.json"
    if [ -L "$check_file" ]; then
        target=$(readlink "$check_file")
        if [[ "$target" == *".jiri_root/prebuilts"* ]]; then
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

$FX_WORKTREE_BIN add "$CONFIG_NAME"

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
WT_PATH=$($FX_WORKTREE_BIN lease "$CONFIG_NAME" --print-path-only)
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
$FX_WORKTREE_BIN release "$WT_ID"
WT_PATH2=$($FX_WORKTREE_BIN lease "$CONFIG_NAME" --print-path-only)
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
$FX_WORKTREE_BIN sync "$WT_ID"
cd "$WT_PATH"
scripts/fx build
check_noop "$WT_PATH"

# ==============================================================================
# 6. Enable Cache & Migrate Both
# ==============================================================================
echo "[Progress] Enabling cache in main tree..."
cd "$TEST_ROOT"
"$JIRI_BIN" init -prebuilt-cache=true
echo "[Progress] Updating main tree (migration)..."
run_jiri_update "$JIRI_BIN"
scripts/fx build
check_noop "$TEST_ROOT"

echo "[Progress] Syncing worktree (migration)..."
$FX_WORKTREE_BIN sync "$WT_ID"
cd "$WT_PATH"
scripts/fx build
check_noop "$WT_PATH"

# ==============================================================================
# 7. Add New Worktree (Cache On)
# ==============================================================================
echo "[Progress] Adding second worktree (cache on)..."
cd "$TEST_ROOT"
$FX_WORKTREE_BIN add "$CONFIG_NAME"

echo "[Progress] Leasing second worktree..."
WT_PATH_NEW=$($FX_WORKTREE_BIN lease "$CONFIG_NAME" --print-path-only)
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
# 8. Cleanup
# ==============================================================================
echo "[Progress] Releasing and removing test worktrees..."
$FX_WORKTREE_BIN release "$WT_ID"
$FX_WORKTREE_BIN release "$WT_ID_NEW"
$FX_WORKTREE_BIN remove "$WT_ID" --force
$FX_WORKTREE_BIN remove "$WT_ID_NEW" --force

# The EXIT trap will handle restoring Jiri if failed, or installing new Jiri on success.
