use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{self, IsTerminal};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use agentctl_core::compiler::provider_capabilities;
use agentctl_core::diagnostic::Diagnostic;
use agentctl_core::dsl::{ProviderKind, SecretReference, Workflow, parse_workflow, schema_json};
use agentctl_core::pack::{PackManifest, verify_pack};
use agentctl_core::policy::PolicyEngine;
use agentctl_core::provider::{ContentBlock, Message, ModelProvider, ProviderRequest};
use agentctl_core::{MACHINE_OUTPUT_VERSION, compile};
use agentctl_protocols::{A2aClient, McpClient, ProtocolActionHandler, ProtocolHttpConfig};
use agentctl_providers::{
    AnthropicProvider, FakeProvider, GoogleProvider, HttpProviderConfig, OpenAiProvider,
};
use agentctl_runtime::{BuiltinToolExecutor, RunOptions, Runtime, RuntimeRegistry};
use agentctl_store::{ApprovalResolution, SqliteStore, StoreError};
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
static VERBOSE_OUTPUT: AtomicBool = AtomicBool::new(false);
static COLOR_OUTPUT: AtomicBool = AtomicBool::new(false);

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
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
enum OutputFormat {
    Human,
    Json,
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
    /// Durably request cancellation.
    Cancel(RunIdArgs),
    /// Inspect durable run, task, and audit state.
    Inspect(RunIdArgs),
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
struct ApprovalArgs {
    #[arg(long, default_value = ".agentctl/runtime.db")]
    db: PathBuf,
    #[command(subcommand)]
    command: ApprovalCommand,
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
    let args = std::env::args_os().collect::<Vec<_>>();
    let requested_output = requested_output(&args);
    let cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(error)
            if requested_output == OutputFormat::Json
                && !matches!(
                    error.kind(),
                    clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
                ) =>
        {
            let error = CliError::validation(error.to_string());
            render_error(OutputFormat::Json, &error);
            std::process::exit(i32::from(error.code));
        }
        Err(error) => error.exit(),
    };
    let output = cli.output;
    VERBOSE_OUTPUT.store(cli.verbose, Ordering::Relaxed);
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

fn requested_output(args: &[OsString]) -> OutputFormat {
    args.windows(2)
        .find_map(|pair| (pair[0] == "--output").then(|| pair[1].to_str()).flatten())
        .or_else(|| {
            args.iter()
                .find_map(|arg| arg.to_str().and_then(|arg| arg.strip_prefix("--output=")))
        })
        .filter(|value| *value == "json")
        .map_or(OutputFormat::Human, |_| OutputFormat::Json)
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
                    "plan {}\norder: {}\npredictability: {:?}\nproviders: {}\ntools: {}\neffects: {}",
                    plan.plan_digest,
                    plan.order.join(" -> "),
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
            let runtime = Runtime::new(store, current_dir()?);
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
            Ok(EXIT_OK)
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
            let registry = build_registry(&workflow, &base)?;
            let runtime = Runtime::new(store, &base).with_registry(registry);
            let cancellation = cancellation_token(args.timeout_seconds);
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
            let traces = store
                .trace_events(&args.run_id)
                .map_err(CliError::persistence)?;
            let human = format!(
                "{} {:?}; {} tasks; {} effects; {} checkpoints; {} audit events; {} traces",
                args.run_id,
                run.state,
                tasks.len(),
                effects.len(),
                checkpoints.len(),
                audit.len(),
                traces.len(),
            );
            let value = serde_json::json!({
                "run": run,
                "tasks": tasks,
                "effects": effects,
                "approvals": approvals,
                "checkpoints": checkpoints,
                "providerSessions": provider_sessions,
                "toolCalls": tool_calls,
                "audit": audit,
                "traces": traces,
            });
            print_value(output, "RunInspection", &value, Vec::new(), human)?;
            Ok(EXIT_OK)
        }
        Command::Approvals(args) => approval_command(output, args),
        Command::Providers(args) => provider_command(output, args).await,
        Command::Auth(args) => auth_command(output, args),
        Command::Schema(args) => schema_command(output, args),
        Command::Migrate(args) => migrate_command(output, args),
        Command::Packs(args) => pack_command(output, args),
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
                    "command": "cargo install --locked agentctl"
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
    let registry = build_registry(&workflow, &base)?;
    let store = open_store(&args.db)?;
    let runtime = Runtime::new(store, &base).with_registry(registry);
    let cancellation = cancellation_token(args.timeout_seconds);
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
    let registry = build_registry(&workflow, &base)?;
    let runtime = Runtime::new(store, &base).with_registry(registry);
    let cancellation = cancellation_token(args.timeout_seconds);
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
    Ok(match outcome.state {
        agentctl_core::state::RunState::Succeeded => EXIT_OK,
        agentctl_core::state::RunState::Paused => EXIT_POLICY,
        agentctl_core::state::RunState::Cancelled => EXIT_CANCELLED,
        agentctl_core::state::RunState::Failed => EXIT_RUN_FAILED,
        agentctl_core::state::RunState::Running => EXIT_RUN_FAILED,
    })
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

async fn provider_command(output: OutputFormat, args: ProviderArgs) -> Result<u8, CliError> {
    match args.command {
        ProviderCommand::Inspect(args) => {
            let (workflow, _, diagnostics) = load_and_compile(&args.file)?;
            let data = workflow
                .spec
                .providers
                .iter()
                .map(|(name, definition)| {
                    serde_json::json!({
                        "name": name,
                        "kind": definition.kind,
                        "capabilities": provider_capabilities(definition.kind.clone())
                            .into_iter()
                            .map(agentctl_core::compiler::ProviderCapability::as_str)
                            .collect::<Vec<_>>(),
                        "credentialConfigured": definition.credential.as_ref().is_some_and(|secret| std::env::var_os(&secret.env).is_some()),
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
    let status = workflow
        .spec
        .providers
        .iter()
        .map(|(name, definition)| {
            let env = definition
                .credential
                .as_ref()
                .map(|secret| secret.env.clone())
                .unwrap_or_else(|| default_credential_env(definition.kind.clone()).to_owned());
            serde_json::json!({"provider": name, "environment": env, "present": std::env::var_os(&env).is_some()})
        })
        .collect::<Vec<_>>();
    print_value(
        output,
        "AuthStatus",
        &status,
        diagnostics,
        format!(
            "checked {} credential reference(s); values were not read",
            status.len()
        ),
    )?;
    Ok(EXIT_OK)
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
                    "schema {}: {} runs, {} effects",
                    stats.schema_version, stats.runs, stats.effects
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
    load_packs(&mut workflow, path)?;
    let plan = compile(&workflow, &path.display().to_string()).map_err(diagnostics_error)?;
    Ok((workflow, plan, parsed.diagnostics))
}

fn load_packs(workflow: &mut Workflow, workflow_path: &Path) -> Result<(), CliError> {
    let base = workflow_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let canonical_base = std::fs::canonicalize(base)
        .map_err(|error| CliError::validation(format!("{}: {error}", base.display())))?;
    for reference in workflow.spec.packs.clone() {
        let relative = Path::new(&reference.path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(CliError::validation(format!(
                "pack `{}` path must remain under the workflow directory",
                reference.name
            )));
        }
        let path = base.join(relative);
        let canonical = std::fs::canonicalize(&path)
            .map_err(|error| CliError::validation(format!("{}: {error}", path.display())))?;
        if !canonical.starts_with(&canonical_base) {
            return Err(CliError::validation(format!(
                "pack `{}` resolves outside the workflow directory",
                reference.name
            )));
        }
        verify_pack(&canonical, &reference.integrity)
            .map_err(|error| CliError::validation(error.to_string()))?;
        let source = read_text(&canonical)?;
        let mut pack: PackManifest = serde_yaml_ng::from_str(&source)
            .map_err(|error| CliError::validation(format!("{}: {error}", canonical.display())))?;
        pack.validate()
            .map_err(|error| CliError::validation(error.to_string()))?;
        if pack.name != reference.name || pack.version != reference.version {
            return Err(CliError::validation(format!(
                "pack reference `{}@{}` does not match manifest `{}@{}`",
                reference.name, reference.version, pack.name, pack.version
            )));
        }
        let qualify = |name: &str| format!("{}.{}", pack.name, name);
        for agent in pack.agents.values_mut() {
            agent.tools = agent.tools.iter().map(|name| qualify(name)).collect();
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
    }
    Ok(())
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

fn build_registry(workflow: &Workflow, base: &Path) -> Result<RuntimeRegistry, CliError> {
    let mut registry = RuntimeRegistry::default();
    for (name, definition) in &workflow.spec.providers {
        let credential = definition
            .credential
            .clone()
            .unwrap_or_else(|| SecretReference {
                env: default_credential_env(definition.kind.clone()).to_owned(),
            });
        if definition.kind != ProviderKind::Fake && std::env::var_os(&credential.env).is_none() {
            return Err(CliError {
                code: EXIT_REMOTE,
                message: format!(
                    "provider `{name}` requires environment variable `{}`; configure it or run `agentctl auth check`",
                    credential.env
                ),
                diagnostics: Vec::new(),
                run_id: None,
                trace_id: None,
            });
        }
        match definition.kind {
            ProviderKind::Fake => {
                registry = registry.with_provider(name, Arc::new(FakeProvider::default()));
            }
            ProviderKind::Openai => {
                let mut config = HttpProviderConfig::openai(credential.env);
                config.headers = resolve_protocol_headers(&definition.headers, workflow)?;
                if let Some(endpoint) = &definition.endpoint {
                    config.endpoint = endpoint.clone();
                }
                registry = registry.with_provider(
                    name,
                    Arc::new(OpenAiProvider::new(config).map_err(remote_error)?),
                );
            }
            ProviderKind::Anthropic => {
                let mut config = HttpProviderConfig::anthropic(credential.env);
                config.headers = resolve_protocol_headers(&definition.headers, workflow)?;
                if let Some(endpoint) = &definition.endpoint {
                    config.endpoint = endpoint.clone();
                }
                registry = registry.with_provider(
                    name,
                    Arc::new(AnthropicProvider::new(config).map_err(remote_error)?),
                );
            }
            ProviderKind::Google => {
                let mut config = HttpProviderConfig::google(credential.env);
                config.headers = resolve_protocol_headers(&definition.headers, workflow)?;
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
                let config = HttpProviderConfig {
                    endpoint,
                    credential,
                    organization: None,
                    project: None,
                    api_version: definition
                        .api_version
                        .clone()
                        .or_else(|| Some("v1".to_owned())),
                    headers: resolve_protocol_headers(&definition.headers, workflow)?,
                };
                registry = registry.with_provider(
                    name,
                    Arc::new(OpenAiProvider::azure(config).map_err(remote_error)?),
                );
            }
        }
    }

    let tool_policy =
        PolicyEngine::new(workflow.spec.policy.clone(), base).map_err(|error| CliError {
            code: EXIT_POLICY,
            message: error.to_string(),
            diagnostics: Vec::new(),
            run_id: None,
            trace_id: None,
        })?;
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
    for (name, definition) in &workflow.spec.mcp_servers {
        let headers = resolve_protocol_headers(&definition.headers, workflow)?;
        let client = McpClient::new(ProtocolHttpConfig {
            url: Url::parse(&definition.url).map_err(|error| {
                CliError::validation(format!("MCP server `{name}` URL: {error}"))
            })?,
            headers,
            timeout: Duration::from_secs(definition.timeout_seconds),
        })
        .map_err(remote_error)?;
        mcp.insert(name.clone(), Arc::new(client));
    }
    let mut a2a = BTreeMap::new();
    for (name, definition) in &workflow.spec.a2a_peers {
        let headers = resolve_protocol_headers(&definition.headers, workflow)?;
        let client = A2aClient::new(ProtocolHttpConfig {
            url: Url::parse(&definition.card_url).map_err(|error| {
                CliError::validation(format!("A2A peer `{name}` card URL: {error}"))
            })?,
            headers,
            timeout: Duration::from_secs(definition.timeout_seconds),
        })
        .map_err(remote_error)?;
        a2a.insert(name.clone(), Arc::new(client));
    }
    if !mcp.is_empty() || !a2a.is_empty() {
        registry = registry.with_external_actions(Arc::new(ProtocolActionHandler::new(mcp, a2a)));
    }
    Ok(registry)
}

fn resolve_protocol_headers(
    headers: &BTreeMap<String, SecretReference>,
    workflow: &Workflow,
) -> Result<BTreeMap<String, String>, CliError> {
    headers
        .iter()
        .map(|(name, reference)| {
            if !workflow.spec.policy.environment_allowlist.contains(&reference.env) {
                return Err(CliError {
                    code: EXIT_POLICY,
                    message: format!(
                        "header `{name}` secret environment `{}` is not in policy.environmentAllowlist",
                        reference.env
                    ),
                    diagnostics: Vec::new(),
                    run_id: None,
                    trace_id: None,
                });
            }
            let value = std::env::var(&reference.env).map_err(|_| CliError {
                code: EXIT_POLICY,
                message: format!("required environment variable `{}` is unavailable", reference.env),
                diagnostics: Vec::new(),
                run_id: None,
                trace_id: None,
            })?;
            Ok((name.clone(), value))
        })
        .collect()
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
    std::fs::read_to_string(path)
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
        OutputFormat::Json => println!(
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
        OutputFormat::Json => {
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
            "cancel",
            "inspect",
            "approvals",
            "providers",
            "auth",
            "schema",
            "migrate",
            "packs",
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
}
