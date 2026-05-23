#!/bin/bash
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style
# license that can be found in the LICENSE file.

# Benchmark for fx-worktree operations on a real Fuchsia directory.
# Runs add, lease, sync, release, and remove, measuring execution times.

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

# Variable to store the worktree ID we create
WT_ID=""

cleanup() {
    if [ -n "$WT_ID" ]; then
        echo "Cleaning up worktree $WT_ID..."
        # Release if leased (ignore error if not leased)
        $FX_WORKTREE_BIN release "$WT_ID" >/dev/null 2>&1 || true
        # Remove
        $FX_WORKTREE_BIN remove "$WT_ID" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

# 1. Benchmark 'add'
echo -e "\n--- Running 'add' benchmark ---"
# Capture output to get the ID
add_output=$(mktemp)
measure_time "fx-worktree add" $FX_WORKTREE_BIN --json add "$CONFIG" > "$add_output"
WT_ID=$(json_pp < "$add_output" | grep "environment_id" | cut -d'"' -f4 || true)
if [ -z "$WT_ID" ]; then
    # Fallback parsing if json_pp is not available
    WT_ID=$(grep -o '"environment_id":"[^"]*' "$add_output" | cut -d'"' -f4 || true)
fi
rm -f "$add_output"

if [ -z "$WT_ID" ]; then
    echo -e "${RED}ERROR: Failed to parse worktree ID from add output.${NC}"
    exit 1
fi
echo "Created worktree: $WT_ID"

# 2. Benchmark 'lease' (with sync)
echo -e "\n--- Running 'lease (with sync)' benchmark ---"
# We lease by config, it should reuse the one we just created
measure_time "fx-worktree lease (sync=true)" $FX_WORKTREE_BIN lease "$CONFIG" --sync --print-path-only >/dev/null

# 3. Benchmark 'sync' (no-op)
echo -e "\n--- Running 'sync (no-op)' benchmark ---"
measure_time "fx-worktree sync (no-op)" $FX_WORKTREE_BIN sync "$WT_ID"


# 5. Benchmark 'release'
echo -e "\n--- Running 'release' benchmark ---"
measure_time "fx-worktree release" $FX_WORKTREE_BIN release "$WT_ID"

# 6. Benchmark 'lease' (no sync)
echo -e "\n--- Running 'lease (no sync)' benchmark ---"
measure_time "fx-worktree lease (sync=false)" $FX_WORKTREE_BIN lease "$CONFIG" --print-path-only >/dev/null

# Release again before remove
$FX_WORKTREE_BIN release "$WT_ID" >/dev/null

# 7. Benchmark 'remove'
echo -e "\n--- Running 'remove' benchmark ---"
measure_time "fx-worktree remove" $FX_WORKTREE_BIN remove "$WT_ID"

# Clear WT_ID so cleanup trap doesn't try to remove it again
WT_ID=""

echo -e "\n${GREEN}Benchmark completed successfully.${NC}"
