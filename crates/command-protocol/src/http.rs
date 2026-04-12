use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapRequest {
    pub node_id: String,
    pub auth_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapResponse {
    pub node_id: String,
    pub ws_url: String,
    pub heartbeat_interval_secs: u64,
    #[serde(default)]
    pub protocol_version: String,
    #[serde(default)]
    pub transport_stack: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolSnapshot {
    pub protocol_version: String,
    pub transport_stack: String,
    pub heartbeat_interval_secs: u64,
    pub max_result_output_bytes: u64,
    pub max_task_pull_response_items: u32,
    #[serde(default)]
    pub capabilities: Vec<String>,
}
