use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};
use fx_worktree::cli::{Cli, Commands};
use fx_worktree::config::Config;
use fx_worktree::{add, lease, list, locate, release, remove, selftest, sync};

fn main() -> Result<()> {
    // Initialize logger (default to warn to silence info logs by default)
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let cli = Cli::parse();

    if cli.helpfull {
        let mut cmd = Cli::command();
        for sub in cmd.get_subcommands_mut() {
            *sub = sub.clone().hide(false);
        }
        cmd.print_help()?;
        return Ok(());
    }

    if cli.help || cli.command.is_none() {
        let mut cmd = Cli::command();
        let subcmd_name = cli.command.as_ref().map(|c| match c {
            Commands::Add { .. } => "add",
            Commands::Remove { .. } => "remove",
            Commands::List => "list",
            Commands::Lease { .. } => "lease",
            Commands::Sync { .. } => "sync",
            Commands::Release { .. } => "release",
            Commands::Cd { .. } => "cd",
            Commands::Locate { .. } => "locate",
            Commands::SelfTest { .. } => "self-test",
            Commands::Completions { .. } => "completions",
        });

        if let Some(name) = subcmd_name {
            if let Some(subcmd) = cmd.find_subcommand_mut(name) {
                subcmd.print_help()?;
            } else {
                cmd.print_help()?;
            }
        } else {
            cmd.print_help()?;
        }
        return Ok(());
    }

    match cli.command.unwrap() {
        Commands::Add { config: cfg } => {
            let config = Config::new(cli.fuchsia_dir)?;
            config.init_topology()?;
            let env_id = add::add_environment(&config, &cfg, cli.json)?;
            if cli.json {
                println!(
                    "{{\"environment_id\":\"{}\",\"config\":\"{}\"}}",
                    env_id, cfg
                );
            } else {
                println!(
                    "✔ Worktree {} successfully added for config {}.",
                    env_id, cfg
                );
            }
        }
        Commands::Remove { id, force } => {
            let id = match id {
                Some(id) => id,
                None => {
                    let mut cmd = Cli::command();
                    let subcmd = cmd.find_subcommand_mut("remove").unwrap();
                    println!("{}", subcmd.render_usage());
                    return Err(anyhow::anyhow!(
                        "error: the following required arguments were not provided:\n  <ID>"
                    ));
                }
            };
            let config = Config::new(cli.fuchsia_dir)?;
            config.init_topology()?;
            remove::remove_environment(&config, &id, force, cli.json)?;
            if cli.json {
                println!("{{\"removed\":true,\"environment_id\":\"{}\"}}", id);
            } else {
                println!("✔ Worktree {} successfully removed.", id);
            }
        }
        Commands::List => {
            let config = Config::new(cli.fuchsia_dir)?;
            config.init_topology()?;
            list::list_environments(&config, cli.json)?;
        }
        Commands::Lease {
            config: cfg,
            agent_id,
            sync,
            print_path_only,
        } => {
            let config = Config::new(cli.fuchsia_dir)?;
            config.init_topology()?;
            let agent_id = agent_id.unwrap_or_else(|| {
                let uuid = uuid::Uuid::new_v4().to_string();
                format!("agent-{}", &uuid[0..8])
            });
            let env_info = lease::lease_environment(
                &config,
                &cfg,
                &agent_id,
                sync,
                cli.json || print_path_only,
            )?;
            if cli.json {
                let json = serde_json::to_string(&env_info)?;
                println!("{}", json);
            } else if print_path_only {
                println!("{}", env_info.path.to_string_lossy());
            } else {
                println!("✔ Worktree leased successfully!\n");
                println!("  Worktree ID  : {}", env_info.environment_id);
                println!("  Agent ID     : {}", env_info.agent_id);
                println!("  Config       : {}", env_info.config);
                println!("  Path         : {}", env_info.path.to_string_lossy());
                println!("\nTo change directory into the worktree:");
                println!(
                    "  $ fx-worktree cd {}  # Navigate to this specific worktree",
                    env_info.environment_id
                );
                println!(
                    "  $ fx-worktree cd                     # Navigate to the last leased worktree"
                );
            }
        }
        Commands::Sync { id } => {
            let config = Config::new(cli.fuchsia_dir)?;
            config.init_topology()?;
            sync::sync_environment_by_id(&config, &id, cli.json, false)?;
            if cli.json {
                println!("{{\"synced\":true,\"environment_id\":\"{}\"}}", id);
            } else {
                println!("✔ Environment {} successfully synced.", id);
            }
        }
        Commands::Release { id } => {
            let config = Config::new(cli.fuchsia_dir)?;
            config.init_topology()?;
            if !cli.json {
                eprintln!("Resetting worktree {}...", id);
            }
            let released_id = release::release_worktree(&config, &id)?;
            if cli.json {
                println!(
                    "{{\"released\":true,\"environment_id\":\"{}\"}}",
                    released_id
                );
            } else {
                println!("✔ Worktree {} successfully released.", released_id);
            }
        }
        Commands::Cd { .. } => {
            return Err(anyhow::anyhow!(
                "The 'cd' command requires the fx-worktree shell wrapper. Make sure your shell is initialized correctly."
            ));
        }
        Commands::Locate { id } => {
            let config = Config::new(cli.fuchsia_dir)?;
            let path = locate::locate_path(&config, id)?;
            println!("{}", path.to_string_lossy());
        }
        Commands::SelfTest { id } => {
            let config = Config::new(cli.fuchsia_dir)?;
            config.init_topology()?;
            selftest::run_self_test(&config, id)?;
        }

        Commands::Completions { shell } => {
            let mut cmd = Cli::command();
            let mut buf = Vec::new();
            clap_complete::generate(shell, &mut cmd, "fx-worktree", &mut buf);
            let mut script =
                String::from_utf8(buf).context("Failed to parse generated completions as UTF-8")?;

            if shell == clap_complete::Shell::Zsh {
                // Patch positional ID completions
                script = script.replace(
                    "':id -- Worktree ID to remove (must be free):_default'",
                    "':id -- Worktree ID to remove (must be free):_fx_worktree_free_ids'",
                );
                script = script.replace(
                    "':id -- Worktree ID to release (must be leased):_default'",
                    "':id -- Worktree ID to release (must be leased):_fx_worktree_leased_ids'",
                );
                script = script.replace(
                    "'::id -- Worktree ID:_default'",
                    "'::id -- Worktree ID:_fx_worktree_all_ids'",
                );
                script = script.replace(
                    "':id -- Worktree ID to sync:_default'",
                    "':id -- Worktree ID to sync:_fx_worktree_all_ids'",
                );
                script = script.replace(
                    "':id -- Worktree ID to use for the test:_default'",
                    "':id -- Worktree ID to use for the test:_fx_worktree_free_ids'",
                );

                // Patch positional config completions
                script = script.replace(
                    "':config -- Configuration name (e.g. fuchsia.x64):_default'",
                    "':config -- Configuration name (e.g. fuchsia.x64):_fx_worktree_configs'",
                );

                // Move entry point block to the very end of the file
                let entry_point = r#"if [ "$funcstack[1]" = "_fx-worktree" ]; then
    _fx-worktree "$@"
else
    compdef _fx-worktree fx-worktree
fi"#;
                if let Some(idx) = script.find(entry_point) {
                    script.replace_range(idx..idx + entry_point.len(), "");

                    script.push_str("\n\n# Custom dynamic completion helpers\n");
                    script.push_str(
                        r#"_fx_worktree_free_ids() {
    local -a ids
    ids=($(fx-worktree list 2>/dev/null | grep -E '\s+Free$' | awk '{print $2}'))
    _describe -t ids 'free worktree ID' ids
}

_fx_worktree_leased_ids() {
    local -a ids
    ids=($(fx-worktree list 2>/dev/null | grep -E 'In Use' | awk '{print $2}'))
    _describe -t ids 'leased worktree ID' ids
}

_fx_worktree_all_ids() {
    local -a ids
    ids=($(fx-worktree list 2>/dev/null | tail -n +2 | awk '{print $2}'))
    _describe -t ids 'worktree ID' ids
}

_fx_worktree_configs() {
    local -a configs
    configs=($(fx-worktree list 2>/dev/null | tail -n +2 | awk '{print $1}' | sort -u))
    _describe -t configs 'configuration' configs
}
"#,
                    );
                    script.push_str("\n");
                    script.push_str(entry_point);
                    script.push_str("\n");
                }
            }

            use std::io::Write;
            std::io::stdout()
                .write_all(script.as_bytes())
                .context("Failed to write completions to stdout")?;
        }
    }

    Ok(())
}
