use crate::config::ClientConfig;
use ru_command_protocol::{ExecutionResult, MAX_RESULT_OUTPUT_BYTES};
use std::process::Stdio;
use std::time::Instant;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

pub(crate) enum StartTaskOutcome {
    Started(RunningCommand),
    Completed(ExecutionResult),
}

struct OutputCapture {
    text: String,
    truncated: bool,
}

pub(crate) struct RunningCommand {
    child: Child,
    stdout_reader: JoinHandle<std::io::Result<OutputCapture>>,
    stderr_reader: JoinHandle<std::io::Result<OutputCapture>>,
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
                    stdout_truncated: false,
                    stderr_truncated: false,
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
                    stdout_truncated: false,
                    stderr_truncated: false,
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
            stdout: stdout.text,
            stderr: stderr.text,
            stdout_truncated: stdout.truncated,
            stderr_truncated: stderr.truncated,
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
        stdout_truncated: false,
        stderr_truncated: false,
        duration_ms: started_at.elapsed().as_millis() as u64,
        error: Some(message.to_string()),
    }
}

async fn read_output(
    reader: Option<impl tokio::io::AsyncRead + Unpin>,
) -> std::io::Result<OutputCapture> {
    let Some(mut reader) = reader else {
        return Ok(OutputCapture {
            text: String::new(),
            truncated: false,
        });
    };

    let mut output = Vec::new();
    let mut buffer = [0_u8; 4096];
    let mut truncated = false;

    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }

        if output.len() < MAX_RESULT_OUTPUT_BYTES {
            let remaining = MAX_RESULT_OUTPUT_BYTES - output.len();
            let to_copy = remaining.min(read);
            output.extend_from_slice(&buffer[..to_copy]);
            if to_copy < read {
                truncated = true;
            }
        } else {
            truncated = true;
        }
    }

    Ok(OutputCapture {
        text: String::from_utf8_lossy(&output).into_owned(),
        truncated,
    })
}

async fn join_output(handle: JoinHandle<std::io::Result<OutputCapture>>) -> OutputCapture {
    match handle.await {
        Ok(Ok(capture)) => capture,
        Ok(Err(error)) => OutputCapture {
            text: format!("output read failed: {error}"),
            truncated: false,
        },
        Err(error) => OutputCapture {
            text: format!("output join failed: {error}"),
            truncated: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::read_output;
    use ru_command_protocol::MAX_RESULT_OUTPUT_BYTES;
    use tokio::io::{duplex, AsyncWriteExt};

    #[tokio::test]
    async fn truncates_large_output_without_failing() {
        let (mut writer, reader) = duplex((MAX_RESULT_OUTPUT_BYTES * 2) as usize);
        let payload = vec![b'a'; MAX_RESULT_OUTPUT_BYTES + 1024];

        let write_task = tokio::spawn(async move {
            writer.write_all(&payload).await.unwrap();
            writer.shutdown().await.unwrap();
        });

        let capture = read_output(Some(reader)).await.unwrap();
        write_task.await.unwrap();

        assert_eq!(capture.text.len(), MAX_RESULT_OUTPUT_BYTES);
        assert!(capture.truncated);
    }
}
