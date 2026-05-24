# fx-worktree (Fuchsia Worktree Manager)

`fx-worktree` is a stateless, concurrent-safe CLI tool designed to provision
instantaneous, isolated development environments for parallel agents working on
Fuchsia.

`fx-worktree` relies on `jiri worktree` to manage the worktrees and prebuilts, while `fx-worktree` remains responsible for curating a multi-agent workflow.

![fx-worktree Demo](docs/demo.gif)

## Installation

Compile and install the binary locally:
```bash
cargo install --path . --force
```


## Usage

In order to achieve fast incremental builds, worktrees are created and kept around in a pool.
Agents will `lease` a worktree from the pool, complete their work, then `release` it back to the pool.

### 1. Add a Worktree
Add a new worktree with a dedicated outdir to the pool.
```bash
fx-worktree add <config_name>
```

### 2. Lease a Worktree
Lease a worktree from the pool to start work.
```bash
fx-worktree lease <config_name> [--agent-id <agent_name>] [--sync] [--json]
```
*   `--sync`: Opt-in to update the worktree to the latest code in the main
    fuchsia checkout, clean it, and download/isolate prebuilts.

*   **Default Output (Human Friendly):**
    ```none
    ✔ Worktree leased successfully!

      Worktree ID  : fuchsia.x64-d704c897
      Agent ID     : agent-2f26359d
      Config       : fuchsia.x64
      Path         : /usr/local/google/home/username/fuchsia/.jiri_root/worktrees/fuchsia.x64-d704c897

    To change directory into the worktree:
      $ fx-worktree cd fuchsia.x64-d704c897  # Navigate to this specific worktree
      $ fx-worktree cd                     # Navigate to the last leased worktree
    ```

*   **JSON Output (via `--json`):**
    ```json
    {"environment_id":"fuchsia.x64-d704c897","agent_id":"agent-2f26359d","config":"fuchsia.x64","pid":2549294,"timestamp_sec":1779221652,"path":"/usr/local/google/home/username/fuchsia/.jiri_root/worktrees/fuchsia.x64-d704c897"}
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
    CONFIG        WORKTREE ID            STATUS
    fuchsia.x64   fuchsia.x64-37954053   Free
    fuchsia.x64   fuchsia.x64-d704c897   In Use (agent-2f26359d)
    ```

### 5. Release a Worktree
Reset a worktree (git reset) and release it back to the pool.
```bash
fx-worktree release <worktree_id> [--json]
```

### 6. Remove a Worktree
Remove a worktree from the pool and its dedicated outdir.
```bash
fx-worktree remove <worktree_id>
```

### 7. Change Directory into Worktree
Change directory to a worktree (shell wrapper required).
```bash
fx-worktree cd [worktree_id]
```

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

## Worktree to Outdir 1:1 Pairing

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
