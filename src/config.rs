use anyhow::{Context, Result, anyhow};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub fenv_root: PathBuf,
    pub fuchsia_dir: PathBuf,
}

impl Config {
    pub fn new(fuchsia_dir_arg: Option<PathBuf>) -> Result<Self> {
        let fenv_root = match std::env::var("FENV_ROOT") {
            Ok(val) => PathBuf::from(val),
            Err(_) => {
                let home =
                    std::env::var("HOME").context("Failed to get HOME environment variable")?;
                PathBuf::from(home).join(".fuchsia-agents")
            }
        };

        let fuchsia_dir = match fuchsia_dir_arg {
            Some(dir) => dir,
            None => match std::env::var("FUCHSIA_DIR") {
                Ok(val) => PathBuf::from(val),
                Err(_) => {
                    return Err(anyhow!(
                        "Fuchsia directory not specified. Use --fuchsia-dir or set FUCHSIA_DIR environment variable."
                    ));
                }
            },
        };

        // Ensure fuchsia_dir exists and is a directory
        if !fuchsia_dir.is_dir() {
            return Err(anyhow!(
                "Fuchsia directory {:?} does not exist or is not a directory",
                fuchsia_dir
            ));
        }

        Ok(Config {
            fenv_root,
            fuchsia_dir,
        })
    }

    pub fn init_topology(&self) -> Result<()> {
        let outdirs_dir = self.outdirs_dir();
        let leases_dir = self.fenv_root.join("leases");
        let workspaces_dir = self.fenv_root.join("workspaces");

        std::fs::create_dir_all(&outdirs_dir)
            .with_context(|| format!("Failed to create outdirs directory {:?}", outdirs_dir))?;
        std::fs::create_dir_all(&leases_dir)
            .with_context(|| format!("Failed to create leases directory {:?}", leases_dir))?;
        std::fs::create_dir_all(&workspaces_dir).with_context(|| {
            format!("Failed to create workspaces directory {:?}", workspaces_dir)
        })?;

        Ok(())
    }

    pub fn outdirs_dir(&self) -> PathBuf {
        self.fuchsia_dir.join("out/fenv")
    }

    pub fn leases_dir(&self) -> PathBuf {
        self.fenv_root.join("leases")
    }

    pub fn workspaces_dir(&self) -> PathBuf {
        self.fenv_root.join("workspaces")
    }
}
