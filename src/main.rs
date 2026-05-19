use anyhow::{Context, Result};
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
                    let agent_id = agent_id.unwrap_or_else(|| {
                        let uuid = uuid::Uuid::new_v4().to_string();
                        format!("agent-{}", &uuid[0..8])
                    });
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
            let mut buf = Vec::new();
            clap_complete::generate(shell, &mut cmd, "fxenv", &mut buf);
            let mut script = String::from_utf8(buf).context("Failed to parse generated completions as UTF-8")?;

            if shell == clap_complete::Shell::Zsh {
                script = script.replacen(
                    "':id -- Outdir ID (e.g. out_1234):_default'",
                    "':id -- Outdir ID (e.g. out_1234):_fxenv_outdir_ids'",
                    1,
                );
                script = script.replacen(
                    "':id -- Worktree ID:_default'",
                    "':id -- Worktree ID:_fxenv_worktree_ids'",
                    1,
                );

                // Move entry point block to the very end of the file
                let entry_point = r#"if [ "$funcstack[1]" = "_fxenv" ]; then
    _fxenv "$@"
else
    compdef _fxenv fxenv
fi"#;
                if let Some(idx) = script.find(entry_point) {
                    script.replace_range(idx..idx + entry_point.len(), "");

                    script.push_str("\n\n# Custom dynamic completion helpers\n");
                    script.push_str(r#"_fxenv_outdir_ids() {
    local -a ids
    ids=($(fxenv outdir list 2>/dev/null | grep -E '^\s+-' | awk '{print $2}'))
    _describe -t ids 'outdir ID' ids
}

_fxenv_worktree_ids() {
    local -a ids
    ids=($(fxenv worktree list 2>/dev/null | grep -E 'Worktree ID:' | awk '{print $3}'))
    _describe -t ids 'worktree ID' ids
}
"#);
                    script.push_str("\n");
                    script.push_str(entry_point);
                    script.push_str("\n");
                }
            }

            use std::io::Write;
            std::io::stdout().write_all(script.as_bytes()).context("Failed to write completions to stdout")?;
        }
    }

    Ok(())
}
