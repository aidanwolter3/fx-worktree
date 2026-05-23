#!/bin/bash
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style
# license that can be found in the LICENSE file.

# Self-contained E2E test for fx-worktree.
# Tests:
# 1. lease, build, build => is no-op
# 2. lease, build, release, lease, build => is no-op
# 3. lease, build, jiri update main tree, build => is no-op

set -e

# Colors for output
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m' # No Color

# Setup temp directory
TEST_DIR=$(mktemp -d -t fx-worktree-e2e-XXXXXX)
echo "Using temporary test directory: $TEST_DIR"

# Cleanup handler
cleanup() {
    if [ $? -eq 0 ]; then
        echo "Test PASSED. Cleaning up $TEST_DIR..."
        rm -rf "$TEST_DIR"
    else
        echo -e "${RED}Test FAILED. Keeping $TEST_DIR for debugging.${NC}"
    fi
}
trap cleanup EXIT

# Paths
MANIFEST_REPO="$TEST_DIR/manifest_repo"
SOURCE_REPO1="$TEST_DIR/source_repo1"
SOURCE_REPO2="$TEST_DIR/source_repo2"
TEST_ROOT="$TEST_DIR/test_root"
FX_WORKTREE_ROOT="$TEST_DIR/.fx_worktree_root"

# Find fx-worktree source root (parent of tests/ directory)
FX_WORKTREE_SRC=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
FX_WORKTREE_BIN="$FX_WORKTREE_SRC/target/debug/fx-worktree"

# Build fx-worktree
echo "Building fx-worktree..."
cargo build --manifest-path "$FX_WORKTREE_SRC/Cargo.toml"

# Export env vars for fx-worktree
export FUCHSIA_DIR="$TEST_ROOT"
export FX_WORKTREE_ROOT="$FX_WORKTREE_ROOT"
mkdir -p "$FX_WORKTREE_ROOT"

# Jiri binary to use (arg $1, or env JIRI_BIN, or default)
JIRI_BIN="${1:-${JIRI_BIN:-/usr/local/google/home/awolter/fuchsia/.jiri_root/bin/jiri}}"
GN_BIN="/usr/local/google/home/awolter/fuchsia/prebuilt/third_party/gn/linux-x64/gn"
NINJA_BIN="/usr/bin/ninja"

# ==============================================================================
# 1. Prepare Mock Repositories
# ==============================================================================
echo "Preparing mock repositories..."

# --- source_repo1 ---
mkdir -p "$SOURCE_REPO1"
cd "$SOURCE_REPO1"
git init -q

