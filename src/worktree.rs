use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct WorktreeInfo {
    pub worktree_id: String,
    pub agent_id: String,
    pub config: String,
    pub pid: u32,
    pub timestamp_sec: u64,
    pub workspace_path: PathBuf,
    pub outdir_path: PathBuf,
}
