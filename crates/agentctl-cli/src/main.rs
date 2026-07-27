mod packs;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agentctl_core::compiler::provider_capabilities;
use agentctl_core::diagnostic::{Diagnostic, DiagnosticCode, Severity};
use agentctl_core::dsl::{ProviderKind, SecretReference, Workflow, parse_workflow, schema_json};
use agentctl_core::pack::{PackManifest, verify_pack};
use agentctl_core::policy::PolicyEngine;
use agentctl_core::provider::{ContentBlock, Message, ModelProvider, ProviderRequest};
use agentctl_core::secret::SecretValue;
use agentctl_core::{MACHINE_OUTPUT_VERSION, compile};
use agentctl_protocols::{A2aClient, McpClient, ProtocolActionHandler, ProtocolHttpConfig};
use agentctl_providers::{
    AnthropicProvider, FakeProvider, GoogleProvider, HttpProviderConfig, OpenAiProvider,
};
use agentctl_runtime::secret::{SecretResolutionError, SecretResolver};
use agentctl_runtime::{
    BuiltinToolExecutor, EffectReconciliationInput, RunOptions, Runtime, RuntimeRegistry,
    StreamEventSink,
};
use agentctl_store::{
    ApprovalResolution, ReconciliationStatus, RunMode, SqliteStore, StoreError, StreamEventRecord,
    TaskDisposition,
};
use chrono::{Duration as ChronoDuration, Utc};
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use serde::Serialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use url::Url;

const EXIT_OK: u8 = 0;
const EXIT_VALIDATION: u8 = 2;
const EXIT_POLICY: u8 = 3;
const EXIT_RUN_FAILED: u8 = 4;
const EXIT_PERSISTENCE: u8 = 5;
const EXIT_REMOTE: u8 = 6;
const EXIT_CANCELLED: u8 = 130;
const MAX_TEXT_FILE_BYTES: u64 = 1024 * 1024;
static VERBOSE_OUTPUT: AtomicBool = AtomicBool::new(false);
static COLOR_OUTPUT: AtomicBool = AtomicBool::new(false);
static STREAM_OUTPUT_LOCK: Mutex<()> = Mutex::new(());
static PACK_OFFLINE: AtomicBool = AtomicBool::new(false);
static PACK_LOCKED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Parser)]
#[command(
    name = "agentctl",
    version,
    about = "Deterministic, declarative control plane for policy-constrained agentic automation",
    disable_help_subcommand = true
)]
struct Cli {
    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Human)]
    output: OutputFormat,
    #[arg(long, global = true, value_enum, default_value_t = ColorMode::Auto)]
    color: ColorMode,
    #[arg(long, global = true)]
    verbose: bool,
    /// Forbid pack network access and require cached Git/archive sources.
    #[arg(long, global = true)]
    offline: bool,
    /// Require agentctl.pack.lock and reject all source or graph drift.
    #[arg(long, global = true)]
    locked: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
enum OutputFormat {
    Human,
    Json,
    Jsonl,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ColorMode {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Validate syntax, schema, references, capabilities, policy, and templates.
    Check(WorkflowFile),
    /// Print the deterministic compiled plan.
    Plan(WorkflowFile),
    /// Execute a workflow, or predict it with --check.
    Run(RunArgs),
    /// Continue an interrupted or approval-paused run.
    Resume(ResumeArgs),
    /// Reconstruct a terminal run only from recorded state and results.
    Replay(RunIdArgs),
    /// Create a new run from a prior workflow with fresh effects.
    Fork(ForkArgs),
    /// Create a new run that reuses compatible upstream results and executes a repaired suffix.
    Repair(RepairArgs),
    /// Retry failed or selected boundaries of an identical terminal workflow.
    Retry(RetryArgs),
    /// Execute explicitly declared best-effort compensation for a terminal run.
    Compensate(CompensateArgs),
    /// Analyze or upgrade retained legacy run records for selective reuse.
    Runs(RunsArgs),
    /// Durably request cancellation.
    Cancel(RunIdArgs),
    /// Inspect durable run, task, and audit state.
    Inspect(RunIdArgs),
    /// Inspect or narrowly reconcile uncertain effects.
    Effects(EffectArgs),
    /// List or resolve durable approval requests.
    Approvals(ApprovalArgs),
    /// Inspect provider capabilities or run the opt-in OpenAI smoke.
    Providers(ProviderArgs),
    /// Check configured secret references without revealing values.
    Auth(AuthArgs),
    /// Print or write the generated workflow JSON Schema.
    Schema(SchemaArgs),
    /// Translate an unversioned TypeScript-era workflow into v1alpha1.
    Migrate(MigrateArgs),
    /// Inspect and verify a local reusable pack.
    Packs(PackArgs),
    /// Inspect, verify, export, or collect durable artifacts.
    Artifacts(ArtifactArgs),
    /// Inspect the runtime database.
    Db(DbArgs),
    /// Read or write namespaced long-term memory.
    Memory(MemoryArgs),
    /// Garbage-collect expired memory and old terminal runs.
    Gc(GcArgs),
    /// Generate completion for a supported shell.
    Completion(CompletionArgs),
    /// Print the exact build version.
    Version,
    /// Explain safe update options without modifying the installation.
    Update,
}

#[derive(Debug, Args)]
struct WorkflowFile {
    file: PathBuf,
}

#[derive(Debug, Args)]
struct RunArgs {
    file: PathBuf,
    #[arg(long, default_value = ".agentctl/runtime.db")]
    db: PathBuf,
    #[arg(long)]
    inputs: Option<String>,
    #[arg(long, conflicts_with = "inputs")]
    inputs_file: Option<PathBuf>,
    #[arg(long = "input", value_name = "KEY=VALUE")]
    input: Vec<String>,
    #[arg(long)]
    workspace: Option<PathBuf>,
    #[arg(long)]
    timeout_seconds: Option<u64>,
    #[arg(long)]
    check: bool,
    #[arg(long)]
    diff: bool,
    #[arg(long)]
    interactive: bool,
}

#[derive(Debug, Args)]
struct ResumeArgs {
    run_id: String,
    #[arg(long, default_value = ".agentctl/runtime.db")]
    db: PathBuf,
    #[arg(long)]
    diff: bool,
    #[arg(long)]
    interactive: bool,
    #[arg(long)]
    workspace: Option<PathBuf>,
    #[arg(long)]
    timeout_seconds: Option<u64>,
}

#[derive(Debug, Args)]
struct RunIdArgs {
    run_id: String,
    #[arg(long, default_value = ".agentctl/runtime.db")]
    db: PathBuf,
}

#[derive(Debug, Args)]
struct ForkArgs {
    run_id: String,
    #[arg(long, default_value = ".agentctl/runtime.db")]
    db: PathBuf,
    #[arg(long)]
    interactive: bool,
    #[arg(long)]
    diff: bool,
    #[arg(long)]
    workspace: Option<PathBuf>,
    #[arg(long)]
    timeout_seconds: Option<u64>,
}

#[derive(Debug, Args)]
struct RepairArgs {
    file: PathBuf,
    source_run_id: String,
    #[arg(long = "from", required = true)]
    from: Vec<String>,
    #[arg(long)]
    plan: bool,
    #[arg(long)]
    restart_successful: bool,
    #[arg(long)]
    reason: Option<String>,
    #[arg(long, default_value = ".agentctl/runtime.db")]
    db: PathBuf,
    #[arg(long)]
    interactive: bool,
    #[arg(long)]
    diff: bool,
    #[arg(long)]
    workspace: Option<PathBuf>,
    #[arg(long)]
    timeout_seconds: Option<u64>,
}

#[derive(Debug, Args)]
struct RetryArgs {
    file: PathBuf,
    source_run_id: String,
    #[arg(long, conflicts_with = "from", required_unless_present = "from")]
    failed: bool,
    #[arg(
        long = "from",
        conflicts_with = "failed",
        required_unless_present = "failed"
    )]
    from: Vec<String>,
    #[arg(long)]
    plan: bool,
    #[arg(long)]
    restart_successful: bool,
    #[arg(long)]
    reason: Option<String>,
    #[arg(long, default_value = ".agentctl/runtime.db")]
    db: PathBuf,
    #[arg(long)]
    interactive: bool,
    #[arg(long)]
    diff: bool,
    #[arg(long)]
    workspace: Option<PathBuf>,
    #[arg(long)]
    timeout_seconds: Option<u64>,
}

#[derive(Debug, Args)]
struct CompensateArgs {
    source_run_id: String,
    #[arg(long = "task")]
    task: Vec<String>,
    #[arg(long)]
    plan: bool,
    #[arg(long, default_value = ".agentctl/runtime.db")]
    db: PathBuf,
    #[arg(long)]
    interactive: bool,
    #[arg(long)]
    diff: bool,
    #[arg(long)]
    workspace: Option<PathBuf>,
    #[arg(long)]
    timeout_seconds: Option<u64>,
}

#[derive(Debug, Args)]
struct RunsArgs {
    #[arg(long, default_value = ".agentctl/runtime.db")]
    db: PathBuf,
    #[command(subcommand)]
    command: RunsCommand,
}

#[derive(Debug, Subcommand)]
enum RunsCommand {
    /// Prove reusable legacy metadata without changing the source run.
    Analyze { run_id: String },
    /// Transactionally persist every legacy field that can be proven.
    Upgrade {
        run_id: String,
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Args)]
struct ApprovalArgs {
    #[arg(long, default_value = ".agentctl/runtime.db")]
    db: PathBuf,
    #[command(subcommand)]
    command: ApprovalCommand,
}

#[derive(Debug, Args)]
struct EffectArgs {
    #[arg(long, default_value = ".agentctl/runtime.db")]
    db: PathBuf,
    #[command(subcommand)]
    command: EffectCommand,
}

