use std::collections::BTreeMap;
use std::env;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use agentctl_core::dsl::{ActionDefinition, ContainerRuntime, ProcessIsolation};
use agentctl_core::secret::SecretValue;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

const PIPE_CHUNK_BYTES: usize = 8 * 1024;
const PIPE_CHANNEL_CAPACITY: usize = 8;
const CONTAINER_PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(10);
const CONTAINER_PREFLIGHT_OUTPUT_BYTES: u64 = 64 * 1024;
const CONTAINER_TMPFS_BYTES: u64 = 16 * 1024 * 1024;
const CONTAINER_ENGINE_ENVIRONMENT: &[&str] = &[
    "CONTAINER_CONNECTION",
    "CONTAINER_HOST",
    "DOCKER_CERT_PATH",
    "DOCKER_CONFIG",
    "DOCKER_CONTEXT",
    "DOCKER_HOST",
    "DOCKER_TLS_VERIFY",
    "HOME",
    "PATH",
    "PODMAN_CONNECTIONS_CONF",
    "TEMP",
    "TMP",
    "TMPDIR",
    "XDG_CONFIG_HOME",
    "XDG_RUNTIME_DIR",
];

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
    #[error("failed to write subprocess stdin: {0}")]
    Write(String),
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
    #[error("requested process isolation is unavailable: {0}")]
    IsolationUnavailable(String),
    #[error("failed to clean up an isolated container after {original}: {cleanup}")]
    ContainerCleanup { original: String, cleanup: String },
}

impl ProcessRunError {
    pub(crate) fn clear_captured_output(&mut self) {
        if let Self::OutputLimitExceeded { stdout, stderr, .. } = self {
            stdout.fill(0);
            stderr.fill(0);
        }
    }
}

#[derive(Debug, Clone)]
pub enum PreparedProcessIsolation {
    Process,
    Container(ContainerBackend),
}

