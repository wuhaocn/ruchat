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
            commands: Vec::new(),
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
            stdout_truncated: value.stdout_truncated,
            stderr_truncated: value.stderr_truncated,
            duration_ms: value.duration_ms,
            error: value.error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_hello_conversion_does_not_carry_commands() {
        let hello = PbClientHello {
            node_id: "node-1".to_string(),
            hostname: "host-a".to_string(),
            platform: "linux-amd64".to_string(),
            poll_interval_secs: 5,
        };

        let registration = NodeRegistration::from(hello);
        assert_eq!(registration.node_id, "node-1");
        assert_eq!(registration.hostname, "host-a");
        assert_eq!(registration.platform, "linux-amd64");
        assert_eq!(registration.poll_interval_secs, 5);
        assert!(registration.commands.is_empty());
    }

    #[test]
    fn pending_task_conversion_maps_retry_count_to_attempt() {
        let task = PendingTask {
            task_id: 7,
            node_id: "node-1".to_string(),
            command_name: "echo".to_string(),
            args: vec!["hello".to_string()],
            created_at_unix_secs: 1_700_000_000,
            timeout_secs: Some(30),
            retry_count: 2,
        };

        let assignment = PbTaskAssignment::from(&task);
        assert_eq!(assignment.task_id, 7);
        assert_eq!(assignment.command_name, "echo");
        assert_eq!(assignment.args, vec!["hello"]);
        assert_eq!(assignment.timeout_secs, Some(30));
        assert_eq!(assignment.attempt, 3);
    }
}
