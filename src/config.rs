//! Runtime configuration and checkout-scoped state management.
//!
//! This module defines the [`Config`] struct which handles:
//! 1. Resolving the active Fuchsia checkout path (`fuchsia_dir`) from arguments or environment variables.
//! 2. Defining and creating the topology paths for Jiri worktrees (stored under `.jiri_root/worktrees/`).
//! 3. Tracking the checkout-scoped `last_active` file, which records the last leased worktree path
//!    for shell navigation (`fx-worktree cd`).

use anyhow::{Context, Result, anyhow};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Config {
    pub fuchsia_dir: PathBuf,
}

impl Config {
    pub fn new(fuchsia_dir_arg: Option<PathBuf>) -> Result<Self> {
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
            fuchsia_dir,
        })
    }

    pub fn init_topology(&self) -> Result<()> {
        let worktrees_dir = self.worktrees_dir();

        std::fs::create_dir_all(&worktrees_dir).with_context(|| {
            format!(
                "Failed to create Jiri worktrees directory {:?}",
                worktrees_dir
            )
        })?;

        Ok(())
    }

    pub fn worktrees_dir(&self) -> PathBuf {
        self.fuchsia_dir.join(".jiri_root").join("worktrees")
    }

    pub fn last_active_file(&self) -> PathBuf {
        self.worktrees_dir().join("last_active")
    }

    pub fn record_last_active(&self, path: &Path) -> Result<()> {
        std::fs::write(self.last_active_file(), path.to_string_lossy().as_bytes())
            .with_context(|| format!("Failed to write last_active file"))
    }

    pub fn read_last_active(&self) -> Result<PathBuf> {
        let file = self.last_active_file();
        if !file.exists() {
            return Err(anyhow!("No worktree has been active yet."));
        }
        let path_str = std::fs::read_to_string(&file)
            .with_context(|| format!("Failed to read last_active file"))?;
        Ok(PathBuf::from(path_str.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::tempdir;

    static CONFIG_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_config_new_overridden_dir() {
        let _lock = CONFIG_TEST_LOCK.lock().unwrap();
        let dir = tempdir().unwrap();
        
        let config = Config::new(Some(dir.path().to_path_buf())).unwrap();
        assert_eq!(config.fuchsia_dir, dir.path().canonicalize().unwrap());
    }

    #[test]
    fn test_config_new_env_var() {
        let _lock = CONFIG_TEST_LOCK.lock().unwrap();
        let dir = tempdir().unwrap();
        let canonical_dir = dir.path().canonicalize().unwrap();
        
        let old_fuchsia_dir = std::env::var("FUCHSIA_DIR").ok();
        unsafe {
            std::env::set_var("FUCHSIA_DIR", &canonical_dir);
        }

        let config = Config::new(None).unwrap();
        assert_eq!(config.fuchsia_dir, canonical_dir);

        unsafe {
            if let Some(old) = old_fuchsia_dir {
                std::env::set_var("FUCHSIA_DIR", old);
            } else {
                std::env::remove_var("FUCHSIA_DIR");
            }
        }
    }

    #[test]
    fn test_config_new_missing_err() {
        let _lock = CONFIG_TEST_LOCK.lock().unwrap();
        
        let old_fuchsia_dir = std::env::var("FUCHSIA_DIR").ok();
        unsafe {
            std::env::remove_var("FUCHSIA_DIR");
        }

        let config = Config::new(None);
        assert!(config.is_err());
        assert!(config.unwrap_err().to_string().contains("Fuchsia directory not specified"));

        unsafe {
            if let Some(old) = old_fuchsia_dir {
                std::env::set_var("FUCHSIA_DIR", old);
            }
        }
    }

    #[test]
    fn test_config_new_nonexistent_err() {
        let _lock = CONFIG_TEST_LOCK.lock().unwrap();
        let nonexistent_path = PathBuf::from("/nonexistent/fuchsia/directory");
        
        let config = Config::new(Some(nonexistent_path));
        assert!(config.is_err());
        assert!(config.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn test_config_init_topology() {
        let _lock = CONFIG_TEST_LOCK.lock().unwrap();
        let dir = tempdir().unwrap();
        let config = Config::new(Some(dir.path().to_path_buf())).unwrap();

        assert!(!config.worktrees_dir().exists());
        config.init_topology().unwrap();
        assert!(config.worktrees_dir().exists());
    }

    #[test]
    fn test_config_last_active_recording() {
        let _lock = CONFIG_TEST_LOCK.lock().unwrap();
        let dir = tempdir().unwrap();
        let config = Config::new(Some(dir.path().to_path_buf())).unwrap();
        config.init_topology().unwrap();

        let active_wt = Path::new("/some/active/worktree");

        // Read before write should error
        assert!(config.read_last_active().is_err());

        // Write and read
        config.record_last_active(active_wt).unwrap();
        let read_path = config.read_last_active().unwrap();
        assert_eq!(read_path, active_wt);
    }
}
