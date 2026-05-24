---
name: fx-worktree
description: >-
  Manages Fuchsia build directories (outdirs) and isolated git worktrees (workspaces)
  for parallel agent executions, supporting RBE and cache reuse.
tags: [fuchsia, workspace, build, RBE, cache, worktree]
support_tier: primary
category: infrastructure
version: 1.2.0
---

# `fx-worktree` (Fuchsia Worktree Manager)

`fx-worktree` is a CLI tool designed to manage parallel development environments (worktrees) inside a Fuchsia checkout. It allows multiple AI agents or developers to compile and test code concurrently without conflicting build directories or git states.

## When to Use

Use `fx-worktree` when:
*   You need to lease an isolated worktree to make changes and compile them.
*   You want to run builds in parallel without conflicting with the main checkout or other agents.
*   You want to reuse a shared pool of persistent worktrees to speed up compilation (preserving Ninja cache).
*   You want to verify your fx-worktree setup via the E2E test suite.

## CLI Commands

### 1. Worktree Pool Management
These commands manage the pool of persistent worktrees. The worktrees (including their build directories) reside physically under `~/.fuchsia/worktrees/environments/<worktree_id>`.

*   **Add a Worktree**:
    ```bash
    fx-worktree add <config_name>
    ```
    Add a new worktree with a dedicated outdir. E.g.:
    ```bash
    fx-worktree add fuchsia_internal.x64
    ```

*   **Remove a Worktree**:
    ```bash
    fx-worktree remove <worktree_id>
    ```
    Remove a worktree and its dedicated outdir (cannot remove if currently leased).

*   **List Worktrees**:
    ```bash
    fx-worktree list
    ```
    List worktrees.

### 2. Lease & Lifecycle
*   **Lease a Worktree**:
    ```bash
    fx-worktree lease <config_name> [--agent-id <agent_name>] [--sync]
    ```
    Lease a worktree to start work.
    By default, it does NOT sync the worktree. Pass `--sync` to update it to the latest code in the main fuchsia checkout and update prebuilts.

*   **Sync a Worktree**:
    ```bash
    fx-worktree sync <worktree_id>
    ```
    Update a worktree to the latest code in the main fuchsia checkout.

*   **Release a Worktree**:
    ```bash
    fx-worktree release <worktree_id>
    ```
    Release and reset a worktree (does a git reset).

*   **Change Directory**:
    ```bash
    fx-worktree cd [<worktree_id>]
    ```
    Change directory to a worktree (shell wrapper required).

*   **Locate Worktree Path (Hidden Command)**:
    ```bash
    fx-worktree locate [<worktree_id>]
    ```
    Prints the absolute path of the worktree directory.

### 3. Verification & Testing
*   **Run E2E Test Suite**:
    ```bash
    ./tests/e2e.sh
    ```
    Runs a comprehensive end-to-end test in Mock Mode (no Fuchsia checkout needed) to verify the `fx-worktree` tool's behavior (leasing, build regeneration, cache migration).

*   **Generate Shell Completions (Hidden Command)**:
    ```bash
    fx-worktree completions <bash|elvish|fish|powershell|zsh>
    ```

---

## Technical Design & Constraints

### 1. RBE (Remote Build Execution) Support
Fuchsia's RBE compiler wrappers require the build directory to be a subdirectory of the execution root (the workspace).
`fx-worktree` satisfies this by creating the build directory directly inside the worktree (`<worktree_path>/out/default`).
Since the worktree is a self-contained directory under `~/.fuchsia/worktrees/environments/`, RBE builds succeed.

### 2. Prebuilt Isolation & Shared Cache
Fuchsia checkouts contain hundreds of prebuilt packages (toolchains, SDKs, firmware) managed by Jiri and CIPD. Sharing the parent's `prebuilt/` directory causes workspaces to dirty each other's builds when the parent updates.

To isolate them while retaining cache sharing:
*   **Querying**: `fx-worktree` queries the required packages for the workspace's revision using `jiri package` in the workspace.
*   **Grouping**: Packages are grouped by their target path. This is critical because Jiri allows multiple packages to overlap in the same destination (e.g., Rust host compiler and target libraries both install to `prebuilt/third_party/rust/linux-x64`).
*   **Shared Cache**: Packages are installed into a central cache at `~/.fuchsia/worktrees/shared-prebuilts/merged/<target_path_escaped>/<hash>/`. The hash is a SipHash of the sorted package names and versions in the group, ensuring different workspaces on the same revision share the exact same merged directories.
*   **Mtime Clamping**: Some prebuilt packages contain files with artificial modification times in the far future (e.g., Bazel uses `2042-07-28` for determinism). Since Ninja tracks these as dynamic inputs (recorded in `.ninja_deps`), they always trigger rebuilds because today's build outputs are older than `2042`. To fix this, `fx-worktree` recursively clamps the modification times of all files in newly downloaded cache directories to a fixed past date (`2020-01-01 00:00:00 UTC`), preserving build cache no-ops.
*   **Symlinking**: Individual package target directories in the workspace are symlinked to their corresponding directory in the shared cache.
*   **Wheel Extraction**: Some dependencies (like `pydantic-core` and `protobuf-py3`) are distributed as CIPD wheel packages but extracted into the source tree by Jiri hooks. Since we isolate `prebuilt/` and avoid Jiri's slow `run-hooks` (which triggers network fetches), `fx-worktree` manually runs the local wheel extraction scripts (`extract_pydantic_core_wheel.sh` and `extract_protobuf_py3_wheel.sh`) during allocation to extract them locally in the workspace.

### 3. Jiri Metadata in Git Worktrees
Jiri stores project metadata (remote URL, branch) inside `.git/jiri/`. Because `git worktree add` creates a new Git directory under the parent's `.git/worktrees/<name>`, it lacks this Jiri metadata. Without it, `jiri` commands run in the workspace (such as `jiri package` to resolve prebuilts) fail to recognize projects as local and attempt to clone them from the network, causing severe performance hits.

To resolve this, `fx-worktree` automatically symlinks the parent project's Jiri metadata directory (`.git/jiri`) into the worktree's Git directory (`.git/worktrees/<name>/jiri`) during setup/allocation. This makes `jiri` offline-friendly and extremely fast (completes in 1 second).

### 4. Topology
*   **Global Config Directory**: `~/.fuchsia/worktrees/`
    *   `leases/`: Active lease files (`<worktree_id>.lease`).
    *   `environments/`: Parent directory for active isolated workspaces (worktrees).
    *   `shared-prebuilts/`: Central CIPD cache with merged subdirectories.
