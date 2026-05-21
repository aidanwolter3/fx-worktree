# fx-worktree (Fuchsia Worktree Manager)

`fx-worktree` is a stateless, concurrent-safe CLI tool designed to provision
instantaneous, isolated development environments for parallel agents working on
Fuchsia.

![fx-worktree Demo](docs/demo.gif)

---

## Why fx-worktree?

Fuchsia is a massive codebase. Standard development workflows often suffer
from:
*   **State corruption**: Sharing a single build directory between parallel
    tasks or agents leads to clobbered builds and race conditions.
*   **Resource waste**: Recreating environments from scratch for every task is
    itself inefficient.

`fx-worktree` solves these problems by pooling persistent **Worktrees** (git
worktrees) and pairing them 1:1 with dedicated Fuchsia **outdirs** (build
output directories). This ensures:
*   **Isolation**: Parallel agents work in completely separate environments,
    preventing state leakage.
*   **Instantaneous setups**: Leasing a pre-warmed workspace takes seconds.
*   **Extreme speed**: Reusing existing workspaces preserves Ninja build
    timestamps and remote compiler caches, enabling **no-op incremental
    builds in under 3 seconds**.

---

## Technical Rationale: 1:1 Worktree to Outdir Pairing

A core design principle of `fx-worktree` is the strict **1:1 pairing between
a Git Worktree and a Fuchsia Output Directory (`outdir`)**.

### The Problem with Shared Build Directories
In Fuchsia, the build configuration (`args.gn`) and build artifacts are stored
in the `outdir`. This configuration contains absolute paths referencing the
source tree (the worktree).
*   **Path Invalidations**: If multiple worktrees shared a single `outdir`,
    switching between worktrees would constantly invalidate build paths,
    forcing GN to regenerate and Ninja to re-compile most of the codebase.
    This destroys the possibility of incremental builds.
*   **State Desynchronization**: If a single worktree tried to use multiple
    `outdirs` dynamically, the build state would become desynchronized,
    leading to unexpected rebuilds or build failures.

### The 1:1 Solution
`fx-worktree` manages this complexity by ensuring that when a worktree is
created, a dedicated `outdir` is provisioned alongside it.
*   **Persistent Association**: The worktree is permanently paired with its
    dedicated `outdir`.
*   **No-Op Incremental Builds**: Because the source files and build artifacts
    remain in sync, subsequent builds in the same leased environment can
    determine that nothing has changed and complete in **less than 3
    seconds**.

---

## Installation

Compile and install the binary locally:
```bash
cargo install --path . --force
```

### Zsh Shell Integration (Required for `fx-worktree cd`)
Add the shell wrapper function to your `~/.zshrc` to support the directory
navigation feature:

```zsh
# fx-worktree shell wrapper for cd command
fx-worktree() {
    if [[ "$1" == "cd" ]]; then
        local target_path
        target_path=$(command fx-worktree locate "$2")
        if [[ $? -eq 0 && -n "$target_path" ]]; then
            cd "$target_path"
        else
            return 1
        fi
    else
        command fx-worktree "$@"
    fi
}
```

Set up Zsh completions (optional):
```bash
mkdir -p ~/.zsh/completion
fx-worktree completions zsh > ~/.zsh/completion/_fx-worktree
# Add ~/.zsh/completion to your fpath in ~/.zshrc before compinit
```

---

## Commands and Usage

### 1. Add a Worktree
Add a new worktree with a dedicated outdir.
```bash
fx-worktree add <config_name>
```

### 2. Lease a Worktree
Lease a worktree to start work.
```bash
fx-worktree lease <config_name> [--agent-id <agent_name>] [--sync] [--json]
```
*   `--sync`: Opt-in to update the worktree to the latest code in the main
    fuchsia checkout, clean it, and download/isolate prebuilts.

*   **Default Output (Human Friendly):**
    ```none
    ✔ Worktree leased successfully!

      ℹ Worktree ID    : fuchsia_internal.x64_d704c897-f2f2-4a6b-8a95-16d74d9788a5
      ℹ Agent ID       : agent-2f26359d
      ℹ Config         : fuchsia_internal.x64
      ℹ Path           : /home/user/.fuchsia/worktrees/environments/fuchsia_internal.x64_d704c897-f2f2-4a6b-8a95-16d74d9788a5

    To change directory into the worktree:
      $ fx-worktree cd fuchsia_internal.x64_d704c897-f2f2-4a6b-8a95-16d74d9788a5  # Navigate to this specific worktree
      $ fx-worktree cd                     # Navigate to the last leased worktree
    ```

*   **JSON Output (via `--json`):**
    ```json
    {"environment_id":"fuchsia_internal.x64_d704c897-f2f2-4a6b-8a95-16d74d9788a5","agent_id":"agent-2f26359d","config":"fuchsia_internal.x64","pid":2549294,"timestamp_sec":1779221652,"path":"/home/user/.fuchsia/worktrees/environments/fuchsia_internal.x64_d704c897-f2f2-4a6b-8a95-16d74d9788a5"}
    ```

### 3. Update a Worktree (Sync)
Update a worktree to the latest code in the main fuchsia checkout.
```bash
fx-worktree sync <worktree_id>
```

### 4. List Worktrees
List worktrees.
```bash
fx-worktree list [--json]
```

*   **Default Output:**
    ```none
    CONFIG                 WORKTREE ID                                        STATUS
    fuchsia.x64            fuchsia.x64_37954053-f927-45f1-9086-01d7b07c35bf   Free
    fuchsia_internal.x64   fuchsia_internal.x64_d704c897-f2f2-4a6b-8a95...    In Use (agent_1)
    ```

### 5. Release a Worktree
Release and reset a worktree (does a git reset).
```bash
fx-worktree release <worktree_id> [--json]
```

### 6. Remove a Worktree
Remove a worktree and its dedicated outdir.
```bash
fx-worktree remove <worktree_id>
```

### 7. Change Directory into Worktree
Change directory to a worktree (shell wrapper required).
```bash
fx-worktree cd [worktree_id]
```

### 8. Run Self-Test
Runs a programmatic verification of the `fx-worktree` lifecycle (leasing,
build regeneration, cache preservation, and cleanup) using an existing
worktree.
```bash
fx-worktree self-test <worktree_id>
```
> [!IMPORTANT]
> The target worktree must be "Free" (not leased by any agent) and should
> ideally be "warmed" (built at least once) to run the test quickly.
