use anyhow::{Context, Result};
use std::fs;
use std::time::SystemTime;

use crate::config::Config;
use crate::free::free_environment_internal;
use crate::environment::EnvironmentInfo;

pub fn garbage_collect(config: &Config, timeout_sec: u64) -> Result<()> {
    log::info!(
        "Starting garbage collection of environments (timeout: {}s)...",
        timeout_sec
    );

    let leases_dir = config.leases_dir();
    if !leases_dir.exists() {
        return Ok(());
    }

    let entries = fs::read_dir(&leases_dir)
        .with_context(|| format!("Failed to read leases directory {:?}", leases_dir))?;

    let current_time = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("lease") {
            log::debug!("Checking lease file {:?}", path);

            let env_json = match fs::read_to_string(&path) {
                Ok(content) => content,
                Err(e) => {
                    log::error!("Failed to read lease file {:?}: {:?}", path, e);
                    continue;
                }
            };

            let env_info: EnvironmentInfo = match serde_json::from_str(&env_json) {
                Ok(w) => w,
                Err(e) => {
                    log::error!("Failed to parse EnvironmentInfo JSON in {:?}: {:?}", path, e);
                    continue;
                }
            };

            let is_dead = !is_process_alive(env_info.pid);
            let is_expired = (current_time - env_info.timestamp_sec) >= timeout_sec;

            if is_dead || is_expired {
                if is_dead {
                    log::info!(
                        "Environment {} is orphaned (PID {} is dead)",
                        env_info.environment_id,
                        env_info.pid
                    );
                } else {
                    log::info!(
                        "Environment {} is expired (age: {}s)",
                        env_info.environment_id,
                        current_time - env_info.timestamp_sec
                    );
                }

                match free_environment_internal(&env_info) {
                    Ok(_) => {
                        if let Err(e) = fs::remove_file(&path) {
                            log::error!("Failed to remove lease file {:?}: {:?}", path, e);
                        } else {
                            log::info!("Cleaned up environment {}", env_info.environment_id);
                        }
                    }
                    Err(e) => {
                        log::error!(
                            "Failed to free environment {}: {:?}",
                            env_info.environment_id,
                            e
                        );
                    }
                }
            }
        }
    }

    Ok(())
}

fn is_process_alive(pid: u32) -> bool {
    unsafe {
        let res = libc::kill(pid as libc::pid_t, 0);
        if res == 0 {
            true
        } else {
            let err = std::io::Error::last_os_error();
            err.raw_os_error() != Some(libc::ESRCH)
        }
    }
}
