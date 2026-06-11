use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};
use fx_worktree::cli::{Cli, Commands};
use fx_worktree::colors::Colors;
use fx_worktree::config::Config;
use fx_worktree::{lease, list, locate, mark_free, mark_reserved, release, worktree};

fn main() -> Result<()> {
    // Initialize logger (default to warn to silence info logs by default)
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let cli = Cli::parse();
    let colors = Colors::new();

    if cli.helpfull {
        let mut cmd = Cli::command();
        for sub in cmd.get_subcommands_mut() {
            *sub = sub.clone().hide(false);
        }
        cmd.print_help()?;
        return Ok(());
    }

    if cli.command.is_none() {
        let mut cmd = Cli::command();
        let subcmd_name = cli.command.as_ref().map(|c| match c {
            Commands::List => "list",
            Commands::Add { .. } => "add",
            Commands::Remove { .. } => "remove",
            Commands::Lease { .. } => "lease",
            Commands::Release { .. } => "release",
            Commands::Cd { .. } => "cd",
            Commands::MarkFree { .. } => "mark-free",
            Commands::MarkReserved { .. } => "mark-reserved",
            Commands::Locate { .. } => "locate",
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
        Commands::List => {
            let config = Config::new(cli.fuchsia_dir)?;
            config.init_topology()?;
            list::list_worktrees(&config, cli.json)?;
        }
        Commands::Add { name } => {
            let config = Config::new(cli.fuchsia_dir)?;
            config.init_topology()?;
            worktree::add_worktree(&config, &name)?;
        }
        Commands::Remove { name, force } => {
            let config = Config::new(cli.fuchsia_dir)?;
            config.init_topology()?;
            worktree::remove_worktree(&config, &name, force)?;
            if cli.json {
                println!("{{\"worktree_id\":\"{}\",\"removed\":true}}", name);
            } else {
                println!(
                    "{} Worktree '{}' successfully removed.",
                    colors.green("✔"),
                    colors.blue(&name)
                );
            }
        }
        Commands::Lease {
            name,
            any,
            agent_id,
            sync,
            print_path_only,
        } => {
            let config = Config::new(cli.fuchsia_dir)?;
            config.init_topology()?;
            let wt_info = lease::lease_worktree(
                &config,
                name.as_deref(),
                any,
                agent_id.as_deref(),
                sync,
                cli.json || print_path_only,
            )?;
            if cli.json {
                let json = serde_json::to_string(&wt_info)?;
                println!("{}", json);
            } else if print_path_only {
                println!("{}", wt_info.path.to_string_lossy());
            } else {
                println!("{} Worktree leased successfully!\n", colors.green("✔"));
                println!("  Worktree ID  : {}", colors.blue(&wt_info.worktree_id));
                if let Some(agent) = &wt_info.agent_id {
                    println!("  Agent ID     : {}", colors.blue(agent));
                }
                println!(
                    "  Path         : {}",
                    colors.blue(&wt_info.path.to_string_lossy())
                );
                println!("\nTo change directory into the worktree:");
                println!(
                    "  $ fx-worktree cd {}  # Navigate to this specific worktree",
                    colors.blue(&wt_info.worktree_id)
                );
                println!(
                    "  $ fx-worktree cd                     # Navigate to the last leased worktree"
                );
            }
        }
        Commands::Release { name } => {
            let config = Config::new(cli.fuchsia_dir)?;
            config.init_topology()?;
            if !cli.json {
                eprintln!("Resetting worktree {}...", colors.blue(&name));
            }
            let released_id = release::release_worktree(&config, &name)?;
            if cli.json {
                println!("{{\"released\":true,\"worktree_id\":\"{}\"}}", released_id);
            } else {
                println!(
                    "{} Worktree {} successfully released.",
                    colors.green("✔"),
                    colors.blue(&released_id)
                );
            }
        }
        Commands::MarkFree { name } => {
            let config = Config::new(cli.fuchsia_dir)?;
            config.init_topology()?;
            let wt_id = mark_free::mark_free_worktree(&config, &name, cli.json)?;
            if cli.json {
                println!("{{\"worktree_id\":\"{}\",\"marked_free\":true}}", wt_id);
            } else {
                println!(
                    "{} Worktree {} successfully marked as free.",
                    colors.green("✔"),
                    colors.blue(&wt_id)
                );
            }
        }
        Commands::MarkReserved { name } => {
            let config = Config::new(cli.fuchsia_dir)?;
            config.init_topology()?;
            // Resolve path first to get the canonical ID (for output)
            let path = locate::locate_path(&config, Some(name.clone()))?;
            let id = path.file_name().unwrap().to_string_lossy().into_owned();

            mark_reserved::mark_reserved_worktree(&config, &name, cli.json)?;
            if cli.json {
                println!("{{\"worktree_id\":\"{}\",\"marked_reserved\":true}}", id);
            } else {
                println!(
                    "{} Worktree {} successfully marked as reserved.",
                    colors.green("✔"),
                    colors.blue(&id)
                );
            }
        }
        Commands::Cd { .. } => {
            return Err(anyhow::anyhow!(
                "The 'cd' command requires the fx-worktree shell wrapper. Make sure your shell is initialized correctly."
            ));
        }
        Commands::Locate { name } => {
            let config = Config::new(cli.fuchsia_dir)?;
            let path = locate::locate_path(&config, name)?;
            println!("{}", path.to_string_lossy());
        }

        Commands::Completions { shell } => {
            let mut cmd = Cli::command();
            let mut buf = Vec::new();
            clap_complete::generate(shell, &mut cmd, "fx-worktree", &mut buf);
            let mut script =
                String::from_utf8(buf).context("Failed to parse generated completions as UTF-8")?;

            if shell == clap_complete::Shell::Zsh {
                // Patch positional name completions
                script = script.replace(
                    "':name -- Worktree name to release (must be leased):_default'",
                    "':name -- Worktree name to release (must be leased):_fx_worktree_leased_ids'",
                );
                script = script.replace(
                    "'::name -- Worktree name:_default'",
                    "'::name -- Worktree name:_fx_worktree_all_ids'",
                );
                script = script.replace(
                    "':name -- Worktree name to mark reserved:_default'",
                    "':name -- Worktree name to mark reserved:_fx_worktree_free_ids'",
                );
                script = script.replace(
                    "':name -- Worktree name to mark free:_default'",
                    "':name -- Worktree name to mark free:_fx_worktree_reserved_ids'",
                );
                script = script.replace(
                    "':name -- Name of the worktree to remove:_default'",
                    "':name -- Name of the worktree to remove:_fx_worktree_all_ids'",
                );

                // Patch lease name completions
                script = script.replace(
                    "'::name -- Worktree name to lease:_default'",
                    "'::name -- Worktree name to lease:_fx_worktree_free_ids'",
                );

                // Hide locate subcommand from completions list
                script = script.replace("'locate:Locate the path of a worktree' \\\n", "");

                // Hide completions subcommand from completions list
                script = script.replace(
                    "'completions:Generate shell completion scripts to stdout' \\\n",
                    "",
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
    local -a paths ids
    paths=($(fx-worktree list 2>/dev/null | grep -vE '^[ └├]|No worktrees' | grep -E '\s+Free$' | awk '{print $1}'))
    ids=("${(@)paths:t}")
    _describe -t ids 'free worktree name' ids
}

_fx_worktree_leased_ids() {
    local -a paths ids
    paths=($(fx-worktree list 2>/dev/null | grep -vE '^[ └├]|No worktrees' | grep -E 'In Use' | awk '{print $1}'))
    ids=("${(@)paths:t}")
    _describe -t ids 'leased worktree name' ids
}

_fx_worktree_reserved_ids() {
    local -a paths ids
    paths=($(fx-worktree list 2>/dev/null | grep -vE '^[ └├]|No worktrees' | grep -E 'Reserved' | awk '{print $1}'))
    ids=("${(@)paths:t}")
    _describe -t ids 'reserved worktree name' ids
}

_fx_worktree_all_ids() {
    local -a paths ids
    paths=($(fx-worktree list 2>/dev/null | grep -vE '^[ └├]|No worktrees' | awk '{print $1}'))
    ids=("${(@)paths:t}")
    _describe -t ids 'worktree name' ids
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
