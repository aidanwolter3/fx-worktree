---
name: fxenv
description: >-
  Manages Fuchsia build directories (outdirs) and isolated git worktrees (workspaces)
  for parallel agent executions, supporting RBE and cache reuse.
tags: [fuchsia, workspace, build, RBE, cache, worktree]
support_tier: primary
category: infrastructure
version: 1.0.0
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

### 1. Build Directory (Outdir) Management

Manages the shared pool of build directories. These directories reside physically under `~/fuchsia/out/fxenv/<config_name>/out_<uuid>`.

*   **Create an Outdir**:
    ```bash
    fxenv outdir create --config <config_name>
    ```
    Creates a new build configuration inside the pool (runs `fx set`). E.g.:
    ```bash
    fxenv outdir create --config fuchsia.x64
    ```

*   **List Outdirs**:
    ```bash
    fxenv outdir list
    ```
    Lists all outdirs in the pool and whether they are currently "In Use" (leased) or "Free".

*   **Delete an Outdir**:
    ```bash
    fxenv outdir delete --id <outdir_id>
    ```
    Deletes the specified outdir from the pool (cannot delete if currently leased).

### 2. Workspace (Worktree) Management

Allocates and frees isolated development environments.

*   **Allocate a Workspace**:
    ```bash
    fxenv worktree create --config <config_name> --agent-id <agent_name>
    ```
    Leases a free outdir from the pool and creates an isolated git worktree mapped to it. 
    Returns a JSON string containing the workspace details:
    ```json
    {
      "worktree_id": "fuchsia.x64_out_uuid",
      "workspace_path": "/usr/local/google/home/awolter/.fuchsia-agents/workspaces/fuchsia.x64_out_uuid",
      "outdir_path": "/usr/local/google/home/awolter/fuchsia/out/fxenv/fuchsia.x64/out_uuid",
      "agent_id": "my_agent",
      "config": "fuchsia.x64",
      "pid": 12345,
      "timestamp_sec": 1779213509
    }
    ```

*   **Free a Workspace**:
    ```bash
    fxenv worktree delete --id <worktree_id>
    ```
    Cleans up the git worktree and **moves the build directory back to the pool**, preserving the build cache.

*   **List Workspaces**:
    ```bash
    fxenv worktree list
    ```
    Lists all currently active workspaces and their leases.

*   **Garbage Collection**:
    ```bash
    fxenv worktree gc [--timeout <seconds>]
    ```
    Finds and cleans up orphaned workspaces where the owning process (recorded PID) has died, or the lease has expired (default timeout: 2 hours).

### 3. Self-Test Verification

Runs a full programmatic verification of the `fxenv` lifecycle (warming, allocation, incremental compiles, cache preservation, and cleanup).

*   **Run Automated Self-Test**:
    ```bash
    fxenv self-test
    ```

*   **Reuse Existing Build Cache**:
    ```bash
    fxenv self-test --use-outdir <id|path>
    ```
    Runs the self-test reusing an existing build directory (either by ID inside the pool, or an absolute path to a standard build directory like `~/fuchsia/out/fuchsia_internal.arm64-balanced`). This skips the slow warming build phase and restores both the build cache (`.o` files) and `build.ninja` configuration at the end.

### 4. Shell Completions

*   **Generate Completions**:
    ```bash
    fxenv completions <bash|elvish|fish|powershell|zsh>
    ```
    Prints the shell completion script to stdout. E.g. for Zsh:
    ```bash
    fxenv completions zsh > ~/.zsh/completion/_fxenv
    ```

## Technical Design & Constraints

### 1. RBE (Remote Build Execution) Support
Fuchsia's RBE compiler wrappers require the build directory to be a subdirectory of the execution root (the workspace). 
To support RBE:
*   **During Allocation**: `fxenv` physically **moves (renames)** the leased outdir into the workspace (`workspace_path/out/default`). Relative symlinks inside `gen/` resolve correctly, and RBE builds succeed.
*   **During Free**: The outdir is moved back to the pool (`out/fxenv/`).
*   **Performance**: Since the workspaces (defaults to `~/.fuchsia-agents/workspaces`) and the Fuchsia checkout are typically on the same filesystem, this move is instantaneous.

### 2. Topology
*   **Global Config Directory**: `~/.fuchsia-agents/`
    *   `leases/`: Active lease files (`<config>_<uuid>.lease`).
    *   `workspaces/`: Parent directory for active git worktrees.
*   **Build Directory Pool**: `~/fuchsia/out/fxenv/`
