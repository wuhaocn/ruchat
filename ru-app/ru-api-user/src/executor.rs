use crate::config::ClientConfig;
use ru_command_protocol::ExecutionResult;
use std::process::Stdio;
use std::time::Instant;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

pub(crate) enum StartTaskOutcome {
    Started(RunningCommand),
    Completed(ExecutionResult),
}

pub(crate) struct RunningCommand {
    child: Child,
    stdout_reader: JoinHandle<std::io::Result<Vec<u8>>>,
    stderr_reader: JoinHandle<std::io::Result<Vec<u8>>>,
    started_at: Instant,
    cancel_reason: Option<String>,
}

impl RunningCommand {
    pub(crate) fn start(
        config: &ClientConfig,
        command_name: &str,
        args: &[String],
    ) -> StartTaskOutcome {
        let started_at = Instant::now();
        let Some(command) = config
            .commands
            .iter()
            .find(|command| command.name == command_name)
        else {
            return StartTaskOutcome::Completed(invalid_task_result(
                started_at,
                "command is not configured on this client",
            ));
        };

        if !command.allow_extra_args && !args.is_empty() {
            return StartTaskOutcome::Completed(invalid_task_result(
                started_at,
                "command does not allow extra args",
            ));
        }

        let mut process = Command::new(&command.program);
        process.stdout(Stdio::piped());
        process.stderr(Stdio::piped());
        process.args(&command.default_args);
        if command.allow_extra_args {
            process.args(args);
        }

        let mut child = match process.spawn() {
            Ok(child) => child,
            Err(error) => {
                return StartTaskOutcome::Completed(ExecutionResult {
                    success: false,
                    exit_code: None,
                    stdout: String::new(),
                    stderr: String::new(),
                    duration_ms: started_at.elapsed().as_millis() as u64,
                    error: Some(error.to_string()),
                });
            }
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let stdout_reader = tokio::spawn(read_output(stdout));
        let stderr_reader = tokio::spawn(read_output(stderr));

        StartTaskOutcome::Started(Self {
            child,
            stdout_reader,
            stderr_reader,
            started_at,
            cancel_reason: None,
        })
    }

    pub(crate) fn request_cancel(&mut self, reason: Option<String>) -> std::io::Result<()> {
        if self.cancel_reason.is_none() {
            self.cancel_reason = reason.or_else(|| Some("task canceled by server".to_string()));
        }
        self.child.start_kill()
    }

    pub(crate) fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.child.try_wait()
    }

    pub(crate) async fn finish(mut self) -> ExecutionResult {
        let status = match self.child.wait().await {
            Ok(status) => status,
            Err(error) => {
                return ExecutionResult {
                    success: false,
                    exit_code: None,
                    stdout: String::new(),
                    stderr: String::new(),
                    duration_ms: self.started_at.elapsed().as_millis() as u64,
                    error: Some(error.to_string()),
                };
            }
        };

        let stdout = join_output(self.stdout_reader).await;
        let stderr = join_output(self.stderr_reader).await;

        ExecutionResult {
            success: status.success() && self.cancel_reason.is_none(),
            exit_code: status.code(),
            stdout,
            stderr,
            duration_ms: self.started_at.elapsed().as_millis() as u64,
            error: self.cancel_reason.take(),
        }
    }
}

fn invalid_task_result(started_at: Instant, message: &str) -> ExecutionResult {
    ExecutionResult {
        success: false,
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        duration_ms: started_at.elapsed().as_millis() as u64,
        error: Some(message.to_string()),
    }
}

async fn read_output(
    reader: Option<impl tokio::io::AsyncRead + Unpin>,
) -> std::io::Result<Vec<u8>> {
    let Some(mut reader) = reader else {
        return Ok(Vec::new());
    };

    let mut output = Vec::new();
    reader.read_to_end(&mut output).await?;
    Ok(output)
}

async fn join_output(handle: JoinHandle<std::io::Result<Vec<u8>>>) -> String {
    match handle.await {
        Ok(Ok(bytes)) => String::from_utf8_lossy(&bytes).into_owned(),
        Ok(Err(error)) => format!("output read failed: {error}"),
        Err(error) => format!("output join failed: {error}"),
    }
}
