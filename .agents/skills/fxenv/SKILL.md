---
name: fxenv
description: >-
  Manages Fuchsia build directories (outdirs) and isolated git worktrees (workspaces)
  for parallel agent executions, supporting RBE and cache reuse.
tags: [fuchsia, workspace, build, RBE, cache, worktree]
support_tier: primary
category: infrastructure
version: 1.1.0
---

# `fxenv` (Fuchsia Environment Manager)

`fxenv` is a CLI tool designed to manage parallel development environments (workspaces) inside a Fuchsia checkout. It allows multiple AI agents or developers to compile and test code concurrently without conflicting build directories or git states.

## When to Use

Use `fxenv` when:
*   You need to allocate an isolated workspace (git worktree) to make changes and compile them.
*   You want to run builds in parallel without conflicting with the main checkout or other agents.
*   You want to reuse a shared pool of pre-built build directories (outdirs) to speed up compilation.
*   You want to verify your fxenv setup via the `self-test` command.

## CLI Commands

### 1. Environment Pool Management
These commands manage the pool of persistent environments. The build directories reside physically under `~/fuchsia/out/fxenv/<config_name>_<uuid>`.

*   **Create an Environment**:
    ```bash
    fxenv create <config_name>
    ```
    Creates a new build environment inside the pool (runs `fx set` and performs an initial build). E.g.:
    ```bash
    fxenv create fuchsia_internal.x64
    ```

*   **Delete an Environment**:
    ```bash
    fxenv delete <env_id>
    ```
    Deletes the specified environment from the pool (cannot delete if currently leased).

*   **List Environments**:
    ```bash
    fxenv list
    ```
    Lists all environments in the pool, their IDs, and whether they are currently "In Use" (leased) or "Free".

### 2. Allocation & Lifecycle
*   **Use (Allocate) an Environment**:
    ```bash
    fxenv use <config_name> [--agent-id <agent_name>]
    ```
    Leases a free environment from the pool matching the config and creates an isolated git worktree mapped to it. 
    Prints allocation details (path, agent ID, etc.) and runs `fx gen`.

*   **Free (Release) an Environment**:
    ```bash
    fxenv free <env_id>
    ```
    Cleans up the git worktree and **moves the build directory back to the pool**, preserving the build cache.

*   **Locate Environment Path**:
    ```bash
    fxenv locate [<env_id>]
    ```
    Prints the absolute path of the workspace directory for the environment.

*   **Change Directory**:
    ```bash
    fxenv cd [<env_id>]
    ```
    Helper to navigate to the workspace directory (requires shell wrapper configuration).

### 3. Verification & Maintenance
*   **Run Automated Self-Test**:
    ```bash
    fxenv self-test [--use-env <env_id>]
    ```
    Runs a full programmatic verification of the `fxenv` lifecycle (warming, allocation, incremental compiles, cache preservation, and cleanup).

*   **Garbage Collection**:
    ```bash
    fxenv gc [--timeout <seconds>]
    ```
    Finds and cleans up orphaned environments where the owning process (recorded PID) has died, or the lease has expired (default: cleans all orphaned leases).

*   **Generate Shell Completions**:
    ```bash
    fxenv completions <bash|elvish|fish|powershell|zsh>
    ```

---

## Technical Design & Constraints

### 1. RBE (Remote Build Execution) Support
Fuchsia's RBE compiler wrappers require the build directory to be a subdirectory of the execution root (the workspace). 
To support RBE:
*   **During Allocation**: `fxenv` physically **moves (renames)** the leased outdir into the workspace (`workspace_path/out/default`). Relative symlinks inside `gen/` resolve correctly, and RBE builds succeed.
*   **During Free**: The outdir is moved back to the pool (`~/fuchsia/out/fxenv/`).
*   **Performance**: Since the workspaces (defaults to `~/.fuchsia-agents/environments/`) and the Fuchsia checkout are on the same filesystem, this move is instantaneous.

### 2. Prebuilt Isolation & Shared Cache
Fuchsia checkouts contain hundreds of prebuilt packages (toolchains, SDKs, firmware) managed by Jiri and CIPD. Sharing the parent's `prebuilt/` directory causes workspaces to dirty each other's builds when the parent updates.

To isolate them while retaining cache sharing:
*   **Querying**: `fxenv` queries the required packages for the workspace's revision using `jiri package` in the workspace.
*   **Grouping**: Packages are grouped by their target path. This is critical because Jiri allows multiple packages to overlap in the same destination (e.g., Rust host compiler and target libraries both install to `prebuilt/third_party/rust/linux-x64`).
*   **Shared Cache**: Packages are installed into a central cache at `~/.fuchsia-agents/shared-prebuilts/merged/<target_path_escaped>/<hash>/`. The hash is a SipHash of the sorted package names and versions in the group, ensuring different workspaces on the same revision share the exact same merged directories.
*   **Mtime Clamping**: Some prebuilt packages contain files with artificial modification times in the far future (e.g., Bazel uses `2042-07-28` for determinism). Since Ninja tracks these as dynamic inputs (recorded in `.ninja_deps`), they always trigger rebuilds because today's build outputs are older than `2042`. To fix this, `fxenv` recursively clamps the modification times of all files in newly downloaded cache directories to a fixed past date (`2020-01-01 00:00:00 UTC`), preserving build cache no-ops.
*   **Symlinking**: Individual package target directories in the workspace are symlinked to their corresponding directory in the shared cache.
*   **Wheel Extraction**: Some dependencies (like `pydantic-core` and `protobuf-py3`) are distributed as CIPD wheel packages but extracted into the source tree by Jiri hooks. Since we isolate `prebuilt/` and avoid Jiri's slow `run-hooks` (which triggers network fetches), `fxenv` manually runs the local wheel extraction scripts (`extract_pydantic_core_wheel.sh` and `extract_protobuf_py3_wheel.sh`) during allocation to extract them locally in the workspace.

### 3. Jiri Metadata in Git Worktrees
Jiri stores project metadata (remote URL, branch) inside `.git/jiri/`. Because `git worktree add` creates a new Git directory under the parent's `.git/worktrees/<name>`, it lacks this Jiri metadata. Without it, `jiri` commands run in the workspace (such as `jiri package` to resolve prebuilts) fail to recognize projects as local and attempt to clone them from the network, causing severe performance hits.

To resolve this, `fxenv` automatically symlinks the parent project's Jiri metadata directory (`.git/jiri`) into the worktree's Git directory (`.git/worktrees/<name>/jiri`) during setup/allocation. This makes `jiri` offline-friendly and extremely fast (completes in 1 second).

### 4. Topology
*   **Global Config Directory**: `~/.fuchsia-agents/`
    *   `leases/`: Active lease files (`<env_id>.lease`).
    *   `environments/`: Parent directory for active isolated workspaces.
    *   `shared-prebuilts/`: Central CIPD cache with merged subdirectories.
*   **Build Directory Pool**: `~/fuchsia/out/fxenv/`
