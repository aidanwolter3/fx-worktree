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
You create worktrees using Jiri directly:
```bash
jiri worktree add .jiri_root/worktrees/<name>
```
Once created, you can navigate into the worktree and configure its build directories (e.g. `fx set fuchsia.x64`).

By default, newly created worktrees are **Reserved** (intended for local manual work).
To make a worktree available for automated agents to lease, you must explicitly mark it as **Free**.

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
List all Jiri-managed worktrees, highlighting their status (`Reserved`, `Free`, or `In Use`) and their build configurations.
```bash
fx-worktree list [--json]
```

*   **Default Output:**
    ```none
    ../../fuchsia/.jiri_root/worktrees/worktree1    Reserved
    ├── out/fuchsia.x64 (fuchsia.x64)
    └── out/fuchsia.arm64 (fuchsia.arm64)
    ../../fuchsia/.jiri_root/worktrees/worktree2    Free
    └── out/fuchsia.x64 (fuchsia.x64)
    ../../fuchsia/.jiri_root/worktrees/worktree3    In Use (agent-2f26359d)
    └── out/fuchsia.x64 (fuchsia.x64)
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
    remain in sync, subsequent builds in the same leased worktree can
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