#[derive(Debug, Subcommand)]
enum EffectCommand {
    List {
        run_id: String,
        #[arg(long)]
        task: Option<String>,
    },
    Inspect {
        effect_id: String,
    },
    /// Resume observation of a persisted remote task without resubmitting it.
    ContinueRemote {
        effect_id: String,
        #[arg(long, default_value = "cli-user")]
        actor: String,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        approved: bool,
        #[arg(long)]
        timeout_seconds: Option<u64>,
    },
    Reconcile {
        effect_id: String,
        #[arg(long, value_enum)]
        status: ReconciledOutcome,
        #[arg(long, default_value = "cli-user")]
        actor: String,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        evidence_file: Option<PathBuf>,
        #[arg(long)]
        result_file: Option<PathBuf>,
        #[arg(long)]
        result_schema_file: Option<PathBuf>,
        #[arg(long)]
        compensation_effect: Option<String>,
        #[arg(long)]
        approved: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ReconciledOutcome {
    Applied,
    NotApplied,
    Compensated,
}

#[derive(Debug, Subcommand)]
enum ApprovalCommand {
    List { run_id: String },
    Approve(ResolutionArgs),
    Reject(ResolutionArgs),
}

#[derive(Debug, Args)]
struct ResolutionArgs {
    approval_id: String,
    #[arg(long, default_value = "cli-user")]
    actor: String,
    #[arg(long)]
    reason: String,
}

#[derive(Debug, Args)]
struct ProviderArgs {
    #[command(subcommand)]
    command: ProviderCommand,
}

#[derive(Debug, Subcommand)]
enum ProviderCommand {
    Inspect(WorkflowFile),
    SmokeOpenai {
        /// Required acknowledgement that this performs one bounded live request.
        #[arg(long, required = true)]
        live: bool,
        #[arg(long, default_value = "gpt-5.6")]
        model: String,
    },
}

#[derive(Debug, Args)]
struct AuthArgs {
    #[command(subcommand)]
    command: AuthCommand,
}

#[derive(Debug, Subcommand)]
enum AuthCommand {
    Check(WorkflowFile),
}

#[derive(Debug, Args)]
struct SchemaArgs {
    #[arg(long)]
    write: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct MigrateArgs {
    file: PathBuf,
    #[arg(long)]
    write: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct PackArgs {
    #[command(subcommand)]
    command: PackCommand,
}

#[derive(Debug, Subcommand)]
enum PackCommand {
    Inspect {
        manifest: PathBuf,
    },
    Verify {
        manifest: PathBuf,
        #[arg(long)]
        integrity: String,
    },
    /// Resolve the complete graph and write agentctl.pack.lock.
    Lock {
        workflow: PathBuf,
    },
    /// Refresh the locked graph from immutable sources.
    Update {
        workflow: PathBuf,
        #[arg(long)]
        pack: Option<String>,
    },
    /// Verify a lockfile, source digests, signatures, and trust policy.
    VerifyLock {
        workflow: PathBuf,
    },
}

#[derive(Debug, Args)]
struct ArtifactArgs {
    #[arg(long, default_value = ".agentctl/runtime.db")]
    db: PathBuf,
    #[command(subcommand)]
    command: ArtifactCommand,
}

#[derive(Debug, Subcommand)]
enum ArtifactCommand {
    List {
        #[arg(long)]
        run: Option<String>,
        #[arg(long, requires = "run")]
        task: Option<String>,
    },
    Inspect {
        digest: String,
    },
    Verify {
        digest: Option<String>,
        #[arg(long, conflicts_with = "digest")]
        all: bool,
    },
    Export {
        digest: String,
        destination: PathBuf,
        #[arg(long)]
        overwrite: bool,
    },
    Gc {
        #[arg(long, default_value_t = 30)]
        older_than_days: i64,
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Args)]
struct DbArgs {
    #[arg(long, default_value = ".agentctl/runtime.db")]
    db: PathBuf,
    #[command(subcommand)]
    command: DbCommand,
}

#[derive(Debug, Subcommand)]
enum DbCommand {
    Stats,
    Migrate,
    Encryption {
        #[command(subcommand)]
        command: DbEncryptionCommand,
    },
}

#[derive(Debug, Subcommand)]
enum DbEncryptionCommand {
    /// Inventory protected fields without exposing their values.
    Inventory,
    /// Transactionally encrypt every identified sensitive field.
    Enable {
        #[arg(long)]
        key_id: String,
        /// Environment variable containing a base64-encoded 32-byte key.
        #[arg(long)]
        key_env: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Transactionally decrypt and re-encrypt every protected field with a new key.
    Rotate {
        #[arg(long)]
        key_id: String,
        /// Environment variable containing a base64-encoded 32-byte key.
        #[arg(long)]
        key_env: String,
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Args)]
struct MemoryArgs {
    #[arg(long, default_value = ".agentctl/runtime.db")]
    db: PathBuf,
    #[command(subcommand)]
    command: MemoryCommand,
}

#[derive(Debug, Subcommand)]
enum MemoryCommand {
    Get {
        namespace: String,
        key: String,
    },
    Put {
        namespace: String,
        key: String,
        value: String,
        #[arg(long)]
        retention_days: Option<i64>,
    },
}

#[derive(Debug, Args)]
struct GcArgs {
    #[arg(long, default_value = ".agentctl/runtime.db")]
    db: PathBuf,
    #[arg(long, default_value_t = 30)]
    older_than_days: i64,
}

#[derive(Debug, Args)]
struct CompletionArgs {
    #[arg(value_enum)]
    shell: Shell,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Envelope<T: Serialize> {
    api_version: &'static str,
    kind: &'static str,
    ok: bool,
    data: T,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Debug)]
struct CliError {
    code: u8,
    message: String,
    diagnostics: Vec<Diagnostic>,
    run_id: Option<String>,
    trace_id: Option<String>,
}

impl CliError {
    fn validation(message: impl Into<String>) -> Self {
        Self {
            code: EXIT_VALIDATION,
            message: message.into(),
            diagnostics: Vec::new(),
            run_id: None,
            trace_id: None,
        }
    }

    fn persistence(error: impl ToString) -> Self {
        Self {
            code: EXIT_PERSISTENCE,
            message: error.to_string(),
            diagnostics: Vec::new(),
            run_id: None,
            trace_id: None,
        }
    }
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args_os().collect::<Vec<_>>();
    normalize_binary_name(&mut args);
    let requested_output = requested_output(&args);
    let cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(error)
            if requested_output != OutputFormat::Human
                && !matches!(
                    error.kind(),
                    clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
                ) =>
        {
            let error = CliError::validation(error.to_string());
            render_error(requested_output, &error);
            std::process::exit(i32::from(error.code));
        }
        Err(error) => error.exit(),
    };
    let output = cli.output;
    VERBOSE_OUTPUT.store(cli.verbose, Ordering::Relaxed);
    PACK_OFFLINE.store(cli.offline, Ordering::Relaxed);
    PACK_LOCKED.store(cli.locked, Ordering::Relaxed);
    COLOR_OUTPUT.store(
        color_enabled(cli.color, io::stdout().is_terminal()),
        Ordering::Relaxed,
    );
    let result = execute(cli).await;
    match result {
        Ok(code) => {
            if code != EXIT_OK {
                std::process::exit(i32::from(code));
            }
        }
        Err(error) => {
            render_error(output, &error);
            std::process::exit(i32::from(error.code));
        }
    }
}

fn normalize_binary_name(args: &mut [OsString]) {
    if let Some(binary_name) = args.first_mut() {
        *binary_name = OsString::from("agentctl");
    }
}

fn requested_output(args: &[OsString]) -> OutputFormat {
    args.windows(2)
        .find_map(|pair| (pair[0] == "--output").then(|| pair[1].to_str()).flatten())
        .or_else(|| {
            args.iter()
                .find_map(|arg| arg.to_str().and_then(|arg| arg.strip_prefix("--output=")))
        })
        .map_or(OutputFormat::Human, |value| match value {
            "json" => OutputFormat::Json,
            "jsonl" => OutputFormat::Jsonl,
            _ => OutputFormat::Human,
        })
}

async fn execute(cli: Cli) -> Result<u8, CliError> {
    let output = cli.output;
    match cli.command {
        Command::Check(args) => {
            let (workflow, plan, diagnostics) = load_and_compile(&args.file)?;
            print_value(
                output,
                "CheckResult",
                &serde_json::json!({
                    "valid": true,
                    "workflow": workflow.metadata.name,
                    "workflowDigest": plan.workflow_digest,
                    "planDigest": plan.plan_digest,
                    "tasks": plan.order.len(),
                }),
                diagnostics,
                format!(
                    "valid: {} ({} tasks)",
                    workflow.metadata.name,
                    plan.order.len()
                ),
            )?;
            Ok(EXIT_OK)
        }
        Command::Plan(args) => {
            let (_, plan, diagnostics) = load_and_compile(&args.file)?;
            print_value(
                output,
                "Plan",
                &plan,
                diagnostics,
                format!(
                    "plan {}\norder: {}\nmax concurrency: {}\npredictability: {:?}\nproviders: {}\ntools: {}\neffects: {}",
                    plan.plan_digest,
                    plan.order.join(" -> "),
                    plan.max_concurrency,
                    plan.predictability,
                    plan.requirements
                        .providers
                        .iter()
                        .map(|provider| provider.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    plan.requirements
                        .tools
                        .iter()
                        .map(|tool| format!("{}:{}", tool.name, tool.capability))
                        .collect::<Vec<_>>()
                        .join(", "),
                    plan.requirements.effects.len(),
                ),
            )?;
            Ok(EXIT_OK)
        }
        Command::Run(args) => run_workflow(output, args).await,
        Command::Resume(args) => resume_run(output, args).await,
        Command::Replay(args) => {
            let store = open_store(&args.db)?;
            let runtime = runtime_for_output(output, store, current_dir()?);
            let outcome = runtime
                .replay(&args.run_id)
                .await
                .map_err(map_runtime_error)?;
            print_value(
                output,
                "RunOutcome",
                &outcome,
                Vec::new(),
                format!(
                    "{} {:?} (recorded replay; no effects)",
                    outcome.run_id, outcome.state
                ),
            )?;
            Ok(outcome_exit_code(outcome.state))
        }
        Command::Fork(args) => {
            validate_interactive(args.interactive)?;
            let store = open_store(&args.db)?;
            let source = store
                .load_run(&args.run_id)
                .map_err(CliError::persistence)?;
            let workflow: Workflow = serde_json::from_value(source.workflow.clone())
                .map_err(|error| CliError::persistence(error.to_string()))?;
            let base = resolve_base_path(
                args.workspace
                    .as_deref()
                    .or_else(|| source.base_path.as_deref().map(Path::new)),
            )?;
            let cancellation = cancellation_token(args.timeout_seconds);
            let registry = build_registry(&workflow, &base, &cancellation, None).await?;
            let runtime = runtime_for_output(output, store, &base).with_registry(registry);
            let outcome = runtime
                .fork(
                    &args.run_id,
                    RunOptions {
                        check: false,
                        diff: args.diff,
                        interactive: args.interactive,
                    },
                    &cancellation,
                )
                .await
                .map_err(map_runtime_error)?;
            print_outcome(output, &outcome)
        }
        Command::Repair(args) => repair_workflow(output, args).await,
        Command::Retry(args) => retry_workflow(output, args).await,
        Command::Compensate(args) => compensate_workflow(output, args).await,
        Command::Runs(args) => runs_command(output, args),
        Command::Cancel(args) => {
            let store = open_store(&args.db)?;
            store
                .request_cancellation(&args.run_id, Utc::now(), "cli-cancel")
                .map_err(CliError::persistence)?;
            print_value(
                output,
                "Cancellation",
                &serde_json::json!({"runId": args.run_id, "requested": true}),
                Vec::new(),
                "cancellation requested".to_owned(),
            )?;
            Ok(EXIT_OK)
        }
        Command::Inspect(args) => {
            let store = open_store(&args.db)?;
            let run = store
                .load_run(&args.run_id)
                .map_err(CliError::persistence)?;
            let tasks = store
                .list_tasks(&args.run_id)
                .map_err(CliError::persistence)?;
            let audit = store
                .audit_events(&args.run_id)
                .map_err(CliError::persistence)?;
            let effects = store
                .list_effects(&args.run_id)
                .map_err(CliError::persistence)?;
            let effect_reconciliations = store
                .run_effect_reconciliations(&args.run_id)
                .map_err(CliError::persistence)?;
            let approvals = store
                .pending_approvals(&args.run_id)
                .map_err(CliError::persistence)?;
            let checkpoints = store
                .checkpoints(&args.run_id)
                .map_err(CliError::persistence)?;
            let provider_sessions = store
                .provider_sessions(&args.run_id)
                .map_err(CliError::persistence)?;
            let tool_calls = store
                .tool_calls(&args.run_id)
                .map_err(CliError::persistence)?;
            let stream_events = store
                .stream_events(&args.run_id)
                .map_err(CliError::persistence)?;
            let protocol_sessions = store
                .protocol_sessions(&args.run_id)
                .map_err(CliError::persistence)?;
            let protocol_calls = store
                .protocol_calls(&args.run_id)
                .map_err(CliError::persistence)?;
            let traces = store
                .trace_events(&args.run_id)
                .map_err(CliError::persistence)?;
            let artifacts = store
                .artifact_references(Some(&args.run_id), None)
                .map_err(CliError::persistence)?;
            let summary = format!(
                "{} {:?}; {} tasks; {} effects; {} protocol calls; {} stream events; {} artifacts; {} checkpoints; {} audit events; {} traces",
                args.run_id,
                run.state,
                tasks.len(),
                effects.len(),
                protocol_calls.len(),
                stream_events.len(),
                artifacts.len(),
                checkpoints.len(),
                audit.len(),
                traces.len(),
            );
            let human = if matches!(
                run.mode,
                RunMode::Repair | RunMode::Retry | RunMode::Compensation
            ) {
                let reused = tasks
                    .iter()
                    .filter(|task| task.disposition == TaskDisposition::Reused)
                    .map(|task| task.task_id.as_str())
                    .collect::<Vec<_>>()
                    .join(",");
                let executed = tasks
                    .iter()
                    .filter(|task| task.disposition == TaskDisposition::Executed)
                    .map(|task| task.task_id.as_str())
                    .collect::<Vec<_>>()
                    .join(",");
                format!(
                    "{summary}; source={}; reused={reused}; executed={executed}",
                    run.source_run_id.as_deref().unwrap_or("unknown")
                )
            } else {
                summary
            };
            let value = serde_json::json!({
                "run": run,
                "tasks": tasks,
                "effects": effects,
                "effectReconciliations": effect_reconciliations,
                "approvals": approvals,
                "checkpoints": checkpoints,
                "providerSessions": provider_sessions,
                "streamEvents": stream_events,
                "protocolSessions": protocol_sessions,
                "protocolCalls": protocol_calls,
                "toolCalls": tool_calls,
                "artifacts": artifacts,
                "audit": audit,
                "traces": traces,
            });
            print_value(output, "RunInspection", &value, Vec::new(), human)?;
            Ok(EXIT_OK)
        }
        Command::Effects(args) => effect_command(output, args).await,
        Command::Approvals(args) => approval_command(output, args),
        Command::Providers(args) => provider_command(output, args).await,
        Command::Auth(args) => auth_command(output, args),
        Command::Schema(args) => schema_command(output, args),
        Command::Migrate(args) => migrate_command(output, args),
        Command::Packs(args) => pack_command(output, args),
        Command::Artifacts(args) => artifact_command(output, args),
        Command::Db(args) => db_command(output, args),
        Command::Memory(args) => memory_command(output, args),
        Command::Gc(args) => gc_command(output, args),
        Command::Completion(args) => {
            let mut command = Cli::command();
            clap_complete::generate(args.shell, &mut command, "agentctl", &mut io::stdout());
            Ok(EXIT_OK)
        }
        Command::Version => {
            print_value(
                output,
                "Version",
                &serde_json::json!({
                    "version": env!("CARGO_PKG_VERSION"),
                    "rust": true,
                    "machineOutput": MACHINE_OUTPUT_VERSION,
                }),
                Vec::new(),
                format!("agentctl {}", env!("CARGO_PKG_VERSION")),
            )?;
            Ok(EXIT_OK)
        }
        Command::Update => {
            print_value(
                output,
                "UpdateInfo",
                &serde_json::json!({
                    "currentVersion": env!("CARGO_PKG_VERSION"),
                    "automaticUpdate": false,
                    "command": "cargo install --locked agentctl-cli"
                }),
                Vec::new(),
                "automatic update is disabled; reinstall from a reviewed release artifact"
                    .to_owned(),
            )?;
            Ok(EXIT_OK)
        }
    }
}

async fn run_workflow(output: OutputFormat, args: RunArgs) -> Result<u8, CliError> {
    validate_interactive(args.interactive)?;
    let (workflow, plan, diagnostics) = load_and_compile(&args.file)?;
    let mut inputs = workflow.spec.inputs.clone();
    let supplied = if let Some(path) = &args.inputs_file {
        parse_inputs(&read_text(path)?, "--inputs-file")?
    } else if let Some(raw) = &args.inputs {
        parse_inputs(raw, "--inputs")?
    } else {
        serde_json::Map::new()
    };
    inputs.extend(supplied);
    for pair in &args.input {
        let (key, raw) = pair
            .split_once('=')
            .ok_or_else(|| CliError::validation("--input must use KEY=VALUE syntax"))?;
        if key.is_empty() {
            return Err(CliError::validation("--input key cannot be empty"));
        }
        let value = serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_owned()));
        inputs.insert(key.to_owned(), value);
    }
    let default_base = args
        .file
        .parent()
        .filter(|path| !path.as_os_str().is_empty());
    let base = resolve_base_path(args.workspace.as_deref().or(default_base))?;
    let cancellation = cancellation_token(args.timeout_seconds);
    let registry = build_registry(&workflow, &base, &cancellation, None).await?;
    let store = open_store(&args.db)?;
    let runtime = runtime_for_output(output, store, &base).with_registry(registry);
    let outcome = runtime
        .start(
            &workflow,
            &plan,
            serde_json::to_value(inputs)
                .map_err(|error| CliError::validation(error.to_string()))?,
            RunOptions {
                check: args.check,
                diff: args.diff,
                interactive: args.interactive,
            },
            &cancellation,
        )
        .await
        .map_err(map_runtime_error)?;
    if !diagnostics.is_empty() && output == OutputFormat::Human {
        for diagnostic in diagnostics {
            eprintln!("warning: {}", diagnostic.message);
        }
    }
    print_outcome(output, &outcome)
}

async fn resume_run(output: OutputFormat, args: ResumeArgs) -> Result<u8, CliError> {
    validate_interactive(args.interactive)?;
    let store = open_store(&args.db)?;
    let run = store
        .load_run(&args.run_id)
        .map_err(CliError::persistence)?;
    let workflow: Workflow = serde_json::from_value(run.workflow.clone())
        .map_err(|error| CliError::persistence(error.to_string()))?;
    let base = resolve_base_path(
        args.workspace
            .as_deref()
            .or_else(|| run.base_path.as_deref().map(Path::new)),
    )?;
    let cancellation = cancellation_token(args.timeout_seconds);
    let resume_tasks = store
        .list_tasks(&args.run_id)
        .map_err(CliError::persistence)?
        .into_iter()
        .filter(|task| {
            !matches!(
                task.state,
                agentctl_core::state::TaskState::Succeeded
                    | agentctl_core::state::TaskState::Skipped
            )
        })
        .map(|task| task.task_id)
        .collect::<BTreeSet<_>>();
    let registry = build_registry(&workflow, &base, &cancellation, Some(&resume_tasks)).await?;
    let runtime = runtime_for_output(output, store, &base).with_registry(registry);
    let outcome = runtime
        .resume(
            &args.run_id,
            RunOptions {
                check: false,
                diff: args.diff,
                interactive: args.interactive,
            },
            &cancellation,
        )
        .await
        .map_err(map_runtime_error)?;
    print_outcome(output, &outcome)
}

async fn repair_workflow(output: OutputFormat, args: RepairArgs) -> Result<u8, CliError> {
    if !args.plan {
        validate_interactive(args.interactive)?;
    }
    let (workflow, compiled, diagnostics) = load_and_compile(&args.file)?;
    let default_base = args
        .file
        .parent()
        .filter(|path| !path.as_os_str().is_empty());
    let base = resolve_base_path(args.workspace.as_deref().or(default_base))?;
    let store = open_store(&args.db)?;
    let planner = Runtime::new(store.clone(), &base);
    let plan = planner
        .plan_repair(
            &args.source_run_id,
            &workflow,
            &compiled,
            &args.from,
            args.restart_successful,
        )
        .map_err(map_runtime_error)?;
    if args.plan || !plan.compatible {
        let human = format!(
            "repair plan: {}\nsource: {}\nreuse: {}\nexecute: {}\nblocked: {}",
            if plan.compatible {
                "compatible"
            } else {
                "blocked"
            },
            plan.source_run_id,
            plan.reused_tasks.join(", "),
            plan.rerun_tasks.join(", "),
            plan.blocked_reuse
                .iter()
                .map(|block| format!("{}: {}", block.task_id, block.message))
                .collect::<Vec<_>>()
                .join("; "),
        );
        print_value(output, "RepairPlan", &plan, diagnostics, human)?;
        return Ok(if plan.compatible {
            EXIT_OK
        } else {
            EXIT_POLICY
        });
    }
    let cancellation = cancellation_token(args.timeout_seconds);
    let repair_tasks = plan.rerun_tasks.iter().cloned().collect::<BTreeSet<_>>();
    let registry = build_registry(&workflow, &base, &cancellation, Some(&repair_tasks)).await?;
    let runtime = runtime_for_output(output, store, &base).with_registry(registry);
    let outcome = runtime
        .repair(
            &workflow,
            &compiled,
            plan,
            args.reason.as_deref(),
            RunOptions {
                check: false,
                diff: args.diff,
                interactive: args.interactive,
            },
            &cancellation,
        )
        .await
        .map_err(map_runtime_error)?;
    print_value(
        output,
        "RepairOutcome",
        &outcome,
        diagnostics,
        format!(
            "{} {:?} source={} reused={} executed={} artifacts={} trace={}",
            outcome.run_id,
            outcome.state,
            outcome.source_run_id,
            outcome.reused_tasks.join(","),
            outcome.executed_tasks.join(","),
            outcome
                .artifacts
                .iter()
                .map(|artifact| artifact.path.as_str())
                .collect::<Vec<_>>()
                .join(","),
            outcome.trace_id,
        ),
    )?;
    Ok(outcome_exit_code(outcome.state))
}

async fn retry_workflow(output: OutputFormat, args: RetryArgs) -> Result<u8, CliError> {
    if !args.plan {
        validate_interactive(args.interactive)?;
    }
    let (workflow, compiled, diagnostics) = load_and_compile(&args.file)?;
    let default_base = args
        .file
        .parent()
        .filter(|path| !path.as_os_str().is_empty());
    let base = resolve_base_path(args.workspace.as_deref().or(default_base))?;
    let store = open_store(&args.db)?;
    let planner = Runtime::new(store.clone(), &base);
    let plan = planner
        .plan_retry(
            &args.source_run_id,
            &workflow,
            &compiled,
            &args.from,
            args.failed,
            args.restart_successful,
        )
        .map_err(map_runtime_error)?;
    if args.plan || !plan.compatible {
        let human = format!(
            "retry plan: {}\nsource: {}\nselection: {}\nreuse: {}\nexecute: {}\nblocked: {}",
            if plan.compatible {
                "compatible"
            } else {
                "blocked"
            },
            plan.source_run_id,
            if plan.failed_only {
                "failed tasks".to_owned()
            } else {
                plan.retry_roots.join(", ")
            },
            plan.reused_tasks.join(", "),
            plan.rerun_tasks.join(", "),
            plan.blocked_reuse
                .iter()
                .map(|block| format!("{}: {}", block.task_id, block.message))
                .collect::<Vec<_>>()
                .join("; "),
        );
        print_value(output, "RetryPlan", &plan, diagnostics, human)?;
        return Ok(if plan.compatible {
            EXIT_OK
        } else {
            EXIT_POLICY
        });
    }
    let cancellation = cancellation_token(args.timeout_seconds);
    let retry_tasks = plan.rerun_tasks.iter().cloned().collect::<BTreeSet<_>>();
    let registry = build_registry(&workflow, &base, &cancellation, Some(&retry_tasks)).await?;
    let runtime = runtime_for_output(output, store, &base).with_registry(registry);
    let outcome = runtime
        .retry(
            &workflow,
            &compiled,
            plan,
            args.reason.as_deref(),
            RunOptions {
                check: false,
                diff: args.diff,
                interactive: args.interactive,
            },
            &cancellation,
        )
        .await
        .map_err(map_runtime_error)?;
    print_value(
        output,
        "RetryOutcome",
        &outcome,
        diagnostics,
        format!(
            "{} {:?} source={} roots={} reused={} executed={} artifacts={} trace={}",
            outcome.run_id,
            outcome.state,
            outcome.source_run_id,
            outcome.retry_roots.join(","),
            outcome.reused_tasks.join(","),
            outcome.executed_tasks.join(","),
            outcome
                .artifacts
                .iter()
                .map(|artifact| artifact.path.as_str())
                .collect::<Vec<_>>()
                .join(","),
            outcome.trace_id,
        ),
    )?;
    Ok(outcome_exit_code(outcome.state))
}

async fn compensate_workflow(output: OutputFormat, args: CompensateArgs) -> Result<u8, CliError> {
    if !args.plan {
        validate_interactive(args.interactive)?;
    }
    let store = open_store(&args.db)?;
    let source = store
        .load_run(&args.source_run_id)
        .map_err(CliError::persistence)?;
    let workflow: Workflow = serde_json::from_value(source.workflow.clone())
        .map_err(|error| CliError::persistence(error.to_string()))?;
    let base = resolve_base_path(
        args.workspace
            .as_deref()
            .or_else(|| source.base_path.as_deref().map(Path::new)),
    )?;
    let planner = Runtime::new(store.clone(), &base);
    let plan = planner
        .plan_compensation(&args.source_run_id, &args.task)
        .map_err(map_runtime_error)?;
    if args.plan || !plan.executable {
        let human = format!(
            "compensation plan: {}\nsource: {}\nexecute: {}\nalready compensated: {}\nblocked: {}",
            if plan.complete {
                "complete"
            } else if plan.executable {
                "partial"
            } else {
                "blocked"
            },
            plan.source_run_id,
            plan.tasks
                .iter()
                .map(|task| format!("{}->{}", task.source_task_id, task.compensation_task_id))
                .collect::<Vec<_>>()
                .join(", "),
            plan.already_compensated_effects.join(", "),
            plan.blocked
                .iter()
                .map(|block| format!("{}: {}", block.task_id, block.message))
                .collect::<Vec<_>>()
                .join("; "),
        );
        let complete = plan.complete;
        let executable = plan.executable;
        print_value(output, "CompensationPlan", &plan, Vec::new(), human)?;
        return Ok(if complete || (!executable && plan.blocked.is_empty()) {
            EXIT_OK
        } else {
            EXIT_POLICY
        });
    }
    let cancellation = cancellation_token(args.timeout_seconds);
    let registry = build_registry(&workflow, &base, &cancellation, None).await?;
    let runtime = runtime_for_output(output, store, &base).with_registry(registry);
    let outcome = runtime
        .compensate(
            plan,
            RunOptions {
                check: false,
                diff: args.diff,
                interactive: args.interactive,
            },
            &cancellation,
        )
        .await
        .map_err(map_runtime_error)?;
    let human = format!(
        "compensation {} source={} state={} compensated={} failed={} blocked={}",
        outcome.run_id.as_deref().unwrap_or("not-created"),
        outcome.source_run_id,
        outcome
            .state
            .map_or_else(|| "not-run".to_owned(), |state| format!("{state:?}")),
        outcome.compensated_tasks.join(","),
        outcome.failed_tasks.join(","),
        outcome
            .blocked
            .iter()
            .map(|block| block.task_id.as_str())
            .collect::<Vec<_>>()
            .join(","),
    );
    print_value(output, "CompensationOutcome", &outcome, Vec::new(), human)?;
    Ok(match outcome.state {
        Some(state) => {
            let code = outcome_exit_code(state);
            if code == EXIT_OK && !outcome.blocked.is_empty() {
                EXIT_POLICY
            } else {
                code
            }
        }
        None if outcome.blocked.is_empty() => EXIT_OK,
        None => EXIT_POLICY,
    })
}

fn print_outcome(
    output: OutputFormat,
    outcome: &agentctl_runtime::RunOutcome,
) -> Result<u8, CliError> {
    print_value(
        output,
        "RunOutcome",
        outcome,
        Vec::new(),
        format!(
            "{} {:?} trace={}",
            outcome.run_id, outcome.state, outcome.trace_id
        ),
    )?;
    Ok(outcome_exit_code(outcome.state))
}

const fn outcome_exit_code(state: agentctl_core::state::RunState) -> u8 {
    match state {
        agentctl_core::state::RunState::Succeeded => EXIT_OK,
        agentctl_core::state::RunState::Paused => EXIT_POLICY,
        agentctl_core::state::RunState::Cancelled => EXIT_CANCELLED,
        agentctl_core::state::RunState::Failed => EXIT_RUN_FAILED,
        agentctl_core::state::RunState::Running => EXIT_RUN_FAILED,
    }
}

fn approval_command(output: OutputFormat, args: ApprovalArgs) -> Result<u8, CliError> {
    let store = open_store(&args.db)?;
    match args.command {
        ApprovalCommand::List { run_id } => {
            let approvals = store
                .pending_approvals(&run_id)
                .map_err(CliError::persistence)?;
            let count = approvals.len();
            print_value(
                output,
                "ApprovalList",
                &approvals,
                Vec::new(),
                format!("{count} pending approval(s)"),
            )?;
        }
        ApprovalCommand::Approve(resolution) => {
            store
                .resolve_approval(
                    &resolution.approval_id,
                    ApprovalResolution::Approved,
                    &resolution.actor,
                    &resolution.reason,
                    Utc::now(),
                )
                .map_err(CliError::persistence)?;
            print_value(
                output,
                "ApprovalResolution",
                &serde_json::json!({"approvalId": resolution.approval_id, "status": "approved"}),
                Vec::new(),
                "approval recorded".to_owned(),
            )?;
        }
        ApprovalCommand::Reject(resolution) => {
            store
                .resolve_approval(
                    &resolution.approval_id,
                    ApprovalResolution::Rejected,
                    &resolution.actor,
                    &resolution.reason,
                    Utc::now(),
                )
                .map_err(CliError::persistence)?;
            print_value(
                output,
                "ApprovalResolution",
                &serde_json::json!({"approvalId": resolution.approval_id, "status": "rejected"}),
                Vec::new(),
                "rejection recorded".to_owned(),
            )?;
        }
    }
    Ok(EXIT_OK)
}

async fn effect_command(output: OutputFormat, args: EffectArgs) -> Result<u8, CliError> {
    let store = open_store(&args.db)?;
    match args.command {
        EffectCommand::List { run_id, task } => {
            let effects = store
                .list_effects(&run_id)
                .map_err(CliError::persistence)?
                .into_iter()
                .filter(|effect| {
                    task.as_ref()
                        .is_none_or(|task| effect.request.task_id == *task)
                })
                .collect::<Vec<_>>();
            let reconciliations = store
                .run_effect_reconciliations(&run_id)
                .map_err(CliError::persistence)?;
            print_value(
                output,
                "EffectList",
                &serde_json::json!({
                    "runId": run_id,
                    "taskId": task,
                    "effects": effects,
                    "reconciliations": reconciliations,
                }),
                Vec::new(),
                format!(
                    "{} effect(s), {} reconciliation record(s)",
                    effects.len(),
                    reconciliations.len()
                ),
            )?;
        }
        EffectCommand::Inspect { effect_id } => {
            let effect = store
                .load_effect(&effect_id)
                .map_err(CliError::persistence)?;
            let reconciliations = store
                .effect_reconciliations(&effect_id)
                .map_err(CliError::persistence)?;
            let protocol_call = store
                .protocol_call(&effect_id)
                .map_err(CliError::persistence)?;
            print_value(
                output,
                "EffectInspection",
                &serde_json::json!({
                    "effect": effect,
                    "reconciliations": reconciliations,
                    "effectiveReconciliation": reconciliations.last(),
                    "protocolCall": protocol_call,
                }),
                Vec::new(),
                format!(
                    "{} {:?}; {} reconciliation record(s)",
                    effect.request.id,
                    effect.status,
                    reconciliations.len()
                ),
            )?;
        }
        EffectCommand::ContinueRemote {
            effect_id,
            actor,
            reason,
            approved,
            timeout_seconds,
        } => {
            let effect = store
                .load_effect(&effect_id)
                .map_err(CliError::persistence)?;
            let run = store
                .load_run(&effect.request.run_id)
                .map_err(CliError::persistence)?;
            let workflow: Workflow = serde_json::from_value(run.workflow)
                .map_err(|error| CliError::persistence(error.to_string()))?;
            let base = resolve_base_path(run.base_path.as_deref().map(Path::new))?;
            let cancellation = cancellation_token(timeout_seconds);
            let execution_tasks = BTreeSet::from([effect.request.task_id.clone()]);
            let registry =
                build_registry(&workflow, &base, &cancellation, Some(&execution_tasks)).await?;
            let runtime = runtime_for_output(output, store, &base).with_registry(registry);
            let reconciliation = runtime
                .continue_external_effect(&effect_id, &actor, &reason, approved, &cancellation)
                .await
                .map_err(map_runtime_error)?;
            print_value(
                output,
                "EffectReconciliation",
                &reconciliation,
                Vec::new(),
                format!(
                    "{} safely continued and reconciled as applied",
                    reconciliation.effect_id
                ),
            )?;
        }
        EffectCommand::Reconcile {
            effect_id,
            status,
            actor,
            reason,
            evidence_file,
            result_file,
            result_schema_file,
            compensation_effect,
            approved,
        } => {
            let effect = store
                .load_effect(&effect_id)
                .map_err(CliError::persistence)?;
            let run = store
                .load_run(&effect.request.run_id)
                .map_err(CliError::persistence)?;
            let workflow: Workflow = serde_json::from_value(run.workflow)
                .map_err(|error| CliError::persistence(error.to_string()))?;
            let base = resolve_base_path(run.base_path.as_deref().map(Path::new))?;
            let cancellation = cancellation_token(None);
            let no_execution_tasks = BTreeSet::new();
            let registry =
                build_registry(&workflow, &base, &cancellation, Some(&no_execution_tasks)).await?;
            let evidence = evidence_file
                .as_deref()
                .map(read_json)
                .transpose()?
                .unwrap_or_else(|| {
                    serde_json::json!({
                        "kind": "operator_statement",
                        "statement": reason,
                    })
                });
            let result = result_file.as_deref().map(read_json).transpose()?;
            let result_schema = result_schema_file.as_deref().map(read_json).transpose()?;
            let status = match status {
                ReconciledOutcome::Applied => ReconciliationStatus::Applied,
                ReconciledOutcome::NotApplied => ReconciliationStatus::NotApplied,
                ReconciledOutcome::Compensated => ReconciliationStatus::Compensated,
            };
            let runtime = Runtime::new(store, base).with_registry(registry);
            let reconciliation = runtime
                .reconcile_effect(EffectReconciliationInput {
                    effect_id,
                    status,
                    actor,
                    reason,
                    evidence,
                    result,
                    result_schema,
                    compensation_effect_id: compensation_effect,
                    approved,
                })
                .map_err(map_runtime_error)?;
            print_value(
                output,
                "EffectReconciliation",
                &reconciliation,
                Vec::new(),
                format!(
                    "{} reconciled as {:?} by {}",
                    reconciliation.effect_id, reconciliation.status, reconciliation.actor
                ),
            )?;
        }
    }
    Ok(EXIT_OK)
}

async fn provider_command(output: OutputFormat, args: ProviderArgs) -> Result<u8, CliError> {
    match args.command {
        ProviderCommand::Inspect(args) => {
            let (workflow, _, diagnostics) = load_and_compile(&args.file)?;
            let base = resolve_base_path(
                args.file
                    .parent()
                    .filter(|path| !path.as_os_str().is_empty()),
            )?;
            let policy =
                PolicyEngine::new(workflow.spec.policy.clone(), &base).map_err(|error| {
                    CliError {
                        code: EXIT_POLICY,
                        message: error.to_string(),
                        diagnostics: Vec::new(),
                        run_id: None,
                        trace_id: None,
                    }
                })?;
            let data = workflow
                .spec
                .providers
                .iter()
                .map(|(name, definition)| {
                    let credential = definition.credential.clone().unwrap_or_else(|| {
                        SecretReference::environment(default_credential_env(
                            definition.kind.clone(),
                        ))
                    });
                    serde_json::json!({
                        "name": name,
                        "kind": definition.kind,
                        "capabilities": provider_capabilities(definition.kind.clone())
                            .into_iter()
                            .map(agentctl_core::compiler::ProviderCapability::as_str)
                            .collect::<Vec<_>>(),
                        "credential": secret_reference_status(&credential, &policy),
                    })
                })
                .collect::<Vec<_>>();
            print_value(
                output,
                "ProviderCapabilities",
                &data,
                diagnostics,
                format!("{} provider(s)", data.len()),
            )?;
            Ok(EXIT_OK)
        }
        ProviderCommand::SmokeOpenai { live, model } => {
            if !live {
                return Err(CliError::validation("--live acknowledgement is required"));
            }
            if std::env::var_os("OPENAI_API_KEY").is_none() {
                return Err(CliError {
                    code: EXIT_REMOTE,
                    message: "OPENAI_API_KEY is not configured".to_owned(),
                    diagnostics: Vec::new(),
                    run_id: None,
                    trace_id: None,
                });
            }
            let provider = OpenAiProvider::new(HttpProviderConfig::openai("OPENAI_API_KEY"))
                .map_err(|error| CliError {
                    code: EXIT_REMOTE,
                    message: error.to_string(),
                    diagnostics: Vec::new(),
                    run_id: None,
                    trace_id: None,
                })?;
            let request = ProviderRequest {
                model,
                instructions: "Reply with exactly: ok".to_owned(),
                messages: vec![Message::User(vec![ContentBlock::Text {
                    text: "health check".to_owned(),
                }])],
                tools: Vec::new(),
                max_output_tokens: 16,
                reasoning: None,
                structured_output: None,
                continuation: None,
                prompt_cache_key: None,
                safety_identifier: None,
                provider_options: BTreeMap::new(),
            };
            let response = tokio::time::timeout(
                Duration::from_secs(30),
                provider.complete(&request, &CancellationToken::new()),
            )
            .await
            .map_err(|_| CliError {
                code: EXIT_REMOTE,
                message: "OpenAI smoke timed out".to_owned(),
                diagnostics: Vec::new(),
                run_id: None,
                trace_id: None,
            })?
            .map_err(|error| CliError {
                code: EXIT_REMOTE,
                message: error.to_string(),
                diagnostics: Vec::new(),
                run_id: None,
                trace_id: None,
            })?;
            if response.text.trim().is_empty() {
                return Err(CliError {
                    code: EXIT_REMOTE,
                    message: "OpenAI smoke returned no text content".to_owned(),
                    diagnostics: Vec::new(),
                    run_id: None,
                    trace_id: None,
                });
            }
            print_value(
                output,
                "LiveProviderSmoke",
                &serde_json::json!({
                    "provider": "openai",
                    "passed": true,
                    "responseIdPresent": response.response_id.is_some(),
                    "usage": response.usage,
                }),
                Vec::new(),
                "OpenAI live smoke passed (response content redacted)".to_owned(),
            )?;
            Ok(EXIT_OK)
        }
    }
}

fn auth_command(output: OutputFormat, args: AuthArgs) -> Result<u8, CliError> {
    let AuthCommand::Check(args) = args.command;
    let (workflow, _, diagnostics) = load_and_compile(&args.file)?;
    let base = resolve_base_path(
        args.file
            .parent()
            .filter(|path| !path.as_os_str().is_empty()),
    )?;
    let policy =
        PolicyEngine::new(workflow.spec.policy.clone(), &base).map_err(|error| CliError {
            code: EXIT_POLICY,
            message: error.to_string(),
            diagnostics: Vec::new(),
            run_id: None,
            trace_id: None,
        })?;
    let status = workflow
        .spec
        .providers
        .iter()
        .map(|(name, definition)| {
            let reference = definition.credential.clone().unwrap_or_else(|| {
                SecretReference::environment(default_credential_env(definition.kind.clone()))
            });
            serde_json::json!({
                "provider": name,
                "credential": secret_reference_status(&reference, &policy),
            })
        })
        .collect::<Vec<_>>();
    print_value(
        output,
        "AuthStatus",
        &status,
        diagnostics,
        format!(
            "checked {} credential reference(s); values and secret processes were not read",
            status.len()
        ),
    )?;
    Ok(EXIT_OK)
}

fn secret_reference_status(reference: &SecretReference, policy: &PolicyEngine) -> Value {
    match reference {
        SecretReference::Environment { env } => serde_json::json!({
            "kind": "environment",
            "reference": env,
            "availability": if std::env::var_os(env).is_some() { "present" } else { "missing" },
        }),
        SecretReference::File { file } => serde_json::json!({
            "kind": "file",
            "reference": file,
            "availability": if policy.resolve_secret_file(file).is_ok() { "present" } else { "missing_or_denied" },
        }),
        SecretReference::Process { process } => serde_json::json!({
            "kind": "process",
            "reference": process.command,
            "availability": "unchecked",
        }),
    }
}

fn schema_command(output: OutputFormat, args: SchemaArgs) -> Result<u8, CliError> {
    let schema = schema_json();
    if let Some(path) = args.write {
        let content = serde_json::to_string_pretty(&schema)
            .map_err(|error| CliError::validation(error.to_string()))?;
        write_text(&path, &(content + "\n"))?;
        print_value(
            output,
            "SchemaWrite",
            &serde_json::json!({"path": path, "written": true}),
            Vec::new(),
            format!("wrote {}", path.display()),
        )?;
    } else {
        print_value(
            output,
            "WorkflowSchema",
            &schema,
            Vec::new(),
            "workflow schema".to_owned(),
        )?;
    }
    Ok(EXIT_OK)
}

fn migrate_command(output: OutputFormat, args: MigrateArgs) -> Result<u8, CliError> {
    let source = read_text(&args.file)?;
    let outcome =
        parse_workflow(&source, &args.file.display().to_string()).map_err(diagnostics_error)?;
    let yaml = serde_yaml_ng::to_string(&outcome.workflow)
        .map_err(|error| CliError::validation(error.to_string()))?;
    if let Some(path) = args.write {
        write_text(&path, &yaml)?;
        print_value(
            output,
            "Migration",
            &serde_json::json!({"source": args.file, "destination": path, "migratedLegacy": outcome.migrated_legacy}),
            outcome.diagnostics,
            format!("wrote migrated workflow to {}", path.display()),
        )?;
    } else if output == OutputFormat::Human {
        print!("{yaml}");
    } else {
        print_value(
            output,
            "Migration",
            &serde_json::json!({"workflow": outcome.workflow, "migratedLegacy": outcome.migrated_legacy}),
            outcome.diagnostics,
            String::new(),
        )?;
    }
    Ok(EXIT_OK)
}

fn pack_command(output: OutputFormat, args: PackArgs) -> Result<u8, CliError> {
    match args.command {
        PackCommand::Inspect { manifest } => {
            let source = read_text(&manifest)?;
            let pack: PackManifest = serde_yaml_ng::from_str(&source).map_err(|error| {
                CliError::validation(format!("{}: {error}", manifest.display()))
            })?;
            pack.validate()
                .map_err(|error| CliError::validation(error.to_string()))?;
            print_value(
                output,
                "PackInspection",
                &pack,
                Vec::new(),
                format!("pack {} {}", pack.name, pack.version),
            )?;
        }
        PackCommand::Verify {
            manifest,
            integrity,
        } => {
            let actual = verify_pack(&manifest, &integrity)
                .map_err(|error| CliError::validation(error.to_string()))?;
            print_value(
                output,
                "PackVerification",
                &serde_json::json!({"path": manifest, "integrity": actual, "valid": true}),
                Vec::new(),
                "pack integrity verified".to_owned(),
            )?;
        }
        PackCommand::Lock { workflow } => {
            let source = read_text(&workflow)?;
            let parsed = parse_workflow(&source, &workflow.display().to_string())
                .map_err(diagnostics_error)?;
            let lock = packs::generate_lock(
                &parsed.workflow,
                &workflow,
                PACK_OFFLINE.load(Ordering::Relaxed),
            )
            .map_err(CliError::validation)?;
            let path = packs::write_lock(&workflow, &lock).map_err(CliError::validation)?;
            print_value(
                output,
                "PackLock",
                &serde_json::json!({"path": path, "lock": lock}),
                parsed.diagnostics,
                format!("locked {} pack(s) at {}", lock.packs.len(), path.display()),
            )?;
        }
        PackCommand::Update { workflow, pack } => {
            let source = read_text(&workflow)?;
            let parsed = parse_workflow(&source, &workflow.display().to_string())
                .map_err(diagnostics_error)?;
            if let Some(name) = &pack
                && !parsed
                    .workflow
                    .spec
                    .packs
                    .iter()
                    .any(|reference| reference.name == *name)
            {
                return Err(CliError::validation(format!(
                    "workflow does not declare root pack `{name}`"
                )));
            }
            let lock = packs::generate_lock(
                &parsed.workflow,
                &workflow,
                PACK_OFFLINE.load(Ordering::Relaxed),
            )
            .map_err(CliError::validation)?;
            let path = packs::write_lock(&workflow, &lock).map_err(CliError::validation)?;
            print_value(
                output,
                "PackLockUpdate",
                &serde_json::json!({"path": path, "updated": pack, "lock": lock}),
                parsed.diagnostics,
                format!("updated {} pack(s) at {}", lock.packs.len(), path.display()),
            )?;
        }
        PackCommand::VerifyLock { workflow } => {
            let source = read_text(&workflow)?;
            let parsed = parse_workflow(&source, &workflow.display().to_string())
                .map_err(diagnostics_error)?;
            let loaded = packs::load_for_workflow(
                &parsed.workflow,
                &workflow,
                packs::PackOptions {
                    offline: PACK_OFFLINE.load(Ordering::Relaxed),
                    locked: true,
                },
            )
            .map_err(CliError::validation)?;
            print_value(
                output,
                "PackLockVerification",
                &serde_json::json!({
                    "path": packs::lock_path(&workflow),
                    "valid": true,
                    "packs": loaded.packs.iter().map(|(entry, _)| entry).collect::<Vec<_>>(),
                    "warnings": loaded.warnings,
                }),
                parsed.diagnostics,
                format!("verified {} locked pack(s)", loaded.packs.len()),
            )?;
        }
    }
    Ok(EXIT_OK)
}

fn runs_command(output: OutputFormat, args: RunsArgs) -> Result<u8, CliError> {
    let store = open_store(&args.db)?;
    let runtime = Runtime::new(store, current_dir()?);
    match args.command {
        RunsCommand::Analyze { run_id }
        | RunsCommand::Upgrade {
            run_id,
            dry_run: true,
        } => {
            let analysis = runtime
                .analyze_legacy_run(&run_id)
                .map_err(map_runtime_error)?;
            let roots = if analysis.recommended_repair_roots.is_empty() {
                "none".to_owned()
            } else {
                analysis.recommended_repair_roots.join(",")
            };
            print_value(
                output,
                "LegacyRunUpgradeAnalysis",
                &analysis,
                Vec::new(),
                format!(
                    "{}: {} upgradeable, {} unavailable; safe repair roots: {roots}",
                    analysis.run_id,
                    analysis.upgradeable_tasks.len(),
                    analysis.unavailable_tasks.len(),
                ),
            )?;
        }
        RunsCommand::Upgrade {
            run_id,
            dry_run: false,
        } => {
            let result = runtime
                .upgrade_legacy_run(&run_id)
                .map_err(map_runtime_error)?;
            let roots = if result.analysis_after.recommended_repair_roots.is_empty() {
                "none".to_owned()
            } else {
                result.analysis_after.recommended_repair_roots.join(",")
            };
            print_value(
                output,
                "LegacyRunUpgrade",
                &result,
                Vec::new(),
                format!(
                    "{}: upgraded {} task(s); safe repair roots: {roots}",
                    result.run_id,
                    result.upgraded_tasks.len(),
                ),
            )?;
        }
    }
    Ok(EXIT_OK)
}

fn cancellation_token(timeout_seconds: Option<u64>) -> CancellationToken {
    let token = CancellationToken::new();
    let signal = token.clone();
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            if let Ok(mut terminate) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            {
                tokio::select! {
                    result = tokio::signal::ctrl_c() => {
                        if result.is_ok() {
                            signal.cancel();
                        }
                    }
                    _ = terminate.recv() => signal.cancel(),
                }
            }
        }
        #[cfg(not(unix))]
        if tokio::signal::ctrl_c().await.is_ok() {
            signal.cancel();
        }
    });
    if let Some(seconds) = timeout_seconds {
        let timeout = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(seconds)).await;
            timeout.cancel();
        });
    }
    token
}

