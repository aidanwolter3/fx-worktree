use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct EnvironmentInfo {
    pub environment_id: String,
    pub agent_id: String,
    pub config: String,
    pub pid: u32,
    pub timestamp_sec: u64,
    pub path: PathBuf,
}
