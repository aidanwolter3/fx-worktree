# fx-worktree (Fuchsia Worktree Manager)

`fx-worktree` is a stateless, concurrent-safe CLI tool designed to provision
instantaneous, isolated development worktrees for parallel agents working on
Fuchsia.

`fx-worktree` relies on `jiri worktree` to manage the worktrees and prebuilts, while `fx-worktree` remains responsible for curating a multi-agent workflow.

![fx-worktree Demo](docs/demo.gif)

## Installation

Compile and install the binary locally:
```bash
cargo install --path . --force
```


## Usage

In order to achieve fast incremental builds, worktrees are kept around in a pool.
You create worktrees using the `add` subcommand:
```bash
fx-worktree add <name> [--set <config>]
```
*   `--set`: Auto-configure any number of build directories upon creation. This is a shortcut to running `fx set` inside the worktree (e.g. `fx-worktree add my-worktree --set fuchsia.x64 --set fuchsia.arm64`).

Once created, you can navigate into the worktree and configure additional build directories manually (e.g. `fx set fuchsia.x64`).

By default, newly created worktrees are **Free** (marked as available for automated agents to lease immediately).
To reserve a worktree for local manual work, you must explicitly mark it as **Reserved**.

### 1. Mark a Worktree as Free
Mark a reserved worktree as free so it can be leased.
```bash
fx-worktree mark-free <name>
```

### 2. Mark a Worktree as Reserved
Mark a free worktree as reserved so it will not be leased by automated agents.
```bash
fx-worktree mark-reserved <name>
```

### 3. Lease a Worktree
Lease a free worktree. You can lease a specific worktree by name, or use `--any` to lease the first available free worktree.
```bash
fx-worktree lease <name> [--agent-id <agent_name>] [--sync] [--json]
# OR
fx-worktree lease --any [--agent-id <agent_name>] [--sync] [--json]
```
*   `--sync`: Opt-in to update the worktree to the latest code in the main
    fuchsia checkout, clean it, and download/isolate prebuilts.
*   `--agent-id`: Optional metadata to track which agent leased the worktree.

*   **Default Output (Human Friendly):**
    ```none
    ✔ Worktree leased successfully!

      Worktree ID  : worktree2
      Path         : /usr/local/google/home/username/fuchsia/.jiri_root/worktrees/worktree2

    To change directory into the worktree:
      $ fx-worktree cd worktree2  # Navigate to this specific worktree
      $ fx-worktree cd            # Navigate to the last leased worktree
    ```

*   **JSON Output (via `--json`):**
    ```json
    {"worktree_id":"worktree2","agent_id":null,"pid":2549294,"timestamp_sec":1779221652,"path":"/usr/local/google/home/username/fuchsia/.jiri_root/worktrees/worktree2"}
    ```

### 4. List Worktrees
List all Jiri-managed worktrees, highlighting their status (`Reserved`, `Free`, or `In Use`), sync status (relative commit count to parent checkout), and build configurations (with last built timestamp).
```bash
fx-worktree list [--json]
```

*   **Default Output:**
    ```none
    worktree1 (Reserved, Synced)
    └── out/fuchsia.x64-balanced:     fuchsia.x64 (1h ago)

    worktree2 (Free, 131 behind, 4 new)
    ├── out/fuchsia.x64-balanced:     fuchsia.x64 (never built)
    └── out/fuchsia.arm64-balanced:   fuchsia.arm64 (never built)

    worktree3 (In Use (agent-2f26359d), 1 behind)
    └── out/fuchsia.x64-balanced:     fuchsia.x64 (never built)
    ```

### 5. Release a Worktree
Reset a leased worktree to the state before the lease and release it back to the pool (marks it `Free` again).
```bash
fx-worktree release <name> [--json]
```

### 6. Change Directory into Worktree
Change directory to a worktree (shell wrapper required).
```bash
fx-worktree cd [name]
```