fn db_command(output: OutputFormat, args: DbArgs) -> Result<u8, CliError> {
    let store = open_store(&args.db)?;
    match args.command {
        DbCommand::Stats => {
            let stats = store.stats().map_err(CliError::persistence)?;
            print_value(
                output,
                "DatabaseStats",
                &stats,
                Vec::new(),
                format!(
                    "schema {}: {} runs, {} effects, {} reconciliations, {} run upgrades, {} artifact blobs, {} artifact references",
                    stats.schema_version,
                    stats.runs,
                    stats.effects,
                    stats.effect_reconciliations,
                    stats.run_upgrades,
                    stats.artifact_blobs,
                    stats.artifact_references
                ),
            )?;
        }
        DbCommand::Migrate => {
            print_value(
                output,
                "DatabaseMigration",
                &serde_json::json!({"schemaVersion": store.schema_version(), "migrated": true}),
                Vec::new(),
                format!("database schema is at version {}", store.schema_version()),
            )?;
        }
        DbCommand::Encryption { command } => match command {
            DbEncryptionCommand::Inventory => {
                let inventory = store
                    .encryption_inventory()
                    .map_err(CliError::persistence)?;
                print_value(
                    output,
                    "EncryptionInventory",
                    &inventory,
                    Vec::new(),
                    format!(
                        "state encryption: {}; protected={}, encrypted={}, plaintext={}, invalid={}",
                        if inventory.enabled {
                            format!(
                                "enabled key={} reference={}",
                                inventory.key_id.as_deref().unwrap_or("unknown"),
                                inventory.key_reference.as_deref().unwrap_or("unknown")
                            )
                        } else {
                            "disabled".to_owned()
                        },
                        inventory.protected_values,
                        inventory.encrypted_values,
                        inventory.plaintext_values,
                        inventory.invalid_envelopes,
                    ),
                )?;
            }
            DbEncryptionCommand::Enable {
                key_id,
                key_env,
                dry_run,
            } => {
                let report = store
                    .enable_encryption(&key_id, &key_env, dry_run, Utc::now())
                    .map_err(CliError::persistence)?;
                print_value(
                    output,
                    "EncryptionMigration",
                    &report,
                    Vec::new(),
                    format!(
                        "{} state encryption with key {}: scanned {}, rewrote {}",
                        if dry_run { "planned" } else { "enabled" },
                        key_id,
                        report.values_scanned,
                        report.values_rewritten,
                    ),
                )?;
            }
            DbEncryptionCommand::Rotate {
                key_id,
                key_env,
                dry_run,
            } => {
                let report = store
                    .rotate_encryption_key(&key_id, &key_env, dry_run, Utc::now())
                    .map_err(CliError::persistence)?;
                print_value(
                    output,
                    "EncryptionMigration",
                    &report,
                    Vec::new(),
                    format!(
                        "{} state-encryption rotation to key {}: scanned {}, rewrote {}",
                        if dry_run { "planned" } else { "completed" },
                        key_id,
                        report.values_scanned,
                        report.values_rewritten,
                    ),
                )?;
            }
        },
    }
    Ok(EXIT_OK)
}

