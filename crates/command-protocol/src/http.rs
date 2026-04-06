use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapRequest {
    pub agent_id: String,
    pub auth_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapResponse {
    pub agent_id: String,
    pub ws_url: String,
    pub heartbeat_interval_secs: u64,
}
