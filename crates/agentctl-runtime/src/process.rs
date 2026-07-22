use std::io;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

const PIPE_CHUNK_BYTES: usize = 8 * 1024;
const PIPE_CHANNEL_CAPACITY: usize = 8;

#[derive(Debug, Clone, Copy)]
pub struct ProcessOutputLimits {
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub combined_bytes: u64,
}

#[derive(Debug)]
pub struct BoundedProcessOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum ProcessRunError {
    #[error("failed to spawn subprocess: {0}")]
    Spawn(#[source] io::Error),
    #[error("failed to read subprocess {stream}: {message}")]
    Read {
        stream: &'static str,
        message: String,
    },
    #[error("failed to wait for subprocess: {0}")]
    Wait(#[source] io::Error),
    #[error("subprocess timed out after {seconds} seconds")]
    Timeout { seconds: u64 },
    #[error("subprocess execution was cancelled")]
    Cancelled,
    #[error("subprocess {stream} exceeded the configured {limit_bytes}-byte output limit")]
    OutputLimitExceeded {
        stream: &'static str,
        limit_bytes: u64,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
}

#[derive(Debug, Clone, Copy)]
enum Stream {
    Stdout,
    Stderr,
}

impl Stream {
    const fn name(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
}

enum PipeEvent {
    Chunk(Stream, Vec<u8>),
    Eof,
    ReadError(Stream, String),
}

pub async fn run_bounded_process(
    mut command: Command,
    limits: ProcessOutputLimits,
    timeout: Duration,
    cancellation: &CancellationToken,
) -> Result<BoundedProcessOutput, ProcessRunError> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(ProcessRunError::Spawn)?;
    let process_id = child.id();
    let stdout = child.stdout.take().ok_or_else(|| ProcessRunError::Read {
        stream: "stdout",
        message: "stdout pipe was unavailable".to_owned(),
    })?;
    let stderr = child.stderr.take().ok_or_else(|| ProcessRunError::Read {
        stream: "stderr",
        message: "stderr pipe was unavailable".to_owned(),
    })?;
    let (sender, mut receiver) = mpsc::channel(PIPE_CHANNEL_CAPACITY);
    let stdout_task = tokio::spawn(read_pipe(stdout, Stream::Stdout, sender.clone()));
    let stderr_task = tokio::spawn(read_pipe(stderr, Stream::Stderr, sender));
    let deadline = Instant::now() + timeout;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut eof_count = 0;

    let outcome = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let event = tokio::select! {
            () = cancellation.cancelled() => break Err(ProcessRunError::Cancelled),
            () = tokio::time::sleep(remaining) => {
                break Err(ProcessRunError::Timeout { seconds: timeout.as_secs() });
            }
            event = receiver.recv() => event,
        };
        match event {
            Some(PipeEvent::Chunk(stream, chunk)) => {
                if let Some((stream, limit_bytes)) =
                    append_bounded(stream, &chunk, &mut stdout, &mut stderr, limits)
                {
                    break Err(ProcessRunError::OutputLimitExceeded {
                        stream,
                        limit_bytes,
                        stdout,
                        stderr,
                    });
                }
            }
            Some(PipeEvent::Eof) => {
                eof_count += 1;
                if eof_count == 2 {
                    break wait_for_child(&mut child, deadline, timeout, cancellation)
                        .await
                        .map(|status| BoundedProcessOutput {
                            status,
                            stdout,
                            stderr,
                        });
                }
            }
            Some(PipeEvent::ReadError(stream, message)) => {
                break Err(ProcessRunError::Read {
                    stream: stream.name(),
                    message,
                });
            }
            None => {
                break Err(ProcessRunError::Read {
                    stream: "output",
                    message: "output readers stopped before both pipes reached EOF".to_owned(),
                });
            }
        }
    };

    if outcome.is_err() {
        terminate_and_reap(&mut child, process_id)
            .await
            .map_err(ProcessRunError::Wait)?;
    }
    finish_readers(stdout_task, stderr_task, outcome.is_err()).await;
    outcome
}

fn append_bounded(
    stream: Stream,
    chunk: &[u8],
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
    limits: ProcessOutputLimits,
) -> Option<(&'static str, u64)> {
    let stream_length = match stream {
        Stream::Stdout => stdout.len() as u64,
        Stream::Stderr => stderr.len() as u64,
    };
    let stream_limit = match stream {
        Stream::Stdout => limits.stdout_bytes,
        Stream::Stderr => limits.stderr_bytes,
    };
    let combined_length = (stdout.len() + stderr.len()) as u64;
    let stream_remaining = stream_limit.saturating_sub(stream_length);
    let combined_remaining = limits.combined_bytes.saturating_sub(combined_length);
    let retained = chunk
        .len()
        .min(stream_remaining as usize)
        .min(combined_remaining as usize);
    match stream {
        Stream::Stdout => stdout.extend_from_slice(&chunk[..retained]),
        Stream::Stderr => stderr.extend_from_slice(&chunk[..retained]),
    }
    if retained == chunk.len() {
        None
    } else if combined_remaining <= stream_remaining {
        Some(("combined output", limits.combined_bytes))
    } else {
        Some((stream.name(), stream_limit))
    }
}

async fn wait_for_child(
    child: &mut Child,
    deadline: Instant,
    timeout: Duration,
    cancellation: &CancellationToken,
) -> Result<ExitStatus, ProcessRunError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    tokio::select! {
        result = child.wait() => result.map_err(ProcessRunError::Wait),
        () = cancellation.cancelled() => Err(ProcessRunError::Cancelled),
        () = tokio::time::sleep(remaining) => {
            Err(ProcessRunError::Timeout { seconds: timeout.as_secs() })
        }
    }
}

async fn terminate_and_reap(child: &mut Child, process_id: Option<u32>) -> io::Result<()> {
    terminate_process_tree(process_id).await;
    if child.try_wait()?.is_none() {
        child.start_kill()?;
        child.wait().await?;
    }
    Ok(())
}

#[cfg(unix)]
async fn terminate_process_tree(process_id: Option<u32>) {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;

    if let Some(process_id) = process_id.and_then(|value| i32::try_from(value).ok()) {
        let _ = killpg(Pid::from_raw(process_id), Signal::SIGKILL);
    }
}

#[cfg(windows)]
async fn terminate_process_tree(process_id: Option<u32>) {
    if let Some(process_id) = process_id {
        let _ = Command::new("taskkill.exe")
            .args(["/PID", &process_id.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
}

#[cfg(not(any(unix, windows)))]
async fn terminate_process_tree(_process_id: Option<u32>) {}

async fn finish_readers(stdout_task: JoinHandle<()>, stderr_task: JoinHandle<()>, abort: bool) {
    if abort {
        stdout_task.abort();
        stderr_task.abort();
    }
    let _ = stdout_task.await;
    let _ = stderr_task.await;
}

async fn read_pipe<R>(mut pipe: R, stream: Stream, sender: mpsc::Sender<PipeEvent>)
where
    R: AsyncRead + Unpin,
{
    let mut buffer = vec![0_u8; PIPE_CHUNK_BYTES];
    loop {
        match pipe.read(&mut buffer).await {
            Ok(0) => {
                let _ = sender.send(PipeEvent::Eof).await;
                return;
            }
            Ok(length) => {
                if sender
                    .send(PipeEvent::Chunk(stream, buffer[..length].to_vec()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Err(error) => {
                let _ = sender
                    .send(PipeEvent::ReadError(stream, error.to_string()))
                    .await;
                return;
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command as StdCommand;

    use tempfile::tempdir;

    fn shell(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    }

    fn limits(stdout_bytes: u64, stderr_bytes: u64, combined_bytes: u64) -> ProcessOutputLimits {
        ProcessOutputLimits {
            stdout_bytes,
            stderr_bytes,
            combined_bytes,
        }
    }

    #[tokio::test]
    async fn caps_stdout_and_reaps_the_child() {
        let directory = tempdir().expect("tempdir");
        let pid_file = directory.path().join("pid");
        let script = format!(
            "echo $$ > '{}'; while :; do printf 1234567890; done",
            pid_file.display()
        );
        let error = run_bounded_process(
            shell(&script),
            limits(64, 64, 128),
            Duration::from_secs(5),
            &CancellationToken::new(),
        )
        .await
        .expect_err("stdout limit");
        assert!(matches!(
            error,
            ProcessRunError::OutputLimitExceeded {
                stream: "stdout",
                limit_bytes: 64,
                ..
            }
        ));
        let pid = fs::read_to_string(pid_file).expect("pid");
        assert!(
            !StdCommand::new("kill")
                .args(["-0", pid.trim()])
                .stderr(Stdio::null())
                .status()
                .expect("probe process")
                .success()
        );
    }

    #[tokio::test]
    async fn caps_stderr() {
        let error = run_bounded_process(
            shell("while :; do printf 1234567890 >&2; done"),
            limits(64, 32, 128),
            Duration::from_secs(5),
            &CancellationToken::new(),
        )
        .await
        .expect_err("stderr limit");
        assert!(matches!(
            error,
            ProcessRunError::OutputLimitExceeded {
                stream: "stderr",
                limit_bytes: 32,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn caps_combined_interleaved_output() {
        let error = run_bounded_process(
            shell("while :; do printf 12345; printf 67890 >&2; done"),
            limits(128, 128, 48),
            Duration::from_secs(5),
            &CancellationToken::new(),
        )
        .await
        .expect_err("combined limit");
        assert!(matches!(
            error,
            ProcessRunError::OutputLimitExceeded {
                stream: "combined output",
                limit_bytes: 48,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn times_out_and_reaps() {
        let directory = tempdir().expect("tempdir");
        let child_pid = directory.path().join("child-pid");
        let script = format!("sleep 10 & echo $! > '{}'; wait", child_pid.display());
        let error = run_bounded_process(
            shell(&script),
            limits(64, 64, 128),
            Duration::from_secs(2),
            &CancellationToken::new(),
        )
        .await
        .expect_err("timeout");
        assert!(matches!(error, ProcessRunError::Timeout { .. }));
        assert_process_gone(&child_pid).await;
    }

    #[tokio::test]
    async fn cancellation_terminates_the_process() {
        let directory = tempdir().expect("tempdir");
        let child_pid = directory.path().join("child-pid");
        let script = format!("sleep 10 & echo $! > '{}'; wait", child_pid.display());
        let cancellation = CancellationToken::new();
        let trigger = cancellation.clone();
        let observed_pid = child_pid.clone();
        tokio::spawn(async move {
            while !observed_pid.exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            trigger.cancel();
        });
        let error = run_bounded_process(
            shell(&script),
            limits(64, 64, 128),
            Duration::from_secs(5),
            &cancellation,
        )
        .await
        .expect_err("cancellation");
        assert!(matches!(error, ProcessRunError::Cancelled));
        assert_process_gone(&child_pid).await;
    }

    async fn assert_process_gone(pid_file: &std::path::Path) {
        let pid = fs::read_to_string(pid_file).expect("child pid");
        for _ in 0..20 {
            if !StdCommand::new("kill")
                .args(["-0", pid.trim()])
                .stderr(Stdio::null())
                .status()
                .expect("probe process")
                .success()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("descendant process {pid} survived termination");
    }

    #[tokio::test]
    async fn returns_normal_parseable_json_output() {
        let output = run_bounded_process(
            shell("printf '{\"ok\":true}'; printf warning >&2"),
            limits(64, 64, 128),
            Duration::from_secs(5),
            &CancellationToken::new(),
        )
        .await
        .expect("output");
        assert!(output.status.success());
        assert_eq!(output.stderr, b"warning");
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
        assert_eq!(value["ok"], true);
    }
}
