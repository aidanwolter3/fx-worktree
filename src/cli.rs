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
    /// Create a new persistent environment in the pool
    Create {
        /// Configuration name (e.g. fuchsia.x64)
        config: String,
    },
    /// Delete a persistent environment from disk
    Delete {
        /// Environment ID to delete (must be free)
        id: String,
    },
    /// List all environments in the pool and their lease status
    List,
    /// Use (allocate) a free environment from the pool
    Use {
        /// Configuration name (e.g. fuchsia.x64)
        config: String,
        /// Optional agent ID (will be randomly generated if omitted)
        #[arg(long)]
        agent_id: Option<String>,
    },
    /// Free (release) an environment back to the pool
    Free {
        /// Environment ID to free (must be leased)
        id: String,
    },
    /// Change directory to an environment (shell wrapper required)
    Cd {
        /// Environment ID
        id: Option<String>,
    },
    /// Locate the path of an environment
    Locate {
        /// Environment ID
        id: Option<String>,
    },
    /// Run a self-test to verify fxenv functionality
    SelfTest {
        /// Use an existing environment ID instead of creating a new one
        #[arg(long)]
        use_env: Option<String>,
    },
    /// Clean up orphaned or expired leases in the pool
    Gc {
        /// Lease expiry threshold in seconds (defaults to 0: cleans all dead/orphaned leases)
        #[arg(long, default_value = "0")]
        timeout: u64,
    },
    /// Generate shell completion scripts to stdout
    Completions {
        /// The shell to generate completions for
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}
