use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "fx-worktree", version, about = "Fuchsia Worktree Manager")]
pub struct Cli {
    /// Path to the main Fuchsia checkout (defaults to $FUCHSIA_DIR)
    #[arg(long, global = true, env = "FUCHSIA_DIR")]
    pub fuchsia_dir: Option<PathBuf>,

    /// Print full help including internal commands
    #[arg(long)]
    pub helpfull: bool,

    /// Output structured JSON instead of human-readable text
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// List worktrees
    List,
    /// Add a new Jiri worktree
    Add {
        /// Name of the new worktree
        name: String,
        /// Auto-configure build directories (e.g. --set fuchsia.x64)
        #[arg(long, value_name = "CONFIG", action = clap::ArgAction::Append)]
        set: Vec<String>,
    },
    /// Remove a Jiri worktree
    Remove {
        /// Name of the worktree to remove
        name: String,
        /// Force removal even if the worktree is leased or has uncommitted changes
        #[arg(long, short)]
        force: bool,
    },
    /// Lease a worktree to start work
    Lease {
        /// Worktree name to lease
        #[arg(required_unless_present = "any")]
        name: Option<String>,
        /// Lease any free worktree
        #[arg(long, conflicts_with = "name")]
        any: bool,
        /// Optional agent ID
        #[arg(long)]
        agent_id: Option<String>,
        /// Sync the worktree to the latest code
        #[arg(long)]
        sync: bool,
        /// Base branch to branch off from (defaults to JIRI_HEAD)
        #[arg(long)]
        base_branch: Option<String>,
        /// Print only the path of the leased worktree
        #[arg(long, short)]
        print_path_only: bool,
    },
    /// Release and reset a worktree to the state before the lease
    Release {
        /// Worktree name to release (must be leased)
        name: String,
    },
    /// Change directory to a worktree (shell wrapper required)
    Cd {
        /// Worktree name
        name: Option<String>,
    },
    /// Mark a worktree as free (available for leasing)
    #[command(name = "mark-free")]
    MarkFree {
        /// Worktree name to mark free
        name: String,
    },
    /// Mark a worktree as reserved (not available for leasing)
    #[command(name = "mark-reserved")]
    MarkReserved {
        /// Worktree name to mark reserved
        name: String,
    },
    /// Locate the path of a worktree
    #[command(hide = true)]
    Locate {
        /// Worktree name
        name: Option<String>,
    },
    /// Generate shell completion scripts to stdout
    #[command(hide = true)]
    Completions {
        /// The shell to generate completions for
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_cli_parse_list() {
        let args = vec!["fx-worktree", "list"];
        let cli = Cli::try_parse_from(args).unwrap();
        assert!(matches!(cli.command, Some(Commands::List)));
    }

    #[test]
    fn test_cli_parse_lease_with_name() {
        let args = vec![
            "fx-worktree",
            "lease",
            "mywt",
            "--sync",
            "--agent-id",
            "myagent",
        ];
        let cli = Cli::try_parse_from(args).unwrap();
        match cli.command {
            Some(Commands::Lease {
                name,
                any,
                agent_id,
                sync,
                base_branch,
                print_path_only,
            }) => {
                assert_eq!(name, Some("mywt".to_string()));
                assert!(!any);
                assert_eq!(agent_id, Some("myagent".to_string()));
                assert!(sync);
                assert_eq!(base_branch, None);
                assert!(!print_path_only);
            }
            _ => panic!("Expected Lease command"),
        }
    }

    #[test]
    fn test_cli_parse_lease_any() {
        let args = vec!["fx-worktree", "lease", "--any"];
        let cli = Cli::try_parse_from(args).unwrap();
        match cli.command {
            Some(Commands::Lease { name, any, .. }) => {
                assert_eq!(name, None);
                assert!(any);
            }
            _ => panic!("Expected Lease command"),
        }
    }

    #[test]
    fn test_cli_parse_lease_missing_name_and_any() {
        let args = vec!["fx-worktree", "lease"];
        let res = Cli::try_parse_from(args);
        assert!(res.is_err());
    }

    #[test]
    fn test_cli_parse_lease_conflicting_args() {
        let args = vec!["fx-worktree", "lease", "mywt", "--any"];
        let res = Cli::try_parse_from(args);
        assert!(res.is_err());
    }

    #[test]
    fn test_cli_parse_release() {
        let args = vec!["fx-worktree", "release", "mywt"];
        let cli = Cli::try_parse_from(args).unwrap();
        match cli.command {
            Some(Commands::Release { name }) => {
                assert_eq!(name, "mywt".to_string());
            }
            _ => panic!("Expected Release command"),
        }
    }

    #[test]
    fn test_cli_parse_add() {
        let args = vec!["fx-worktree", "add", "new-wt"];
        let cli = Cli::try_parse_from(args).unwrap();
        match cli.command {
            Some(Commands::Add { name, set }) => {
                assert_eq!(name, "new-wt".to_string());
                assert!(set.is_empty());
            }
            _ => panic!("Expected Add command"),
        }

        let args = vec![
            "fx-worktree",
            "add",
            "new-wt",
            "--set",
            "fuchsia.x64",
            "--set",
            "fuchsia.arm64",
        ];
        let cli = Cli::try_parse_from(args).unwrap();
        match cli.command {
            Some(Commands::Add { name, set }) => {
                assert_eq!(name, "new-wt".to_string());
                assert_eq!(
                    set,
                    vec!["fuchsia.x64".to_string(), "fuchsia.arm64".to_string()]
                );
            }
            _ => panic!("Expected Add command"),
        }
    }

    #[test]
    fn test_cli_parse_remove() {
        let args = vec!["fx-worktree", "remove", "wt-to-del"];
        let cli = Cli::try_parse_from(args).unwrap();
        match cli.command {
            Some(Commands::Remove { name, force }) => {
                assert_eq!(name, "wt-to-del".to_string());
                assert!(!force);
            }
            _ => panic!("Expected Remove command"),
        }

        let args_forced = vec!["fx-worktree", "remove", "wt-to-del", "--force"];
        let cli_forced = Cli::try_parse_from(args_forced).unwrap();
        match cli_forced.command {
            Some(Commands::Remove { name, force }) => {
                assert_eq!(name, "wt-to-del".to_string());
                assert!(force);
            }
            _ => panic!("Expected Remove command"),
        }
    }
}
