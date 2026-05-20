# fx-worktree (Fuchsia Worktree Manager)

`fx-worktree` is a stateless, concurrent-safe CLI tool designed to provision instantaneous, isolated development environments for parallel agents working on Fuchsia.

It pools persistent **Worktrees** (workspaces) on disk to preserve Ninja build timestamps and remote compiler caches, allowing sequential agents to reuse environments and achieve **no-op incremental build speeds (< 3 seconds)**, while isolating parallel runs.

---

## Installation

Compile and install the binary locally:
```bash
cargo install --path . --force
```

### Zsh Shell Integration (Required for `fx-worktree cd`)
Add the shell wrapper function to your `~/.zshrc` to support the directory navigation feature:

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
*   `--sync`: Opt-in to update the worktree to the latest code in the main fuchsia checkout, clean it, and download/isolate prebuilts.

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
Runs a programmatic verification of the `fx-worktree` lifecycle (leasing, build regeneration, cache preservation, and cleanup) using an existing worktree.
```bash
fx-worktree self-test <worktree_id>
```
> [!IMPORTANT]
> The target worktree must be "Free" (not leased by any agent) and should ideally be "warmed" (built at least once) to run the test quickly.
