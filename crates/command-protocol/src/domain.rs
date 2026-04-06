use serde::{Deserialize, Serialize};

pub const MAX_RESULT_OUTPUT_BYTES: usize = 64 * 1024;

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
    pub commands: Vec<CommandDescriptor>,
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
    pub dispatched_at_unix_secs: Option<u64>,
    pub acked_at_unix_secs: Option<u64>,
    pub started_at_unix_secs: Option<u64>,
    pub finished_at_unix_secs: Option<u64>,
    pub timeout_secs: Option<u64>,
    pub retry_count: u32,
    pub retry_reason: Option<String>,
    pub canceled_at_unix_secs: Option<u64>,
    pub cancel_reason: Option<String>,
    pub result: Option<ExecutionResult>,
}
