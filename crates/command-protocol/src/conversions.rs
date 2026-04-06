use crate::domain::{CommandDescriptor, ExecutionResult, NodeRegistration, PendingTask};
use crate::pb::{PbClientHello, PbCommandDescriptor, PbTaskAssignment, PbTaskResult};

impl From<&CommandDescriptor> for PbCommandDescriptor {
    fn from(value: &CommandDescriptor) -> Self {
        Self {
            name: value.name.clone(),
            description: value.description.clone(),
            default_args: value.default_args.clone(),
            allow_extra_args: value.allow_extra_args,
        }
    }
}

impl From<PbCommandDescriptor> for CommandDescriptor {
    fn from(value: PbCommandDescriptor) -> Self {
        Self {
            name: value.name,
            description: value.description,
            default_args: value.default_args,
            allow_extra_args: value.allow_extra_args,
        }
    }
}

impl From<PbClientHello> for NodeRegistration {
    fn from(value: PbClientHello) -> Self {
        Self {
            node_id: value.node_id,
            hostname: value.hostname,
            platform: value.platform,
            poll_interval_secs: value.poll_interval_secs,
            commands: value.commands.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<&NodeRegistration> for PbClientHello {
    fn from(value: &NodeRegistration) -> Self {
        Self {
            node_id: value.node_id.clone(),
            hostname: value.hostname.clone(),
            platform: value.platform.clone(),
            poll_interval_secs: value.poll_interval_secs,
            commands: value.commands.iter().map(Into::into).collect(),
        }
    }
}

impl From<&PendingTask> for PbTaskAssignment {
    fn from(value: &PendingTask) -> Self {
        Self {
            task_id: value.task_id,
            command_name: value.command_name.clone(),
            args: value.args.clone(),
            created_at_unix_secs: value.created_at_unix_secs,
            timeout_secs: value.timeout_secs,
            attempt: value.retry_count + 1,
        }
    }
}

impl From<PbTaskResult> for ExecutionResult {
    fn from(value: PbTaskResult) -> Self {
        Self {
            success: value.success,
            exit_code: value.exit_code,
            stdout: value.stdout,
            stderr: value.stderr,
            duration_ms: value.duration_ms,
            error: value.error,
        }
    }
}
