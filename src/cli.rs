use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "fx-worktree",
    version,
    about = "Fuchsia Worktree Manager",
    disable_help_flag = true
)]
pub struct Cli {
    /// Path to the main Fuchsia checkout (defaults to $FUCHSIA_DIR)
    #[arg(long, global = true, env = "FUCHSIA_DIR")]
    pub fuchsia_dir: Option<PathBuf>,

    /// Print help
    #[arg(short, long, global = true)]
    pub help: bool,

    /// Print full help including internal commands
    #[arg(long, global = true)]
    pub helpfull: bool,

    /// Output structured JSON instead of human-readable text
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
#[command(disable_help_flag = true)]
pub enum Commands {
    /// Add a new worktree with a dedicated outdir
    Add {
        /// Configuration name (e.g. fuchsia.x64)
        config: String,
    },
    /// Remove a worktree and its dedicated outdir
    Remove {
        /// Worktree ID to remove (must be free)
        id: Option<String>,
        /// Force removal even if the worktree is in an inconsistent state
        #[arg(long, short)]
        force: bool,
    },
    /// List worktrees
    List,
    /// Lease a worktree to start work
    Lease {
        /// Configuration name (e.g. fuchsia.x64)
        config: String,
        /// Optional agent ID (will be randomly generated if omitted)
        #[arg(long)]
        agent_id: Option<String>,
        /// Sync the worktree to the latest code
        #[arg(long)]
        sync: bool,
        /// Print only the path of the leased worktree
        #[arg(long, short)]
        print_path_only: bool,
    },
    /// Update a worktree to the latest code in the main fuchsia checkout
    Sync {
        /// Worktree ID to sync
        id: String,
        /// Force sync even if HEAD has not changed
        #[arg(long, short)]
        force: bool,
    },
    /// Release and reset a worktree (does a git reset)
    Release {
        /// Worktree ID to release (must be leased)
        id: String,
    },
    /// Change directory to a worktree (shell wrapper required)
    Cd {
        /// Worktree ID
        id: Option<String>,
    },
    /// Locate the path of a worktree
    #[command(hide = true)]
    Locate {
        /// Worktree ID
        id: Option<String>,
    },
    /// Run a self-test to verify fx-worktree functionality using an existing worktree
    #[command(hide = true)]
    SelfTest {
        /// Worktree ID to use for the test
        id: String,
    },
    /// Generate shell completion scripts to stdout
    #[command(hide = true)]
    Completions {
        /// The shell to generate completions for
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}
