use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};
use fxenv::cli::{Cli, Commands};
use fxenv::config::Config;
use fxenv::{create, delete, allocate, free, list, locate, selftest, gc, sync};

fn main() -> Result<()> {
    // Initialize logger (default to warn to silence info logs by default)
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Create { config: cfg } => {
            let config = Config::new(cli.fuchsia_dir)?;
            config.init_topology()?;
            let env_id = create::create_environment(&config, &cfg)?;
            if cli.json {
                println!("{{\"environment_id\":\"{}\",\"config\":\"{}\"}}", env_id, cfg);
            } else {
                println!("✔ Environment {} successfully created for config {}.", env_id, cfg);
            }
        }
        Commands::Delete { id } => {
            let config = Config::new(cli.fuchsia_dir)?;
            config.init_topology()?;
            delete::delete_environment(&config, &id)?;
            if cli.json {
                println!("{{\"deleted\":true,\"environment_id\":\"{}\"}}", id);
            } else {
                println!("✔ Environment {} successfully deleted.", id);
            }
        }
        Commands::List => {
            let config = Config::new(cli.fuchsia_dir)?;
            config.init_topology()?;
            list::list_environments(&config, cli.json)?;
        }
        Commands::Use { config: cfg, agent_id, sync } => {
            let config = Config::new(cli.fuchsia_dir)?;
            config.init_topology()?;
            let agent_id = agent_id.unwrap_or_else(|| {
                let uuid = uuid::Uuid::new_v4().to_string();
                format!("agent-{}", &uuid[0..8])
            });
            let env_info = allocate::allocate_environment(&config, &cfg, &agent_id, sync, cli.json)?;
            if cli.json {
                let json = serde_json::to_string(&env_info)?;
                println!("{}", json);
            } else {
                println!("✔ Workspace allocated successfully!\n");
                println!("  ℹ Environment ID : {}", env_info.environment_id);
                println!("  ℹ Agent ID       : {}", env_info.agent_id);
                println!("  ℹ Config         : {}", env_info.config);
                println!("  ℹ Path           : {}", env_info.path.to_string_lossy());
                println!("\nTo change directory into the workspace:");
                println!("  $ fxenv cd {}  # Navigate to this specific workspace", env_info.environment_id);
                println!("  $ fxenv cd                     # Navigate to the last created environment");
            }
        }
        Commands::Sync { id } => {
            let config = Config::new(cli.fuchsia_dir)?;
            config.init_topology()?;
            sync::sync_environment_by_id(&config, &id, cli.json)?;
            if cli.json {
                println!("{{\"synced\":true,\"environment_id\":\"{}\"}}", id);
            } else {
                println!("✔ Environment {} successfully synced.", id);
            }
        }
        Commands::Free { id } => {
            let config = Config::new(cli.fuchsia_dir)?;
            config.init_topology()?;
            free::free_environment_by_id(&config, &id)?;
            if cli.json {
                println!("{{\"freed\":true,\"environment_id\":\"{}\"}}", id);
            } else {
                println!("✔ Environment {} successfully freed.", id);
            }
        }
        Commands::Cd { .. } => {
            return Err(anyhow::anyhow!("The 'cd' command requires the fxenv shell wrapper. Make sure your shell is initialized correctly."));
        }
        Commands::Locate { id } => {
            let config = Config::new(cli.fuchsia_dir)?;
            let path = locate::locate_path(&config, id)?;
            println!("{}", path.to_string_lossy());
        }
        Commands::SelfTest { use_env } => {
            let config = Config::new(cli.fuchsia_dir)?;
            config.init_topology()?;
            selftest::run_self_test(&config, use_env)?;
        }
        Commands::Gc { timeout } => {
            let config = Config::new(cli.fuchsia_dir)?;
            config.init_topology()?;
            gc::garbage_collect(&config, timeout)?;
            if !cli.json {
                println!("✔ Garbage collection completed.");
            }
        }
        Commands::Completions { shell } => {
            let mut cmd = Cli::command();
            let mut buf = Vec::new();
            clap_complete::generate(shell, &mut cmd, "fxenv", &mut buf);
            let mut script = String::from_utf8(buf).context("Failed to parse generated completions as UTF-8")?;

            if shell == clap_complete::Shell::Zsh {
                // Patch positional ID completions
                script = script.replace(
                    "':id -- Environment ID to delete (must be free):_default'",
                    "':id -- Environment ID to delete (must be free):_fxenv_free_env_ids'",
                );
                script = script.replace(
                    "':id -- Environment ID to free (must be leased):_default'",
                    "':id -- Environment ID to free (must be leased):_fxenv_leased_env_ids'",
                );
                script = script.replace(
                    "'::id -- Environment ID:_default'",
                    "'::id -- Environment ID:_fxenv_all_env_ids'",
                );
                script = script.replace(
                    "':id -- Environment ID to sync:_default'",
                    "':id -- Environment ID to sync:_fxenv_all_env_ids'",
                );

                // Patch positional config completions
                script = script.replace(
                    "':config -- Configuration name (e.g. fuchsia.x64):_default'",
                    "':config -- Configuration name (e.g. fuchsia.x64):_fxenv_configs'",
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
                    script.push_str(r#"_fxenv_free_env_ids() {
    local -a ids
    ids=($(fxenv list 2>/dev/null | grep -E '\s+Free$' | awk '{print $2}'))
    _describe -t ids 'free environment ID' ids
}

_fxenv_leased_env_ids() {
    local -a ids
    ids=($(fxenv list 2>/dev/null | grep -E 'In Use' | awk '{print $2}'))
    _describe -t ids 'leased environment ID' ids
}

_fxenv_all_env_ids() {
    local -a ids
    ids=($(fxenv list 2>/dev/null | tail -n +2 | awk '{print $2}'))
    _describe -t ids 'environment ID' ids
}

_fxenv_configs() {
    local -a configs
    configs=($(fxenv list 2>/dev/null | tail -n +2 | awk '{print $1}' | sort -u))
    _describe -t configs 'configuration' configs
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
