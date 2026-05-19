# fxenv (Fuchsia Environment Manager)

`fxenv` is a stateless, concurrent-safe CLI tool designed to provision instantaneous, isolated development workspaces for parallel agents working on Fuchsia.

It leverages `git worktree` for source isolation, shares read-only prebuilts via symlinking, and pools RBE (Remote Build Execution) enabled build directories to maximize incremental compilation speed across workspaces.

---

## Installation

Compile and install the binary locally:
```bash
cargo install --path . --force
```

### Zsh Shell Integration (Required for `fxenv cd`)
Since a compiled binary runs in a child process and cannot change the working directory of your active shell, you must add a shell wrapper function to your `~/.zshrc`:

```zsh
# fxenv shell wrapper for cd command
fxenv() {
    if [[ "$1" == "cd" ]]; then
        local target_path
        target_path=$(command fxenv locate "$2")
        if [[ $? -eq 0 && -n "$target_path" ]]; then
            cd "$target_path"
        else
            return 1
        fi
    else
        command fxenv "$@"
    fi
}
```

---

## Commands and Usage Examples

### 1. Allocate a Workspace
Leases a build directory from the pool and sets up an isolated Git worktree for parallel development.
```bash
fxenv worktree create <config_name> [--agent-id <agent_name>]
```

*   **Default Output (Human Friendly):**
    ```none
    ✔ Workspace allocated successfully!

      ℹ Worktree ID : fuchsia_internal.x64_out_5cca349c-ba43-4b03-bda6-736d5184e397
      ℹ Agent ID    : agent-acf27225
      ℹ Config      : fuchsia_internal.x64
      ℹ Workspace   : /home/user/.fuchsia-agents/workspaces/fuchsia_internal.x64_out_5cca349c-ba43-4b03-bda6-736d5184e397
      ℹ Outdir      : /home/user/fuchsia/out/fxenv/fuchsia_internal.x64/out_5cca349c-ba43-4b03-bda6-736d5184e397

    To change directory into the workspace:
      $ fxenv cd fuchsia_internal.x64_out_5cca349c-ba43-4b03-bda6-736d5184e397
    ```

*   **JSON Output (via `--json`):**
    ```json
    {"worktree_id":"fuchsia_internal.x64_out_5cca349c-ba43-4b03-bda6-736d5184e397","agent_id":"agent-acf27225","config":"fuchsia_internal.x64","pid":2549294,"timestamp_sec":1779221652,"workspace_path":"/home/user/.fuchsia-agents/workspaces/fuchsia_internal.x64_out_5cca349c-ba43-4b03-bda6-736d5184e397","outdir_path":"/home/user/fuchsia/out/fxenv/fuchsia_internal.x64/out_5cca349c-ba43-4b03-bda6-736d5184e397"}
    ```

### 2. List Active Workspaces
Shows currently leased environments.
```bash
fxenv worktree list [--json]
```

*   **Default Output:**
    ```none
    ■ fuchsia_internal.x64_out_5cca349c-ba43-4b03-bda6-736d5184e397
      Agent     : agent-acf27225
      Created   : 5m 12s ago
      Workspace : /home/user/.fuchsia-agents/workspaces/fuchsia_internal.x64_out_5cca349c-ba43-4b03-bda6-736d5184e397
      Outdir    : /home/user/fuchsia/out/fxenv/fuchsia_internal.x64/out_5cca349c-ba43-4b03-bda6-736d5184e397
    ```

### 3. Change Directory to Workspace
Uses the Zsh shell wrapper to navigate directly to the workspace root. If no ID is passed, it navigates to the **last created** outdir or workspace.
```bash
fxenv cd [worktree_id | outdir_id]
```

### 4. Free a Workspace
Cleans up the worktree and safely moves the build directory back to the pool, preserving the build cache.
```bash
fxenv worktree delete <worktree_id> [--json]
```

*   **Default Output:**
    ```none
    ✔ Workspace fuchsia_internal.x64_out_5cca349c-ba43-4b03-bda6-736d5184e397 successfully freed and outdir restored to pool.
    ```

### 5. List Outdir Pool Status
Shows build directory caches in the pool and their availability.
```bash
fxenv outdir list [--json]
```

*   **Default Output:**
    ```none
    CONFIG                 OUTDIR ID                              STATUS
    fuchsia.x64            out_e507270d-7129-4475-935e-f2d4127    Free
    fuchsia_internal.x64   out_5cca349c-ba43-4b03-bda6-736d518    In Use (agent-acf27225)
    ```
