use anyhow::Result;
use clap::{CommandFactory, Parser};
use fxenv::cli::{Cli, Commands, OutdirAction, WorktreeAction};
use fxenv::config::Config;
use fxenv::{alloc, free, gc, list, outdir, selftest};

fn main() -> Result<()> {
    // Initialize logger
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Outdir { action } => {
            let config = Config::new(cli.fuchsia_dir)?;
            config.init_topology()?;
            match action {
                OutdirAction::Create { config: cfg } => {
                    let outdir_id = outdir::create_outdir(&config, &cfg)?;
                    log::info!("Outdir {} created successfully.", outdir_id);
                }
                OutdirAction::List => {
                    list::list_outdirs(&config)?;
                }
                OutdirAction::Delete { id } => {
                    outdir::delete_outdir(&config, &id)?;
                    log::info!("Outdir deleted successfully.");
                }
            }
        }
        Commands::Worktree { action } => {
            let config = Config::new(cli.fuchsia_dir)?;
            config.init_topology()?;
            match action {
                WorktreeAction::Create {
                    config: cfg,
                    agent_id,
                } => {
                    let worktree_info = alloc::allocate(&config, &cfg, &agent_id, None, None)?;
                    let json = serde_json::to_string(&worktree_info)?;
                    println!("{}", json);
                }
                WorktreeAction::Delete { id } => {
                    free::free_worktree_by_id(&config, &id)?;
                    log::info!("Worktree deleted successfully.");
                }
                WorktreeAction::List => {
                    list::list_worktrees(&config)?;
                }
                WorktreeAction::Gc { timeout } => {
                    gc::garbage_collect(&config, timeout)?;
                    log::info!("Garbage collection completed.");
                }
            }
        }
        Commands::SelfTest { use_outdir } => {
            let config = Config::new(cli.fuchsia_dir)?;
            config.init_topology()?;
            selftest::run_self_test(&config, use_outdir)?;
        }
        Commands::Completions { shell } => {
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "fxenv", &mut std::io::stdout());
        }
    }

    Ok(())
}