impl PreparedProcessIsolation {
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Process => "process",
            Self::Container(_) => "container",
        }
    }

    pub const fn backend_name(&self) -> Option<&'static str> {
        match self {
            Self::Process => None,
            Self::Container(backend) => Some(backend.name()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ContainerBackend {
    executable: PathBuf,
    name: &'static str,
}

impl ContainerBackend {
    pub const fn name(&self) -> &'static str {
        self.name
    }
}

pub async fn prepare_process_isolation(
    action: &ActionDefinition,
    cancellation: &CancellationToken,
) -> Result<PreparedProcessIsolation, ProcessRunError> {
    if action.isolation == ProcessIsolation::Process {
        return Ok(PreparedProcessIsolation::Process);
    }
    let container = action.container.as_ref().ok_or_else(|| {
        ProcessRunError::IsolationUnavailable(
            "container mode has no container configuration".to_owned(),
        )
    })?;
    let candidates = container_runtime_candidates(container.runtime);
    if candidates.is_empty() {
        return Err(ProcessRunError::IsolationUnavailable(format!(
            "{} container runtime was not found on {}",
            container_runtime_label(container.runtime),
            env::consts::OS,
        )));
    }

    let mut failures = Vec::new();
    for backend in candidates {
        let mut command = Command::new(&backend.executable);
        configure_container_engine_environment(&mut command);
        command.args(["image", "inspect", &container.image]);
        match run_bounded_process(
            command,
            ProcessOutputLimits {
                stdout_bytes: CONTAINER_PREFLIGHT_OUTPUT_BYTES,
                stderr_bytes: CONTAINER_PREFLIGHT_OUTPUT_BYTES,
                combined_bytes: CONTAINER_PREFLIGHT_OUTPUT_BYTES,
            },
            CONTAINER_PREFLIGHT_TIMEOUT,
            cancellation,
        )
        .await
        {
            Ok(output) if output.status.success() => {
                return Ok(PreparedProcessIsolation::Container(backend));
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                failures.push(format!(
                    "{} could not inspect the pinned image ({}): {}",
                    backend.name,
                    output.status,
                    bounded_diagnostic(&stderr),
                ));
            }
            Err(ProcessRunError::Cancelled) => return Err(ProcessRunError::Cancelled),
            Err(error) => failures.push(format!("{} preflight failed: {error}", backend.name)),
        }
    }

    Err(ProcessRunError::IsolationUnavailable(format!(
        "{} on {}: {}",
        container_runtime_label(container.runtime),
        env::consts::OS,
        failures.join("; "),
    )))
}

pub fn isolated_process_command(
    action: &ActionDefinition,
    isolation: &PreparedProcessIsolation,
    executable: &str,
    arguments: &[String],
    cwd: &Path,
    environment: &BTreeMap<String, SecretValue>,
    container_name: &str,
) -> Command {
    match isolation {
        PreparedProcessIsolation::Process => {
            let mut command = Command::new(executable);
            command
                .args(arguments)
                .current_dir(cwd)
                .env_clear()
                .kill_on_drop(true);
            for (name, value) in environment {
                command.env(name, value.expose());
            }
            command
        }
        PreparedProcessIsolation::Container(backend) => {
            let container = action
                .container
                .as_ref()
                .expect("validated container isolation configuration");
            let mut command = Command::new(&backend.executable);
            configure_container_engine_environment(&mut command);
            for (name, value) in environment {
                command.env(name, value.expose());
            }
            let cpu_limit = format!(
                "{}.{:03}",
                container.cpu_limit_millis / 1000,
                container.cpu_limit_millis % 1000
            );
            command
                .arg("run")
                .arg("--rm")
                .arg("--interactive")
                .arg("--pull=never")
                .arg("--network=none")
                .arg("--read-only")
                .arg("--user=65532:65532")
                .arg("--cap-drop=ALL")
                .arg("--security-opt=no-new-privileges")
                .arg(format!("--memory={}b", container.memory_limit_bytes))
                .arg(format!("--cpus={cpu_limit}"))
                .arg(format!("--pids-limit={}", container.pids_limit))
                .arg(format!(
                    "--tmpfs=/tmp:rw,noexec,nosuid,nodev,size={CONTAINER_TMPFS_BYTES}"
                ))
                .arg("--volume")
                .arg(format!("{}:/workspace:ro", cwd.display()))
                .arg("--workdir=/workspace")
                .arg(format!("--name={container_name}"))
                .arg(format!("--entrypoint={executable}"));
            for name in environment.keys() {
                command.arg("--env").arg(name);
            }
            command
                .arg(&container.image)
                .args(arguments)
                .current_dir(cwd)
                .kill_on_drop(true);
            command
        }
    }
}

pub async fn run_isolated_process(
    isolation: &PreparedProcessIsolation,
    container_name: &str,
    command: Command,
    input: Option<Vec<u8>>,
    limits: ProcessOutputLimits,
    timeout: Duration,
    cancellation: &CancellationToken,
) -> Result<BoundedProcessOutput, ProcessRunError> {
    let result = run_bounded_process_inner(command, input, limits, timeout, cancellation).await;
    if result.is_err()
        && let PreparedProcessIsolation::Container(backend) = isolation
        && let Err(cleanup) = cleanup_container(backend, container_name).await
    {
        return Err(ProcessRunError::ContainerCleanup {
            original: result
                .as_ref()
                .expect_err("container result is an error")
                .to_string(),
            cleanup,
        });
    }
    result
}

pub fn container_invocation_name(effect_id: &str, phase: &str) -> String {
    let suffix = effect_id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(32)
        .collect::<String>();
    format!("agentctl-{phase}-{suffix}")
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
    InputError(String),
}

pub async fn run_bounded_process(
    command: Command,
    limits: ProcessOutputLimits,
    timeout: Duration,
    cancellation: &CancellationToken,
) -> Result<BoundedProcessOutput, ProcessRunError> {
    run_bounded_process_inner(command, None, limits, timeout, cancellation).await
}

async fn run_bounded_process_inner(
    mut command: Command,
    input: Option<Vec<u8>>,
    limits: ProcessOutputLimits,
    timeout: Duration,
    cancellation: &CancellationToken,
) -> Result<BoundedProcessOutput, ProcessRunError> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    if input.is_some() {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }
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
    let stderr_task = tokio::spawn(read_pipe(stderr, Stream::Stderr, sender.clone()));
    if let Some(input) = input {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| ProcessRunError::Write("stdin pipe was unavailable".to_owned()))?;
        tokio::spawn(async move {
            let result = async {
                stdin.write_all(&input).await?;
                stdin.shutdown().await
            }
            .await;
            if let Err(error) = result {
                let _ = sender.send(PipeEvent::InputError(error.to_string())).await;
            }
        });
    }
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
            Some(PipeEvent::InputError(message)) => {
                break Err(ProcessRunError::Write(message));
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

fn container_runtime_candidates(runtime: ContainerRuntime) -> Vec<ContainerBackend> {
    let names: &[(&str, &'static str)] = match runtime {
        ContainerRuntime::Auto => &[("docker", "docker"), ("podman", "podman")],
        ContainerRuntime::Docker => &[("docker", "docker")],
        ContainerRuntime::Podman => &[("podman", "podman")],
    };
    let mut candidates = names
        .iter()
        .filter_map(|(executable, name)| {
            executable_on_path(executable).map(|executable| ContainerBackend { executable, name })
        })
        .collect::<Vec<_>>();
    if matches!(runtime, ContainerRuntime::Auto | ContainerRuntime::Podman) {
        let fallback = PathBuf::from("/opt/podman/bin/podman");
        if fallback.is_file()
            && !candidates
                .iter()
                .any(|candidate| candidate.executable == fallback)
        {
            candidates.push(ContainerBackend {
                executable: fallback,
                name: "podman",
            });
        }
    }
    candidates
}

fn executable_on_path(name: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    for directory in env::split_paths(&paths) {
        let path = directory.join(name);
        if path.is_file() {
            return Some(path);
        }
        #[cfg(windows)]
        {
            let executable = directory.join(format!("{name}.exe"));
            if executable.is_file() {
                return Some(executable);
            }
        }
    }
    None
}

const fn container_runtime_label(runtime: ContainerRuntime) -> &'static str {
    match runtime {
        ContainerRuntime::Auto => "Docker or Podman",
        ContainerRuntime::Docker => "Docker",
        ContainerRuntime::Podman => "Podman",
    }
}

fn configure_container_engine_environment(command: &mut Command) {
    command.env_clear().kill_on_drop(true);
    for name in CONTAINER_ENGINE_ENVIRONMENT {
        if let Some(value) = env::var_os(name) {
            command.env(name, value);
        }
    }
}

fn bounded_diagnostic(value: &str) -> String {
    let value = value.trim().replace(['\r', '\n'], " ");
    let prefix = value.chars().take(512).collect::<String>();
    if prefix.is_empty() {
        "no diagnostic output".to_owned()
    } else if prefix.len() == value.len() {
        prefix
    } else {
        format!("{prefix}...")
    }
}

async fn cleanup_container(backend: &ContainerBackend, container_name: &str) -> Result<(), String> {
    let mut command = Command::new(&backend.executable);
    configure_container_engine_environment(&mut command);
    command.args(["rm", "--force", container_name]);
    let cancellation = CancellationToken::new();
    match run_bounded_process(
        command,
        ProcessOutputLimits {
            stdout_bytes: CONTAINER_PREFLIGHT_OUTPUT_BYTES,
            stderr_bytes: CONTAINER_PREFLIGHT_OUTPUT_BYTES,
            combined_bytes: CONTAINER_PREFLIGHT_OUTPUT_BYTES,
        },
        CONTAINER_PREFLIGHT_TIMEOUT,
        &cancellation,
    )
    .await
    {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => {
            let diagnostic = bounded_diagnostic(&String::from_utf8_lossy(&output.stderr));
            let normalized = diagnostic.to_ascii_lowercase();
            if normalized.contains("no such container")
                || normalized.contains("no container with name or id")
            {
                Ok(())
            } else {
                Err(format!(
                    "{} cleanup exited with {}: {diagnostic}",
                    backend.name, output.status
                ))
            }
        }
        Err(error) => Err(format!("{} cleanup failed: {error}", backend.name)),
    }
}

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
    use std::os::unix::fs::PermissionsExt as _;
    use std::process::Command as StdCommand;

    use agentctl_core::dsl::parse_workflow;
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

    fn container_action() -> ActionDefinition {
        let source = format!(
            r#"
apiVersion: agentctl.dev/v1
kind: Workflow
metadata: {{ name: container-command }}
spec:
  actions:
    fixture:
      kind: builtin.shell.exec
      command: /bin/fixture
      isolation: container
      container:
        image: example.invalid/fixture@sha256:{}
        runtime: podman
        memoryLimitBytes: 33554432
        cpuLimitMillis: 500
        pidsLimit: 16
  tasks:
    - {{ id: fixture, uses: "action:fixture" }}
"#,
            "0".repeat(64)
        );
        parse_workflow(&source, "container.yaml")
            .expect("container action")
            .workflow
            .spec
            .actions
            .remove("fixture")
            .expect("fixture action")
    }

    #[test]
    fn container_command_is_read_only_networkless_non_root_and_resource_bounded() {
        let action = container_action();
        let isolation = PreparedProcessIsolation::Container(ContainerBackend {
            executable: PathBuf::from("/usr/bin/podman"),
            name: "podman",
        });
        let mut environment = BTreeMap::new();
        environment.insert("FIXTURE_TOKEN".to_owned(), SecretValue::from("protected"));
        let command = isolated_process_command(
            &action,
            &isolation,
            "/bin/fixture",
            &["run".to_owned()],
            Path::new("/tmp/workspace"),
            &environment,
            "agentctl-fixture",
        );
        let command = command.as_std();
        let arguments = command
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        for required in [
            "--rm",
            "--interactive",
            "--pull=never",
            "--network=none",
            "--read-only",
            "--user=65532:65532",
            "--cap-drop=ALL",
            "--security-opt=no-new-privileges",
            "--memory=33554432b",
            "--cpus=0.500",
            "--pids-limit=16",
            "--workdir=/workspace",
            "--name=agentctl-fixture",
            "--entrypoint=/bin/fixture",
        ] {
            assert!(arguments.iter().any(|argument| argument == required));
        }
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--env", "FIXTURE_TOKEN"])
        );
        assert!(
            arguments
                .iter()
                .all(|argument| !argument.contains("protected"))
        );
        assert_eq!(command.get_current_dir(), Some(Path::new("/tmp/workspace")));
    }

    #[tokio::test]
    async fn abnormal_container_exit_forces_named_cleanup() {
        let directory = tempdir().expect("tempdir");
        let engine = directory.path().join("fake-engine");
        let cleanup = directory.path().join("cleanup");
        fs::write(
            &engine,
            format!(
                "#!/bin/sh\nif [ \"$1\" = run ]; then sleep 10; else printf '%s' \"$*\" > '{}'; fi\n",
                cleanup.display()
            ),
        )
        .expect("engine");
        fs::set_permissions(&engine, fs::Permissions::from_mode(0o755)).expect("permissions");
        let action = container_action();
        let isolation = PreparedProcessIsolation::Container(ContainerBackend {
            executable: engine,
            name: "podman",
        });
        let name = "agentctl-cleanup-fixture";
        let command = isolated_process_command(
            &action,
            &isolation,
            "/bin/fixture",
            &[],
            directory.path(),
            &BTreeMap::new(),
            name,
        );
        let error = run_isolated_process(
            &isolation,
            name,
            command,
            None,
            limits(64, 64, 128),
            Duration::from_millis(100),
            &CancellationToken::new(),
        )
        .await
        .expect_err("timeout");
        assert!(matches!(error, ProcessRunError::Timeout { .. }));
        assert_eq!(
            fs::read_to_string(cleanup).expect("cleanup invocation"),
            format!("rm --force {name}")
        );
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
            Duration::from_secs(5),
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
