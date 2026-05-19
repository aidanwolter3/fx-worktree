use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "fxenv", version, about = "Fuchsia Agent Environment Manager", disable_help_flag = true)]
pub struct Cli {
    /// Path to the main Fuchsia checkout (defaults to $FUCHSIA_DIR)
    #[arg(long, global = true, env = "FUCHSIA_DIR")]
    pub fuchsia_dir: Option<PathBuf>,

    /// Print help
    #[arg(long, global = true, action = clap::ArgAction::Help)]
    pub help: Option<bool>,

    /// Output structured JSON instead of human-readable text
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
#[command(disable_help_flag = true)]
pub enum Commands {
    /// Manage warm outdirs
    Outdir {
        #[command(subcommand)]
        action: OutdirAction,
    },
    /// Manage isolated worktrees (workspaces)
    Worktree {
        #[command(subcommand)]
        action: WorktreeAction,
    },
    /// Run a self-test to verify fxenv functionality against a Fuchsia checkout.
    ///
    /// This will temporarily create a build directory under out/fxenv and a git worktree,
    /// which will be cleaned up upon completion.
    SelfTest {
        /// Use an existing outdir ID instead of creating a new one.
        ///
        /// The target must have been already built in this outdir.
        /// The outdir will not be deleted, and its build cache will be restored at the end.
        #[arg(long)]
        use_outdir: Option<String>,
    },
    /// Generate shell completion scripts to stdout
    Completions {
        /// The shell to generate completions for
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// Change directory to an outdir or worktree (shell wrapper required)
    Cd {
        /// Outdir or Worktree ID
        id: Option<String>,
    },
    /// Locate the path of an outdir or worktree
    Locate {
        /// Outdir or Worktree ID (optional, resolves last created if omitted)
        id: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
#[command(disable_help_flag = true)]
pub enum OutdirAction {
    /// Create a new warm outdir for a config
    Create {
        /// Configuration name (e.g. fuchsia.x64)
        config: String,
    },
    /// List all outdirs and their status
    List,
    /// Delete an idle outdir
    Delete {
        /// Outdir ID (e.g. out_1234)
        id: String,
    },
}

#[derive(Subcommand, Debug)]
#[command(disable_help_flag = true)]
pub enum WorktreeAction {
    /// Create (allocate) an isolated worktree
    Create {
        /// Configuration name (e.g. fuchsia.x64)
        config: String,
        /// Optional agent ID (will be randomly generated if omitted)
        #[arg(long)]
        agent_id: Option<String>,
    },
    /// Delete (free) a worktree
    Delete {
        /// Worktree ID
        id: String,
    },
    /// List active worktrees
    List,
    /// Garbage collect orphaned worktrees
    Gc {
        /// Timeout in seconds (default: 14400 / 4 hours)
        #[arg(long, default_value_t = 14400)]
        timeout: u64,
    },
}