fn artifact_command(output: OutputFormat, args: ArtifactArgs) -> Result<u8, CliError> {
    let store = open_store(&args.db)?;
    match args.command {
        ArtifactCommand::List { run, task } => {
            let references = store
                .artifact_references(run.as_deref(), task.as_deref())
                .map_err(CliError::persistence)?;
            let blobs = store.artifact_blobs().map_err(CliError::persistence)?;
            print_value(
                output,
                "ArtifactList",
                &serde_json::json!({"references": references, "blobs": blobs}),
                Vec::new(),
                format!(
                    "{} artifact reference(s), {} content-addressed blob(s)",
                    references.len(),
                    blobs.len()
                ),
            )?;
        }
        ArtifactCommand::Inspect { digest } => {
            let blob = store
                .artifact_blob(&digest)
                .map_err(CliError::persistence)?;
            let references = store
                .artifact_references(None, None)
                .map_err(CliError::persistence)?
                .into_iter()
                .filter(|reference| reference.digest == digest)
                .collect::<Vec<_>>();
            print_value(
                output,
                "ArtifactInspection",
                &serde_json::json!({"blob": blob, "references": references}),
                Vec::new(),
                format!(
                    "{}: {} bytes, {} reference(s)",
                    blob.digest,
                    blob.size_bytes,
                    references.len()
                ),
            )?;
        }
        ArtifactCommand::Verify { digest, all } => {
            if digest.is_none() && !all {
                return Err(CliError::validation(
                    "provide an artifact digest or use --all".to_owned(),
                ));
            }
            let digests = if let Some(digest) = digest {
                vec![digest]
            } else {
                store
                    .artifact_blobs()
                    .map_err(CliError::persistence)?
                    .into_iter()
                    .map(|blob| blob.digest)
                    .collect()
            };
            let verifications = digests
                .iter()
                .map(|digest| store.verify_artifact(digest, Utc::now()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(CliError::persistence)?;
            print_value(
                output,
                "ArtifactVerification",
                &serde_json::json!({"valid": true, "artifacts": verifications}),
                Vec::new(),
                format!("verified {} artifact blob(s)", verifications.len()),
            )?;
        }
        ArtifactCommand::Export {
            digest,
            destination,
            overwrite,
        } => {
            store
                .export_artifact(&digest, &destination, overwrite)
                .map_err(CliError::persistence)?;
            print_value(
                output,
                "ArtifactExport",
                &serde_json::json!({
                    "digest": digest,
                    "destination": destination,
                    "overwritten": overwrite,
                }),
                Vec::new(),
                format!("exported {} to {}", digest, destination.display()),
            )?;
        }
        ArtifactCommand::Gc {
            older_than_days,
            dry_run,
        } => {
            if older_than_days < 0 {
                return Err(CliError::validation(
                    "--older-than-days must be zero or greater".to_owned(),
                ));
            }
            let before = Utc::now() - ChronoDuration::days(older_than_days);
            let report = store
                .garbage_collect_artifacts(before, dry_run)
                .map_err(CliError::persistence)?;
            print_value(
                output,
                "ArtifactGarbageCollection",
                &serde_json::json!({
                    "dryRun": dry_run,
                    "before": before,
                    "report": report,
                }),
                Vec::new(),
                format!(
                    "{} {} artifact blob(s) and {} temporary file(s), {} reclaimable byte(s)",
                    if dry_run { "considered" } else { "removed" },
                    if dry_run {
                        report.considered
                    } else {
                        u64::try_from(report.removed.len()).unwrap_or(u64::MAX)
                    },
                    if dry_run {
                        report.temporary_files_considered
                    } else {
                        report.temporary_files_removed
                    },
                    report.reclaimed_bytes
                ),
            )?;
        }
    }
    Ok(EXIT_OK)
}

fn memory_command(output: OutputFormat, args: MemoryArgs) -> Result<u8, CliError> {
    let store = open_store(&args.db)?;
    match args.command {
        MemoryCommand::Get { namespace, key } => {
            let value = store
                .get_long_term_memory(&namespace, &key, Utc::now())
                .map_err(CliError::persistence)?;
            print_value(
                output,
                "MemoryValue",
                &serde_json::json!({"namespace": namespace, "key": key, "value": value}),
                Vec::new(),
                if value.is_some() {
                    "memory found"
                } else {
                    "memory not found"
                }
                .to_owned(),
            )?;
        }
        MemoryCommand::Put {
            namespace,
            key,
            value,
            retention_days,
        } => {
            let value: Value = serde_json::from_str(&value)
                .map_err(|error| CliError::validation(format!("value must be JSON: {error}")))?;
            let expires = retention_days.map(|days| Utc::now() + ChronoDuration::days(days));
            store
                .put_long_term_memory(&namespace, &key, &value, expires, Utc::now())
                .map_err(CliError::persistence)?;
            print_value(
                output,
                "MemoryWrite",
                &serde_json::json!({"namespace": namespace, "key": key, "written": true, "expiresAt": expires}),
                Vec::new(),
                "memory written".to_owned(),
            )?;
        }
    }
    Ok(EXIT_OK)
}

fn gc_command(output: OutputFormat, args: GcArgs) -> Result<u8, CliError> {
    if args.older_than_days < 0 {
        return Err(CliError::validation(
            "--older-than-days must be non-negative",
        ));
    }
    let store = open_store(&args.db)?;
    let before = Utc::now() - ChronoDuration::days(args.older_than_days);
    let removed = store
        .garbage_collect(before)
        .map_err(CliError::persistence)?;
    print_value(
        output,
        "GarbageCollection",
        &serde_json::json!({"removed": removed, "before": before}),
        Vec::new(),
        format!("removed {removed} record(s)"),
    )?;
    Ok(EXIT_OK)
}

fn load_and_compile(
    path: &Path,
) -> Result<(Workflow, agentctl_core::CompiledPlan, Vec<Diagnostic>), CliError> {
    let source = read_text(path)?;
    let parsed = parse_workflow(&source, &path.display().to_string()).map_err(diagnostics_error)?;
    let mut workflow = parsed.workflow;
    let loaded = packs::load_for_workflow(
        &workflow,
        path,
        packs::PackOptions {
            offline: PACK_OFFLINE.load(Ordering::Relaxed),
            locked: PACK_LOCKED.load(Ordering::Relaxed),
        },
    )
    .map_err(CliError::validation)?;
    let mut diagnostics = parsed.diagnostics;
    diagnostics.extend(loaded.warnings.into_iter().map(|message| {
        Diagnostic {
        code: DiagnosticCode::PolicyDenied,
        severity: Severity::Warning,
        message,
        file: path.display().to_string(),
        line: None,
        column: None,
        path: Some("$.spec.packs".to_owned()),
        help: Some(
            "sign the pack with a trusted Sigstore identity or choose an explicit unsigned policy"
                .to_owned(),
        ),
    }
    }));
    load_packs(&mut workflow, loaded.packs)?;
    let plan = compile(&workflow, &path.display().to_string()).map_err(diagnostics_error)?;
    Ok((workflow, plan, diagnostics))
}

fn load_packs(
    workflow: &mut Workflow,
    packs: Vec<(agentctl_core::pack::PackLockEntry, PackManifest)>,
) -> Result<(), CliError> {
    for (entry, mut pack) in packs {
        if pack.name != entry.name || pack.version != entry.version {
            return Err(CliError::validation(format!(
                "locked pack `{}@{}` does not match manifest `{}@{}`",
                entry.name, entry.version, pack.name, pack.version
            )));
        }
        let qualify = |name: &str| format!("{}.{}", pack.name, name);
        for agent in pack.agents.values_mut() {
            agent.tools = agent.tools.iter().map(|name| qualify(name)).collect();
        }
        for definition in pack.workflows.values_mut() {
            for task in &mut definition.tasks {
                qualify_pack_task(task, &qualify);
            }
        }
        for (name, action) in pack.actions {
            insert_pack_item(
                &mut workflow.spec.actions,
                qualify(&name),
                action,
                &pack.name,
            )?;
        }
        for (name, tool) in pack.tools {
            insert_pack_item(&mut workflow.spec.tools, qualify(&name), tool, &pack.name)?;
        }
        for (name, agent) in pack.agents {
            insert_pack_item(&mut workflow.spec.agents, qualify(&name), agent, &pack.name)?;
        }
        for (name, definition) in pack.workflows {
            insert_pack_item(
                &mut workflow.spec.subworkflows,
                qualify(&name),
                definition,
                &pack.name,
            )?;
        }
    }
    Ok(())
}

fn qualify_pack_task(
    task: &mut agentctl_core::dsl::TaskDefinition,
    qualify: &impl Fn(&str) -> String,
) {
    for prefix in ["action:", "agent:", "workflow:"] {
        if let Some(name) = task.uses.strip_prefix(prefix) {
            task.uses = format!("{prefix}{}", qualify(name));
            break;
        }
    }
    if let Some(compensate) = &mut task.compensate
        && let Some(name) = compensate.uses.strip_prefix("action:")
    {
        compensate.uses = format!("action:{}", qualify(name));
    }
}

fn insert_pack_item<T>(
    target: &mut BTreeMap<String, T>,
    name: String,
    value: T,
    pack: &str,
) -> Result<(), CliError> {
    if target.insert(name.clone(), value).is_some() {
        Err(CliError::validation(format!(
            "pack `{pack}` collides at `{name}`"
        )))
    } else {
        Ok(())
    }
}

async fn build_registry(
    workflow: &Workflow,
    base: &Path,
    cancellation: &CancellationToken,
    execution_tasks: Option<&BTreeSet<String>>,
) -> Result<RuntimeRegistry, CliError> {
    let mut registry = RuntimeRegistry::default();
    let tool_policy =
        PolicyEngine::new(workflow.spec.policy.clone(), base).map_err(|error| CliError {
            code: EXIT_POLICY,
            message: error.to_string(),
            diagnostics: Vec::new(),
            run_id: None,
            trace_id: None,
        })?;
    let provider_secrets = SecretResolver::provider_credentials(tool_policy.clone());
    let restricted_secrets = SecretResolver::restricted(tool_policy.clone());
    let required_providers = workflow
        .spec
        .tasks
        .iter()
        .filter(|task| execution_tasks.is_none_or(|selected| selected.contains(&task.id)))
        .filter_map(|task| task.uses.strip_prefix("agent:"))
        .filter_map(|agent| workflow.spec.agents.get(agent))
        .map(|agent| agent.provider.as_str())
        .collect::<BTreeSet<_>>();
    let mut selected_action_kinds = workflow
        .spec
        .tasks
        .iter()
        .filter(|task| execution_tasks.is_none_or(|selected| selected.contains(&task.id)))
        .filter_map(|task| task.uses.strip_prefix("action:"))
        .filter_map(|action| workflow.spec.actions.get(action))
        .map(|action| action.kind)
        .collect::<Vec<_>>();
    selected_action_kinds.extend(
        workflow
            .spec
            .tasks
            .iter()
            .filter_map(|task| task.compensate.as_ref())
            .filter_map(|compensate| compensate.uses.strip_prefix("action:"))
            .filter_map(|action| workflow.spec.actions.get(action))
            .map(|action| action.kind),
    );
    selected_action_kinds.extend(
        workflow
            .spec
            .subworkflows
            .values()
            .flat_map(|definition| definition.tasks.iter())
            .filter_map(|task| task.compensate.as_ref())
            .filter_map(|compensate| compensate.uses.strip_prefix("action:"))
            .filter_map(|action| workflow.spec.actions.get(action))
            .map(|action| action.kind),
    );
    for (name, definition) in &workflow.spec.providers {
        let credential = definition.credential.clone().unwrap_or_else(|| {
            SecretReference::environment(default_credential_env(definition.kind.clone()))
        });
        match definition.kind {
            ProviderKind::Fake => {
                registry = registry.with_provider(name, Arc::new(FakeProvider::default()));
            }
            ProviderKind::Openai => {
                let mut config =
                    HttpProviderConfig::openai(default_credential_env(ProviderKind::Openai));
                config.credential = credential.clone();
                config.resolved_credential = preflight_provider_credential(
                    name,
                    &credential,
                    &required_providers,
                    &provider_secrets,
                    cancellation,
                )
                .await?;
                config.credential_resolver = Some(Arc::new(provider_secrets.clone()));
                if required_providers.contains(name.as_str()) {
                    config.headers = resolve_protocol_headers(
                        &definition.headers,
                        &restricted_secrets,
                        cancellation,
                    )
                    .await?;
                }
                if let Some(endpoint) = &definition.endpoint {
                    config.endpoint = endpoint.clone();
                }
                registry = registry.with_provider(
                    name,
                    Arc::new(OpenAiProvider::new(config).map_err(remote_error)?),
                );
            }
            ProviderKind::Anthropic => {
                let mut config =
                    HttpProviderConfig::anthropic(default_credential_env(ProviderKind::Anthropic));
                config.credential = credential.clone();
                config.resolved_credential = preflight_provider_credential(
                    name,
                    &credential,
                    &required_providers,
                    &provider_secrets,
                    cancellation,
                )
                .await?;
                config.credential_resolver = Some(Arc::new(provider_secrets.clone()));
                if required_providers.contains(name.as_str()) {
                    config.headers = resolve_protocol_headers(
                        &definition.headers,
                        &restricted_secrets,
                        cancellation,
                    )
                    .await?;
                }
                if let Some(endpoint) = &definition.endpoint {
                    config.endpoint = endpoint.clone();
                }
                registry = registry.with_provider(
                    name,
                    Arc::new(AnthropicProvider::new(config).map_err(remote_error)?),
                );
            }
            ProviderKind::Google => {
                let mut config =
                    HttpProviderConfig::google(default_credential_env(ProviderKind::Google));
                config.credential = credential.clone();
                config.resolved_credential = preflight_provider_credential(
                    name,
                    &credential,
                    &required_providers,
                    &provider_secrets,
                    cancellation,
                )
                .await?;
                config.credential_resolver = Some(Arc::new(provider_secrets.clone()));
                if required_providers.contains(name.as_str()) {
                    config.headers = resolve_protocol_headers(
                        &definition.headers,
                        &restricted_secrets,
                        cancellation,
                    )
                    .await?;
                }
                if let Some(endpoint) = &definition.endpoint {
                    config.endpoint = endpoint.clone();
                }
                registry = registry.with_provider(
                    name,
                    Arc::new(GoogleProvider::new(config).map_err(remote_error)?),
                );
            }
            ProviderKind::AzureOpenai => {
                let endpoint = definition.endpoint.clone().ok_or_else(|| {
                    CliError::validation(format!(
                        "Azure OpenAI provider `{name}` requires endpoint"
                    ))
                })?;
                let resolved_credential = preflight_provider_credential(
                    name,
                    &credential,
                    &required_providers,
                    &provider_secrets,
                    cancellation,
                )
                .await?;
                let config = HttpProviderConfig {
                    endpoint,
                    credential,
                    resolved_credential,
                    credential_resolver: Some(Arc::new(provider_secrets.clone())),
                    organization: None,
                    project: None,
                    api_version: definition
                        .api_version
                        .clone()
                        .or_else(|| Some("v1".to_owned())),
                    headers: if required_providers.contains(name.as_str()) {
                        resolve_protocol_headers(
                            &definition.headers,
                            &restricted_secrets,
                            cancellation,
                        )
                        .await?
                    } else {
                        BTreeMap::new()
                    },
                };
                registry = registry.with_provider(
                    name,
                    Arc::new(OpenAiProvider::azure(config).map_err(remote_error)?),
                );
            }
        }
    }

    for (name, definition) in &workflow.spec.tools {
        registry = registry.with_tool(
            name,
            Arc::new(BuiltinToolExecutor::new(
                name,
                definition,
                tool_policy.clone(),
            )),
        );
    }

    let mut mcp = BTreeMap::new();
    if selected_action_kinds.contains(&agentctl_core::dsl::ActionKind::McpCall) {
        for (name, definition) in &workflow.spec.mcp_servers {
            let headers =
                resolve_protocol_headers(&definition.headers, &restricted_secrets, cancellation)
                    .await?;
            let client = McpClient::new(ProtocolHttpConfig {
                url: Url::parse(&definition.url).map_err(|error| {
                    CliError::validation(format!("MCP server `{name}` URL: {error}"))
                })?,
                headers,
                header_references: definition.headers.clone(),
                header_resolver: Some(Arc::new(restricted_secrets.clone())),
                timeout: Duration::from_secs(definition.timeout_seconds),
            })
            .map_err(remote_error)?;
            mcp.insert(name.clone(), Arc::new(client));
        }
    }
    let mut a2a = BTreeMap::new();
    if selected_action_kinds.contains(&agentctl_core::dsl::ActionKind::A2aDelegate) {
        for (name, definition) in &workflow.spec.a2a_peers {
            let headers =
                resolve_protocol_headers(&definition.headers, &restricted_secrets, cancellation)
                    .await?;
            let client = A2aClient::new(ProtocolHttpConfig {
                url: Url::parse(&definition.card_url).map_err(|error| {
                    CliError::validation(format!("A2A peer `{name}` card URL: {error}"))
                })?,
                headers,
                header_references: definition.headers.clone(),
                header_resolver: Some(Arc::new(restricted_secrets.clone())),
                timeout: Duration::from_secs(definition.timeout_seconds),
            })
            .map_err(remote_error)?
            .with_poll_bounds(
                definition.max_polls,
                Duration::from_millis(definition.poll_interval_ms),
            );
            a2a.insert(name.clone(), Arc::new(client));
        }
    }
    if !mcp.is_empty() || !a2a.is_empty() {
        registry = registry.with_external_actions(Arc::new(ProtocolActionHandler::new(mcp, a2a)));
    }
    Ok(registry)
}

async fn preflight_provider_credential(
    name: &str,
    reference: &SecretReference,
    required_providers: &BTreeSet<&str>,
    resolver: &SecretResolver,
    cancellation: &CancellationToken,
) -> Result<Option<SecretValue>, CliError> {
    if !required_providers.contains(name) {
        return Ok(None);
    }
    resolver
        .resolve(reference, cancellation)
        .await
        .map(Some)
        .map_err(|error| secret_cli_error(name, error, EXIT_REMOTE))
}

async fn resolve_protocol_headers(
    headers: &BTreeMap<String, SecretReference>,
    resolver: &SecretResolver,
    cancellation: &CancellationToken,
) -> Result<BTreeMap<String, SecretValue>, CliError> {
    let mut resolved = BTreeMap::new();
    for (name, reference) in headers {
        let value = resolver
            .resolve(reference, cancellation)
            .await
            .map_err(|error| secret_cli_error(name, error, EXIT_POLICY))?;
        resolved.insert(name.clone(), value);
    }
    Ok(resolved)
}

fn secret_cli_error(owner: &str, error: SecretResolutionError, fallback_code: u8) -> CliError {
    let code = if matches!(error, SecretResolutionError::Policy(_)) {
        EXIT_POLICY
    } else {
        fallback_code
    };
    CliError {
        code,
        message: format!("secret for `{owner}` could not be resolved: {error}"),
        diagnostics: Vec::new(),
        run_id: None,
        trace_id: None,
    }
}

fn default_credential_env(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Fake => "AGENTCTL_FAKE_PROVIDER",
        ProviderKind::Openai => "OPENAI_API_KEY",
        ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
        ProviderKind::Google => "GEMINI_API_KEY",
        ProviderKind::AzureOpenai => "AZURE_OPENAI_API_KEY",
    }
}

fn validate_interactive(interactive: bool) -> Result<(), CliError> {
    if interactive && !(io::stdin().is_terminal() && io::stdout().is_terminal()) {
        Err(CliError {
            code: EXIT_POLICY,
            message: "--interactive requires terminal stdin and stdout".to_owned(),
            diagnostics: Vec::new(),
            run_id: None,
            trace_id: None,
        })
    } else {
        Ok(())
    }
}

fn open_store(path: &Path) -> Result<SqliteStore, CliError> {
    SqliteStore::open(path).map_err(CliError::persistence)
}

fn current_dir() -> Result<PathBuf, CliError> {
    std::env::current_dir().map_err(|error| CliError::validation(error.to_string()))
}

fn resolve_base_path(path: Option<&Path>) -> Result<PathBuf, CliError> {
    let path = path.map_or_else(current_dir, |value| Ok(value.to_path_buf()))?;
    std::fs::canonicalize(&path)
        .map_err(|error| CliError::validation(format!("workspace {}: {error}", path.display())))
}

fn parse_inputs(raw: &str, source: &str) -> Result<serde_json::Map<String, Value>, CliError> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|error| CliError::validation(format!("{source} must be JSON: {error}")))?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| CliError::validation(format!("{source} must contain a JSON object")))
}

