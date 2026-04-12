use serde::{Deserialize, Serialize};

pub const MAX_RESULT_OUTPUT_BYTES: usize = 64 * 1024;
pub const CONTROL_PROTOCOL_VERSION: &str = "command-plane-control/v1alpha1";
pub const MAX_TASK_PULL_RESPONSE_ITEMS: u32 = 1;
pub const CONTROL_TRANSPORT_STACK: &str = "http-bootstrap/ws+mqtt+protobuf";
pub const CONTROL_CAPABILITIES: &[&str] = &[
    "session_info",
    "command_catalog",
    "task_push",
    "task_pull",
    "task_cancel",
    "single_inflight",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandDescriptor {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub default_args: Vec<String>,
    #[serde(default)]
    pub allow_extra_args: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRegistration {
    pub node_id: String,
    pub hostname: String,
    pub platform: String,
    pub poll_interval_secs: u64,
    #[serde(default)]
    pub commands: Vec<CommandDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSnapshot {
    pub node_id: String,
    pub hostname: String,
    pub platform: String,
    pub poll_interval_secs: u64,
    pub registered_at_unix_secs: u64,
    pub last_seen_unix_secs: u64,
    #[serde(default)]
    pub online: bool,
    #[serde(default)]
    pub session_protocol_version: Option<String>,
    #[serde(default)]
    pub session_transport_stack: Option<String>,
    #[serde(default)]
    pub session_heartbeat_interval_secs: Option<u64>,
    #[serde(default)]
    pub session_capabilities: Vec<String>,
    #[serde(default)]
    pub commands: Vec<CommandDescriptor>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionEventKind {
    ConnectRejected,
    AuthFailed,
    SessionOpened,
    NodeRegistered,
    SessionClosed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEvent {
    pub event_id: u64,
    pub node_id: String,
    pub kind: SessionEventKind,
    pub message: String,
    pub created_at_unix_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTaskRequest {
    pub node_id: String,
    pub command_name: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelTaskRequest {
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingTask {
    pub task_id: u64,
    pub node_id: String,
    pub command_name: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub created_at_unix_secs: u64,
    pub timeout_secs: Option<u64>,
    pub retry_count: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Dispatched,
    Running,
    Succeeded,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    #[serde(default)]
    pub stdout_truncated: bool,
    #[serde(default)]
    pub stderr_truncated: bool,
    pub duration_ms: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitTaskResultRequest {
    pub node_id: String,
    pub result: ExecutionResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub task_id: u64,
    pub node_id: String,
    pub command_name: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub status: TaskStatus,
    pub created_at_unix_secs: u64,
    #[serde(default)]
    pub created_via: Option<String>,
    #[serde(default)]
    pub created_by: Option<String>,
    pub dispatched_at_unix_secs: Option<u64>,
    pub acked_at_unix_secs: Option<u64>,
    pub started_at_unix_secs: Option<u64>,
    pub finished_at_unix_secs: Option<u64>,
    pub timeout_secs: Option<u64>,
    pub retry_count: u32,
    pub retry_reason: Option<String>,
    pub canceled_at_unix_secs: Option<u64>,
    pub cancel_reason: Option<String>,
    #[serde(default)]
    pub canceled_via: Option<String>,
    #[serde(default)]
    pub canceled_by: Option<String>,
    pub result: Option<ExecutionResult>,
}
