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
        fi
        echo "Cleaning up $TEST_DIR..."
        rm -rf "$TEST_DIR"
    else
        echo -e "${RED}Uber E2E Test FAILED!${NC}"
        if [ "$REAL_MODE" = "true" ]; then
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
  </packages>
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
group("all") {
  deps = [
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
    local explain_output
    explain_output=$(scripts/fx ninja -d explain -n -v 2>&1 | grep "ninja explain:" | head -n 1 || true)
    if [ -n "$explain_output" ]; then
        echo -e "${RED}FAIL: Build was not a no-op in $dir${NC}"
        echo "Ninja explain output:"
        echo "$explain_output"
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
    "$OLD_JIRI" update

    # Initial build with old Jiri
    echo "[Progress] Running initial build with old Jiri..."
    scripts/fx set "$CONFIG_NAME"
    scripts/fx build
else
    echo "[Progress] Restoring official (old) Jiri to real repo..."
    cd "$TEST_ROOT"
    # Download and overwrite with official Jiri
    curl -s "https://fuchsia.googlesource.com/jiri/+/HEAD/scripts/bootstrap_jiri?format=TEXT" | base64 --decode | bash -s .
    # Make sure cache is disabled initially
    if [ -f .jiri_root/config ]; then
        sed -i 's/<enabled>true<\/enabled>/<enabled>false<\/enabled>/g' .jiri_root/config
    fi
    ./.jiri_root/bin/jiri update
    
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
"$JIRI_BIN" update

# Verify build still works and is no-op
scripts/fx build
check_noop "$TEST_ROOT"

# ==============================================================================
# 4. Toggle Prebuilt Cache (Main Tree)
# ==============================================================================
echo "[Progress] Enabling prebuilt cache in main tree..."
"$JIRI_BIN" init -prebuilt-cache=true
echo "[Progress] Running jiri update to migrate to cache..."
"$JIRI_BIN" update

echo "[Progress] Building after cache enablement..."
scripts/fx build
check_noop "$TEST_ROOT"

echo "[Progress] Disabling prebuilt cache in main tree..."
"$JIRI_BIN" init -prebuilt-cache=false
echo "[Progress] Running jiri update to restore from cache..."
"$JIRI_BIN" update

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

echo "[Progress] Testing release and re-lease..."
$FX_WORKTREE_BIN release "$WT_ID"
WT_PATH2=$($FX_WORKTREE_BIN lease "$CONFIG_NAME" --print-path-only)
if [ "$WT_PATH" != "$WT_PATH2" ]; then
    echo -e "${RED}FAIL: Leased a different worktree path: $WT_PATH2 (expected $WT_PATH)${NC}"
    exit 1
fi
cd "$WT_PATH"
check_noop "$WT_PATH"

echo "[Progress] Updating main tree and verifying worktree no-op..."
cd "$TEST_ROOT"
"$JIRI_BIN" update

cd "$WT_PATH"
check_noop "$WT_PATH"

echo "[Progress] Syncing worktree..."
$FX_WORKTREE_BIN sync "$WT_ID"
check_noop "$WT_PATH"

# ==============================================================================
# 6. Enable Cache & Migrate Both
# ==============================================================================
echo "[Progress] Enabling cache in main tree..."
cd "$TEST_ROOT"
"$JIRI_BIN" init -prebuilt-cache=true
echo "[Progress] Updating main tree (migration)..."
"$JIRI_BIN" update
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