fn read_text(path: &Path) -> Result<String, CliError> {
    use std::io::Read as _;

    let file = std::fs::File::open(path)
        .map_err(|error| CliError::validation(format!("{}: {error}", path.display())))?;
    let mut reader = file.take(MAX_TEXT_FILE_BYTES + 1);
    let mut content = String::new();
    reader
        .read_to_string(&mut content)
        .map_err(|error| CliError::validation(format!("{}: {error}", path.display())))?;
    if content.len() as u64 > MAX_TEXT_FILE_BYTES {
        return Err(CliError::validation(format!(
            "{} exceeds {MAX_TEXT_FILE_BYTES} bytes",
            path.display()
        )));
    }
    Ok(content)
}

fn read_json(path: &Path) -> Result<Value, CliError> {
    serde_json::from_str(&read_text(path)?)
        .map_err(|error| CliError::validation(format!("{}: {error}", path.display())))
}

fn write_text(path: &Path, content: &str) -> Result<(), CliError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|error| CliError::validation(format!("{}: {error}", parent.display())))?;
    }
    std::fs::write(path, content)
        .map_err(|error| CliError::validation(format!("{}: {error}", path.display())))
}

fn runtime_for_output(
    output: OutputFormat,
    store: SqliteStore,
    base_path: impl Into<PathBuf>,
) -> Runtime {
    let runtime = Runtime::new(store, base_path);
    match output {
        OutputFormat::Human | OutputFormat::Jsonl => {
            runtime.with_stream_event_sink(Arc::new(CliStreamEventSink { output }))
        }
        OutputFormat::Json => runtime,
    }
}

