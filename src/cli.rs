use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "fenv", version, about = "Fuchsia Agent Environment Manager")]
pub struct Cli {
    /// Path to the main Fuchsia checkout (defaults to $FUCHSIA_DIR)
    #[arg(long, global = true, env = "FUCHSIA_DIR")]
    pub fuchsia_dir: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
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
    /// Run a self-test to verify fenv functionality against a Fuchsia checkout.
    ///
    /// This will temporarily create a build directory under out/fenv and a git worktree,
    /// which will be cleaned up upon completion.
    SelfTest {
        /// Use an existing outdir ID instead of creating a new one.
        ///
        /// The target must have been already built in this outdir.
        /// The outdir will not be deleted, and its build cache will be restored at the end.
        #[arg(long)]
        use_outdir: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum OutdirAction {
    /// Create a new warm outdir for a config
    Create {
        #[arg(long)]
        config: String,
        /// Extra arguments to pass to `fx set` (e.g. --release)
        #[arg(long, num_args = 1.., allow_hyphen_values = true)]
        fx_args: Vec<String>,
    },
    /// List all outdirs and their status
    List,
    /// Delete an idle outdir
    Delete {
        #[arg(long)]
        config: String,
        #[arg(long)]
        id: String, // e.g. "out_1234"
    },
}

#[derive(Subcommand, Debug)]
pub enum WorktreeAction {
    /// Create (allocate) an isolated worktree
    Create {
        #[arg(long)]
        config: String,
        #[arg(long)]
        agent_id: String,
    },
    /// Delete (free) a worktree
    Delete {
        #[arg(long)]
        id: String, // worktree_id, formerly lease_id
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
