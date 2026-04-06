use prost::Message;

#[derive(Clone, PartialEq, Message)]
pub struct PbCommandDescriptor {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(string, tag = "2")]
    pub description: String,
    #[prost(string, repeated, tag = "3")]
    pub default_args: Vec<String>,
    #[prost(bool, tag = "4")]
    pub allow_extra_args: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct PbClientHello {
    #[prost(string, tag = "1")]
    pub node_id: String,
    #[prost(string, tag = "2")]
    pub hostname: String,
    #[prost(string, tag = "3")]
    pub platform: String,
    #[prost(uint64, tag = "4")]
    pub poll_interval_secs: u64,
    #[prost(message, repeated, tag = "5")]
    pub commands: Vec<PbCommandDescriptor>,
}

#[derive(Clone, PartialEq, Message)]
pub struct PbTaskAssignment {
    #[prost(uint64, tag = "1")]
    pub task_id: u64,
    #[prost(string, tag = "2")]
    pub command_name: String,
    #[prost(string, repeated, tag = "3")]
    pub args: Vec<String>,
    #[prost(uint64, tag = "4")]
    pub created_at_unix_secs: u64,
    #[prost(uint64, optional, tag = "5")]
    pub timeout_secs: Option<u64>,
    #[prost(uint32, tag = "6")]
    pub attempt: u32,
}

#[derive(Clone, PartialEq, Message)]
pub struct PbTaskAck {
    #[prost(uint64, tag = "1")]
    pub task_id: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct PbTaskCancel {
    #[prost(uint64, tag = "1")]
    pub task_id: u64,
    #[prost(string, optional, tag = "2")]
    pub reason: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct PbTaskResult {
    #[prost(uint64, tag = "1")]
    pub task_id: u64,
    #[prost(bool, tag = "2")]
    pub success: bool,
    #[prost(int32, optional, tag = "3")]
    pub exit_code: Option<i32>,
    #[prost(string, tag = "4")]
    pub stdout: String,
    #[prost(string, tag = "5")]
    pub stderr: String,
    #[prost(uint64, tag = "6")]
    pub duration_ms: u64,
    #[prost(string, optional, tag = "7")]
    pub error: Option<String>,
    #[prost(bool, tag = "8")]
    pub stdout_truncated: bool,
    #[prost(bool, tag = "9")]
    pub stderr_truncated: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct PbNodeError {
    #[prost(string, tag = "1")]
    pub message: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct PbNodePayloadEnvelope {
    #[prost(oneof = "pb_node_payload_envelope::Body", tags = "1, 2, 3, 4, 5, 6")]
    pub body: Option<pb_node_payload_envelope::Body>,
}

pub mod pb_node_payload_envelope {
    use super::{
        PbClientHello, PbNodeError, PbTaskAck, PbTaskAssignment, PbTaskCancel, PbTaskResult,
    };
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Body {
        #[prost(message, tag = "1")]
        ClientHello(PbClientHello),
        #[prost(message, tag = "2")]
        TaskAssignment(PbTaskAssignment),
        #[prost(message, tag = "3")]
        TaskResult(PbTaskResult),
        #[prost(message, tag = "4")]
        TaskAck(PbTaskAck),
        #[prost(message, tag = "5")]
        TaskCancel(PbTaskCancel),
        #[prost(message, tag = "6")]
        Error(PbNodeError),
    }
}