struct CliStreamEventSink {
    output: OutputFormat,
}

impl StreamEventSink for CliStreamEventSink {
    fn record(&self, event: &StreamEventRecord) {
        let _guard = STREAM_OUTPUT_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match self.output {
            OutputFormat::Human => {
                if let Some(delta) = event.payload.get("delta").and_then(Value::as_str) {
                    eprintln!(
                        "[{} stream {}] {}",
                        event.task_id,
                        event.sequence,
                        delta.escape_debug()
                    );
                } else {
                    eprintln!(
                        "[{} stream {}] {}",
                        event.task_id, event.sequence, event.event_type
                    );
                }
            }
            OutputFormat::Jsonl => {
                let value = serde_json::json!({
                    "apiVersion": MACHINE_OUTPUT_VERSION,
                    "kind": "StreamEvent",
                    "ok": true,
                    "data": event,
                    "diagnostics": [],
                });
                println!("{value}");
            }
            OutputFormat::Json => {}
        }
    }
}

fn print_value<T: Serialize>(
    output: OutputFormat,
    kind: &'static str,
    data: &T,
    diagnostics: Vec<Diagnostic>,
    human: String,
) -> Result<(), CliError> {
    match output {
        OutputFormat::Human => {
            if COLOR_OUTPUT.load(Ordering::Relaxed) {
                println!("\u{1b}[32m{human}\u{1b}[0m");
            } else {
                println!("{human}");
            }
            if VERBOSE_OUTPUT.load(Ordering::Relaxed) {
                println!(
                    "{}",
                    serde_json::to_string_pretty(data)
                        .map_err(|error| CliError::validation(error.to_string()))?
                );
            }
        }
        OutputFormat::Json | OutputFormat::Jsonl => println!(
            "{}",
            serde_json::to_string(&Envelope {
                api_version: MACHINE_OUTPUT_VERSION,
                kind,
                ok: true,
                data,
                diagnostics,
            })
            .map_err(|error| CliError::validation(error.to_string()))?
        ),
    }
    Ok(())
}

