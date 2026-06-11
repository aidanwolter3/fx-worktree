#!/bin/bash
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style
# license that can be found in the LICENSE file.

# Benchmark for fx-worktree operations on a real Fuchsia directory.
# Measures jiri worktree creation, fx-worktree lease/release, and cleanup.

set -e

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

# Config to use for benchmark
CONFIG="${1:-bringup.x64}"

# Paths
FX_WORKTREE_SRC=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
FX_WORKTREE_BIN="$FX_WORKTREE_SRC/target/debug/fx-worktree"

# Verify FUCHSIA_DIR is set
if [ -z "$FUCHSIA_DIR" ]; then
    echo -e "${RED}ERROR: FUCHSIA_DIR environment variable is not set.${NC}"
    exit 1
fi

echo "Benchmarking fx-worktree on real Fuchsia directory: $FUCHSIA_DIR"
echo "Using config: $CONFIG"
echo "Using binary: $FX_WORKTREE_BIN"

# Build latest binary
echo "Building fx-worktree..."
cargo build --manifest-path "$FX_WORKTREE_SRC/Cargo.toml"

run_jiri() {
    (cd "$FUCHSIA_DIR" && jiri "$@")
}

measure_time() {
    local label="$1"
    shift
    local start=$(date +%s%N)
    set +e
    "$@"
    local exit_code=$?
    set -e
    local end=$(date +%s%N)
    local duration=$(( (end - start) / 1000000 )) # ms
    
    if [ $exit_code -eq 0 ]; then
        echo -e "${GREEN}BENCHMARK: $label took ${duration}ms${NC}" >&2
    else
        echo -e "${RED}BENCHMARK: $label FAILED with exit code $exit_code (took ${duration}ms)${NC}" >&2
        return $exit_code
    fi
}

# Determine matching outdir in the parent fuchsia directory
PARENT_OUTDIR=""
for d in "$FUCHSIA_DIR"/out/*; do
    if [ -f "$d/args.gn" ]; then
        p=$(grep "build_info_product" "$d/args.gn" | cut -d'"' -f2 || true)
        b=$(grep "build_info_board" "$d/args.gn" | cut -d'"' -f2 || true)
        if [ "$p.$b" = "$CONFIG" ] || [ "$p" = "$CONFIG" ]; then
            PARENT_OUTDIR="$d"
            break
        fi
    fi
done

if [ -z "$PARENT_OUTDIR" ]; then
    echo -e "${RED}ERROR: Could not find build outdir in $FUCHSIA_DIR matching config $CONFIG.${NC}"
    echo "Make sure you have run 'fx set' for this config in the main tree."
    exit 1
fi

OUTDIR_REL=$(basename "$PARENT_OUTDIR")
echo "Found matching parent outdir: out/$OUTDIR_REL"

WT_NAME="bench-wt-$$"
WT_PATH="$FUCHSIA_DIR/.jiri_root/worktrees/$WT_NAME"

cleanup() {
    if [ -d "$WT_PATH" ]; then
        echo "Cleaning up worktree at $WT_PATH..."
        # Release if leased
        $FX_WORKTREE_BIN release "$WT_NAME" >/dev/null 2>&1 || true
        # Remove Jiri worktree
        run_jiri worktree remove -force "$WT_PATH" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

# 1. Benchmark 'jiri worktree add'
echo -e "\n--- Running 'jiri worktree add' benchmark ---"
measure_time "jiri worktree add" run_jiri worktree add "$WT_PATH"

# 2. Provision outdir & args.gn inside worktree
echo "Provisioning GN outdir out/$OUTDIR_REL in worktree..."
mkdir -p "$WT_PATH/out/$OUTDIR_REL"
cp "$PARENT_OUTDIR/args.gn" "$WT_PATH/out/$OUTDIR_REL/args.gn"
# Also mock .fx-build-dir
echo "out/$OUTDIR_REL" > "$WT_PATH/.fx-build-dir"

# 3. Mark the worktree free
echo "Marking worktree $WT_NAME as free..."
$FX_WORKTREE_BIN mark-free "$WT_NAME" >/dev/null

# 4. Benchmark 'lease' (with sync)
echo -e "\n--- Running 'lease (with sync)' benchmark ---"
measure_time "fx-worktree lease (sync=true)" $FX_WORKTREE_BIN lease "$WT_NAME" --sync --print-path-only >/dev/null

# 5. Benchmark 'release'
echo -e "\n--- Running 'release' benchmark ---"
measure_time "fx-worktree release" $FX_WORKTREE_BIN release "$WT_NAME"

# 6. Benchmark 'lease' (no sync)
echo -e "\n--- Running 'lease (no sync)' benchmark ---"
measure_time "fx-worktree lease (sync=false)" $FX_WORKTREE_BIN lease "$WT_NAME" --print-path-only >/dev/null

# Release again before remove
$FX_WORKTREE_BIN release "$WT_NAME" >/dev/null

# 7. Benchmark 'jiri worktree remove'
echo -e "\n--- Running 'jiri worktree remove' benchmark ---"
measure_time "jiri worktree remove" run_jiri worktree remove -force "$WT_PATH"

# Clear WT_PATH so cleanup trap doesn't try to remove it again
WT_PATH=""

echo -e "\n${GREEN}Benchmark completed successfully.${NC}"