cat << 'EOF' > BUILD.gn
action("sim_link") {
  script = "link_tool.py"
  sources = [ "source.txt" ]
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
echo "dummy" > dummy.txt
git add .
git commit -m "initial commit" -q

# --- manifest_repo ---
mkdir -p "$MANIFEST_REPO"
cd "$MANIFEST_REPO"
git init -q

# Create minimal.xml dynamically to include absolute paths of temp repos
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
group("all") {
  deps = [
    "//source_repo1:sim_link",
  ]
}
EOF

# Mock fx script
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
elif [ "\$cmd" = "build" ]; then
  "$NINJA_BIN" -C "\$dir" "\$@"
elif [ "\$cmd" = "get-build-dir" ]; then
  echo "\$dir"
fi
EOF
chmod +x scripts/fx

git add .
git commit -m "initial commit" -q

# ==============================================================================
# 2. Initialize Jiri root (main tree)
# ==============================================================================
echo "Initializing Jiri root..."
mkdir -p "$TEST_ROOT"
cd "$TEST_ROOT"
"$JIRI_BIN" init -shared=true -enable-lockfile=false -analytics-opt=false .
# Copy bootstrap jiri to .jiri_root/bin/jiri so fx-worktree uses it
mkdir -p .jiri_root/bin
cp "$JIRI_BIN" .jiri_root/bin/jiri

./.jiri_root/bin/jiri import minimal.xml "$MANIFEST_REPO"
./.jiri_root/bin/jiri update

# ==============================================================================
# 3. Add worktree via fx-worktree
# ==============================================================================
echo "Adding worktree via fx-worktree..."
$FX_WORKTREE_BIN add mock_config

# ==============================================================================
# Scenario 1: lease, build, build => is no-op
# ==============================================================================
echo -e "\n--- Running Scenario 1: lease, build, build => is no-op ---"

# Lease the worktree
WT_PATH=$($FX_WORKTREE_BIN lease mock_config --print-path-only)
WT_ID=$(basename "$WT_PATH")
echo "Leased worktree $WT_ID at $WT_PATH"

cd "$WT_PATH"

# First build
echo "Running first build..."
# Sleep to ensure ctime changes to a different second relative to checkout
sleep 1
scripts/fx build

# Sleep to ensure ctime/mtime comparison is valid if we build again
sleep 1

# Verify no-op before second build
echo "Verifying no-op before second build..."
explain_output=$(scripts/fx ninja -d explain -n -v 2>&1 | grep "ninja explain:" || true)
if [ -n "$explain_output" ]; then
    echo -e "${RED}FAIL: Build was not a no-op immediately after run${NC}"
    echo "$explain_output"
    exit 1
fi

# Second build
echo "Running second build..."
set +e
build_output=$(scripts/fx build 2>&1)
exit_code=$?
set -e

if [ $exit_code -ne 0 ]; then
    echo -e "${RED}FAIL: Second build failed${NC}"
    echo "$build_output"
    exit 1
fi

if ! echo "$build_output" | grep -q "no work to do"; then
    echo -e "${RED}FAIL: Second build did work (expected no work to do)${NC}"
    echo "Build output:"
    echo "$build_output"
    exit 1
fi
echo "Scenario 1 PASSED."

# ==============================================================================
# Scenario 2: lease, build, release, lease, build => is no-op
# ==============================================================================
echo -e "\n--- Running Scenario 2: lease, build, release, lease, build => is no-op ---"

# Currently leased. We need to release it.
echo "Releasing worktree $WT_ID..."
$FX_WORKTREE_BIN release "$WT_ID"

# Lease it again
echo "Leasing worktree again..."
WT_PATH2=$($FX_WORKTREE_BIN lease mock_config --print-path-only)
if [ "$WT_PATH" != "$WT_PATH2" ]; then
    echo -e "${RED}FAIL: Leased a different worktree: $WT_PATH2 (expected $WT_PATH)${NC}"
    exit 1
fi

cd "$WT_PATH2"

# Verify no-op BEFORE building (this is where the ctime bug would trigger rebuild)
echo "Verifying no-op after lease..."
explain_output=$(scripts/fx ninja -d explain -n -v 2>&1 | grep "ninja explain:" || true)
if [ -n "$explain_output" ]; then
    echo -e "${RED}FAIL: Build is NOT a no-op after lease!${NC}"
    echo "Ninja explanation:"
    echo "$explain_output"
    exit 1
fi

# Build again (should be no-op)
echo "Running build..."
set +e
build_output=$(scripts/fx build 2>&1)
exit_code=$?
set -e

if [ $exit_code -ne 0 ]; then
    echo -e "${RED}FAIL: Build failed${NC}"
    echo "$build_output"
    exit 1
fi

if ! echo "$build_output" | grep -q "no work to do"; then
    echo -e "${RED}FAIL: Build did work after lease/release (expected no work to do)${NC}"
    echo "Build output:"
    echo "$build_output"
    exit 1
fi
echo "Scenario 2 PASSED."

# ==============================================================================
# Scenario 3: lease, build, jiri update main tree, build (in worktree still) => is no-op
# ==============================================================================
echo -e "\n--- Running Scenario 3: lease, build, jiri update main tree, build => is no-op ---"

# Currently leased (from Scenario 2).
# We run a build to ensure we are clean (we just did, it was no-op).

# Go to main tree and run jiri update
echo "Running jiri update in main tree..."
cd "$TEST_ROOT"
.jiri_root/bin/jiri update

# Go back to worktree and build
echo "Running build in worktree again..."
cd "$WT_PATH2"

set +e
build_output=$(scripts/fx build 2>&1)
exit_code=$?
set -e

if [ $exit_code -ne 0 ]; then
    echo -e "${RED}FAIL: Build failed in Scenario 3${NC}"
    echo "$build_output"
    exit 1
fi

if ! echo "$build_output" | grep -q "no work to do"; then
    echo -e "${RED}FAIL: Build did work after main tree update (expected no work to do)${NC}"
    echo "Build output:"
    echo "$build_output"
    exit 1
fi
echo "Scenario 3 PASSED."

echo -e "\n${GREEN}ALL SCENARIOS PASSED!${NC}"