fn render_error(output: OutputFormat, error: &CliError) {
    match output {
        OutputFormat::Human => {
            if COLOR_OUTPUT.load(Ordering::Relaxed) {
                eprintln!("\u{1b}[31merror: {}\u{1b}[0m", error.message);
            } else {
                eprintln!("error: {}", error.message);
            }
            for diagnostic in &error.diagnostics {
                let location = match (diagnostic.line, diagnostic.column) {
                    (Some(line), Some(column)) => format!("{}:{line}:{column}", diagnostic.file),
                    _ => diagnostic.file.clone(),
                };
                eprintln!("  {location}: {}", diagnostic.message);
                if let Some(help) = &diagnostic.help {
                    eprintln!("  help: {help}");
                }
            }
            if let Some(run_id) = &error.run_id {
                eprintln!("  run: {run_id}");
            }
            if let Some(trace_id) = &error.trace_id {
                eprintln!("  trace: {trace_id}");
            }
        }
        OutputFormat::Json | OutputFormat::Jsonl => {
            let value = serde_json::json!({
                "apiVersion": MACHINE_OUTPUT_VERSION,
                "kind": "Error",
                "ok": false,
                "error": {
                    "message": error.message,
                    "exitCode": error.code,
                    "runId": error.run_id,
                    "traceId": error.trace_id,
                },
                "diagnostics": error.diagnostics,
            });
            eprintln!("{value}");
        }
    }
}

