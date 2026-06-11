---
name: fx-worktree
description: >-
  Leases and manages Fuchsia worktrees from a pool of Jiri worktrees,
  supporting RBE and incremental builds.
tags: [fuchsia, workspace, build, worktree, lease]
support_tier: primary
category: infrastructure
version: 2.0.0
---

# `fx-worktree` (Fuchsia Worktree Manager)

`fx-worktree` is a CLI tool designed to manage parallel development worktrees inside a Fuchsia checkout. It allows multiple AI agents or developers to compile and test code concurrently without conflicting build directories or git states.

`fx-worktree` relies on native `jiri worktree` commands for creation and removal, and manages the pool lifecycle (marking as free, leasing, and releasing).

## When to Use

Use `fx-worktree` when:
*   You need to lease an isolated worktree to make changes and compile them.
*   You want to run builds in parallel without conflicting with the main checkout.
*   You want to reuse a shared pool of persistent worktrees to speed up compilation (preserving Ninja cache).
*   You want to verify your fx-worktree setup via the E2E test suite or benchmark utility.

## CLI Commands

### 1. Worktree Pool Management
You create and manage the physical directories of worktrees using `jiri` directly. Worktrees reside under `.jiri_root/worktrees/`.

*   **Mark a Worktree as Free**:
    ```bash
    fx-worktree mark-free <name>
    ```
    Marks a worktree as free (available for leasing by agents).

*   **Mark a Worktree as Reserved**:
    ```bash
    fx-worktree mark-reserved <name>
    ```
    Marks a free worktree as reserved (not available for leasing).

*   **List Worktrees**:
    ```bash
    fx-worktree list
    ```
    Lists all Jiri-managed worktrees, highlighting their status (`Reserved`, `Free`, or `In Use`) and their build configurations.

### 2. Lease & Lifecycle
*   **Lease a Worktree**:
    ```bash
    fx-worktree lease <name> [--agent-id <agent_name>] [--sync]
    # OR
    fx-worktree lease --any [--agent-id <agent_name>] [--sync]
    ```
    Leases a free worktree.
    *   `--sync`: Opt-in to update the worktree to the latest code in the main fuchsia checkout and update Jiri projects.
    *   `--agent-id`: Metadata to track which agent leased the worktree.

*   **Release a Worktree**:
    ```bash
    fx-worktree release <name>
    ```
    Resets the leased worktree and releases it back to the pool (marks it `Free` again).

*   **Change Directory**:
    ```bash
    fx-worktree cd [<name>]
    ```
    Changes directory to a worktree (shell wrapper required).

*   **Locate Worktree Path (Hidden Command)**:
    ```bash
    fx-worktree locate [<name>]
    ```
    Prints the absolute path of the worktree directory.

### 3. Verification & Benchmarking
*   **Run E2E Test Suite**:
    ```bash
    ./tests/e2e.sh
    ```
    Runs E2E tests in Mock Mode (no Fuchsia checkout needed) to verify the `fx-worktree` tool's behavior.

*   **Run Benchmark Script**:
    ```bash
    ./tests/benchmark.sh
    ```
    Measures the execution time of worktree creation, lease (with/without sync), release, and removal on a real Fuchsia checkout.

---

## Technical Design & Constraints

### 1. Worktree to Outdir 1:N Pairing
To achieve fast incremental builds, a Jiri worktree can house multiple dedicated build directories (outdirs), but different worktrees must never share the same build directory. This avoids path invalidation issues in GN/Ninja and ensures subsequent builds in the same worktree can complete in under 3 seconds. Configuration files (`args.gn`) are backed up at lease time and restored upon release.

### 2. State Isolation
Lease state is kept completely decentralized. The tool writes `lease.json` (locking metadata) and `last_active` inside the `.jiri_root/worktrees/` directory. There is no global config folder anymore.
