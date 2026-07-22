use std::env;
use std::io::Read;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc::{self, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

const STREAM_LIMIT_BYTES: usize = 4 * 1024 * 1024;
const COMBINED_LIMIT_BYTES: usize = 8 * 1024 * 1024;
const DIAGNOSTIC_BYTES: usize = 4 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const PIPE_CHUNK_BYTES: usize = 8 * 1024;

enum Event {
    Chunk(Stream, Vec<u8>),
    Eof,
    ReadError(Stream, String),
}

#[derive(Clone, Copy)]
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

pub(crate) fn bounded_output(mut command: Command, label: &str) -> Result<Output> {
    configure_piped_command(&mut command);
    let child = command.spawn().with_context(|| format!("spawn {label}"))?;
    bounded_wait(child, label)
}

pub(crate) fn output_diagnostics(output: &Output) -> String {
    diagnostics(&output.stdout, &output.stderr)
}

pub(crate) fn bounded_wait(mut child: Child, label: &str) -> Result<Output> {
    let process_id = child.id();
    let stdout = child
        .stdout
        .take()
        .with_context(|| format!("{label} stdout was not piped"))?;
    let stderr = child
        .stderr
        .take()
        .with_context(|| format!("{label} stderr was not piped"))?;
    let (sender, receiver) = mpsc::sync_channel(8);
    let stdout_thread = read_pipe(stdout, Stream::Stdout, sender.clone());
    let stderr_thread = read_pipe(stderr, Stream::Stderr, sender);
    let started = Instant::now();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut status = None;
    let mut eof_count = 0;

    loop {
        if status.is_none() {
            status = child.try_wait().with_context(|| format!("poll {label}"))?;
        }
        if status.is_some() && eof_count == 2 {
            join_readers(stdout_thread, stderr_thread, label)?;
            return Ok(Output {
                status: status.expect("status checked"),
                stdout,
                stderr,
            });
        }
        if started.elapsed() >= COMMAND_TIMEOUT {
            terminate(&mut child, process_id);
            drop(receiver);
            join_readers(stdout_thread, stderr_thread, label)?;
            bail!(
                "{label} timed out after {} seconds",
                COMMAND_TIMEOUT.as_secs()
            );
        }
        match receiver.recv_timeout(Duration::from_millis(25)) {
            Ok(Event::Chunk(stream, chunk)) => {
                if let Some((stream, limit)) =
                    append_bounded(stream, &chunk, &mut stdout, &mut stderr)
                {
                    terminate(&mut child, process_id);
                    drop(receiver);
                    join_readers(stdout_thread, stderr_thread, label)?;
                    bail!(
                        "{label} exceeded the {limit}-byte {stream} capture limit\n{}",
                        diagnostics(&stdout, &stderr)
                    );
                }
            }
            Ok(Event::Eof) => eof_count += 1,
            Ok(Event::ReadError(stream, message)) => {
                terminate(&mut child, process_id);
                drop(receiver);
                join_readers(stdout_thread, stderr_thread, label)?;
                bail!("failed to read {label} {}: {message}", stream.name());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if eof_count == 2 {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                }
                terminate(&mut child, process_id);
                drop(receiver);
                join_readers(stdout_thread, stderr_thread, label)?;
                bail!("{label} output readers stopped before both streams reached EOF");
            }
        }
    }
}

fn append_bounded(
    stream: Stream,
    chunk: &[u8],
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
) -> Option<(&'static str, usize)> {
    let stream_length = match stream {
        Stream::Stdout => stdout.len(),
        Stream::Stderr => stderr.len(),
    };
    let combined_length = stdout.len() + stderr.len();
    let stream_remaining = STREAM_LIMIT_BYTES.saturating_sub(stream_length);
    let combined_remaining = COMBINED_LIMIT_BYTES.saturating_sub(combined_length);
    let retained = chunk.len().min(stream_remaining).min(combined_remaining);
    match stream {
        Stream::Stdout => stdout.extend_from_slice(&chunk[..retained]),
        Stream::Stderr => stderr.extend_from_slice(&chunk[..retained]),
    }
    if retained == chunk.len() {
        None
    } else if combined_remaining <= stream_remaining {
        Some(("combined output", COMBINED_LIMIT_BYTES))
    } else {
        Some((stream.name(), STREAM_LIMIT_BYTES))
    }
}

fn read_pipe<R>(mut pipe: R, stream: Stream, sender: SyncSender<Event>) -> JoinHandle<()>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut buffer = vec![0_u8; PIPE_CHUNK_BYTES];
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) => {
                    let _ = sender.send(Event::Eof);
                    return;
                }
                Ok(length) => {
                    if sender
                        .send(Event::Chunk(stream, buffer[..length].to_vec()))
                        .is_err()
                    {
                        return;
                    }
                }
                Err(error) => {
                    let _ = sender.send(Event::ReadError(stream, error.to_string()));
                    return;
                }
            }
        }
    })
}

pub(crate) fn configure_piped_command(command: &mut Command) {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
}

fn terminate(child: &mut Child, process_id: u32) {
    terminate_process_tree(process_id);
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[cfg(unix)]
fn terminate_process_tree(process_id: u32) {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;

    if let Ok(process_id) = i32::try_from(process_id) {
        let _ = killpg(Pid::from_raw(process_id), Signal::SIGKILL);
    }
}

#[cfg(windows)]
fn terminate_process_tree(process_id: u32) {
    let _ = Command::new("taskkill.exe")
        .args(["/PID", &process_id.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(not(any(unix, windows)))]
fn terminate_process_tree(_process_id: u32) {}

fn join_readers(stdout: JoinHandle<()>, stderr: JoinHandle<()>, label: &str) -> Result<()> {
    stdout
        .join()
        .map_err(|_| anyhow::anyhow!("{label} stdout reader panicked"))?;
    stderr
        .join()
        .map_err(|_| anyhow::anyhow!("{label} stderr reader panicked"))?;
    Ok(())
}

fn diagnostics(stdout: &[u8], stderr: &[u8]) -> String {
    if provider_secret_is_present() {
        return "subprocess diagnostics omitted because provider credentials are present in the environment"
            .to_owned();
    }
    format!(
        "stdout prefix: {}\nstderr prefix: {}",
        String::from_utf8_lossy(&stdout[..stdout.len().min(DIAGNOSTIC_BYTES)]),
        String::from_utf8_lossy(&stderr[..stderr.len().min(DIAGNOSTIC_BYTES)])
    )
}

fn provider_secret_is_present() -> bool {
    [
        "OPENAI_API_KEY",
        "AZURE_OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "GOOGLE_API_KEY",
        "GEMINI_API_KEY",
    ]
    .iter()
    .any(|name| env::var_os(name).is_some())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn captures_normal_output() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf ok; printf warning >&2"]);
        let output = bounded_output(command, "fixture").expect("output");
        assert!(output.status.success());
        assert_eq!(output.stdout, b"ok");
        assert_eq!(output.stderr, b"warning");
    }

    #[test]
    fn rejects_unbounded_helper_output() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "while :; do printf 1234567890; done"]);
        let error = bounded_output(command, "fixture").expect_err("limit");
        assert!(error.to_string().contains("stdout capture limit"));
    }
}
