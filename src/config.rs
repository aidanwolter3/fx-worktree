use anyhow::{Context, Result, anyhow};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Config {
    pub fxenv_root: PathBuf,
    pub fuchsia_dir: PathBuf,
}

impl Config {
    pub fn new(fuchsia_dir_arg: Option<PathBuf>) -> Result<Self> {
        let fxenv_root = match std::env::var("FXENV_ROOT") {
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
            fxenv_root,
            fuchsia_dir,
        })
    }

    pub fn init_topology(&self) -> Result<()> {
        let leases_dir = self.leases_dir();
        let environments_dir = self.environments_dir();

        std::fs::create_dir_all(&leases_dir)
            .with_context(|| format!("Failed to create leases directory {:?}", leases_dir))?;
        std::fs::create_dir_all(&environments_dir).with_context(|| {
            format!("Failed to create environments directory {:?}", environments_dir)
        })?;

        Ok(())
    }

    pub fn leases_dir(&self) -> PathBuf {
        self.fxenv_root.join("leases")
    }

    pub fn environments_dir(&self) -> PathBuf {
        self.fxenv_root.join("environments")
    }

    pub fn last_created_file(&self) -> PathBuf {
        self.fxenv_root.join("last_created")
    }

    pub fn record_last_created(&self, path: &Path) -> Result<()> {
        std::fs::write(self.last_created_file(), path.to_string_lossy().as_bytes())
            .with_context(|| format!("Failed to write last_created file"))
    }

    pub fn read_last_created(&self) -> Result<PathBuf> {
        let file = self.last_created_file();
        if !file.exists() {
            return Err(anyhow!("No environment has been created yet."));
        }
        let path_str = std::fs::read_to_string(&file)
            .with_context(|| format!("Failed to read last_created file"))?;
        Ok(PathBuf::from(path_str.trim()))
    }
}