### 7. Remove a Worktree
Safely remove a Jiri worktree.
```bash
fx-worktree remove <name> [--force]
```
*   `--force` / `-f`: Bypasses safety checks. Required to delete a worktree that is currently leased or contains uncommitted changes.

## Running Tests

To verify `fx-worktree` functionality, you can run the comprehensive E2E test suite.

Run the tests in **Mock Mode** (requires no real Fuchsia checkout, using mock repositories):
```bash
./tests/e2e.sh
```

Run the tests in **Real Mode** (against a real Fuchsia checkout, verifying real builds):
```bash
./tests/e2e.sh <path_to_fuchsia_dir> <config_name>
```

## Benchmarks

You can run the benchmark utility on a real Fuchsia directory to measure the execution times of various operations:
```bash
./tests/benchmark.sh
```

Typical execution times on a local workstation:
*   **`jiri worktree add`** (creation of worktree): `~19,500ms - 21,200ms`
*   **`fx-worktree lease (sync=true)`** (allocate worktree + sync with parent branch + isolate packages): `~7,500ms - 8,100ms`
*   **`fx-worktree release`** (reset worktree + revert GN build config): `~2,000ms - 6,600ms` (depending on cleanup overhead)
*   **`fx-worktree lease (sync=false)`** (instant lease without synchronizing code): `~3ms`
*   **`jiri worktree remove`** (cleanup and delete worktree): `~10,000ms - 10,400ms`

## Worktree to Outdir 1:N Pairing

A core design principle of `fx-worktree` is the strict isolation of build directories. A Jiri worktree can have **multiple dedicated build directories (outdirs)** (1:N pairing), but **multiple worktrees must never share the same build directory**.

### The Problem with Shared Build Directories
In Fuchsia, the build configuration (`args.gn`) and build artifacts are stored in the `outdir`. This configuration contains absolute paths referencing the source tree (the worktree).
*   **Path Invalidations**: If multiple worktrees shared a single `outdir`, switching between worktrees would constantly invalidate build paths, forcing GN to regenerate and Ninja to re-compile most of the codebase. This destroys the possibility of incremental builds.
*   **State Desynchronization**: Sharing an outdir between different git states leads to desynchronized build files and frequent full rebuilds.

### The 1:N Solution
`fx-worktree` allows a single worktree to contain multiple build configurations (e.g. `out/fuchsia.x64` and `out/fuchsia.arm64`) as long as they are dedicated to that worktree.
*   **Isolation**: Each outdir remains locally isolated inside its parent worktree.
*   **Configuration Backups**: `fx-worktree` automatically backs up the build configuration (`args.gn`) of all outdirs when a lease is acquired, and restores them at release time, preventing configuration drift.
*   **No-Op Incremental Builds**: Because the build directory's artifacts remain in sync with the worktree's source state, subsequent builds in the same leased worktree can determine that nothing has changed and complete in **less than 3 seconds**.

### Zsh Completions

`fx-worktree` supports rich, dynamic shell completions for Zsh.
To enable these completions:

1.  **Generate the completion script**:
    Create a dedicated completion directory and dump the generated Zsh completion script into it:
    ```zsh
    mkdir -p ~/.zsh/completion
    fx-worktree completions zsh > ~/.zsh/completion/_fx-worktree
    ```

2.  **Configure your `~/.zshrc`**:
    Add the completion directory to your `fpath` **before** the completion system (`compinit`) is initialized.

    Add the following lines to your `~/.zshrc`:
    ```zsh
    # Enable fx-worktree completions
    fpath=(~/.zsh/completion $fpath)

    # Initialize completions (if not already done)
    autoload -Uz compinit
    compinit
    ```

    *Note: If you already have `compinit` in your `~/.zshrc`, ensure the `fpath` line is placed above it.*

    For dynamic completions to work, the `fx-worktree` binary must be available in your `PATH` (typically `~/.cargo/bin` if installed via `cargo install`).

## Zsh Shell Integration (Required for `fx-worktree cd`)

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
