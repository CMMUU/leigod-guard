//! Wire contract shared with the cloud; no credentials or process lists.
use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct CycleRequest {
    pub op: String,
    pub provider: String,
    pub run_generation: i64,
    pub grant_revision: i64,
    #[serde(default)]
    pub sequence: i64,
    #[serde(default)]
    pub cycle_id: Option<String>,
    #[serde(default)]
    pub action_id: Option<String>,
    #[serde(default)]
    pub request_id: String,
    #[serde(default)]
    pub phase: String,
    #[serde(default)]
    pub observer_age: u32,
    #[serde(default)]
    pub executor_age: u32,
    #[serde(default)]
    pub prepare_seconds: u32,
    #[serde(default)]
    pub billing_paused: Option<bool>,
    #[serde(default)]
    pub billing_age: u32,
    #[serde(default)]
    pub startup_seconds: u32,
    #[serde(default)]
    pub exit_seconds: u32,
    #[serde(default)]
    pub manual: bool,
    #[serde(default)]
    pub result: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct CycleView {
    pub enabled: bool,
    pub cycle_id: Option<String>,
    pub revision: i64,
    pub policy_revision: i64,
    pub startup_seconds: u32,
    pub exit_seconds: u32,
    pub mode: String,
    pub state: String,
    pub remaining_seconds: u64,
    pub action_id: Option<String>,
    pub lease_seconds: u64,
    pub message: String,
}
