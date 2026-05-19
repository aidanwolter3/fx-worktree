# fxenv (Fuchsia Environment Manager)

`fxenv` is a stateless, concurrent-safe CLI tool designed to provision instantaneous, isolated development environments for parallel agents working on Fuchsia.

It pools persistent **Environments** (workspaces) on disk to preserve Ninja build timestamps and remote compiler caches, allowing sequential agents to reuse environments and achieve **no-op incremental build speeds (< 3 seconds)**, while isolating parallel runs.

---

## Installation

Compile and install the binary locally:
```bash
cargo install --path . --force
```

### Zsh Shell Integration (Required for `fxenv cd`)
Add the shell wrapper function to your `~/.zshrc` to support the directory navigation feature:

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

Set up Zsh completions (optional):
```bash
mkdir -p ~/.zsh/completion
fxenv completions zsh > ~/.zsh/completion/_fxenv
# Add ~/.zsh/completion to your fpath in ~/.zshrc before compinit
```

---

## Commands and Usage

### 1. Create a Pool Slot (Environment)
Creates and bootstraps a new persistent environment slot in the pool (runs `git worktree add`, `fx set`, and registers the slot).
```bash
fxenv create <config_name>
```

### 2. Allocate an Environment (Use)
Leases a free environment of the config type, cleans it, and updates its Git worktrees to the agent's target revisions.
```bash
fxenv use <config_name> [--agent-id <agent_name>] [--json]
```

*   **Default Output (Human Friendly):**
    ```none
    ✔ Workspace allocated successfully!

      ℹ Environment ID : fuchsia_internal.x64_d704c897-f2f2-4a6b-8a95-16d74d9788a5
      ℹ Agent ID       : agent-2f26359d
      ℹ Config         : fuchsia_internal.x64
      ℹ Path           : /home/user/.fuchsia-agents/environments/fuchsia_internal.x64_d704c897-f2f2-4a6b-8a95-16d74d9788a5

    To change directory into the workspace:
      $ fxenv cd fuchsia_internal.x64_d704c897-f2f2-4a6b-8a95-16d74d9788a5  # Navigate to this specific workspace
      $ fxenv cd                     # Navigate to the last created environment
    ```

*   **JSON Output (via `--json`):**
    ```json
    {"environment_id":"fuchsia_internal.x64_d704c897-f2f2-4a6b-8a95-16d74d9788a5","agent_id":"agent-2f26359d","config":"fuchsia_internal.x64","pid":2549294,"timestamp_sec":1779221652,"path":"/home/user/.fuchsia-agents/environments/fuchsia_internal.x64_d704c897-f2f2-4a6b-8a95-16d74d9788a5"}
    ```

### 3. List Environments
Shows all environments in the pool and their lease status.
```bash
fxenv list [--json]
```

*   **Default Output:**
    ```none
    CONFIG                 ENVIRONMENT ID                                     STATUS
    fuchsia.x64            fuchsia.x64_37954053-f927-45f1-9086-01d7b07c35bf   Free
    fuchsia_internal.x64   fuchsia_internal.x64_d704c897-f2f2-4a6b-8a95...    In Use (agent_1)
    ```

### 4. Free an Environment
Cleans the environment (resets git, runs `git clean` excluding the build cache in `out/`) and releases the lease.
```bash
fxenv free <environment_id> [--json]
```

### 5. Delete an Environment
Completely removes the environment from disk and unregisters the worktrees.
```bash
fxenv delete <environment_id>
```

### 6. Change Directory into Environment
Cds into the environment folder (resolves ID or short suffix, falls back to the last allocated environment if omitted).
```bash
fxenv cd [environment_id]
```