const fn color_enabled(mode: ColorMode, terminal: bool) -> bool {
    match mode {
        ColorMode::Auto => terminal,
        ColorMode::Always => true,
        ColorMode::Never => false,
    }
}

fn diagnostics_error(diagnostics: Vec<Diagnostic>) -> CliError {
    CliError {
        code: EXIT_VALIDATION,
        message: "workflow validation failed".to_owned(),
        diagnostics,
        run_id: None,
        trace_id: None,
    }
}

fn remote_error(error: impl ToString) -> CliError {
    CliError {
        code: EXIT_REMOTE,
        message: error.to_string(),
        diagnostics: Vec::new(),
        run_id: None,
        trace_id: None,
    }
}

fn map_runtime_error(error: agentctl_runtime::RuntimeError) -> CliError {
    let (run_id, trace_id) = match &error {
        agentctl_runtime::RuntimeError::RunFailed {
            run_id, trace_id, ..
        } => (Some(run_id.clone()), Some(trace_id.clone())),
        agentctl_runtime::RuntimeError::UncertainEffect {
            run_id, trace_id, ..
        } => (Some(run_id.clone()), Some(trace_id.clone())),
        _ => (None, None),
    };
    let code = match &error {
        agentctl_runtime::RuntimeError::Store(StoreError::UnknownSchema { .. })
        | agentctl_runtime::RuntimeError::Store(StoreError::Corrupt(_))
        | agentctl_runtime::RuntimeError::Store(StoreError::Incompatible(_)) => EXIT_PERSISTENCE,
        agentctl_runtime::RuntimeError::Policy(_) => EXIT_POLICY,
        agentctl_runtime::RuntimeError::Provider(_) => EXIT_REMOTE,
        agentctl_runtime::RuntimeError::Cancelled => EXIT_CANCELLED,
        agentctl_runtime::RuntimeError::UncertainEffect { .. } => EXIT_POLICY,
        agentctl_runtime::RuntimeError::RepairBlocked { .. }
        | agentctl_runtime::RuntimeError::RetryBlocked { .. } => EXIT_POLICY,
        _ => EXIT_RUN_FAILED,
    };
    CliError {
        code,
        message: error.to_string(),
        diagnostics: Vec::new(),
        run_id,
        trace_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;

    #[test]
    fn cli_has_all_stable_commands() {
        let mut command = Cli::command();
        command.build();
        let names = command
            .get_subcommands()
            .map(|command| command.get_name())
            .collect::<Vec<_>>();
        for expected in [
            "check",
            "plan",
            "run",
            "resume",
            "replay",
            "fork",
            "repair",
            "retry",
            "runs",
            "cancel",
            "inspect",
            "effects",
            "approvals",
            "providers",
            "auth",
            "schema",
            "migrate",
            "packs",
            "artifacts",
            "db",
            "memory",
            "gc",
            "completion",
            "version",
            "update",
        ] {
            assert!(names.contains(&expected), "missing {expected}");
        }
    }

    #[test]
    fn malformed_command_is_usage_exit_two() {
        let error = Cli::try_parse_from(["agentctl", "run"]).expect_err("missing file");
        assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
        assert_eq!(error.exit_code(), i32::from(EXIT_VALIDATION));
    }

    #[test]
    fn executable_suffix_does_not_change_help_reference() {
        let mut args = vec![OsString::from("agentctl.exe"), OsString::from("--help")];
        normalize_binary_name(&mut args);
        let error = Cli::try_parse_from(args).expect_err("help exits through clap");
        assert_eq!(error.kind(), ErrorKind::DisplayHelp);
        assert!(error.to_string().contains("Usage: agentctl "));
        assert!(!error.to_string().contains("agentctl.exe"));
    }

    #[test]
    fn requested_json_output_is_detected_before_clap_parsing() {
        assert_eq!(
            requested_output(&[
                OsString::from("agentctl"),
                OsString::from("--output"),
                OsString::from("json"),
                OsString::from("unknown"),
            ]),
            OutputFormat::Json
        );
        assert_eq!(
            requested_output(&[
                OsString::from("agentctl"),
                OsString::from("unknown"),
                OsString::from("--output=json"),
            ]),
            OutputFormat::Json
        );
        assert_eq!(
            requested_output(&[
                OsString::from("agentctl"),
                OsString::from("--output=jsonl"),
                OsString::from("run"),
            ]),
            OutputFormat::Jsonl
        );
    }

    #[test]
    fn machine_envelope_is_versioned() {
        let envelope = Envelope {
            api_version: MACHINE_OUTPUT_VERSION,
            kind: "Fixture",
            ok: true,
            data: serde_json::json!({"value": 1}),
            diagnostics: Vec::new(),
        };
        let value = serde_json::to_value(envelope).expect("serialize");
        assert_eq!(value["apiVersion"], MACHINE_OUTPUT_VERSION);
    }

    #[test]
    fn color_modes_are_tty_aware_and_json_is_selected_explicitly() {
        assert!(color_enabled(ColorMode::Always, false));
        assert!(color_enabled(ColorMode::Auto, true));
        assert!(!color_enabled(ColorMode::Auto, false));
        assert!(!color_enabled(ColorMode::Never, true));
        let cli = Cli::try_parse_from([
            "agentctl", "--output", "json", "--color", "never", "version",
        ])
        .expect("valid flags");
        assert_eq!(cli.output, OutputFormat::Json);
    }

    #[test]
    fn text_inputs_are_read_with_a_hard_size_limit() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("oversized.yaml");
        std::fs::write(&path, vec![b'x'; MAX_TEXT_FILE_BYTES as usize + 1])
            .expect("oversized fixture");
        let error = read_text(&path).expect_err("oversized input must fail");
        assert_eq!(error.code, EXIT_VALIDATION);
        assert!(error.message.contains("exceeds 1048576 bytes"));
    }

    #[tokio::test]
    async fn registry_preflights_only_reachable_provider_file_credentials() {
        let directory = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(directory.path().join("secrets")).expect("secret directory");
        std::fs::write(directory.path().join("secrets/openai"), "file-secret\n")
            .expect("secret file");
        let reachable = parse_workflow(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: reachable-secret }
spec:
  policy:
    secretFileRoots: [secrets]
    networkAllowlist: [api.openai.com]
  providers:
    openai:
      kind: openai
      credential: { file: secrets/openai }
  agents:
    answer:
      provider: openai
      model: gpt-5.6
      instructions: answer
  tasks:
    - id: answer
      uses: agent:answer
      with: { prompt: hello }
"#,
            "reachable.yaml",
        )
        .expect("reachable workflow")
        .workflow;
        build_registry(
            &reachable,
            directory.path(),
            &CancellationToken::new(),
            None,
        )
        .await
        .expect("file credential preflight");

        std::fs::remove_file(directory.path().join("secrets/openai")).expect("remove secret");
        let error = build_registry(
            &reachable,
            directory.path(),
            &CancellationToken::new(),
            None,
        )
        .await
        .err()
        .expect("reachable provider requires its credential");
        assert_eq!(error.code, EXIT_POLICY);
        assert!(error.message.contains("secret file"));

        let no_tasks = BTreeSet::new();
        build_registry(
            &reachable,
            directory.path(),
            &CancellationToken::new(),
            Some(&no_tasks),
        )
        .await
        .expect("reused provider task does not require its credential");

        let unused = parse_workflow(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: unused-secret }
spec:
  providers:
    unused:
      kind: openai
      credential: { env: AGENTCTL_INTENTIONALLY_MISSING_UNUSED_KEY }
  actions:
    assign: { kind: builtin.assign }
  tasks:
    - id: local
      uses: action:assign
      with: { value: local }
"#,
            "unused.yaml",
        )
        .expect("unused workflow")
        .workflow;
        build_registry(&unused, directory.path(), &CancellationToken::new(), None)
            .await
            .expect("unused provider does not require a credential");
    }
}
