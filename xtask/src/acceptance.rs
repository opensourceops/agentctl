use std::collections::BTreeSet;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::process::{bounded_output, bounded_wait, configure_piped_command, output_diagnostics};

const VERIFY_TOKEN: &str = "AGENTCTL_MOCK_FIXTURE_VERIFIED";
const LIVE_VERIFY_TOKEN: &str = "AGENTCTL_LIVE_FIXTURE_VERIFIED";
const ACCEPTANCE_SCENARIOS: usize = 46;

pub fn run(root: &Path) -> Result<()> {
    command(root, "cargo", &["build", "-p", "agentctl-cli", "--locked"])?;
    let binary = debug_binary(root);
    let directory = tempfile::tempdir()?;
    let workspace = directory.path().join("workspace");
    fs::create_dir_all(workspace.join("artifacts"))?;

    scenario(1, "deterministic check, plan, and run");
    let hello = root.join("examples/v1/hello.yaml");
    successful_json(
        &binary,
        root,
        &strings([
            "check",
            path(&hello)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    let plan = successful_json(
        &binary,
        root,
        &strings([
            "plan",
            path(&hello)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure!(
        plan.pointer("/data/planDigest")
            .and_then(Value::as_str)
            .is_some()
    );
    let hello_db = directory.path().join("hello.db");
    let hello_run = successful_json(&binary, root, &run_args(&hello, &hello_db, root, &[]))?;
    ensure_eq(&hello_run, "/data/output/greeting", "hello, world")?;

    scenario(2, "invalid YAML includes structured location diagnostics");
    let invalid = workspace.join("invalid.yaml");
    write(&invalid, "apiVersion: [\n")?;
    let invalid_result = json_with_code(
        &binary,
        &workspace,
        &strings([
            "check",
            path(&invalid)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
        2,
    )?;
    ensure!(
        invalid_result["diagnostics"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );

    scenario(
        3,
        "missing references and capability violations fail compilation",
    );
    let missing = workspace.join("missing-reference.yaml");
    write(
        &missing,
        &deterministic_workflow("missing-reference", "action:not_declared", "{}"),
    )?;
    expect_code(
        &binary,
        &workspace,
        &strings(["check", path(&missing)?, "--output", "json"]),
        2,
    )?;
    expect_code(
        &binary,
        root,
        &strings([
            "check",
            path(&root.join("examples/v1/capability-failure.yaml"))?,
            "--output",
            "json",
        ]),
        2,
    )?;

    scenario(
        4,
        "unsupported provider options fail and stateless tools compile",
    );
    for (name, option) in [
        ("stream", "stream: true"),
        ("ptc", "programmaticToolCalling: true"),
    ] {
        let fixture = workspace.join(format!("unsupported-{name}.yaml"));
        write(&fixture, &agent_workflow("unsupported", option, "", ""))?;
        expect_code(
            &binary,
            &workspace,
            &strings(["check", path(&fixture)?, "--output", "json"]),
            2,
        )?;
    }
    let stateless_tools = workspace.join("openai-stateless-tools.yaml");
    write(&stateless_tools, OPENAI_STATELESS_TOOL_WORKFLOW)?;
    successful_json(
        &binary,
        &workspace,
        &strings(["check", path(&stateless_tools)?, "--output", "json"]),
    )?;

    scenario(
        5,
        "fake provider completion is executable and deterministic",
    );
    let fake_db = directory.path().join("fake.db");
    let fake = root.join("examples/v1/fake-provider.yaml");
    successful_json(&binary, root, &run_args(&fake, &fake_db, root, &[]))?;

    scenario(
        6,
        "fake provider tool call, continuation, schemas, traces, and artifact",
    );
    let mock_workspace = directory.path().join("mock-tool");
    copy_example(root, "examples/acceptance/mock-tool", &mock_workspace)?;
    let mock_db = mock_workspace.join("runtime.db");
    let mock_run = successful_json(
        &binary,
        &mock_workspace,
        &run_args(
            &mock_workspace.join("workflow.yaml"),
            &mock_db,
            &mock_workspace,
            &[],
        ),
    )?;
    ensure_eq(&mock_run, "/data/output/verdict", VERIFY_TOKEN)?;
    ensure!(fs::read_to_string(mock_workspace.join("artifacts/mock-report.txt"))? == VERIFY_TOKEN);
    let mock_id = string_at(&mock_run, "/data/runId")?;
    let mock_inspect = inspect(&binary, &mock_workspace, &mock_db, mock_id)?;
    ensure!(array_len(&mock_inspect, "/data/effects")? >= 3);
    ensure!(array_len(&mock_inspect, "/data/toolCalls")? == 1);
    ensure!(array_len(&mock_inspect, "/data/providerSessions")? == 1);
    ensure!(array_len(&mock_inspect, "/data/checkpoints")? > 0);
    ensure!(array_len(&mock_inspect, "/data/audit")? > 0);
    ensure!(array_len(&mock_inspect, "/data/traces")? > 0);
    ensure!(array_len(&mock_inspect, "/data/artifacts")? == 1);
    ensure!(
        mock_inspect
            .pointer("/data/toolCalls/0/callId")
            .and_then(Value::as_str)
            != mock_inspect
                .pointer("/data/toolCalls/0/effectId")
                .and_then(Value::as_str)
    );
    let artifact_digest = string_at(&mock_inspect, "/data/artifacts/0/digest")?;
    let artifact_list = successful_json(
        &binary,
        &mock_workspace,
        &strings([
            "artifacts",
            "--db",
            path(&mock_db)?,
            "list",
            "--run",
            mock_id,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure!(array_len(&artifact_list, "/data/references")? == 1);
    ensure!(array_len(&artifact_list, "/data/blobs")? == 1);
    let artifact_inspect = successful_json(
        &binary,
        &mock_workspace,
        &strings([
            "artifacts",
            "--db",
            path(&mock_db)?,
            "inspect",
            artifact_digest,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&artifact_inspect, "/data/blob/referenceCount", 1_u64)?;
    fs::remove_file(mock_workspace.join("artifacts/mock-report.txt"))?;
    let artifact_verify = successful_json(
        &binary,
        &mock_workspace,
        &strings([
            "artifacts",
            "--db",
            path(&mock_db)?,
            "verify",
            artifact_digest,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&artifact_verify, "/data/valid", true)?;
    let exported_artifact = mock_workspace.join("exports/mock-report.txt");
    successful_json(
        &binary,
        &mock_workspace,
        &strings([
            "artifacts",
            "--db",
            path(&mock_db)?,
            "export",
            artifact_digest,
            path(&exported_artifact)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure!(fs::read_to_string(exported_artifact)? == VERIFY_TOKEN);

    scenario(
        7,
        "tool input and output schema failures retain run and trace metadata",
    );
    let bad_input = workspace.join("bad-tool-input.yaml");
    write(
        &bad_input,
        &agent_workflow(
            "bad-input",
            "toolInput: { text: 7 }",
            echo_tool(),
            "finalText: never",
        ),
    )?;
    let bad_input_error = run_error(
        &binary,
        &workspace,
        &bad_input,
        directory.path(),
        "bad-input.db",
    )?;
    assert_error_metadata(&bad_input_error)?;
    let bad_output = workspace.join("bad-tool-output.yaml");
    write(
        &bad_output,
        &agent_workflow(
            "bad-output",
            "toolInput: { text: hello }",
            mismatched_echo_tool(),
            "finalText: never",
        ),
    )?;
    let bad_output_error = run_error(
        &binary,
        &workspace,
        &bad_output,
        directory.path(),
        "bad-output.db",
    )?;
    assert_error_metadata(&bad_output_error)?;
    let invalid_utf8 = workspace.join("fixture-invalid.bin");
    fs::write(&invalid_utf8, [0xff, 0xfe])?;
    let invalid_utf8_workflow = workspace.join("invalid-utf8-tool.yaml");
    write(
        &invalid_utf8_workflow,
        &read_tool_workflow("invalid-utf8", "fixture-invalid.bin"),
    )?;
    assert_error_metadata(&run_error(
        &binary,
        &workspace,
        &invalid_utf8_workflow,
        directory.path(),
        "invalid-utf8.db",
    )?)?;
    let oversized = workspace.join("fixture-oversized.txt");
    fs::write(&oversized, vec![b'x'; 1_048_577])?;
    let oversized_workflow = workspace.join("oversized-tool.yaml");
    write(
        &oversized_workflow,
        &read_tool_workflow("oversized", "fixture-oversized.txt"),
    )?;
    assert_error_metadata(&run_error(
        &binary,
        &workspace,
        &oversized_workflow,
        directory.path(),
        "oversized.db",
    )?)?;

    scenario(8, "policy denial prevents filesystem mutation");
    let denied_db = directory.path().join("denied.db");
    expect_code(
        &binary,
        root,
        &run_args(
            &root.join("examples/v1/policy-denial.yaml"),
            &denied_db,
            root,
            &[],
        ),
        4,
    )?;
    ensure!(!root.join("examples/v1/artifacts/denied.txt").exists());

    scenario(9, "non-TTY approval pauses durably with exit 3");
    let approval_workflow = workspace.join("approval.yaml");
    write(&approval_workflow, APPROVAL_WORKFLOW)?;
    let approval_db = directory.path().join("approval.db");
    let paused = json_with_code(
        &binary,
        &workspace,
        &run_args(&approval_workflow, &approval_db, &workspace, &[]),
        3,
    )?;
    ensure_eq(&paused, "/data/state", "paused")?;
    let paused_id = string_at(&paused, "/data/runId")?;
    let list = approvals(&binary, &workspace, &approval_db, paused_id)?;
    ensure!(array_len(&list, "/data")? == 1);
    let approval_id = string_at(&list, "/data/0/approvalId")?;

    scenario(
        10,
        "approve then resume executes the confirmed effect exactly once",
    );
    successful_json(
        &binary,
        &workspace,
        &strings([
            "approvals",
            "--db",
            path(&approval_db)?,
            "approve",
            approval_id,
            "--reason",
            "acceptance approved",
            "--output",
            "json",
        ]),
    )?;
    let resumed = successful_json(
        &binary,
        &workspace,
        &strings([
            "resume",
            paused_id,
            "--db",
            path(&approval_db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&resumed, "/data/state", "succeeded")?;
    ensure!(fs::read_to_string(workspace.join("artifacts/approved.txt"))? == "approved");

    scenario(11, "rejection blocks resume and leaves no artifact");
    fs::remove_file(workspace.join("artifacts/approved.txt"))?;
    let reject_db = directory.path().join("reject.db");
    let rejected_run = json_with_code(
        &binary,
        &workspace,
        &run_args(&approval_workflow, &reject_db, &workspace, &[]),
        3,
    )?;
    let rejected_id = string_at(&rejected_run, "/data/runId")?;
    let rejected_list = approvals(&binary, &workspace, &reject_db, rejected_id)?;
    let rejected_approval = string_at(&rejected_list, "/data/0/approvalId")?;
    successful_json(
        &binary,
        &workspace,
        &strings([
            "approvals",
            "--db",
            path(&reject_db)?,
            "reject",
            rejected_approval,
            "--reason",
            "acceptance rejected",
            "--output",
            "json",
        ]),
    )?;
    let rejected = json_with_code(
        &binary,
        &workspace,
        &strings([
            "resume",
            rejected_id,
            "--db",
            path(&reject_db)?,
            "--output",
            "json",
        ]),
        4,
    )?;
    ensure_eq(&rejected, "/data/state", "failed")?;
    ensure_eq(&rejected, "/data/runId", rejected_id)?;
    ensure!(!workspace.join("artifacts/approved.txt").exists());

    scenario(
        12,
        "completed provider effects are not repeated across approval resume",
    );
    let confirmed_workflow = workspace.join("confirmed-before-approval.yaml");
    write(&confirmed_workflow, CONFIRMED_BEFORE_APPROVAL_WORKFLOW)?;
    let confirmed_db = directory.path().join("confirmed.db");
    let confirmed_paused = json_with_code(
        &binary,
        &workspace,
        &run_args(&confirmed_workflow, &confirmed_db, &workspace, &[]),
        3,
    )?;
    let confirmed_id = string_at(&confirmed_paused, "/data/runId")?;
    let before = inspect(&binary, &workspace, &confirmed_db, confirmed_id)?;
    ensure!(model_effects(&before) == 1);
    let confirmed_list = approvals(&binary, &workspace, &confirmed_db, confirmed_id)?;
    let confirmed_approval = string_at(&confirmed_list, "/data/0/approvalId")?;
    successful_json(
        &binary,
        &workspace,
        &strings([
            "approvals",
            "--db",
            path(&confirmed_db)?,
            "approve",
            confirmed_approval,
            "--reason",
            "continue",
            "--output",
            "json",
        ]),
    )?;
    successful_json(
        &binary,
        &workspace,
        &strings([
            "resume",
            confirmed_id,
            "--db",
            path(&confirmed_db)?,
            "--output",
            "json",
        ]),
    )?;
    let after = inspect(&binary, &workspace, &confirmed_db, confirmed_id)?;
    ensure!(model_effects(&after) == 1);

    scenario(13, "recorded replay is keyless and creates no effects");
    let replay = json_with_removed_env(
        &binary,
        &mock_workspace,
        &strings([
            "replay",
            mock_id,
            "--db",
            path(&mock_db)?,
            "--output",
            "json",
        ]),
        "OPENAI_API_KEY",
        0,
    )?;
    let replay_id = string_at(&replay, "/data/runId")?;
    ensure!(replay_id != mock_id);
    let replay_inspect = inspect(&binary, &mock_workspace, &mock_db, replay_id)?;
    ensure!(array_len(&replay_inspect, "/data/effects")? == 0);
    ensure!(array_len(&replay_inspect, "/data/artifacts")? == 1);
    let failed_replay = json_with_code(
        &binary,
        &workspace,
        &strings([
            "replay",
            rejected_id,
            "--db",
            path(&reject_db)?,
            "--output",
            "json",
        ]),
        4,
    )?;
    ensure_eq(&failed_replay, "/data/state", "failed")?;

    scenario(14, "fork creates a distinct run with fresh effects");
    let fork = successful_json(
        &binary,
        &mock_workspace,
        &strings(["fork", mock_id, "--db", path(&mock_db)?, "--output", "json"]),
    )?;
    let fork_id = string_at(&fork, "/data/runId")?;
    ensure!(fork_id != mock_id);
    let fork_inspect = inspect(&binary, &mock_workspace, &mock_db, fork_id)?;
    ensure!(array_len(&fork_inspect, "/data/effects")? >= 3);

    scenario(
        15,
        "provider timeout is bounded and ambiguous effects block resume",
    );
    let timeout_workflow = workspace.join("timeout.yaml");
    write(
        &timeout_workflow,
        &agent_workflow("timeout", "delayMs: 2500", "", ""),
    )?;
    let timeout_db = directory.path().join("timeout.db");
    let timeout_error = json_with_code(
        &binary,
        &workspace,
        &run_args(&timeout_workflow, &timeout_db, &workspace, &[]),
        4,
    )?;
    assert_error_metadata(&timeout_error)?;
    let timeout_id = string_at(&timeout_error, "/error/runId")?;
    let timeout_inspect = inspect(&binary, &workspace, &timeout_db, timeout_id)?;
    ensure_eq(&timeout_inspect, "/data/effects/0/status", "uncertain")?;
    let resume_error = json_with_code(
        &binary,
        &workspace,
        &strings([
            "resume",
            timeout_id,
            "--db",
            path(&timeout_db)?,
            "--output",
            "json",
        ]),
        3,
    )?;
    ensure_eq(&resume_error, "/error/runId", timeout_id)?;
    ensure!(
        resume_error
            .pointer("/error/traceId")
            .and_then(Value::as_str)
            .is_some()
    );

    scenario(
        16,
        "bounded same-run retry recovers a definitive transient provider failure",
    );
    let retry_workflow = workspace.join("retry.yaml");
    write(&retry_workflow, RETRY_WORKFLOW)?;
    let retry_db = directory.path().join("retry.db");
    let retry = successful_json(
        &binary,
        &workspace,
        &run_args(&retry_workflow, &retry_db, &workspace, &[]),
    )?;
    let retry_id = string_at(&retry, "/data/runId")?;
    let retry_inspect = inspect(&binary, &workspace, &retry_db, retry_id)?;
    ensure!(model_effects(&retry_inspect) == 2);

    scenario(
        17,
        "missing credential fails before a database or run is created",
    );
    let auth_workflow = workspace.join("missing-auth.yaml");
    write(&auth_workflow, OPENAI_AUTH_WORKFLOW)?;
    let auth_db = directory.path().join("missing-auth.db");
    let auth = json_with_removed_env(
        &binary,
        &workspace,
        &run_args(&auth_workflow, &auth_db, &workspace, &[]),
        "OPENAI_API_KEY",
        6,
    )?;
    ensure_eq(&auth, "/error/exitCode", 6_u64)?;
    ensure!(!auth_db.exists());

    scenario(
        18,
        "JSON and human output contracts are stable and color-safe",
    );
    let version = successful_json(
        &binary,
        root,
        &strings(["version", "--output", "json", "--color", "never"]),
    )?;
    ensure_eq(&version, "/apiVersion", "agentctl.dev/cli/v1")?;
    for arguments in [
        strings(["unknown", "--output", "json"]),
        strings(["run", "--output", "json"]),
        strings(["--output", "json", "--color", "invalid", "version"]),
    ] {
        let error = json_with_code(&binary, root, &arguments, 2)?;
        ensure_eq(&error, "/apiVersion", "agentctl.dev/cli/v1")?;
        ensure_eq(&error, "/error/exitCode", 2_u64)?;
    }
    let human = output_with_code(
        command_for(
            &binary,
            root,
            &strings(["version", "--output", "human", "--color", "never"]),
        ),
        0,
        "human output",
    )?;
    ensure!(!human.stdout.contains(&27));

    scenario(
        19,
        "input files and repeated KEY=VALUE overrides compose predictably",
    );
    let inputs_workflow = workspace.join("inputs.yaml");
    write(&inputs_workflow, INPUTS_WORKFLOW)?;
    let inputs_file = workspace.join("inputs.json");
    write(&inputs_file, r#"{"name":"file","count":2}"#)?;
    let inputs_db = directory.path().join("inputs.db");
    let input_run = successful_json(
        &binary,
        &workspace,
        &run_args(
            &inputs_workflow,
            &inputs_db,
            &workspace,
            &[
                "--inputs-file".to_owned(),
                path(&inputs_file)?.to_owned(),
                "--input".to_owned(),
                "count=3".to_owned(),
            ],
        ),
    )?;
    ensure_eq(&input_run, "/data/output/name", "file")?;
    ensure_eq(&input_run, "/data/output/count", 3_u64)?;

    scenario(
        20,
        "artifact traversal is rejected without writing outside the workspace",
    );
    let traversal = workspace.join("traversal.yaml");
    write(&traversal, TRAVERSAL_WORKFLOW)?;
    let traversal_db = directory.path().join("traversal.db");
    expect_code(
        &binary,
        &workspace,
        &run_args(&traversal, &traversal_db, &workspace, &[]),
        4,
    )?;
    ensure!(!directory.path().join("escaped.txt").exists());
    read_only_write_acceptance(&binary, directory.path())?;

    scenario(21, "concurrent runs share one SQLite database safely");
    let concurrent_db = directory.path().join("concurrent.db");
    successful_json(
        &binary,
        root,
        &strings([
            "db",
            "--db",
            path(&concurrent_db)?,
            "migrate",
            "--output",
            "json",
        ]),
    )?;
    let arguments = run_args(&hello, &concurrent_db, root, &[]);
    let mut first_command = command_for(&binary, root, &arguments);
    configure_piped_command(&mut first_command);
    let mut second_command = command_for(&binary, root, &arguments);
    configure_piped_command(&mut second_command);
    let first = first_command.spawn()?;
    let second = second_command.spawn()?;
    let first_wait = thread::spawn(move || bounded_wait(first, "first concurrent agentctl run"));
    let second_wait = thread::spawn(move || bounded_wait(second, "second concurrent agentctl run"));
    ensure!(
        first_wait
            .join()
            .map_err(|_| anyhow::anyhow!("first concurrent wait panicked"))??
            .status
            .success()
    );
    ensure!(
        second_wait
            .join()
            .map_err(|_| anyhow::anyhow!("second concurrent wait panicked"))??
            .status
            .success()
    );

    scenario(22, "SIGTERM produces a durable cancelled run");
    signal_acceptance(&binary, &workspace, directory.path())?;

    scenario(
        23,
        "copied packaged-style binary works outside the repository",
    );
    let isolated = directory.path().join("isolated");
    fs::create_dir_all(isolated.join("bin"))?;
    let copied = isolated.join("bin/agentctl");
    fs::copy(&binary, &copied)?;
    let help = output_with_code(
        command_for(&copied, &isolated, &strings(["--help"])),
        0,
        "isolated --help",
    )?;
    ensure!(!help.stdout.is_empty());
    successful_json(
        &copied,
        &isolated,
        &strings(["version", "--output", "json", "--color", "never"]),
    )?;
    successful_json(
        &copied,
        &isolated,
        &strings(["schema", "--output", "json", "--color", "never"]),
    )?;
    successful_json(
        &copied,
        &isolated,
        &strings([
            "providers",
            "inspect",
            path(&mock_workspace.join("workflow.yaml"))?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    let completion = output_with_code(
        command_for(&copied, &isolated, &strings(["completion", "zsh"])),
        0,
        "isolated completion",
    )?;
    ensure!(!completion.stdout.is_empty());

    scenario(
        24,
        "cron-like empty environment and non-TTY execution succeeds",
    );
    let cron_db = directory.path().join("cron.db");
    let mut cron = command_for(&copied, &isolated, &run_args(&hello, &cron_db, root, &[]));
    cron.env_clear();
    output_with_code(cron, 0, "cron-like run")?;

    scenario(
        25,
        "quickstart mock workflow succeeds from an isolated directory",
    );
    let quickstart = isolated.join("quickstart");
    copy_example(root, "examples/acceptance/mock-tool", &quickstart)?;
    let quickstart_db = quickstart.join("runtime.db");
    let quickstart_run = successful_json(
        &copied,
        &quickstart,
        &run_args(
            &quickstart.join("workflow.yaml"),
            &quickstart_db,
            &quickstart,
            &[],
        ),
    )?;
    ensure_eq(&quickstart_run, "/data/output/verdict", VERIFY_TOKEN)?;

    scenario(
        26,
        "selective repair plans reuse, executes the suffix, and exposes lineage",
    );
    let repair_source = workspace.join("repair-source.yaml");
    let repair_target = workspace.join("repair-target.yaml");
    write(&repair_source, SELECTIVE_REPAIR_SOURCE_WORKFLOW)?;
    write(&repair_target, SELECTIVE_REPAIR_TARGET_WORKFLOW)?;
    let repair_db = directory.path().join("repair.db");
    let source_failure = json_with_code(
        &binary,
        &workspace,
        &run_args(&repair_source, &repair_db, &workspace, &[]),
        4,
    )?;
    let source_run_id = string_at(&source_failure, "/error/runId")?;
    let repair_plan = successful_json(
        &binary,
        &workspace,
        &strings([
            "repair",
            path(&repair_target)?,
            source_run_id,
            "--from",
            "second",
            "--plan",
            "--db",
            path(&repair_db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&repair_plan, "/data/compatible", true)?;
    ensure_eq(&repair_plan, "/data/reusedTasks/0", "first")?;
    ensure_eq(&repair_plan, "/data/rerunTasks/0", "second")?;
    ensure_eq(&repair_plan, "/data/rerunTasks/1", "third")?;
    let repair = successful_json(
        &binary,
        &workspace,
        &strings([
            "repair",
            path(&repair_target)?,
            source_run_id,
            "--from",
            "second",
            "--reason",
            "acceptance fixture",
            "--db",
            path(&repair_db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&repair, "/data/state", "succeeded")?;
    ensure_eq(&repair, "/data/sourceRunId", source_run_id)?;
    ensure_eq(&repair, "/data/reusedTasks/0", "first")?;
    let repair_run_id = string_at(&repair, "/data/runId")?;
    let repair_inspect = inspect(&binary, &workspace, &repair_db, repair_run_id)?;
    ensure_eq(&repair_inspect, "/data/run/mode", "repair")?;
    ensure_eq(&repair_inspect, "/data/run/sourceRunId", source_run_id)?;
    ensure_eq(&repair_inspect, "/data/tasks/0/disposition", "reused")?;
    ensure_eq(&repair_inspect, "/data/tasks/1/disposition", "executed")?;
    ensure!(array_len(&repair_inspect, "/data/effects")? == 0);
    let human_inspect = output_with_code(
        command_for(
            &binary,
            &workspace,
            &strings([
                "inspect",
                repair_run_id,
                "--db",
                path(&repair_db)?,
                "--output",
                "human",
                "--color",
                "never",
            ]),
        ),
        0,
        "human repair inspection",
    )?;
    let human_inspect = String::from_utf8_lossy(&human_inspect.stdout);
    ensure!(human_inspect.contains(&format!("source={source_run_id}")));
    ensure!(human_inspect.contains("reused=first"));
    ensure!(human_inspect.contains("executed=second,third"));

    scenario(
        27,
        "blocked repair plans are parseable and create no partial run",
    );
    let incompatible = workspace.join("repair-incompatible.yaml");
    write(
        &incompatible,
        &SELECTIVE_REPAIR_TARGET_WORKFLOW.replace("value: durable", "value: changed"),
    )?;
    let blocked = json_with_code(
        &binary,
        &workspace,
        &strings([
            "repair",
            path(&incompatible)?,
            source_run_id,
            "--from",
            "second",
            "--plan",
            "--db",
            path(&repair_db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
        3,
    )?;
    ensure_eq(&blocked, "/data/compatible", false)?;
    ensure_eq(
        &blocked,
        "/data/blockedReuse/0/rule",
        "definition_fingerprint_mismatch",
    )?;

    scenario(
        28,
        "effect inspection and not-applied reconciliation unblock repair",
    );
    uncertain_repair_acceptance(&binary, &workspace, directory.path())?;

    scenario(
        29,
        "legacy dry-run analysis and transactional upgrade expose a safe repair root",
    );
    let connection = rusqlite::Connection::open(&repair_db)?;
    connection.execute(
        "UPDATE task_states SET metadata_version = NULL, definition_fingerprint = NULL, input_digest = NULL, output_contract_fingerprint = NULL, output_digest = NULL, state_delta_json = NULL, state_delta_digest = NULL, artifact_manifest_json = NULL, reuse_decision_json = NULL WHERE run_id = ?1 AND task_id = 'first'",
        [source_run_id],
    )?;
    drop(connection);
    let legacy_analysis = successful_json(
        &binary,
        &workspace,
        &strings([
            "runs",
            "--db",
            path(&repair_db)?,
            "upgrade",
            source_run_id,
            "--dry-run",
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&legacy_analysis, "/data/upgradeableTasks/0", "first")?;
    ensure_eq(&legacy_analysis, "/data/recommendedRepairRoots/0", "second")?;
    let still_legacy = successful_json(
        &binary,
        &workspace,
        &strings([
            "runs",
            "--db",
            path(&repair_db)?,
            "analyze",
            source_run_id,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&still_legacy, "/data/upgradeableTasks/0", "first")?;
    let upgraded = successful_json(
        &binary,
        &workspace,
        &strings([
            "runs",
            "--db",
            path(&repair_db)?,
            "upgrade",
            source_run_id,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&upgraded, "/data/upgradedTasks/0", "first")?;
    ensure_eq(
        &upgraded,
        "/data/analysisAfter/recommendedRepairRoots/0",
        "second",
    )?;

    scenario(
        30,
        "terminal retry reuses the successful prefix and creates distinct lineage",
    );
    let terminal_retry_workflow = workspace.join("terminal-retry.yaml");
    write(&terminal_retry_workflow, TERMINAL_RETRY_WORKFLOW)?;
    let terminal_retry_db = directory.path().join("terminal-retry.db");
    let source = json_with_code(
        &binary,
        &workspace,
        &run_args(
            &terminal_retry_workflow,
            &terminal_retry_db,
            &workspace,
            &[],
        ),
        4,
    )?;
    let source_id = string_at(&source, "/error/runId")?;
    let retry_plan = successful_json(
        &binary,
        &workspace,
        &strings([
            "retry",
            path(&terminal_retry_workflow)?,
            source_id,
            "--failed",
            "--plan",
            "--db",
            path(&terminal_retry_db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&retry_plan, "/data/compatible", true)?;
    ensure_eq(&retry_plan, "/data/failedOnly", true)?;
    ensure_eq(&retry_plan, "/data/retryRoots/0", "work")?;
    ensure_eq(&retry_plan, "/data/reusedTasks/0", "first")?;
    ensure_eq(&retry_plan, "/data/rerunTasks/0", "work")?;
    ensure_eq(&retry_plan, "/data/rerunTasks/1", "third")?;
    let retried = successful_json(
        &binary,
        &workspace,
        &strings([
            "retry",
            path(&terminal_retry_workflow)?,
            source_id,
            "--failed",
            "--reason",
            "acceptance terminal retry",
            "--db",
            path(&terminal_retry_db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&retried, "/data/state", "succeeded")?;
    ensure_eq(&retried, "/data/sourceRunId", source_id)?;
    ensure_eq(&retried, "/data/reusedTasks/0", "first")?;
    ensure_eq(&retried, "/data/executedTasks/0", "work")?;
    ensure_eq(&retried, "/data/executedTasks/1", "third")?;
    let retry_run_id = string_at(&retried, "/data/runId")?;
    let retry_inspect = inspect(&binary, &workspace, &terminal_retry_db, retry_run_id)?;
    ensure_eq(&retry_inspect, "/data/run/mode", "retry")?;
    ensure_eq(&retry_inspect, "/data/run/sourceRunId", source_id)?;
    ensure_eq(&retry_inspect, "/data/run/retryFailedOnly", true)?;
    ensure_eq(&retry_inspect, "/data/run/retryRoots/0", "work")?;
    ensure_eq(&retry_inspect, "/data/tasks/0/disposition", "reused")?;
    ensure_eq(&retry_inspect, "/data/tasks/1/disposition", "executed")?;
    ensure_eq(&retry_inspect, "/data/tasks/2/disposition", "executed")?;
    let source_inspect = inspect(&binary, &workspace, &terminal_retry_db, source_id)?;
    ensure_eq(&source_inspect, "/data/run/state", "failed")?;
    let replay = successful_json(
        &binary,
        &workspace,
        &strings([
            "replay",
            retry_run_id,
            "--db",
            path(&terminal_retry_db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    let replay_id = string_at(&replay, "/data/runId")?;
    let replay_inspect = inspect(&binary, &workspace, &terminal_retry_db, replay_id)?;
    ensure!(array_len(&replay_inspect, "/data/effects")? == 0);

    scenario(
        31,
        "state encryption migrates, rotates, inspects, and replays through the packaged CLI",
    );
    let encrypted_db = directory.path().join("encrypted.db");
    let encrypted_source = successful_json(
        &binary,
        &workspace,
        &run_args(&hello, &encrypted_db, &workspace, &[]),
    )?;
    let encrypted_source_id = string_at(&encrypted_source, "/data/runId")?;
    let key_one = STANDARD.encode([31_u8; 32]);
    let key_two = STANDARD.encode([47_u8; 32]);
    let key_one_env = "AGENTCTL_TEST_STATE_KEY_ONE";
    let key_two_env = "AGENTCTL_TEST_STATE_KEY_TWO";
    let encryption_plan = json_with_env(
        &binary,
        &workspace,
        &strings([
            "db",
            "--db",
            path(&encrypted_db)?,
            "encryption",
            "enable",
            "--key-id",
            "acceptance-key-one",
            "--key-env",
            key_one_env,
            "--dry-run",
            "--output",
            "json",
            "--color",
            "never",
        ]),
        &[(key_one_env, key_one.as_str())],
        0,
    )?;
    ensure_eq(&encryption_plan, "/data/dryRun", true)?;
    let enabled = json_with_env(
        &binary,
        &workspace,
        &strings([
            "db",
            "--db",
            path(&encrypted_db)?,
            "encryption",
            "enable",
            "--key-id",
            "acceptance-key-one",
            "--key-env",
            key_one_env,
            "--output",
            "json",
            "--color",
            "never",
        ]),
        &[(key_one_env, key_one.as_str())],
        0,
    )?;
    ensure_eq(&enabled, "/data/operation", "enable")?;
    let inventory = json_with_env(
        &binary,
        &workspace,
        &strings([
            "db",
            "--db",
            path(&encrypted_db)?,
            "encryption",
            "inventory",
            "--output",
            "json",
            "--color",
            "never",
        ]),
        &[(key_one_env, key_one.as_str())],
        0,
    )?;
    ensure_eq(&inventory, "/data/enabled", true)?;
    ensure_eq(&inventory, "/data/keyId", "acceptance-key-one")?;
    ensure_eq(&inventory, "/data/plaintextValues", 0_u64)?;
    ensure_eq(&inventory, "/data/invalidEnvelopes", 0_u64)?;
    let encrypted_inspect = json_with_env(
        &binary,
        &workspace,
        &strings([
            "inspect",
            encrypted_source_id,
            "--db",
            path(&encrypted_db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
        &[(key_one_env, key_one.as_str())],
        0,
    )?;
    ensure_eq(&encrypted_inspect, "/data/run/runId", encrypted_source_id)?;
    let rotated = json_with_env(
        &binary,
        &workspace,
        &strings([
            "db",
            "--db",
            path(&encrypted_db)?,
            "encryption",
            "rotate",
            "--key-id",
            "acceptance-key-two",
            "--key-env",
            key_two_env,
            "--output",
            "json",
            "--color",
            "never",
        ]),
        &[
            (key_one_env, key_one.as_str()),
            (key_two_env, key_two.as_str()),
        ],
        0,
    )?;
    ensure_eq(&rotated, "/data/operation", "rotate")?;
    let encrypted_replay = json_with_env(
        &binary,
        &workspace,
        &strings([
            "replay",
            encrypted_source_id,
            "--db",
            path(&encrypted_db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
        &[(key_two_env, key_two.as_str())],
        0,
    )?;
    ensure_eq(&encrypted_replay, "/data/state", "succeeded")?;
    let raw_connection = rusqlite::Connection::open(&encrypted_db)?;
    let stored_inputs: String = raw_connection.query_row(
        "SELECT inputs_json FROM runs WHERE run_id = ?1",
        [encrypted_source_id],
        |row| row.get(0),
    )?;
    ensure!(stored_inputs.starts_with("agentctl.encrypted.v1:acceptance-key-two:"));
    ensure!(!serde_json::to_string(&inventory)?.contains(&key_one));
    ensure!(!serde_json::to_string(&rotated)?.contains(&key_two));

    scenario(
        32,
        "file and process secret providers are bounded, redacted, and never persisted",
    );
    let secret_workspace = workspace.join("secret-providers");
    fs::create_dir_all(secret_workspace.join("secrets"))?;
    let secret_marker = "agentctl-mounted-secret-acceptance-marker";
    fs::write(
        secret_workspace.join("secrets/token"),
        format!("{secret_marker}\n"),
    )?;
    let secret_workflow = secret_workspace.join("workflow.yaml");
    let binary_path = path(&binary)?;
    let secret_document = serde_json::json!({
        "apiVersion": "agentctl.dev/v1alpha1",
        "kind": "Workflow",
        "metadata": {"name": "secret-providers"},
        "spec": {
            "policy": {
                "workspaceRoot": ".",
                "processAllowlist": ["agentctl"],
                "environmentAllowlist": ["FILE_SECRET", "PROCESS_SECRET"],
                "secretFileRoots": ["secrets"],
                "secretProcessAllowlist": ["agentctl"],
                "approval": "never"
            },
            "actions": {
                "consume": {
                    "kind": "builtin.shell.exec",
                    "command": binary_path,
                    "args": ["version"],
                    "timeoutSeconds": 5,
                    "env": {
                        "FILE_SECRET": {"file": "secrets/token"},
                        "PROCESS_SECRET": {
                            "process": {
                                "command": binary_path,
                                "args": ["version"],
                                "timeoutSeconds": 5,
                                "outputLimitBytes": 128
                            }
                        }
                    }
                }
            },
            "tasks": [{"id": "consume", "uses": "action:consume"}]
        }
    });
    write(
        &secret_workflow,
        &serde_json::to_string_pretty(&secret_document)?,
    )?;
    let secret_db = directory.path().join("secret-providers.db");
    let secret_run = successful_json(
        &binary,
        &secret_workspace,
        &run_args(&secret_workflow, &secret_db, &secret_workspace, &[]),
    )?;
    let secret_run_id = string_at(&secret_run, "/data/runId")?;
    let secret_inspect = inspect(&binary, &secret_workspace, &secret_db, secret_run_id)?;
    let secret_inspect_text = serde_json::to_string(&secret_inspect)?;
    ensure!(secret_inspect_text.contains("[REDACTED]"));
    ensure!(!secret_inspect_text.contains(secret_marker));
    let database_bytes = fs::read(&secret_db)?;
    ensure!(
        !database_bytes
            .windows(secret_marker.len())
            .any(|window| window == secret_marker.as_bytes())
    );

    fs::remove_file(secret_workspace.join("secrets/token"))?;
    let missing_secret_db = directory.path().join("missing-secret-provider.db");
    let missing_secret = json_with_code(
        &binary,
        &secret_workspace,
        &run_args(&secret_workflow, &missing_secret_db, &secret_workspace, &[]),
        4,
    )?;
    ensure!(
        missing_secret
            .pointer("/error/message")
            .and_then(Value::as_str)
            .is_some_and(|message| message.contains("secret file"))
    );
    ensure!(!serde_json::to_string(&missing_secret)?.contains(secret_marker));

    scenario(
        33,
        "packaged CLI runs, inspects, and replays an atomic parallel batch",
    );
    let parallel = root.join("examples/v1/parallel.yaml");
    let parallel_plan = successful_json(
        &binary,
        root,
        &strings([
            "plan",
            path(&parallel)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&parallel_plan, "/data/maxConcurrency", 2_u64)?;
    ensure_eq(
        &parallel_plan,
        "/data/tasks/write-left/memoryWrites/0",
        "left",
    )?;
    ensure_eq(
        &parallel_plan,
        "/data/tasks/write-right/memoryWrites/0",
        "right",
    )?;
    let parallel_db = directory.path().join("parallel.db");
    let parallel_run =
        successful_json(&binary, root, &run_args(&parallel, &parallel_db, root, &[]))?;
    ensure_eq(&parallel_run, "/data/state", "succeeded")?;
    let parallel_run_id = string_at(&parallel_run, "/data/runId")?;
    let parallel_inspect = inspect(&binary, root, &parallel_db, parallel_run_id)?;
    ensure_eq(&parallel_inspect, "/data/run/workingMemory/left", "one")?;
    ensure_eq(&parallel_inspect, "/data/run/workingMemory/right", "two")?;
    let parallel_replay = successful_json(
        &binary,
        root,
        &strings([
            "replay",
            parallel_run_id,
            "--db",
            path(&parallel_db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    let parallel_replay_id = string_at(&parallel_replay, "/data/runId")?;
    let parallel_replay_inspect = inspect(&binary, root, &parallel_db, parallel_replay_id)?;
    ensure!(array_len(&parallel_replay_inspect, "/data/effects")? == 0);

    scenario(
        34,
        "packaged CLI expands, aggregates, inspects, and replays a bounded matrix",
    );
    let matrix = root.join("examples/v1/matrix.yaml");
    let matrix_plan = successful_json(
        &binary,
        root,
        &strings([
            "plan",
            path(&matrix)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    let matrix_children = matrix_plan
        .pointer("/data/tasks/verify/uses/name")
        .and_then(Value::as_array)
        .context("matrix aggregate child list")?;
    ensure!(matrix_children.len() == 4);
    let first_matrix_child = matrix_children[0]
        .as_str()
        .context("first matrix child ID")?;
    ensure!(first_matrix_child.starts_with("verify--0000-"));
    ensure_eq(
        &matrix_plan,
        &format!("/data/tasks/{first_matrix_child}/expansion/parent"),
        "verify",
    )?;
    let matrix_db = directory.path().join("matrix.db");
    let matrix_run = successful_json(&binary, root, &run_args(&matrix, &matrix_db, root, &[]))?;
    ensure_eq(&matrix_run, "/data/state", "succeeded")?;
    ensure_eq(
        &matrix_run,
        "/data/output/results/0/output/output/platform",
        "linux",
    )?;
    ensure_eq(
        &matrix_run,
        "/data/output/results/3/output/output/profile",
        "release",
    )?;
    let matrix_run_id = string_at(&matrix_run, "/data/runId")?;
    let matrix_inspect = inspect(&binary, root, &matrix_db, matrix_run_id)?;
    ensure!(array_len(&matrix_inspect, "/data/tasks")? == 5);
    let matrix_replay = successful_json(
        &binary,
        root,
        &strings([
            "replay",
            matrix_run_id,
            "--db",
            path(&matrix_db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    let matrix_replay_id = string_at(&matrix_replay, "/data/runId")?;
    let matrix_replay_inspect = inspect(&binary, root, &matrix_db, matrix_replay_id)?;
    ensure!(array_len(&matrix_replay_inspect, "/data/effects")? == 0);

    scenario(
        35,
        "packaged CLI persists, inspects, and replays a typed route decision",
    );
    let router = root.join("examples/v1/router.yaml");
    let router_plan = successful_json(
        &binary,
        root,
        &strings([
            "plan",
            path(&router)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&router_plan, "/data/tasks/route/uses/kind", "router")?;
    ensure_eq(
        &router_plan,
        "/data/tasks/ship/routeGuards/0/router",
        "route",
    )?;
    let router_db = directory.path().join("router.db");
    let router_run = successful_json(&binary, root, &run_args(&router, &router_db, root, &[]))?;
    ensure_eq(&router_run, "/data/state", "succeeded")?;
    ensure_eq(&router_run, "/data/output/selected", "ship")?;
    ensure_eq(&router_run, "/data/output/ship/output/result", "shipped")?;
    ensure_eq(
        &router_run,
        "/data/output/hold/reason",
        "route not selected",
    )?;
    let router_run_id = string_at(&router_run, "/data/runId")?;
    let router_inspect = inspect(&binary, root, &router_db, router_run_id)?;
    ensure_eq(
        &router_inspect,
        "/data/tasks/3/output/route/router",
        "route",
    )?;
    let router_replay = successful_json(
        &binary,
        root,
        &strings([
            "replay",
            router_run_id,
            "--db",
            path(&router_db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&router_replay, "/data/state", "succeeded")?;
    let router_replay_id = string_at(&router_replay, "/data/runId")?;
    let router_replay_inspect = inspect(&binary, root, &router_db, router_replay_id)?;
    ensure!(array_len(&router_replay_inspect, "/data/effects")? == 0);

    scenario(
        36,
        "packaged CLI runs, inspects, and replays bounded durable loop iterations",
    );
    let loop_workflow = root.join("examples/v1/loop.yaml");
    let loop_plan = successful_json(
        &binary,
        root,
        &strings([
            "plan",
            path(&loop_workflow)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&loop_plan, "/data/tasks/refine/uses/kind", "loop_aggregate")?;
    let loop_children = loop_plan
        .pointer("/data/tasks/refine/uses/name/children")
        .and_then(Value::as_array)
        .context("loop iteration child list")?;
    ensure!(loop_children.len() == 3);
    let first_loop_child = loop_children[0]
        .as_str()
        .context("first loop iteration ID")?;
    ensure!(first_loop_child.starts_with("refine--0000-"));
    let loop_db = directory.path().join("loop.db");
    let loop_run = successful_json(
        &binary,
        root,
        &run_args(&loop_workflow, &loop_db, root, &[]),
    )?;
    ensure_eq(&loop_run, "/data/state", "succeeded")?;
    ensure_eq(&loop_run, "/data/output/iterations", 2_u64)?;
    ensure_eq(
        &loop_run,
        "/data/output/results/1/output/output/iteration",
        1_u64,
    )?;
    ensure_eq(&loop_run, "/data/output/results/2/state", "skipped")?;
    let loop_run_id = string_at(&loop_run, "/data/runId")?;
    let loop_inspect = inspect(&binary, root, &loop_db, loop_run_id)?;
    ensure!(array_len(&loop_inspect, "/data/tasks")? == 4);
    let loop_replay = successful_json(
        &binary,
        root,
        &strings([
            "replay",
            loop_run_id,
            "--db",
            path(&loop_db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&loop_replay, "/data/state", "succeeded")?;
    let loop_replay_id = string_at(&loop_replay, "/data/runId")?;
    let loop_replay_inspect = inspect(&binary, root, &loop_db, loop_replay_id)?;
    ensure!(array_len(&loop_replay_inspect, "/data/effects")? == 0);

    scenario(
        37,
        "packaged CLI expands, inspects, and replays a typed sub-workflow",
    );
    let subworkflow = root.join("examples/v1/subworkflow.yaml");
    let subworkflow_plan = successful_json(
        &binary,
        root,
        &strings([
            "plan",
            path(&subworkflow)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(
        &subworkflow_plan,
        "/data/tasks/summary/uses/kind",
        "subworkflow_aggregate",
    )?;
    ensure_eq(
        &subworkflow_plan,
        "/data/tasks/summary/uses/name/version",
        "1.0.0",
    )?;
    ensure!(
        subworkflow_plan
            .pointer("/data/tasks")
            .and_then(Value::as_object)
            .is_some_and(|tasks| tasks.keys().any(|id| id.starts_with("summary--inputs-")))
    );
    let subworkflow_db = directory.path().join("subworkflow.db");
    let subworkflow_run = successful_json(
        &binary,
        root,
        &run_args(&subworkflow, &subworkflow_db, root, &[]),
    )?;
    ensure_eq(&subworkflow_run, "/data/state", "succeeded")?;
    ensure_eq(&subworkflow_run, "/data/output/result", "durable")?;
    let subworkflow_run_id = string_at(&subworkflow_run, "/data/runId")?;
    let subworkflow_inspect = inspect(&binary, root, &subworkflow_db, subworkflow_run_id)?;
    ensure!(array_len(&subworkflow_inspect, "/data/tasks")? == 4);
    let subworkflow_replay = successful_json(
        &binary,
        root,
        &strings([
            "replay",
            subworkflow_run_id,
            "--db",
            path(&subworkflow_db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&subworkflow_replay, "/data/state", "succeeded")?;
    let subworkflow_replay_id = string_at(&subworkflow_replay, "/data/runId")?;
    let subworkflow_replay_inspect =
        inspect(&binary, root, &subworkflow_db, subworkflow_replay_id)?;
    ensure!(array_len(&subworkflow_replay_inspect, "/data/effects")? == 0);

    scenario(
        38,
        "packaged CLI plans, executes, inspects, and replays compensation",
    );
    let compensation_workflow = workspace.join("compensation.yaml");
    write(&compensation_workflow, COMPENSATION_WORKFLOW)?;
    let compensation_db = directory.path().join("compensation.db");
    let source = json_with_code(
        &binary,
        root,
        &run_args(&compensation_workflow, &compensation_db, &workspace, &[]),
        4,
    )?;
    let source_run_id = string_at(&source, "/error/runId")?;
    let compensation_plan = successful_json(
        &binary,
        root,
        &strings([
            "compensate",
            source_run_id,
            "--plan",
            "--db",
            path(&compensation_db)?,
            "--workspace",
            path(&workspace)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&compensation_plan, "/data/executable", true)?;
    ensure_eq(
        &compensation_plan,
        "/data/tasks/0/sourceTaskId",
        "provision",
    )?;
    let compensation = successful_json(
        &binary,
        root,
        &strings([
            "compensate",
            source_run_id,
            "--db",
            path(&compensation_db)?,
            "--workspace",
            path(&workspace)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&compensation, "/data/state", "succeeded")?;
    ensure_eq(&compensation, "/data/compensatedTasks/0", "provision")?;
    ensure!(fs::read_to_string(workspace.join("artifacts/compensation.txt"))? == "compensated");
    let compensation_run_id = string_at(&compensation, "/data/runId")?;
    let compensation_inspect = inspect(&binary, root, &compensation_db, compensation_run_id)?;
    ensure_eq(
        &compensation_inspect,
        "/data/run/sourceRunId",
        source_run_id,
    )?;
    ensure_eq(&compensation_inspect, "/data/run/mode", "compensation")?;
    let source_inspect = inspect(&binary, root, &compensation_db, source_run_id)?;
    ensure_eq(
        &source_inspect,
        "/data/effectReconciliations/0/status",
        "compensated",
    )?;
    let compensation_replay = successful_json(
        &binary,
        root,
        &strings([
            "replay",
            compensation_run_id,
            "--db",
            path(&compensation_db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&compensation_replay, "/data/state", "succeeded")?;
    let repeat_plan = successful_json(
        &binary,
        root,
        &strings([
            "compensate",
            source_run_id,
            "--plan",
            "--db",
            path(&compensation_db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&repeat_plan, "/data/executable", false)?;
    ensure!(array_len(&repeat_plan, "/data/alreadyCompensatedEffects")? == 1);

    scenario(
        39,
        "packaged CLI preserves typed handoffs across retry and replay",
    );
    let structured_handoff = root.join("examples/v1/structured-handoff.yaml");
    let handoff_workspace = root.join("examples/v1");
    let handoff_db = directory.path().join("structured-handoff.db");
    let handoff_run = successful_json(
        &binary,
        root,
        &run_args(&structured_handoff, &handoff_db, &handoff_workspace, &[]),
    )?;
    ensure_eq(&handoff_run, "/data/state", "succeeded")?;
    ensure_eq(&handoff_run, "/data/output/decision", "approved")?;
    ensure_eq(
        &handoff_run,
        "/data/output/handoff/payload/evidence",
        "TEAM_EVIDENCE_VERIFIED",
    )?;
    let handoff_run_id = string_at(&handoff_run, "/data/runId")?;
    let handoff_inspect = inspect(&binary, root, &handoff_db, handoff_run_id)?;
    ensure_eq(&handoff_inspect, "/data/tasks/1/taskId", "handoff")?;
    ensure_eq(
        &handoff_inspect,
        "/data/tasks/1/output/output/payload/evidence",
        "TEAM_EVIDENCE_VERIFIED",
    )?;
    ensure!(array_len(&handoff_inspect, "/data/toolCalls")? == 1);
    let handoff_retry = successful_json(
        &binary,
        root,
        &strings([
            "retry",
            path(&structured_handoff)?,
            handoff_run_id,
            "--from",
            "review",
            "--restart-successful",
            "--db",
            path(&handoff_db)?,
            "--workspace",
            path(&handoff_workspace)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&handoff_retry, "/data/state", "succeeded")?;
    ensure_eq(&handoff_retry, "/data/reusedTasks/0", "collect")?;
    ensure_eq(&handoff_retry, "/data/reusedTasks/1", "handoff")?;
    ensure_eq(&handoff_retry, "/data/executedTasks/0", "review")?;
    ensure_eq(&handoff_retry, "/data/executedTasks/1", "verify")?;
    let handoff_retry_id = string_at(&handoff_retry, "/data/runId")?;
    let repaired_handoff = workspace.join("structured-handoff-repaired.yaml");
    let repaired_handoff_source = fs::read_to_string(&structured_handoff)?.replace(
        "Review only the typed handoff payload.",
        "Independently review only the typed handoff payload.",
    );
    write(&repaired_handoff, &repaired_handoff_source)?;
    let handoff_repair = successful_json(
        &binary,
        root,
        &strings([
            "repair",
            path(&repaired_handoff)?,
            handoff_run_id,
            "--from",
            "review",
            "--restart-successful",
            "--db",
            path(&handoff_db)?,
            "--workspace",
            path(&handoff_workspace)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&handoff_repair, "/data/state", "succeeded")?;
    ensure_eq(&handoff_repair, "/data/reusedTasks/0", "collect")?;
    ensure_eq(&handoff_repair, "/data/reusedTasks/1", "handoff")?;
    ensure_eq(&handoff_repair, "/data/executedTasks/0", "review")?;
    ensure_eq(&handoff_repair, "/data/executedTasks/1", "verify")?;
    let handoff_replay = successful_json(
        &binary,
        root,
        &strings([
            "replay",
            handoff_retry_id,
            "--db",
            path(&handoff_db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    let handoff_replay_id = string_at(&handoff_replay, "/data/runId")?;
    let handoff_replay_inspect = inspect(&binary, root, &handoff_db, handoff_replay_id)?;
    ensure!(array_len(&handoff_replay_inspect, "/data/effects")? == 0);

    scenario(
        40,
        "packaged CLI persists and replays bounded human and JSONL streams",
    );
    let streaming = root.join("examples/v1/streaming.yaml");
    let streaming_db = directory.path().join("streaming.db");
    let streaming_run = successful_json(
        &binary,
        root,
        &run_args(&streaming, &streaming_db, root, &[]),
    )?;
    ensure_eq(&streaming_run, "/data/output/result", "STREAM_COMPLETE")?;
    let streaming_run_id = string_at(&streaming_run, "/data/runId")?;
    let streaming_inspect = inspect(&binary, root, &streaming_db, streaming_run_id)?;
    ensure!(array_len(&streaming_inspect, "/data/streamEvents")? >= 3);

    let jsonl_db = directory.path().join("streaming-jsonl.db");
    let jsonl_output = output_with_code(
        command_for(
            &binary,
            root,
            &strings([
                "run",
                path(&streaming)?,
                "--db",
                path(&jsonl_db)?,
                "--workspace",
                path(root)?,
                "--output",
                "jsonl",
                "--color",
                "never",
            ]),
        ),
        0,
        "agentctl streaming JSONL",
    )?;
    ensure!(
        jsonl_output.stderr.iter().all(u8::is_ascii_whitespace),
        "JSONL streaming wrote unexpected stderr: {}",
        String::from_utf8_lossy(&jsonl_output.stderr)
    );
    let jsonl = String::from_utf8(jsonl_output.stdout)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    ensure!(jsonl.len() >= 4, "JSONL stream did not include progress");
    ensure!(
        jsonl[..jsonl.len() - 1]
            .iter()
            .all(|line| line["kind"] == "StreamEvent")
    );
    ensure_eq(
        jsonl.last().context("JSONL final line")?,
        "/kind",
        "RunOutcome",
    )?;
    let jsonl_run_id = string_at(jsonl.last().context("JSONL final line")?, "/data/runId")?;
    let replay_output = output_with_code(
        command_for(
            &binary,
            root,
            &strings([
                "replay",
                jsonl_run_id,
                "--db",
                path(&jsonl_db)?,
                "--output",
                "jsonl",
                "--color",
                "never",
            ]),
        ),
        0,
        "agentctl streaming replay JSONL",
    )?;
    let replay_jsonl = String::from_utf8(replay_output.stdout)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    ensure!(replay_jsonl[..replay_jsonl.len() - 1].iter().all(|line| {
        line["kind"] == "StreamEvent"
            && line.pointer("/data/sourceRunId").and_then(Value::as_str) == Some(jsonl_run_id)
    }));
    let replay_run_id = string_at(
        replay_jsonl.last().context("replay JSONL final line")?,
        "/data/runId",
    )?;
    let replay_inspect = inspect(&binary, root, &jsonl_db, replay_run_id)?;
    ensure!(array_len(&replay_inspect, "/data/effects")? == 0);

    let human_db = directory.path().join("streaming-human.db");
    let human_output = output_with_code(
        command_for(
            &binary,
            root,
            &strings([
                "run",
                path(&streaming)?,
                "--db",
                path(&human_db)?,
                "--workspace",
                path(root)?,
                "--output",
                "human",
                "--color",
                "never",
            ]),
        ),
        0,
        "agentctl streaming human",
    )?;
    ensure!(
        String::from_utf8_lossy(&human_output.stderr).contains("[stream stream"),
        "human stream progress was not written to stderr"
    );

    scenario(
        41,
        "packaged CLI safely reconnects MCP and continues A2A without resubmission",
    );
    let protocol_server = ProtocolFixtureServer::start()?;
    let protocol_workspace = directory.path().join("protocol-resilience");
    fs::create_dir_all(&protocol_workspace)?;
    let mcp_workflow = protocol_workspace.join("mcp.yaml");
    write(
        &mcp_workflow,
        &PROTOCOL_MCP_WORKFLOW.replace(
            "__MCP_URL__",
            &format!("http://{}/mcp", protocol_server.address()),
        ),
    )?;
    let mcp_db = protocol_workspace.join("mcp.db");
    let mcp_run = successful_json(
        &binary,
        &protocol_workspace,
        &run_args(&mcp_workflow, &mcp_db, &protocol_workspace, &[]),
    )?;
    ensure_eq(&mcp_run, "/data/state", "succeeded")?;
    let mcp_run_id = string_at(&mcp_run, "/data/runId")?;
    let mcp_inspect = inspect(&binary, &protocol_workspace, &mcp_db, mcp_run_id)?;
    ensure_eq(&mcp_inspect, "/data/protocolSessions/0/generation", 2_u64)?;
    ensure_eq(&mcp_inspect, "/data/protocolCalls/0/status", "succeeded")?;
    ensure!(
        mcp_inspect["data"]["streamEvents"]
            .as_array()
            .is_some_and(|events| events.iter().any(|event| {
                event.get("eventType").and_then(Value::as_str) == Some("mcp.reconnected")
            }))
    );
    ensure!(protocol_server.mcp_calls() == 2);

    let a2a_workflow = protocol_workspace.join("a2a.yaml");
    write(
        &a2a_workflow,
        &PROTOCOL_A2A_WORKFLOW.replace(
            "__A2A_CARD_URL__",
            &format!("http://{}/agent-card.json", protocol_server.address()),
        ),
    )?;
    let a2a_db = protocol_workspace.join("a2a.db");
    let interrupted = json_with_code(
        &binary,
        &protocol_workspace,
        &run_args(&a2a_workflow, &a2a_db, &protocol_workspace, &[]),
        4,
    )?;
    let a2a_source_id = string_at(&interrupted, "/error/runId")?;
    let a2a_source = inspect(&binary, &protocol_workspace, &a2a_db, a2a_source_id)?;
    ensure_eq(&a2a_source, "/data/effects/0/status", "uncertain")?;
    ensure_eq(
        &a2a_source,
        "/data/protocolCalls/0/state/remoteTaskId",
        "remote-task-1",
    )?;
    let a2a_effect_id = string_at(&a2a_source, "/data/effects/0/request/id")?;
    let continuation = successful_json(
        &binary,
        &protocol_workspace,
        &strings([
            "effects",
            "--db",
            path(&a2a_db)?,
            "continue-remote",
            a2a_effect_id,
            "--actor",
            "acceptance",
            "--reason",
            "resume persisted A2A task",
            "--approved",
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&continuation, "/data/status", "applied")?;
    ensure!(protocol_server.a2a_sends() == 1);
    let retry_plan = successful_json(
        &binary,
        &protocol_workspace,
        &strings([
            "retry",
            path(&a2a_workflow)?,
            a2a_source_id,
            "--failed",
            "--plan",
            "--db",
            path(&a2a_db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&retry_plan, "/data/reusedTasks/0", "delegate")?;
    ensure_eq(&retry_plan, "/data/rerunTasks/0", "consume")?;
    let retried = successful_json(
        &binary,
        &protocol_workspace,
        &strings([
            "retry",
            path(&a2a_workflow)?,
            a2a_source_id,
            "--failed",
            "--reason",
            "continue durable A2A boundary",
            "--db",
            path(&a2a_db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&retried, "/data/state", "succeeded")?;
    ensure_eq(&retried, "/data/reusedTasks/0", "delegate")?;
    ensure_eq(&retried, "/data/executedTasks/0", "consume")?;
    ensure!(protocol_server.a2a_sends() == 1);
    let retry_run_id = string_at(&retried, "/data/runId")?;
    let retry_inspect = inspect(&binary, &protocol_workspace, &a2a_db, retry_run_id)?;
    ensure_eq(&retry_inspect, "/data/protocolCalls/0/status", "recorded")?;
    ensure_eq(
        &retry_inspect,
        "/data/artifacts/0/logicalPath",
        "a2a/report/report.txt",
    )?;
    let replay = successful_json(
        &binary,
        &protocol_workspace,
        &strings([
            "replay",
            retry_run_id,
            "--db",
            path(&a2a_db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    let replay_run_id = string_at(&replay, "/data/runId")?;
    let replay_inspect = inspect(&binary, &protocol_workspace, &a2a_db, replay_run_id)?;
    ensure!(array_len(&replay_inspect, "/data/effects")? == 0);
    ensure_eq(&replay_inspect, "/data/protocolCalls/0/status", "recorded")?;

    scenario(
        42,
        "packaged CLI locks transitive packs and gates bounded process extensions",
    );
    let repository_pack = root.join("examples/v1/reusable-pack.yaml");
    let repository_lock = successful_json(
        &binary,
        root,
        &strings([
            "packs",
            "verify-lock",
            path(&repository_pack)?,
            "--locked",
            "--offline",
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&repository_lock, "/data/valid", true)?;
    ensure!(array_len(&repository_lock, "/data/packs")? == 2);

    let pack_workspace = directory.path().join("pack-extension");
    fs::create_dir_all(&pack_workspace)?;
    let extension = env::current_exe()?;
    let extension_command = serde_json::to_string(path(&extension)?)?;
    let extension_basename = extension
        .file_name()
        .and_then(OsStr::to_str)
        .context("xtask executable basename")?;
    let invocation_marker = pack_workspace.join("invocations");
    let marker_argument = serde_json::to_string(path(&invocation_marker)?)?;
    let extension_manifest = pack_workspace.join("extension.pack.yaml");
    write(
        &extension_manifest,
        &format!(
            r#"apiVersion: agentctl.dev/pack/v1alpha1
name: example.extension
version: 1.0.0
agentctl: ">=0.2.0, <1.0.0"
actions:
  transform:
    kind: extension.process
    command: {extension_command}
    args: [extension-fixture, {marker_argument}]
    protocolVersion: agentctl.dev/process-extension/v1
    idempotency: keyed
    capabilities: [transform]
    inputSchema:
      type: object
      required: [value]
      properties: {{ value: {{ type: string }} }}
      additionalProperties: false
    outputSchema:
      type: object
      required: [value]
      properties: {{ value: {{ type: string }} }}
      additionalProperties: false
"#
        ),
    )?;
    let extension_workflow = pack_workspace.join("workflow.yaml");
    let workflow_source = |allow_unsigned_process: bool| {
        format!(
            r#"apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: {{ name: process-extension-acceptance }}
spec:
  outputs: {{ value: "${{{{ tasks.extend.output.value }}}}" }}
  packTrust:
    unsigned: warn
    allowUnsignedProcess: {allow_unsigned_process}
  packs:
    - name: example.extension
      version: "^1.0"
      source: {{ path: extension.pack.yaml }}
  policy:
    workspaceRoot: .
    processAllowlist: [{extension_basename}]
    approval: never
  tasks:
    - id: extend
      uses: action:example.extension.transform
      with: {{ value: acceptance }}
"#
        )
    };
    write(&extension_workflow, &workflow_source(false))?;
    let locked = successful_json(
        &binary,
        &pack_workspace,
        &strings([
            "packs",
            "lock",
            path(&extension_workflow)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure!(array_len(&locked, "/data/lock/packs")? == 1);
    let denied = json_with_code(
        &binary,
        &pack_workspace,
        &strings([
            "run",
            path(&extension_workflow)?,
            "--db",
            path(&pack_workspace.join("denied.db"))?,
            "--workspace",
            path(&pack_workspace)?,
            "--locked",
            "--offline",
            "--output",
            "json",
            "--color",
            "never",
        ]),
        2,
    )?;
    ensure!(
        string_at(&denied, "/error/message")?.contains("declares process execution"),
        "unsigned process denial was not explicit"
    );
    ensure!(!invocation_marker.exists());

    write(&extension_workflow, &workflow_source(true))?;
    let extension_db = pack_workspace.join("extension.db");
    let extension_run = successful_json(
        &binary,
        &pack_workspace,
        &strings([
            "run",
            path(&extension_workflow)?,
            "--db",
            path(&extension_db)?,
            "--workspace",
            path(&pack_workspace)?,
            "--locked",
            "--offline",
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&extension_run, "/data/output/value", "extended")?;
    let extension_run_id = string_at(&extension_run, "/data/runId")?;
    let extension_inspect = inspect(&binary, &pack_workspace, &extension_db, extension_run_id)?;
    ensure_eq(
        &extension_inspect,
        "/data/effects/0/request/operation",
        "extension.process",
    )?;
    let extension_replay = successful_json(
        &binary,
        &pack_workspace,
        &strings([
            "replay",
            extension_run_id,
            "--db",
            path(&extension_db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&extension_replay, "/data/output/value", "extended")?;
    ensure!(fs::read_to_string(&invocation_marker)? == "invocation");

    scenario(
        43,
        "packaged CLI retrieves, promotes, repairs, and replays semantic memory",
    );
    let memory_workflow = root.join("examples/v1/long-term-memory.yaml");
    let memory_db = directory.path().join("semantic-memory.db");
    let memory_run = successful_json(
        &binary,
        root,
        &run_args(&memory_workflow, &memory_db, root, &[]),
    )?;
    ensure_eq(&memory_run, "/data/state", "succeeded")?;
    ensure!(array_len(&memory_run, "/data/output/recalled")? == 1);
    let memory_run_id = string_at(&memory_run, "/data/runId")?;
    let memory_inspect = inspect(&binary, root, &memory_db, memory_run_id)?;
    ensure!(count_task_items(&memory_inspect, "/data/effects", "search", true) == 1);
    ensure!(count_task_items(&memory_inspect, "/data/effects", "promote", true) == 1);
    successful_json(
        &binary,
        root,
        &strings([
            "memory",
            "--db",
            path(&memory_db)?,
            "put",
            "example",
            "repair",
            "\"hello durable repair\"",
            "--metadata",
            "{\"topic\":\"greeting\"}",
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    let memory_reindex = successful_json(
        &binary,
        root,
        &strings([
            "memory",
            "--db",
            path(&memory_db)?,
            "reindex",
            "example",
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&memory_reindex, "/data/entriesReindexed", 2)?;
    let memory_search = successful_json(
        &binary,
        root,
        &strings([
            "memory",
            "--db",
            path(&memory_db)?,
            "search",
            "example",
            "durable hello",
            "--mode",
            "hybrid",
            "--filter",
            "topic=\"greeting\"",
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure!(array_len(&memory_search, "/data/results")? == 2);
    let memory_repair = successful_json(
        &binary,
        root,
        &strings([
            "repair",
            path(&memory_workflow)?,
            memory_run_id,
            "--from",
            "search",
            "--restart-successful",
            "--db",
            path(&memory_db)?,
            "--workspace",
            path(root)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&memory_repair, "/data/state", "succeeded")?;
    ensure!(array_len(&memory_repair, "/data/output/recalled")? == 2);
    ensure_eq(&memory_repair, "/data/reusedTasks/0", "write")?;
    ensure_eq(&memory_repair, "/data/reusedTasks/1", "read")?;
    ensure_eq(&memory_repair, "/data/executedTasks/0", "search")?;
    ensure_eq(&memory_repair, "/data/executedTasks/1", "promote")?;
    let memory_repair_id = string_at(&memory_repair, "/data/runId")?;
    let memory_replay = successful_json(
        &binary,
        root,
        &strings([
            "replay",
            memory_repair_id,
            "--db",
            path(&memory_db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure!(array_len(&memory_replay, "/data/output/recalled")? == 2);
    let memory_replay_id = string_at(&memory_replay, "/data/runId")?;
    let memory_replay_inspect = inspect(&binary, root, &memory_db, memory_replay_id)?;
    ensure!(array_len(&memory_replay_inspect, "/data/effects")? == 0);

    scenario(
        44,
        "network preflight denies private destinations before persistence or I/O",
    );
    let network_denied_workflow = workspace.join("network-private-denied.yaml");
    write(&network_denied_workflow, NETWORK_PRIVATE_DENIED_WORKFLOW)?;
    let network_denied_db = directory.path().join("network-private-denied.db");
    let network_denied = json_with_code(
        &binary,
        &workspace,
        &run_args(
            &network_denied_workflow,
            &network_denied_db,
            &workspace,
            &[],
        ),
        3,
    )?;
    ensure!(
        string_at(&network_denied, "/error/message")?.contains("network address is not authorized")
    );
    ensure!(!network_denied_db.exists());

    scenario(
        45,
        "requested container backend fails closed before process effect dispatch",
    );
    let isolation_workflow = workspace.join("container-isolation-unavailable.yaml");
    write(
        &isolation_workflow,
        CONTAINER_ISOLATION_UNAVAILABLE_WORKFLOW,
    )?;
    let isolation_db = directory.path().join("container-isolation-unavailable.db");
    let empty_path = directory.path().join("empty-path");
    fs::create_dir(&empty_path)?;
    let isolation_failure = json_with_path(
        &binary,
        &workspace,
        &run_args(&isolation_workflow, &isolation_db, &workspace, &[]),
        &empty_path,
        4,
    )?;
    ensure!(
        string_at(&isolation_failure, "/error/message")?
            .contains("requested process isolation is unavailable")
    );
    let isolation_run_id = string_at(&isolation_failure, "/error/runId")?;
    let isolation_inspect = inspect(&binary, root, &isolation_db, isolation_run_id)?;
    ensure!(array_len(&isolation_inspect, "/data/effects")? == 0);

    scenario(
        46,
        "provider request budget terminates durably before the next dispatch",
    );
    let budget_workflow = workspace.join("resource-budget.yaml");
    write(&budget_workflow, RESOURCE_BUDGET_WORKFLOW)?;
    let budget_db = directory.path().join("resource-budget.db");
    let budget_failure = json_with_code(
        &binary,
        &workspace,
        &run_args(&budget_workflow, &budget_db, &workspace, &[]),
        4,
    )?;
    ensure!(string_at(&budget_failure, "/error/message")?.contains("providerRequests"));
    let budget_run_id = string_at(&budget_failure, "/error/runId")?;
    let budget_inspect = inspect(&binary, root, &budget_db, budget_run_id)?;
    ensure_eq(&budget_inspect, "/data/budget/usage/providerRequests", 1)?;
    ensure_eq(
        &budget_inspect,
        "/data/budget/exceeded/dimension",
        "providerRequests",
    )?;
    ensure_eq(&budget_inspect, "/data/budget/exceeded/limit", 1)?;
    ensure_eq(&budget_inspect, "/data/budget/exceeded/attempted", 2)?;
    ensure!(array_len(&budget_inspect, "/data/effects")? == 2);
    ensure_eq(&budget_inspect, "/data/effects/1/status", "requested")?;

    println!("agentctl credential-free acceptance passed ({ACCEPTANCE_SCENARIOS} scenarios)");
    Ok(())
}

pub fn completeness(root: &Path) -> Result<()> {
    super::package(root)?;
    let binary = packaged_binary(root)?;
    let directory = tempfile::tempdir()?;

    println!("[1/3] durable pipeline recovery composition");
    durable_composite_acceptance(root, &binary, directory.path())?;

    println!("[2/3] operational protocol and compensation composition");
    operational_composite_acceptance(root, &binary, directory.path())?;

    println!("[3/3] structured role handoff composition");
    structured_handoff_composite_acceptance(root, &binary, directory.path())?;

    println!(
        "agentctl framework completeness passed: packaged CLI, parallel matrix, typed agent output, approval, artifact CAS, retry, repair, offline replay, sub-workflow, loop, route, MCP reconnect, compensation, reconciliation, cancellation, and structured handoff"
    );
    Ok(())
}

fn durable_composite_acceptance(root: &Path, binary: &Path, directory: &Path) -> Result<()> {
    let workspace = directory.join("durable-pipeline");
    fs::create_dir_all(workspace.join("artifacts"))?;
    let source = workspace.join("durable-pipeline.yaml");
    let target = workspace.join("durable-pipeline-repaired.yaml");
    fs::copy(
        root.join("examples/framework-completeness/durable-pipeline.yaml"),
        &source,
    )?;
    fs::copy(
        root.join("examples/framework-completeness/durable-pipeline-repaired.yaml"),
        &target,
    )?;
    let db = workspace.join("runtime.db");

    for workflow in [&source, &target] {
        credential_free_json(
            binary,
            &workspace,
            &strings([
                "check",
                path(workflow)?,
                "--output",
                "json",
                "--color",
                "never",
            ]),
            0,
        )?;
        credential_free_json(
            binary,
            &workspace,
            &strings([
                "plan",
                path(workflow)?,
                "--output",
                "json",
                "--color",
                "never",
            ]),
            0,
        )?;
    }

    let paused = credential_free_json(
        binary,
        &workspace,
        &run_args(&source, &db, &workspace, &[]),
        3,
    )?;
    ensure_eq(&paused, "/data/state", "paused")?;
    let source_run_id = string_at(&paused, "/data/runId")?;
    let pending = credential_free_json(
        binary,
        &workspace,
        &strings([
            "approvals",
            "--db",
            path(&db)?,
            "list",
            source_run_id,
            "--output",
            "json",
            "--color",
            "never",
        ]),
        0,
    )?;
    ensure!(array_len(&pending, "/data")? == 1);
    let approval_id = string_at(&pending, "/data/0/approvalId")?;
    credential_free_json(
        binary,
        &workspace,
        &strings([
            "approvals",
            "--db",
            path(&db)?,
            "approve",
            approval_id,
            "--reason",
            "framework completeness acceptance",
            "--output",
            "json",
            "--color",
            "never",
        ]),
        0,
    )?;
    let failure = credential_free_json(
        binary,
        &workspace,
        &strings([
            "resume",
            source_run_id,
            "--db",
            path(&db)?,
            "--workspace",
            path(&workspace)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
        4,
    )?;
    ensure_eq(&failure, "/error/runId", source_run_id)?;
    let source_inspect = inspect_without_openai(binary, &workspace, &db, source_run_id)?;
    ensure_eq(&source_inspect, "/data/run/state", "failed")?;
    ensure!(array_len(&source_inspect, "/data/tasks")? == 10);
    ensure!(model_effects(&source_inspect) == 1);
    ensure!(array_len(&source_inspect, "/data/artifacts")? == 1);
    ensure_eq(
        &source_inspect,
        "/data/budget/usage/providerRequests",
        1_u64,
    )?;
    ensure_eq(&source_inspect, "/data/budget/usage/artifactBytes", 25_u64)?;
    let artifact_digest = string_at(&source_inspect, "/data/artifacts/0/digest")?;

    let retry_plan = credential_free_json(
        binary,
        &workspace,
        &strings([
            "retry",
            path(&source)?,
            source_run_id,
            "--failed",
            "--plan",
            "--db",
            path(&db)?,
            "--workspace",
            path(&workspace)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
        0,
    )?;
    ensure_eq(&retry_plan, "/data/compatible", true)?;
    ensure!(array_len(&retry_plan, "/data/reusedTasks")? == 9);
    ensure_eq(&retry_plan, "/data/reusedTasks/7", "analyze")?;
    ensure_eq(&retry_plan, "/data/reusedTasks/8", "publish")?;
    ensure_eq(&retry_plan, "/data/rerunTasks/0", "validate")?;
    let retry_failure = credential_free_json(
        binary,
        &workspace,
        &strings([
            "retry",
            path(&source)?,
            source_run_id,
            "--failed",
            "--reason",
            "verify failed-only terminal retry",
            "--db",
            path(&db)?,
            "--workspace",
            path(&workspace)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
        4,
    )?;
    let retry_run_id = string_at(&retry_failure, "/error/runId")?;
    let retry_inspect = inspect_without_openai(binary, &workspace, &db, retry_run_id)?;
    ensure!(model_effects(&retry_inspect) == 0);
    ensure!(array_len(&retry_inspect, "/data/artifacts")? == 1);
    ensure!(task_has(&retry_inspect, "validate", "failed", "executed"));
    ensure!(task_has(&retry_inspect, "analyze", "succeeded", "reused"));

    fs::remove_file(workspace.join("artifacts/framework-completeness.txt"))?;
    let repair_plan = credential_free_json(
        binary,
        &workspace,
        &strings([
            "repair",
            path(&target)?,
            source_run_id,
            "--from",
            "validate",
            "--plan",
            "--db",
            path(&db)?,
            "--workspace",
            path(&workspace)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
        0,
    )?;
    ensure_eq(&repair_plan, "/data/compatible", true)?;
    ensure!(array_len(&repair_plan, "/data/reusedTasks")? == 9);
    ensure_eq(&repair_plan, "/data/rerunTasks/0", "validate")?;
    let repair = credential_free_json(
        binary,
        &workspace,
        &strings([
            "repair",
            path(&target)?,
            source_run_id,
            "--from",
            "validate",
            "--reason",
            "fix the validation boundary",
            "--db",
            path(&db)?,
            "--workspace",
            path(&workspace)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
        0,
    )?;
    ensure_eq(&repair, "/data/state", "succeeded")?;
    ensure_eq(&repair, "/data/output/verdict", "DURABLE_PIPELINE_VERIFIED")?;
    ensure!(array_len(&repair, "/data/output/matrix")? == 4);
    ensure!(array_len(&repair, "/data/reusedTasks")? == 9);
    ensure_eq(&repair, "/data/executedTasks/0", "validate")?;
    let repair_run_id = string_at(&repair, "/data/runId")?;
    let repair_inspect = inspect_without_openai(binary, &workspace, &db, repair_run_id)?;
    ensure!(model_effects(&repair_inspect) == 0);
    ensure!(array_len(&repair_inspect, "/data/toolCalls")? == 0);
    ensure_eq(&repair_inspect, "/data/artifacts/0/digest", artifact_digest)?;

    let artifact_verify = credential_free_json(
        binary,
        &workspace,
        &strings([
            "artifacts",
            "--db",
            path(&db)?,
            "verify",
            artifact_digest,
            "--output",
            "json",
            "--color",
            "never",
        ]),
        0,
    )?;
    ensure_eq(&artifact_verify, "/data/valid", true)?;
    let exported = workspace.join("exports/framework-completeness.txt");
    credential_free_json(
        binary,
        &workspace,
        &strings([
            "artifacts",
            "--db",
            path(&db)?,
            "export",
            artifact_digest,
            path(&exported)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
        0,
    )?;
    ensure!(fs::read_to_string(exported)? == "DURABLE_PIPELINE_VERIFIED");

    let replay = credential_free_json(
        binary,
        &workspace,
        &strings([
            "replay",
            repair_run_id,
            "--db",
            path(&db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
        0,
    )?;
    ensure_eq(&replay, "/data/output/verdict", "DURABLE_PIPELINE_VERIFIED")?;
    let replay_run_id = string_at(&replay, "/data/runId")?;
    let replay_inspect = inspect_without_openai(binary, &workspace, &db, replay_run_id)?;
    ensure!(array_len(&replay_inspect, "/data/effects")? == 0);
    ensure!(array_len(&replay_inspect, "/data/toolCalls")? == 0);
    ensure!(array_len(&replay_inspect, "/data/providerSessions")? == 0);
    ensure_eq(&replay_inspect, "/data/artifacts/0/digest", artifact_digest)?;
    Ok(())
}

fn operational_composite_acceptance(root: &Path, binary: &Path, directory: &Path) -> Result<()> {
    let server = ProtocolFixtureServer::start()?;
    let workspace = directory.join("operational-workflow");
    fs::create_dir_all(workspace.join("artifacts"))?;
    let workflow = workspace.join("operational-workflow.yaml");
    let source =
        fs::read_to_string(root.join("examples/framework-completeness/operational-workflow.yaml"))?
            .replace(
                "http://127.0.0.1:8765/mcp",
                &format!("http://{}/mcp", server.address()),
            );
    write(&workflow, &source)?;
    let db = workspace.join("runtime.db");

    let failure = credential_free_json(
        binary,
        &workspace,
        &run_args(&workflow, &db, &workspace, &[]),
        4,
    )?;
    let source_run_id = string_at(&failure, "/error/runId")?;
    let inspection = inspect_without_openai(binary, &workspace, &db, source_run_id)?;
    ensure_eq(&inspection, "/data/run/state", "failed")?;
    ensure!(server.mcp_calls() == 2);
    ensure_eq(&inspection, "/data/protocolSessions/0/generation", 2_u64)?;
    ensure_eq(&inspection, "/data/protocolCalls/0/status", "succeeded")?;
    ensure!(task_has(&inspection, "probe", "succeeded", "executed"));
    ensure!(task_has(&inspection, "route", "succeeded", "executed"));
    ensure!(task_has(&inspection, "provision", "succeeded", "executed"));
    ensure!(task_has(&inspection, "hold", "skipped", "executed"));
    ensure!(task_has(&inspection, "fail", "failed", "executed"));
    ensure!(task_prefix_count(&inspection, "normalize--") >= 2);
    ensure!(task_prefix_count(&inspection, "verify-loop--") == 3);
    ensure!(
        task_record(&inspection, "verify-loop").and_then(|task| task.pointer("/output/iterations"))
            == Some(&Value::from(2_u64))
    );
    ensure!(array_len(&inspection, "/data/audit")? > 0);
    ensure!(array_len(&inspection, "/data/traces")? > 0);
    ensure!(
        fs::read_to_string(workspace.join("artifacts/operational-resource.txt"))? == "provisioned"
    );

    let compensation_plan = credential_free_json(
        binary,
        &workspace,
        &strings([
            "compensate",
            source_run_id,
            "--plan",
            "--db",
            path(&db)?,
            "--workspace",
            path(&workspace)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
        0,
    )?;
    ensure_eq(&compensation_plan, "/data/executable", true)?;
    ensure_eq(
        &compensation_plan,
        "/data/tasks/0/sourceTaskId",
        "provision",
    )?;
    let compensation = credential_free_json(
        binary,
        &workspace,
        &strings([
            "compensate",
            source_run_id,
            "--db",
            path(&db)?,
            "--workspace",
            path(&workspace)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
        0,
    )?;
    ensure_eq(&compensation, "/data/state", "succeeded")?;
    ensure_eq(&compensation, "/data/compensatedTasks/0", "provision")?;
    ensure!(
        fs::read_to_string(workspace.join("artifacts/operational-resource.txt"))? == "compensated"
    );
    let compensation_run_id = string_at(&compensation, "/data/runId")?;
    let source_after = inspect_without_openai(binary, &workspace, &db, source_run_id)?;
    ensure!(
        source_after["data"]["effectReconciliations"]
            .as_array()
            .is_some_and(|records| records.iter().any(|record| {
                record.get("status").and_then(Value::as_str) == Some("compensated")
            }))
    );
    let replay = credential_free_json(
        binary,
        &workspace,
        &strings([
            "replay",
            compensation_run_id,
            "--db",
            path(&db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
        0,
    )?;
    let replay_run_id = string_at(&replay, "/data/runId")?;
    let replay_inspect = inspect_without_openai(binary, &workspace, &db, replay_run_id)?;
    ensure!(array_len(&replay_inspect, "/data/effects")? == 0);
    operational_cancellation_acceptance(binary, &workspace, &workflow)?;
    Ok(())
}

fn operational_cancellation_acceptance(
    binary: &Path,
    workspace: &Path,
    workflow: &Path,
) -> Result<()> {
    #[cfg(unix)]
    {
        let db = workspace.join("cancellation.db");
        let args = run_args(
            workflow,
            &db,
            workspace,
            &[
                "--input".to_owned(),
                "mode=cancel".to_owned(),
                "--timeout-seconds".to_owned(),
                "20".to_owned(),
            ],
        );
        let mut command = command_for(binary, workspace, &args);
        command.env_remove("OPENAI_API_KEY");
        configure_piped_command(&mut command);
        let child = command.spawn()?;
        for _ in 0..100 {
            if db.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        thread::sleep(Duration::from_millis(500));
        let status = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()?;
        ensure!(status.success(), "failed to deliver operational SIGTERM");
        let output = bounded_wait(child, "operational composite cancellation")?;
        ensure!(
            output.status.code() == Some(130),
            "operational SIGTERM exit was not 130"
        );
        let cancelled = parse_output(&output)?;
        ensure_eq(&cancelled, "/data/state", "cancelled")?;
        let run_id = string_at(&cancelled, "/data/runId")?;
        let inspection = inspect_without_openai(binary, workspace, &db, run_id)?;
        ensure_eq(&inspection, "/data/run/state", "cancelled")?;
        ensure!(task_has(
            &inspection,
            "cancel-boundary",
            "cancelled",
            "executed"
        ));
    }
    #[cfg(not(unix))]
    {
        let _ = (binary, workspace, workflow);
        println!("operational SIGTERM composition is not applicable on this platform");
    }
    Ok(())
}

fn structured_handoff_composite_acceptance(
    root: &Path,
    binary: &Path,
    directory: &Path,
) -> Result<()> {
    let workflow = root.join("examples/v1/structured-handoff.yaml");
    let workspace = root.join("examples/v1");
    let db = directory.join("structured-handoff.db");
    let run = credential_free_json(binary, root, &run_args(&workflow, &db, &workspace, &[]), 0)?;
    ensure_eq(&run, "/data/state", "succeeded")?;
    ensure_eq(&run, "/data/output/decision", "approved")?;
    ensure_eq(
        &run,
        "/data/output/handoff/payload/evidence",
        "TEAM_EVIDENCE_VERIFIED",
    )?;
    let source_run_id = string_at(&run, "/data/runId")?;
    let inspection = inspect_without_openai(binary, root, &db, source_run_id)?;
    ensure!(array_len(&inspection, "/data/tasks")? == 4);
    ensure!(array_len(&inspection, "/data/toolCalls")? == 1);
    ensure!(task_has(&inspection, "collect", "succeeded", "executed"));
    ensure!(task_has(&inspection, "handoff", "succeeded", "executed"));
    ensure!(task_has(&inspection, "review", "succeeded", "executed"));
    ensure!(task_has(&inspection, "verify", "succeeded", "executed"));

    let retry = credential_free_json(
        binary,
        root,
        &strings([
            "retry",
            path(&workflow)?,
            source_run_id,
            "--from",
            "review",
            "--restart-successful",
            "--db",
            path(&db)?,
            "--workspace",
            path(&workspace)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
        0,
    )?;
    ensure_eq(&retry, "/data/state", "succeeded")?;
    ensure_eq(&retry, "/data/reusedTasks/0", "collect")?;
    ensure_eq(&retry, "/data/reusedTasks/1", "handoff")?;
    ensure_eq(&retry, "/data/executedTasks/0", "review")?;
    ensure_eq(&retry, "/data/executedTasks/1", "verify")?;
    let retry_run_id = string_at(&retry, "/data/runId")?;
    let replay = credential_free_json(
        binary,
        root,
        &strings([
            "replay",
            retry_run_id,
            "--db",
            path(&db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
        0,
    )?;
    let replay_run_id = string_at(&replay, "/data/runId")?;
    let replay_inspect = inspect_without_openai(binary, root, &db, replay_run_id)?;
    ensure!(array_len(&replay_inspect, "/data/effects")? == 0);
    ensure!(array_len(&replay_inspect, "/data/toolCalls")? == 0);
    ensure!(array_len(&replay_inspect, "/data/providerSessions")? == 0);
    Ok(())
}

pub fn container(root: &Path) -> Result<()> {
    let engine = container_engine()?;
    ensure_engine_ready(&engine)?;
    build_image(root, &engine)?;
    container_isolation_acceptance(root, &engine)?;
    let directory = tempfile::tempdir()?;
    let layout = container_layout(directory.path(), false)?;
    let run = run_container(&engine, &layout, false, None)?;
    ensure_eq(&run, "/data/state", "succeeded")?;
    ensure_eq(&run, "/data/output/verdict", VERIFY_TOKEN)?;
    ensure!(fs::read_to_string(layout.artifacts.join("report.txt"))? == VERIFY_TOKEN);
    let run_id = string_at(&run, "/data/runId")?;
    let inspect = inspect_container(&engine, &layout, run_id)?;
    ensure!(array_len(&inspect, "/data/toolCalls")? == 1);
    ensure!(array_len(&inspect, "/data/traces")? > 0);
    ensure!(array_len(&inspect, "/data/effects")? >= 3);

    let replay = replay_container(&engine, &layout, run_id)?;
    let replay_id = string_at(&replay, "/data/runId")?;
    ensure!(replay_id != run_id);
    ensure!(replay.pointer("/data/output") == run.pointer("/data/output"));
    let replay_inspect = inspect_container(&engine, &layout, replay_id)?;
    ensure!(array_len(&replay_inspect, "/data/effects")? == 0);
    ensure!(array_len(&replay_inspect, "/data/toolCalls")? == 0);

    write(
        &layout.config.join("parallel.yaml"),
        &fs::read_to_string(root.join("examples/v1/parallel.yaml"))?,
    )?;
    let parallel = container_agentctl(
        &engine,
        &layout,
        &[
            "run",
            "/config/parallel.yaml",
            "--workspace",
            "/workspace",
            "--db",
            "/state/runtime.db",
            "--output",
            "json",
            "--color",
            "never",
        ],
        0,
        "OCI parallel run",
    )?;
    ensure_eq(&parallel, "/data/state", "succeeded")?;
    let parallel_id = string_at(&parallel, "/data/runId")?;
    let parallel_inspect = inspect_container(&engine, &layout, parallel_id)?;
    ensure_eq(&parallel_inspect, "/data/run/workingMemory/left", "one")?;
    ensure_eq(&parallel_inspect, "/data/run/workingMemory/right", "two")?;
    let parallel_replay = replay_container(&engine, &layout, parallel_id)?;
    let parallel_replay_id = string_at(&parallel_replay, "/data/runId")?;
    let parallel_replay_inspect = inspect_container(&engine, &layout, parallel_replay_id)?;
    ensure!(array_len(&parallel_replay_inspect, "/data/effects")? == 0);

    let repair_directory = tempfile::tempdir()?;
    let repair_layout = container_layout(repair_directory.path(), false)?;
    write(
        &repair_layout.config.join("repair-source.yaml"),
        SELECTIVE_REPAIR_SOURCE_WORKFLOW,
    )?;
    write(
        &repair_layout.config.join("repair-target.yaml"),
        SELECTIVE_REPAIR_TARGET_WORKFLOW,
    )?;
    let source_failure = container_agentctl(
        &engine,
        &repair_layout,
        &[
            "run",
            "/config/repair-source.yaml",
            "--workspace",
            "/workspace",
            "--db",
            "/state/repair.db",
            "--output",
            "json",
            "--color",
            "never",
        ],
        4,
        "OCI repair source",
    )?;
    let source_run_id = string_at(&source_failure, "/error/runId")?;
    let repair_plan = container_agentctl(
        &engine,
        &repair_layout,
        &[
            "repair",
            "/config/repair-target.yaml",
            source_run_id,
            "--from",
            "second",
            "--plan",
            "--workspace",
            "/workspace",
            "--db",
            "/state/repair.db",
            "--output",
            "json",
            "--color",
            "never",
        ],
        0,
        "OCI repair plan",
    )?;
    ensure_eq(&repair_plan, "/data/reusedTasks/0", "first")?;
    let repaired = container_agentctl(
        &engine,
        &repair_layout,
        &[
            "repair",
            "/config/repair-target.yaml",
            source_run_id,
            "--from",
            "second",
            "--workspace",
            "/workspace",
            "--db",
            "/state/repair.db",
            "--output",
            "json",
            "--color",
            "never",
        ],
        0,
        "OCI selective repair",
    )?;
    ensure_eq(&repaired, "/data/state", "succeeded")?;
    ensure_eq(&repaired, "/data/reusedTasks/0", "first")?;

    container_durable_composite_acceptance(root, &engine)?;
    container_compensation_reconciliation_acceptance(&engine)?;

    let missing_directory = tempfile::tempdir()?;
    let missing = container_layout(missing_directory.path(), false)?;
    write(&missing.config.join("workflow.yaml"), OPENAI_AUTH_WORKFLOW)?;
    let missing_output = run_container_with_code(&engine, &missing, 6, "missing-secret OCI run")?;
    ensure_eq(&missing_output, "/error/exitCode", 6_u64)?;
    ensure!(!missing.state.join("runtime.db").exists());

    let invalid_directory = tempfile::tempdir()?;
    let invalid = container_layout(invalid_directory.path(), false)?;
    write(&invalid.config.join("workflow.yaml"), "apiVersion: [\n")?;
    let invalid_output = run_container_with_code(&engine, &invalid, 2, "invalid OCI run")?;
    ensure_eq(&invalid_output, "/error/exitCode", 2_u64)?;
    ensure!(!invalid.state.join("runtime.db").exists());

    container_signal_acceptance(&engine, directory.path())?;
    println!(
        "agentctl OCI acceptance passed: success, artifact, inspect, parallel matrix composite, approval, retry, repair, compensation reconciliation, network-disabled replay, missing-secret, invalid-input, SIGTERM, non-root, read-only root, mounted state/artifacts"
    );
    Ok(())
}

fn container_durable_composite_acceptance(root: &Path, engine: &Path) -> Result<()> {
    let directory = tempfile::tempdir()?;
    let layout = container_layout(directory.path(), false)?;
    for (source, destination) in [
        (
            "examples/framework-completeness/durable-pipeline.yaml",
            "durable-source.yaml",
        ),
        (
            "examples/framework-completeness/durable-pipeline-repaired.yaml",
            "durable-target.yaml",
        ),
    ] {
        let workflow = fs::read_to_string(root.join(source))?
            .replace("writableRoots: [artifacts]", "writableRoots: [/artifacts]")
            .replace(
                "path: artifacts/framework-completeness.txt",
                "path: /artifacts/framework-completeness.txt",
            );
        write(&layout.config.join(destination), &workflow)?;
    }
    let db = "/state/composite.db";
    let paused = container_agentctl(
        engine,
        &layout,
        &[
            "run",
            "/config/durable-source.yaml",
            "--workspace",
            "/workspace",
            "--db",
            db,
            "--output",
            "json",
            "--color",
            "never",
        ],
        3,
        "OCI durable composite approval pause",
    )?;
    ensure_eq(&paused, "/data/state", "paused")?;
    let source_run_id = string_at(&paused, "/data/runId")?;
    let approvals = container_agentctl(
        engine,
        &layout,
        &[
            "approvals",
            "--db",
            db,
            "list",
            source_run_id,
            "--output",
            "json",
            "--color",
            "never",
        ],
        0,
        "OCI durable composite approval list",
    )?;
    let approval_id = string_at(&approvals, "/data/0/approvalId")?;
    container_agentctl(
        engine,
        &layout,
        &[
            "approvals",
            "--db",
            db,
            "approve",
            approval_id,
            "--reason",
            "OCI framework completeness",
            "--output",
            "json",
            "--color",
            "never",
        ],
        0,
        "OCI durable composite approval",
    )?;
    let failure = container_agentctl(
        engine,
        &layout,
        &[
            "resume",
            source_run_id,
            "--workspace",
            "/workspace",
            "--db",
            db,
            "--output",
            "json",
            "--color",
            "never",
        ],
        4,
        "OCI durable composite source failure",
    )?;
    ensure_eq(&failure, "/error/runId", source_run_id)?;
    let source_inspect = container_agentctl(
        engine,
        &layout,
        &[
            "inspect",
            source_run_id,
            "--db",
            db,
            "--output",
            "json",
            "--color",
            "never",
        ],
        0,
        "OCI durable composite source inspect",
    )?;
    ensure!(array_len(&source_inspect, "/data/tasks")? == 10);
    ensure!(model_effects(&source_inspect) == 1);
    ensure!(array_len(&source_inspect, "/data/artifacts")? == 1);
    let artifact_digest = string_at(&source_inspect, "/data/artifacts/0/digest")?;

    let retry_plan = container_agentctl(
        engine,
        &layout,
        &[
            "retry",
            "/config/durable-source.yaml",
            source_run_id,
            "--failed",
            "--plan",
            "--workspace",
            "/workspace",
            "--db",
            db,
            "--output",
            "json",
            "--color",
            "never",
        ],
        0,
        "OCI durable composite retry plan",
    )?;
    ensure!(array_len(&retry_plan, "/data/reusedTasks")? == 9);
    ensure_eq(&retry_plan, "/data/rerunTasks/0", "validate")?;
    let retry = container_agentctl(
        engine,
        &layout,
        &[
            "retry",
            "/config/durable-source.yaml",
            source_run_id,
            "--failed",
            "--workspace",
            "/workspace",
            "--db",
            db,
            "--output",
            "json",
            "--color",
            "never",
        ],
        4,
        "OCI durable composite retry",
    )?;
    let retry_run_id = string_at(&retry, "/error/runId")?;
    let retry_inspect = container_agentctl(
        engine,
        &layout,
        &[
            "inspect",
            retry_run_id,
            "--db",
            db,
            "--output",
            "json",
            "--color",
            "never",
        ],
        0,
        "OCI durable composite retry inspect",
    )?;
    ensure!(model_effects(&retry_inspect) == 0);
    ensure!(task_has(&retry_inspect, "analyze", "succeeded", "reused"));

    fs::remove_file(layout.artifacts.join("framework-completeness.txt"))?;
    let repair = container_agentctl(
        engine,
        &layout,
        &[
            "repair",
            "/config/durable-target.yaml",
            source_run_id,
            "--from",
            "validate",
            "--workspace",
            "/workspace",
            "--db",
            db,
            "--output",
            "json",
            "--color",
            "never",
        ],
        0,
        "OCI durable composite repair",
    )?;
    ensure_eq(&repair, "/data/state", "succeeded")?;
    ensure_eq(&repair, "/data/output/verdict", "DURABLE_PIPELINE_VERIFIED")?;
    ensure!(array_len(&repair, "/data/output/matrix")? == 4);
    ensure!(array_len(&repair, "/data/reusedTasks")? == 9);
    let repair_run_id = string_at(&repair, "/data/runId")?;
    let repair_inspect = container_agentctl(
        engine,
        &layout,
        &[
            "inspect",
            repair_run_id,
            "--db",
            db,
            "--output",
            "json",
            "--color",
            "never",
        ],
        0,
        "OCI durable composite repair inspect",
    )?;
    ensure!(model_effects(&repair_inspect) == 0);
    ensure_eq(&repair_inspect, "/data/artifacts/0/digest", artifact_digest)?;
    container_agentctl(
        engine,
        &layout,
        &[
            "artifacts",
            "--db",
            db,
            "verify",
            artifact_digest,
            "--output",
            "json",
            "--color",
            "never",
        ],
        0,
        "OCI durable composite artifact verify",
    )?;
    container_agentctl(
        engine,
        &layout,
        &[
            "artifacts",
            "--db",
            db,
            "export",
            artifact_digest,
            "/artifacts/framework-completeness-exported.txt",
            "--output",
            "json",
            "--color",
            "never",
        ],
        0,
        "OCI durable composite artifact export",
    )?;
    ensure!(
        fs::read_to_string(layout.artifacts.join("framework-completeness-exported.txt"))?
            == "DURABLE_PIPELINE_VERIFIED"
    );
    let replay = container_agentctl(
        engine,
        &layout,
        &[
            "replay",
            repair_run_id,
            "--db",
            db,
            "--output",
            "json",
            "--color",
            "never",
        ],
        0,
        "OCI durable composite offline replay",
    )?;
    let replay_run_id = string_at(&replay, "/data/runId")?;
    let replay_inspect = container_agentctl(
        engine,
        &layout,
        &[
            "inspect",
            replay_run_id,
            "--db",
            db,
            "--output",
            "json",
            "--color",
            "never",
        ],
        0,
        "OCI durable composite replay inspect",
    )?;
    ensure!(array_len(&replay_inspect, "/data/effects")? == 0);
    ensure!(array_len(&replay_inspect, "/data/toolCalls")? == 0);
    ensure!(array_len(&replay_inspect, "/data/providerSessions")? == 0);
    Ok(())
}

fn container_compensation_reconciliation_acceptance(engine: &Path) -> Result<()> {
    let directory = tempfile::tempdir()?;
    let layout = container_layout(directory.path(), false)?;
    let workflow = COMPENSATION_WORKFLOW
        .replace("writableRoots: [artifacts]", "writableRoots: [/artifacts]")
        .replace(
            "path: artifacts/compensation.txt",
            "path: /artifacts/compensation.txt",
        );
    write(&layout.config.join("compensation.yaml"), &workflow)?;
    let db = "/state/reconciliation.db";
    let failure = container_agentctl(
        engine,
        &layout,
        &[
            "run",
            "/config/compensation.yaml",
            "--workspace",
            "/workspace",
            "--db",
            db,
            "--output",
            "json",
            "--color",
            "never",
        ],
        4,
        "OCI compensation source",
    )?;
    let source_run_id = string_at(&failure, "/error/runId")?;
    let compensation = container_agentctl(
        engine,
        &layout,
        &[
            "compensate",
            source_run_id,
            "--workspace",
            "/workspace",
            "--db",
            db,
            "--output",
            "json",
            "--color",
            "never",
        ],
        0,
        "OCI compensation",
    )?;
    ensure_eq(&compensation, "/data/state", "succeeded")?;
    ensure!(fs::read_to_string(layout.artifacts.join("compensation.txt"))? == "compensated");
    let source_inspect = container_agentctl(
        engine,
        &layout,
        &[
            "inspect",
            source_run_id,
            "--db",
            db,
            "--output",
            "json",
            "--color",
            "never",
        ],
        0,
        "OCI compensation source inspect",
    )?;
    ensure!(
        source_inspect["data"]["effectReconciliations"]
            .as_array()
            .is_some_and(|records| records.iter().any(|record| {
                record.get("status").and_then(Value::as_str) == Some("compensated")
            }))
    );
    let compensation_run_id = string_at(&compensation, "/data/runId")?;
    let replay = container_agentctl(
        engine,
        &layout,
        &[
            "replay",
            compensation_run_id,
            "--db",
            db,
            "--output",
            "json",
            "--color",
            "never",
        ],
        0,
        "OCI compensation offline replay",
    )?;
    let replay_run_id = string_at(&replay, "/data/runId")?;
    let replay_inspect = container_agentctl(
        engine,
        &layout,
        &[
            "inspect",
            replay_run_id,
            "--db",
            db,
            "--output",
            "json",
            "--color",
            "never",
        ],
        0,
        "OCI compensation replay inspect",
    )?;
    ensure!(array_len(&replay_inspect, "/data/effects")? == 0);
    Ok(())
}

pub fn live_openai(root: &Path) -> Result<()> {
    ensure!(
        env::var_os("OPENAI_API_KEY").is_some(),
        "OPENAI_API_KEY is required for the explicit live acceptance command"
    );
    super::package(root)?;
    let binary = packaged_binary(root)?;
    let directory = tempfile::tempdir()?;
    let workspace = directory.path().join("local-live");
    copy_example(root, "examples/openai-live", &workspace)?;
    let workflow = workspace.join("workflow.yaml");
    let db = workspace.join("runtime.db");
    successful_json(
        &binary,
        &workspace,
        &strings(["auth", "check", path(&workflow)?, "--output", "json"]),
    )?;
    successful_json(
        &binary,
        &workspace,
        &strings(["plan", path(&workflow)?, "--output", "json"]),
    )?;
    let run = successful_json(
        &binary,
        &workspace,
        &run_args(&workflow, &db, &workspace, &[]),
    )?;
    ensure_eq(&run, "/data/output/verdict", LIVE_VERIFY_TOKEN)?;
    ensure!(
        fs::read_to_string(workspace.join("artifacts/openai-live-report.txt"))?
            == LIVE_VERIFY_TOKEN
    );
    let run_id = string_at(&run, "/data/runId")?;
    let live_evidence = inspect(&binary, &workspace, &db, run_id)?;
    assert_live_evidence(&live_evidence)?;
    assert_secret_absent(&live_evidence)?;
    let replay = json_with_removed_env(
        &binary,
        &workspace,
        &strings(["replay", run_id, "--db", path(&db)?, "--output", "json"]),
        "OPENAI_API_KEY",
        0,
    )?;
    let replay_id = string_at(&replay, "/data/runId")?;
    let replay_inspect = inspect(&binary, &workspace, &db, replay_id)?;
    ensure!(array_len(&replay_inspect, "/data/effects")? == 0);

    let engine = container_engine()?;
    ensure_engine_ready(&engine)?;
    build_image(root, &engine)?;
    let container_directory = tempfile::tempdir()?;
    let layout = container_layout(container_directory.path(), true)?;
    let container_run = run_container(&engine, &layout, true, Some("OPENAI_API_KEY"))?;
    ensure_eq(&container_run, "/data/output/verdict", LIVE_VERIFY_TOKEN)?;
    let container_run_id = string_at(&container_run, "/data/runId")?;
    let container_inspect = inspect_container(&engine, &layout, container_run_id)?;
    assert_live_evidence(&container_inspect)?;
    assert_secret_absent(&container_inspect)?;
    let container_replay = replay_container(&engine, &layout, container_run_id)?;
    let container_replay_id = string_at(&container_replay, "/data/runId")?;
    let replay_evidence = inspect_container(&engine, &layout, container_replay_id)?;
    ensure!(array_len(&replay_evidence, "/data/effects")? == 0);

    let local_requests = model_effects(&live_evidence);
    let container_requests = model_effects(&container_inspect);
    let usage = usage_totals(&live_evidence).plus(usage_totals(&container_inspect));
    println!(
        "live OpenAI acceptance passed: model=gpt-5.6 localRequests={local_requests} containerRequests={container_requests} inputTokens={} outputTokens={} reasoningTokens={} cacheReadTokens={} cacheWriteTokens={} toolCalls=verified continuations=verified keylessReplays=2",
        usage.input, usage.output, usage.reasoning, usage.cache_read, usage.cache_write,
    );
    Ok(())
}

pub fn live_openai_budget(root: &Path) -> Result<()> {
    ensure!(
        env::var_os("OPENAI_API_KEY").is_some(),
        "OPENAI_API_KEY is required for the explicit live resource-budget command"
    );
    super::package(root)?;
    let binary = packaged_binary(root)?;
    let directory = tempfile::tempdir()?;
    let workspace = directory.path().join("live-resource-budget");
    fs::create_dir_all(&workspace)?;
    let workflow = workspace.join("workflow.yaml");
    let db = workspace.join("runtime.db");
    write(&workflow, OPENAI_RESOURCE_BUDGET_WORKFLOW)?;
    successful_json(
        &binary,
        &workspace,
        &strings(["auth", "check", path(&workflow)?, "--output", "json"]),
    )?;
    successful_json(
        &binary,
        &workspace,
        &strings(["plan", path(&workflow)?, "--output", "json"]),
    )?;
    let failure = json_with_code(
        &binary,
        &workspace,
        &run_args(&workflow, &db, &workspace, &[]),
        4,
    )?;
    ensure!(string_at(&failure, "/error/message")?.contains("providerRequests"));
    let run_id = string_at(&failure, "/error/runId")?;
    let evidence = inspect(&binary, &workspace, &db, run_id)?;
    assert_secret_absent(&evidence)?;
    ensure_eq(&evidence, "/data/run/state", "failed")?;
    ensure_eq(&evidence, "/data/tasks/0/state", "succeeded")?;
    ensure_eq(&evidence, "/data/tasks/1/state", "failed")?;
    ensure_eq(&evidence, "/data/budget/usage/providerRequests", 1)?;
    ensure_eq(&evidence, "/data/budget/usage/turns", 1)?;
    ensure_eq(&evidence, "/data/budget/usage/unpricedProviderRequests", 1)?;
    ensure_eq(
        &evidence,
        "/data/budget/exceeded/dimension",
        "providerRequests",
    )?;
    ensure_eq(&evidence, "/data/budget/exceeded/limit", 1)?;
    ensure_eq(&evidence, "/data/budget/exceeded/attempted", 2)?;
    ensure!(array_len(&evidence, "/data/effects")? == 2);
    ensure_eq(&evidence, "/data/effects/0/status", "succeeded")?;
    ensure_eq(&evidence, "/data/effects/1/status", "requested")?;
    ensure!(array_len(&evidence, "/data/providerSessions")? == 1);
    let usage = usage_totals(&evidence);
    guard_live_budget(1, &usage)?;
    println!(
        "live OpenAI resource-budget verification passed: model=gpt-5.6 requests=1 inputTokens={} outputTokens={} reasoningTokens={} cacheReadTokens={} cacheWriteTokens={} runId={run_id} termination=providerRequests replay=not-applicable-failed-source",
        usage.input, usage.output, usage.reasoning, usage.cache_read, usage.cache_write,
    );
    Ok(())
}

pub fn examples_live_openai(root: &Path) -> Result<()> {
    ensure!(
        env::var_os("OPENAI_API_KEY").is_some(),
        "OPENAI_API_KEY is required for the explicit live example command"
    );
    super::verify_example_matrix(root)?;
    let expected_live = BTreeSet::from([
        "examples/docs/provider-portability/openai.yaml".to_owned(),
        "examples/framework-completeness/live-composite.yaml".to_owned(),
        "examples/openai-live/workflow.yaml".to_owned(),
        "examples/selective-repair-openai/repaired.workflow.yaml".to_owned(),
        "examples/selective-repair-openai/source.workflow.yaml".to_owned(),
        "examples/v1/openai-live.yaml".to_owned(),
        "examples/v1/secret-reference.yaml".to_owned(),
    ]);
    let mut discovered_live = BTreeSet::new();
    collect_openai_workflows(&root.join("examples"), root, &mut discovered_live)?;
    ensure!(
        discovered_live == expected_live,
        "live OpenAI inventory mismatch; discovered={discovered_live:?}, expected={expected_live:?}"
    );
    super::package(root)?;
    let binary = packaged_binary(root)?;
    let mut requests = 0_usize;
    let mut usage = UsageTotals::default();
    let mut tool_calls = 0_usize;
    let mut example_runs = Vec::new();

    for (example, expected_code) in [
        ("examples/openai-live/workflow.yaml", 0),
        ("examples/v1/openai-live.yaml", 0),
        ("examples/v1/secret-reference.yaml", 0),
        ("examples/docs/provider-portability/openai.yaml", 0),
        ("examples/framework-completeness/live-composite.yaml", 0),
    ] {
        let directory = tempfile::tempdir()?;
        let source = root.join(example);
        let source_parent = source.parent().context("live example parent")?;
        copy_directory(source_parent, directory.path())?;
        let workflow = directory
            .path()
            .join(source.file_name().context("live example file name")?);
        let db = directory.path().join("runtime.db");
        successful_json(
            &binary,
            directory.path(),
            &strings([
                "auth",
                "check",
                path(&workflow)?,
                "--output",
                "json",
                "--color",
                "never",
            ]),
        )?;
        successful_json(
            &binary,
            directory.path(),
            &strings([
                "plan",
                path(&workflow)?,
                "--output",
                "json",
                "--color",
                "never",
            ]),
        )?;
        let result = json_with_code(
            &binary,
            directory.path(),
            &run_args(&workflow, &db, directory.path(), &[]),
            expected_code,
        )?;
        let run_id = if expected_code == 0 {
            string_at(&result, "/data/runId")?
        } else {
            string_at(&result, "/error/runId")?
        };
        let evidence = inspect(&binary, directory.path(), &db, run_id)?;
        assert_secret_absent(&evidence)?;
        requests = requests.saturating_add(model_effects(&evidence));
        usage = usage.plus(usage_totals(&evidence));
        tool_calls = tool_calls.saturating_add(array_len(&evidence, "/data/toolCalls")?);
        guard_live_budget(requests, &usage)?;
        let mut replay_run_id = None;
        if example == "examples/openai-live/workflow.yaml" {
            ensure_eq(&result, "/data/output/verdict", LIVE_VERIFY_TOKEN)?;
            ensure!(
                fs::read_to_string(directory.path().join("artifacts/openai-live-report.txt"))?
                    == LIVE_VERIFY_TOKEN
            );
        } else if example == "examples/framework-completeness/live-composite.yaml" {
            ensure_eq(&result, "/data/output/left", "left")?;
            ensure_eq(&result, "/data/output/right", "right")?;
            ensure_eq(&result, "/data/output/route", "execute")?;
            ensure_eq(&result, "/data/output/iterations", 2_u64)?;
            ensure_eq(&result, "/data/output/subworkflow", "ready")?;
            ensure_eq(&result, "/data/output/decision", "approved")?;
            ensure_eq(&result, "/data/output/stream", "LIVE_STREAM_VERIFIED")?;
            ensure!(array_len(&result, "/data/output/matrix")? == 2);
            ensure_eq(&result, "/data/output/matrix/0/output/item", "alpha")?;
            ensure_eq(&result, "/data/output/matrix/1/output/item", "beta")?;
            ensure!(model_effects(&evidence) == 11);
            ensure!(task_has(&evidence, "branch-left", "succeeded", "executed"));
            ensure!(task_has(&evidence, "branch-right", "succeeded", "executed"));
            ensure!(task_has(&evidence, "route-hold", "skipped", "executed"));
            ensure!(task_prefix_count(&evidence, "matrix-agents--") == 2);
            ensure!(task_prefix_count(&evidence, "loop-agents--") == 3);
            ensure!(
                evidence["data"]["streamEvents"]
                    .as_array()
                    .is_some_and(|events| events.iter().any(|event| {
                        event.get("taskId").and_then(Value::as_str) == Some("stream")
                    }))
            );
            let replay = json_with_removed_env(
                &binary,
                directory.path(),
                &strings([
                    "replay",
                    run_id,
                    "--db",
                    path(&db)?,
                    "--output",
                    "json",
                    "--color",
                    "never",
                ]),
                "OPENAI_API_KEY",
                0,
            )?;
            ensure!(replay.pointer("/data/output") == result.pointer("/data/output"));
            let replay_id = string_at(&replay, "/data/runId")?.to_owned();
            let replay_evidence = inspect(&binary, directory.path(), &db, &replay_id)?;
            ensure!(array_len(&replay_evidence, "/data/effects")? == 0);
            ensure!(array_len(&replay_evidence, "/data/toolCalls")? == 0);
            ensure!(array_len(&replay_evidence, "/data/providerSessions")? == 0);
            replay_run_id = Some(replay_id);
        }
        example_runs.push(serde_json::json!({
            "example": example,
            "model": "gpt-5.6",
            "runId": run_id,
            "status": evidence.pointer("/data/run/state"),
            "requestCount": model_effects(&evidence),
            "toolCallCount": array_len(&evidence, "/data/toolCalls")?,
            "replayRunId": replay_run_id,
            "replayFreshEffects": replay_run_id.as_ref().map(|_| 0),
        }));
    }

    let repair_directory = tempfile::tempdir()?;
    let repair_workspace = repair_directory.path().join("selective-repair");
    copy_directory(
        &root.join("examples/selective-repair-openai"),
        &repair_workspace,
    )?;
    let source_workflow = repair_workspace.join("source.workflow.yaml");
    let target_workflow = repair_workspace.join("repaired.workflow.yaml");
    let repair_db = repair_workspace.join("runtime.db");
    successful_json(
        &binary,
        &repair_workspace,
        &strings([
            "plan",
            path(&source_workflow)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    successful_json(
        &binary,
        &repair_workspace,
        &strings([
            "plan",
            path(&target_workflow)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    let source_result = json_with_code(
        &binary,
        &repair_workspace,
        &run_args(&source_workflow, &repair_db, &repair_workspace, &[]),
        4,
    )?;
    let source_run_id = string_at(&source_result, "/error/runId")?;
    let source_before = inspect(&binary, &repair_workspace, &repair_db, source_run_id)?;
    ensure_eq(&source_before, "/data/run/state", "failed")?;
    ensure_eq(&source_before, "/data/tasks/0/state", "succeeded")?;
    ensure_eq(&source_before, "/data/tasks/1/state", "failed")?;
    ensure_eq(
        &source_before,
        "/data/tasks/0/output/marker",
        "SELECTIVE_REPAIR_FIXTURE_CONFIRMED",
    )?;
    ensure!(model_effects(&source_before) >= 3);
    ensure!(
        source_before
            .pointer("/data/providerSessions/1/continuation")
            .is_some(),
        "failed source task did not retain provider continuation evidence"
    );

    let retry_plan = successful_json(
        &binary,
        &repair_workspace,
        &strings([
            "retry",
            path(&source_workflow)?,
            source_run_id,
            "--failed",
            "--plan",
            "--db",
            path(&repair_db)?,
            "--workspace",
            path(&repair_workspace)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&retry_plan, "/data/compatible", true)?;
    ensure_eq(&retry_plan, "/data/reusedTasks/0", "analyze")?;
    ensure_eq(&retry_plan, "/data/rerunTasks/0", "publish")?;
    let retry_result = json_with_code(
        &binary,
        &repair_workspace,
        &strings([
            "retry",
            path(&source_workflow)?,
            source_run_id,
            "--failed",
            "--reason",
            "live deterministic agent failure retry",
            "--db",
            path(&repair_db)?,
            "--workspace",
            path(&repair_workspace)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
        4,
    )?;
    let retry_run_id = string_at(&retry_result, "/error/runId")?;
    let retry_evidence = inspect(&binary, &repair_workspace, &repair_db, retry_run_id)?;
    ensure_eq(&retry_evidence, "/data/run/mode", "retry")?;
    ensure!(task_has(&retry_evidence, "analyze", "succeeded", "reused"));
    ensure!(task_has(&retry_evidence, "publish", "failed", "executed"));
    ensure!(
        model_effects_for_task(&retry_evidence, "analyze") == 0,
        "live retry repeated the successful upstream agent"
    );
    ensure!(
        model_effects_for_task(&retry_evidence, "publish") == 1,
        "live retry did not make exactly one bounded provider attempt"
    );

    let repair_plan = successful_json(
        &binary,
        &repair_workspace,
        &strings([
            "repair",
            path(&target_workflow)?,
            source_run_id,
            "--from",
            "publish",
            "--plan",
            "--db",
            path(&repair_db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    ensure_eq(&repair_plan, "/data/compatible", true)?;
    ensure_eq(&repair_plan, "/data/reusedTasks/0", "analyze")?;
    ensure_eq(&repair_plan, "/data/rerunTasks/0", "publish")?;
    let repair_result = successful_json(
        &binary,
        &repair_workspace,
        &strings([
            "repair",
            path(&target_workflow)?,
            source_run_id,
            "--from",
            "publish",
            "--reason",
            "live selective-repair acceptance",
            "--db",
            path(&repair_db)?,
            "--workspace",
            path(&repair_workspace)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )?;
    let repair_run_id = string_at(&repair_result, "/data/runId")?;
    let repair_evidence = inspect(&binary, &repair_workspace, &repair_db, repair_run_id)?;
    ensure_eq(&repair_evidence, "/data/run/mode", "repair")?;
    ensure_eq(&repair_evidence, "/data/run/sourceRunId", source_run_id)?;
    ensure_eq(&repair_evidence, "/data/tasks/0/disposition", "reused")?;
    ensure_eq(&repair_evidence, "/data/tasks/1/disposition", "executed")?;
    ensure_eq(
        &repair_result,
        "/data/output/upstream",
        "SELECTIVE_REPAIR_FIXTURE_CONFIRMED",
    )?;
    ensure_eq(&repair_result, "/data/output/repaired", true)?;
    ensure!(
        count_task_items(&repair_evidence, "/data/effects", "analyze", true) == 0,
        "reused analyze task emitted a fresh effect"
    );
    ensure!(
        count_task_items(&repair_evidence, "/data/providerSessions", "analyze", false) == 0,
        "reused analyze task created a provider session"
    );
    ensure!(
        count_task_items(&repair_evidence, "/data/toolCalls", "analyze", false) == 0,
        "reused analyze task called a tool"
    );
    ensure!(
        count_task_items(&repair_evidence, "/data/providerSessions", "publish", false) == 1,
        "repaired publish task did not use one fresh task-local provider session"
    );
    ensure!(
        source_before.pointer("/data/providerSessions/1/continuation")
            != repair_evidence.pointer("/data/providerSessions/0/continuation"),
        "repair continued the failed source provider session"
    );
    ensure!(
        count_task_items(&repair_evidence, "/data/toolCalls", "publish", false) == 1,
        "repaired publish task did not execute the model-selected tool"
    );
    let artifact = repair_workspace.join("artifacts/selective-repair-result.txt");
    ensure!(
        fs::read_to_string(&artifact)? == "SELECTIVE_REPAIR_FIXTURE_CONFIRMED",
        "selective repair artifact did not contain the reused upstream marker"
    );
    let artifact_digest_before = hex::encode(Sha256::digest(fs::read(&artifact)?));

    let source_after = inspect(&binary, &repair_workspace, &repair_db, source_run_id)?;
    ensure!(
        source_before.pointer("/data/run") == source_after.pointer("/data/run")
            && source_before.pointer("/data/tasks") == source_after.pointer("/data/tasks"),
        "repair mutated the terminal source run"
    );
    let replay_result = json_with_removed_env(
        &binary,
        &repair_workspace,
        &strings([
            "replay",
            repair_run_id,
            "--db",
            path(&repair_db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
        "OPENAI_API_KEY",
        0,
    )?;
    let replay_run_id = string_at(&replay_result, "/data/runId")?;
    let replay_evidence = inspect(&binary, &repair_workspace, &repair_db, replay_run_id)?;
    ensure!(array_len(&replay_evidence, "/data/effects")? == 0);
    ensure!(array_len(&replay_evidence, "/data/toolCalls")? == 0);
    ensure!(array_len(&replay_evidence, "/data/providerSessions")? == 0);
    ensure!(replay_result.pointer("/data/output") == repair_result.pointer("/data/output"));
    ensure!(
        artifact_digest_before == hex::encode(Sha256::digest(fs::read(&artifact)?)),
        "offline replay changed the repair artifact"
    );

    for evidence in [&source_before, &retry_evidence, &repair_evidence] {
        assert_secret_absent(evidence)?;
        requests = requests.saturating_add(model_effects(evidence));
        usage = usage.plus(usage_totals(evidence));
        tool_calls = tool_calls.saturating_add(array_len(evidence, "/data/toolCalls")?);
    }
    guard_live_budget(requests, &usage)?;
    example_runs.push(serde_json::json!({
        "example": "examples/selective-repair-openai/source.workflow.yaml",
        "model": "gpt-5.6",
        "runId": source_run_id,
        "status": "failed",
        "requestCount": model_effects(&source_before),
        "toolCallCount": array_len(&source_before, "/data/toolCalls")?,
    }));
    example_runs.push(serde_json::json!({
        "example": "examples/selective-repair-openai/source.workflow.yaml",
        "scenario": "terminal retry after deterministic agent failure",
        "model": "gpt-5.6",
        "runId": retry_run_id,
        "status": "failed-as-designed",
        "requestCount": model_effects(&retry_evidence),
        "toolCallCount": array_len(&retry_evidence, "/data/toolCalls")?,
        "upstreamReused": true,
    }));
    example_runs.push(serde_json::json!({
        "example": "examples/selective-repair-openai/repaired.workflow.yaml",
        "model": "gpt-5.6",
        "runId": repair_run_id,
        "status": "succeeded",
        "requestCount": model_effects(&repair_evidence),
        "toolCallCount": array_len(&repair_evidence, "/data/toolCalls")?,
        "reusedUpstream": true,
        "replayRunId": replay_run_id,
        "replayFreshEffects": 0,
    }));

    write_live_summary(
        root,
        &example_runs,
        requests,
        tool_calls,
        &usage,
        source_run_id,
        repair_run_id,
        replay_run_id,
        "local-complete-container-pending",
    )?;

    reserve_live_requests(requests, LIVE_CONTAINER_MAX_REQUESTS)?;
    let container = run_live_openai_container(root)?;
    requests = requests.saturating_add(container.requests);
    usage = usage.plus(container.usage);
    tool_calls = tool_calls.saturating_add(container.tool_calls);
    guard_live_budget(requests, &usage)?;
    write_live_summary(
        root,
        &example_runs,
        requests,
        tool_calls,
        &usage,
        source_run_id,
        repair_run_id,
        replay_run_id,
        "local-and-container-complete",
    )?;
    write_live_container_summary(root, &container)?;
    println!(
        "live OpenAI example verification passed: examples=7 model=gpt-5.6 requests={requests} inputTokens={} outputTokens={} reasoningTokens={} cacheReadTokens={} cacheWriteTokens={} toolCalls={tool_calls} sourceRunId={source_run_id} repairRunId={repair_run_id} replayRunId={replay_run_id} containerSourceRunId={} containerRepairRunId={} containerReplayRunId={}",
        usage.input,
        usage.output,
        usage.reasoning,
        usage.cache_read,
        usage.cache_write,
        container.source_run_id,
        container.repair_run_id,
        container.replay_run_id,
    );
    Ok(())
}

pub fn examples_live_openai_container(root: &Path) -> Result<()> {
    ensure!(
        env::var_os("OPENAI_API_KEY").is_some(),
        "OPENAI_API_KEY is required for the explicit live container continuation"
    );
    super::verify_example_matrix(root)?;
    let summary_path = root.join(".release-evidence/selective-repair/live-summary.json");
    let summary: Value = serde_json::from_slice(
        &fs::read(&summary_path)
            .with_context(|| format!("read prior live summary {}", summary_path.display()))?,
    )?;
    ensure_eq(&summary, "/status", "local-complete-container-pending")?;
    ensure_eq(&summary, "/model", "gpt-5.6")?;
    let examples = summary["examples"]
        .as_array()
        .context("prior live summary examples")?
        .clone();
    let mut requests = summary["requestCount"]
        .as_u64()
        .context("prior live request count")? as usize;
    let mut tool_calls = summary["toolCallCount"]
        .as_u64()
        .context("prior live tool count")? as usize;
    let mut usage = usage_from_summary(&summary)?;
    let source_run_id = string_at(&summary, "/selectiveRepair/sourceRunId")?;
    let repair_run_id = string_at(&summary, "/selectiveRepair/repairRunId")?;
    let replay_run_id = string_at(&summary, "/selectiveRepair/replayRunId")?;

    reserve_live_requests(requests, LIVE_CONTAINER_MAX_REQUESTS)?;
    let container = run_live_openai_container(root)?;
    requests = requests.saturating_add(container.requests);
    tool_calls = tool_calls.saturating_add(container.tool_calls);
    usage = usage.plus(container.usage);
    guard_live_budget(requests, &usage)?;
    write_live_summary(
        root,
        &examples,
        requests,
        tool_calls,
        &usage,
        source_run_id,
        repair_run_id,
        replay_run_id,
        "local-and-container-complete",
    )?;
    write_live_container_summary(root, &container)?;
    println!(
        "live OpenAI container continuation passed: model=gpt-5.6 totalRequests={requests} inputTokens={} outputTokens={} reasoningTokens={} cacheReadTokens={} cacheWriteTokens={} toolCalls={tool_calls} containerSourceRunId={} containerRepairRunId={} containerReplayRunId={}",
        usage.input,
        usage.output,
        usage.reasoning,
        usage.cache_read,
        usage.cache_write,
        container.source_run_id,
        container.repair_run_id,
        container.replay_run_id,
    );
    Ok(())
}

// Source: analyzer 3 + publisher 1; repair: publisher 3.
const LIVE_CONTAINER_MAX_REQUESTS: usize = 7;

struct LiveContainerSummary {
    source_run_id: String,
    repair_run_id: String,
    replay_run_id: String,
    requests: usize,
    tool_calls: usize,
    usage: UsageTotals,
}

fn run_live_openai_container(root: &Path) -> Result<LiveContainerSummary> {
    let engine = container_engine()?;
    ensure_engine_ready(&engine)?;
    build_image(root, &engine)?;
    let directory = tempfile::tempdir()?;
    let layout = container_layout(directory.path(), true)?;
    let source_container =
        fs::read_to_string(root.join("examples/selective-repair-openai/source.workflow.yaml"))?
            .replace("writableRoots: [artifacts]", "writableRoots: [/artifacts]")
            .replace(
                "artifacts/selective-repair-result.txt",
                "/artifacts/selective-repair-result.txt",
            );
    let target_container =
        fs::read_to_string(root.join("examples/selective-repair-openai/repaired.workflow.yaml"))?
            .replace("writableRoots: [artifacts]", "writableRoots: [/artifacts]")
            .replace(
                "artifacts/selective-repair-result.txt",
                "/artifacts/selective-repair-result.txt",
            );
    write(
        &layout.workspace.join("fixture/service.txt"),
        "service=agentctl\nmarker=SELECTIVE_REPAIR_FIXTURE_CONFIRMED\n",
    )?;
    write(&layout.config.join("repair-source.yaml"), &source_container)?;
    write(&layout.config.join("repair-target.yaml"), &target_container)?;
    let source = container_agentctl_with_openai(
        &engine,
        &layout,
        &[
            "run",
            "/config/repair-source.yaml",
            "--workspace",
            "/workspace",
            "--db",
            "/state/runtime.db",
            "--output",
            "json",
            "--color",
            "never",
        ],
        4,
        "live OCI repair source",
    )?;
    let source_run_id = string_at(&source, "/error/runId")?.to_owned();
    let plan = container_agentctl(
        &engine,
        &layout,
        &[
            "repair",
            "/config/repair-target.yaml",
            &source_run_id,
            "--from",
            "publish",
            "--plan",
            "--workspace",
            "/workspace",
            "--db",
            "/state/runtime.db",
            "--output",
            "json",
            "--color",
            "never",
        ],
        0,
        "live OCI repair plan",
    )?;
    ensure_eq(&plan, "/data/reusedTasks/0", "analyze")?;
    let repair = container_agentctl_with_openai(
        &engine,
        &layout,
        &[
            "repair",
            "/config/repair-target.yaml",
            &source_run_id,
            "--from",
            "publish",
            "--workspace",
            "/workspace",
            "--db",
            "/state/runtime.db",
            "--output",
            "json",
            "--color",
            "never",
        ],
        0,
        "live OCI selective repair",
    )?;
    let repair_run_id = string_at(&repair, "/data/runId")?.to_owned();
    let source_evidence = inspect_container(&engine, &layout, &source_run_id)?;
    let repair_evidence = inspect_container(&engine, &layout, &repair_run_id)?;
    ensure!(count_task_items(&repair_evidence, "/data/effects", "analyze", true) == 0);
    ensure!(count_task_items(&repair_evidence, "/data/toolCalls", "analyze", false) == 0);
    ensure!(
        fs::read_to_string(layout.artifacts.join("selective-repair-result.txt"))?
            == "SELECTIVE_REPAIR_FIXTURE_CONFIRMED"
    );
    let replay = replay_container(&engine, &layout, &repair_run_id)?;
    let replay_run_id = string_at(&replay, "/data/runId")?.to_owned();
    let replay_evidence = inspect_container(&engine, &layout, &replay_run_id)?;
    ensure!(array_len(&replay_evidence, "/data/effects")? == 0);
    ensure!(array_len(&replay_evidence, "/data/toolCalls")? == 0);
    ensure!(array_len(&replay_evidence, "/data/providerSessions")? == 0);
    let mut requests = 0_usize;
    let mut tool_calls = 0_usize;
    let mut usage = UsageTotals::default();
    for evidence in [&source_evidence, &repair_evidence] {
        assert_secret_absent(evidence)?;
        requests = requests.saturating_add(model_effects(evidence));
        usage = usage.plus(usage_totals(evidence));
        tool_calls = tool_calls.saturating_add(array_len(evidence, "/data/toolCalls")?);
    }
    Ok(LiveContainerSummary {
        source_run_id,
        repair_run_id,
        replay_run_id,
        requests,
        tool_calls,
        usage,
    })
}

fn run_error(
    binary: &Path,
    cwd: &Path,
    workflow: &Path,
    directory: &Path,
    db_name: &str,
) -> Result<Value> {
    json_with_code(
        binary,
        cwd,
        &run_args(workflow, &directory.join(db_name), cwd, &[]),
        4,
    )
}

fn signal_acceptance(binary: &Path, workspace: &Path, directory: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let workflow = workspace.join("signal.yaml");
        write(
            &workflow,
            &agent_workflow("signal", "delayMs: 10000", "", ""),
        )?;
        let db = directory.join("signal.db");
        let args = run_args(&workflow, &db, workspace, &[]);
        let mut command = command_for(binary, workspace, &args);
        configure_piped_command(&mut command);
        let child = command.spawn()?;
        for _ in 0..50 {
            if db.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        thread::sleep(Duration::from_millis(100));
        let status = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()?;
        ensure!(status.success(), "failed to deliver SIGTERM");
        let output = bounded_wait(child, "SIGTERM agentctl run")?;
        ensure!(
            output.status.code() == Some(130),
            "SIGTERM exit was not 130"
        );
        let value = parse_output(&output)?;
        ensure_eq(&value, "/data/state", "cancelled")?;
    }
    #[cfg(not(unix))]
    {
        let _ = (binary, workspace, directory);
        println!("SIGTERM acceptance is not applicable on this platform");
    }
    Ok(())
}

fn uncertain_repair_acceptance(binary: &Path, workspace: &Path, directory: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let source = workspace.join("repair-uncertain-source.yaml");
        let target = workspace.join("repair-uncertain-target.yaml");
        write(&source, UNCERTAIN_REPAIR_SOURCE_WORKFLOW)?;
        write(&target, UNCERTAIN_REPAIR_TARGET_WORKFLOW)?;
        let db = directory.join("repair-uncertain.db");
        let failure = json_with_code(
            binary,
            workspace,
            &run_args(&source, &db, workspace, &[]),
            4,
        )?;
        let run_id = string_at(&failure, "/error/runId")?;
        let effects = successful_json(
            binary,
            workspace,
            &strings([
                "effects",
                "--db",
                path(&db)?,
                "list",
                run_id,
                "--task",
                "work",
                "--output",
                "json",
                "--color",
                "never",
            ]),
        )?;
        ensure_eq(&effects, "/data/effects/0/status", "uncertain")?;
        let effect_id = string_at(&effects, "/data/effects/0/request/id")?;
        let blocked = json_with_code(
            binary,
            workspace,
            &strings([
                "repair",
                path(&target)?,
                run_id,
                "--from",
                "work",
                "--plan",
                "--db",
                path(&db)?,
                "--output",
                "json",
                "--color",
                "never",
            ]),
            3,
        )?;
        ensure_eq(&blocked, "/data/blockedReuse/0/rule", "unreconciled_effect")?;
        successful_json(
            binary,
            workspace,
            &strings([
                "effects",
                "--db",
                path(&db)?,
                "reconcile",
                effect_id,
                "--status",
                "not-applied",
                "--actor",
                "acceptance",
                "--reason",
                "subprocess was terminated before external mutation",
                "--output",
                "json",
                "--color",
                "never",
            ]),
        )?;
        let repaired = successful_json(
            binary,
            workspace,
            &strings([
                "repair",
                path(&target)?,
                run_id,
                "--from",
                "work",
                "--db",
                path(&db)?,
                "--output",
                "json",
                "--color",
                "never",
            ]),
        )?;
        ensure_eq(&repaired, "/data/state", "succeeded")?;
    }
    #[cfg(not(unix))]
    {
        let _ = (binary, workspace, directory);
        println!("uncertain subprocess repair acceptance is not applicable on this platform");
    }
    Ok(())
}

fn read_only_write_acceptance(binary: &Path, directory: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        let workspace = directory.join("read-only-workspace");
        fs::create_dir_all(&workspace)?;
        let workflow = workspace.join("workflow.yaml");
        write(
            &workflow,
            &TRAVERSAL_WORKFLOW
                .replace("writableRoots: [artifacts]", "writableRoots: [.]")
                .replace("../escaped.txt", "result.txt"),
        )?;
        fs::set_permissions(&workspace, fs::Permissions::from_mode(0o555))?;
        let result = expect_code(
            binary,
            &workspace,
            &run_args(&workflow, &directory.join("read-only.db"), &workspace, &[]),
            4,
        );
        fs::set_permissions(&workspace, fs::Permissions::from_mode(0o755))?;
        result?;
        ensure!(!workspace.join("result.txt").exists());
    }
    #[cfg(not(unix))]
    let _ = (binary, directory);
    Ok(())
}

fn assert_live_evidence(value: &Value) -> Result<()> {
    ensure!(
        model_effects(value) == 2,
        "expected exactly two model requests"
    );
    ensure!(array_len(value, "/data/toolCalls")? == 1);
    ensure!(array_len(value, "/data/providerSessions")? == 1);
    ensure_eq(
        value,
        "/data/providerSessions/0/continuation/kind",
        "conversation",
    )?;
    let call_id = string_at(value, "/data/toolCalls/0/callId")?;
    let continuation_blocks = value
        .pointer("/data/providerSessions/0/continuation/value")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|message| message.get("content"))
        .filter_map(Value::as_array)
        .flatten()
        .collect::<Vec<_>>();
    ensure!(
        continuation_blocks.iter().any(|block| {
            block.get("type").and_then(Value::as_str) == Some("tool_call")
                && block.get("id").and_then(Value::as_str) == Some(call_id)
        }),
        "stateless continuation omitted the correlated tool call"
    );
    ensure!(
        continuation_blocks.iter().any(|block| {
            block.get("type").and_then(Value::as_str) == Some("tool_result")
                && block.get("id").and_then(Value::as_str) == Some(call_id)
        }),
        "stateless continuation omitted the correlated tool result"
    );
    let model_effects = value
        .pointer("/data/effects")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|effect| {
            effect.pointer("/request/effectClass") == Some(&Value::String("model".to_owned()))
        })
        .collect::<Vec<_>>();
    ensure!(
        model_effects.iter().all(|effect| {
            effect.pointer("/request/input/providerOptions/store") == Some(&Value::Bool(false))
        }),
        "live model effect did not request stateless continuation"
    );
    let reasoning_items = model_effects
        .iter()
        .filter_map(|effect| effect.pointer("/result/assistantContent"))
        .filter_map(Value::as_array)
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("opaque_reasoning"))
        .collect::<Vec<_>>();
    ensure!(
        reasoning_items.iter().all(|block| {
            block
                .pointer("/value/encrypted_content")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty())
        }),
        "stateless continuation persisted an unencrypted reasoning item"
    );
    ensure!(array_len(value, "/data/checkpoints")? > 0);
    ensure!(array_len(value, "/data/traces")? > 0);
    ensure_eq(value, "/data/toolCalls/0/status", "succeeded")?;
    Ok(())
}

fn assert_secret_absent(value: &Value) -> Result<()> {
    if let Ok(secret) = env::var("OPENAI_API_KEY") {
        ensure!(
            !value.to_string().contains(&secret),
            "credential appeared in durable evidence"
        );
    }
    Ok(())
}

fn approvals(binary: &Path, cwd: &Path, db: &Path, run_id: &str) -> Result<Value> {
    successful_json(
        binary,
        cwd,
        &strings([
            "approvals",
            "--db",
            path(db)?,
            "list",
            run_id,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )
}

fn inspect(binary: &Path, cwd: &Path, db: &Path, run_id: &str) -> Result<Value> {
    successful_json(
        binary,
        cwd,
        &strings([
            "inspect",
            run_id,
            "--db",
            path(db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
    )
}

fn inspect_without_openai(binary: &Path, cwd: &Path, db: &Path, run_id: &str) -> Result<Value> {
    credential_free_json(
        binary,
        cwd,
        &strings([
            "inspect",
            run_id,
            "--db",
            path(db)?,
            "--output",
            "json",
            "--color",
            "never",
        ]),
        0,
    )
}

fn credential_free_json(binary: &Path, cwd: &Path, args: &[String], code: i32) -> Result<Value> {
    json_with_removed_env(binary, cwd, args, "OPENAI_API_KEY", code)
}

fn task_has(value: &Value, task_id: &str, state: &str, disposition: &str) -> bool {
    task_record(value, task_id).is_some_and(|task| {
        task.get("state").and_then(Value::as_str) == Some(state)
            && task.get("disposition").and_then(Value::as_str) == Some(disposition)
    })
}

fn task_record<'a>(value: &'a Value, task_id: &str) -> Option<&'a Value> {
    value
        .pointer("/data/tasks")
        .and_then(Value::as_array)
        .and_then(|tasks| {
            tasks
                .iter()
                .find(|task| task.get("taskId").and_then(Value::as_str) == Some(task_id))
        })
}

fn task_prefix_count(value: &Value, prefix: &str) -> usize {
    value
        .pointer("/data/tasks")
        .and_then(Value::as_array)
        .map_or(0, |tasks| {
            tasks
                .iter()
                .filter(|task| {
                    task.get("taskId")
                        .and_then(Value::as_str)
                        .is_some_and(|task_id| task_id.starts_with(prefix))
                })
                .count()
        })
}

fn model_effects(value: &Value) -> usize {
    value
        .pointer("/data/effects")
        .and_then(Value::as_array)
        .map_or(0, |effects| {
            effects
                .iter()
                .filter(|effect| {
                    effect.pointer("/request/effectClass")
                        == Some(&Value::String("model".to_owned()))
                })
                .count()
        })
}

fn model_effects_for_task(value: &Value, task_id: &str) -> usize {
    value
        .pointer("/data/effects")
        .and_then(Value::as_array)
        .map_or(0, |effects| {
            effects
                .iter()
                .filter(|effect| {
                    effect.pointer("/request/taskId").and_then(Value::as_str) == Some(task_id)
                        && effect.pointer("/request/effectClass")
                            == Some(&Value::String("model".to_owned()))
                })
                .count()
        })
}

#[derive(Clone, Copy, Debug, Default)]
struct UsageTotals {
    input: u64,
    output: u64,
    reasoning: u64,
    cache_read: u64,
    cache_write: u64,
    cost_microusd: u64,
}

impl UsageTotals {
    const fn plus(self, other: Self) -> Self {
        Self {
            input: self.input.saturating_add(other.input),
            output: self.output.saturating_add(other.output),
            reasoning: self.reasoning.saturating_add(other.reasoning),
            cache_read: self.cache_read.saturating_add(other.cache_read),
            cache_write: self.cache_write.saturating_add(other.cache_write),
            cost_microusd: self.cost_microusd.saturating_add(other.cost_microusd),
        }
    }
}

fn usage_totals(value: &Value) -> UsageTotals {
    let mut total = UsageTotals::default();
    let Some(effects) = value.pointer("/data/effects").and_then(Value::as_array) else {
        return total;
    };
    for usage in effects
        .iter()
        .filter(|effect| {
            effect.pointer("/request/effectClass") == Some(&Value::String("model".to_owned()))
        })
        .filter_map(|effect| effect.pointer("/result/usage"))
    {
        total.input = total
            .input
            .saturating_add(usage["inputTokens"].as_u64().unwrap_or(0));
        total.output = total
            .output
            .saturating_add(usage["outputTokens"].as_u64().unwrap_or(0));
        total.reasoning = total
            .reasoning
            .saturating_add(usage["reasoningTokens"].as_u64().unwrap_or(0));
        total.cache_read = total
            .cache_read
            .saturating_add(usage["cacheReadTokens"].as_u64().unwrap_or(0));
        total.cache_write = total
            .cache_write
            .saturating_add(usage["cacheWriteTokens"].as_u64().unwrap_or(0));
        total.cost_microusd = total
            .cost_microusd
            .saturating_add(usage["costMicrousd"].as_u64().unwrap_or(0));
    }
    total
}

fn usage_from_summary(value: &Value) -> Result<UsageTotals> {
    let usage = value
        .get("usage")
        .and_then(Value::as_object)
        .context("prior live summary usage")?;
    let read = |name: &str| {
        usage
            .get(name)
            .and_then(Value::as_u64)
            .with_context(|| format!("prior live summary usage.{name}"))
    };
    Ok(UsageTotals {
        input: read("inputTokens")?,
        output: read("outputTokens")?,
        reasoning: read("reasoningTokens")?,
        cache_read: read("cacheReadTokens")?,
        cache_write: read("cacheWriteTokens")?,
        cost_microusd: read("providerReportedCostMicrousd")?,
    })
}

fn guard_live_budget(requests: usize, usage: &UsageTotals) -> Result<()> {
    const MAX_REQUESTS: usize = 40;
    const MAX_COST_MICROUSD: u64 = 10_000_000;
    const CONSERVATIVE_INPUT_MICROUSD_PER_MILLION: u64 = 10_000_000;
    const CONSERVATIVE_OUTPUT_MICROUSD_PER_MILLION: u64 = 50_000_000;

    ensure!(
        requests <= MAX_REQUESTS,
        "live request budget exceeded: {requests} > {MAX_REQUESTS}"
    );
    let conservative_cost = usage
        .input
        .saturating_mul(CONSERVATIVE_INPUT_MICROUSD_PER_MILLION)
        .saturating_div(1_000_000)
        .saturating_add(
            usage
                .output
                .saturating_add(usage.reasoning)
                .saturating_mul(CONSERVATIVE_OUTPUT_MICROUSD_PER_MILLION)
                .saturating_div(1_000_000),
        );
    let guarded_cost = if usage.cost_microusd == 0 {
        conservative_cost
    } else {
        usage.cost_microusd.max(conservative_cost)
    };
    ensure!(
        guarded_cost < MAX_COST_MICROUSD,
        "live cost guard reached USD 10"
    );
    Ok(())
}

fn reserve_live_requests(requests: usize, additional: usize) -> Result<()> {
    const MAX_REQUESTS: usize = 40;
    ensure!(
        requests.saturating_add(additional) <= MAX_REQUESTS,
        "live request reservation would exceed the command budget: {requests} + {additional} > {MAX_REQUESTS}"
    );
    Ok(())
}

fn count_task_items(value: &Value, pointer: &str, task_id: &str, nested_request: bool) -> usize {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .map_or(0, |items| {
            items
                .iter()
                .filter(|item| {
                    let pointer = if nested_request {
                        "/request/taskId"
                    } else {
                        "/taskId"
                    };
                    item.pointer(pointer).and_then(Value::as_str) == Some(task_id)
                })
                .count()
        })
}

fn copy_directory(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copy_directory(&source_path, &destination_path)?;
        } else {
            fs::copy(&source_path, &destination_path)?;
        }
    }
    Ok(())
}

fn collect_openai_workflows(
    directory: &Path,
    root: &Path,
    output: &mut BTreeSet<String>,
) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_openai_workflows(&path, root, output)?;
        } else if matches!(
            path.extension().and_then(OsStr::to_str),
            Some("yaml" | "yml")
        ) {
            let source = fs::read_to_string(&path)?;
            if source.contains("apiVersion: agentctl.dev/v1alpha1")
                && source.contains("kind: openai")
            {
                output.insert(
                    path.strip_prefix(root)
                        .context("OpenAI example outside repository")?
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn write_live_summary(
    root: &Path,
    examples: &[Value],
    requests: usize,
    tool_calls: usize,
    usage: &UsageTotals,
    source_run_id: &str,
    repair_run_id: &str,
    replay_run_id: &str,
    status: &str,
) -> Result<()> {
    let directory = root.join(".release-evidence/selective-repair");
    fs::create_dir_all(&directory)?;
    let value = serde_json::json!({
        "formatVersion": 1,
        "status": status,
        "model": "gpt-5.6",
        "requestCount": requests,
        "toolCallCount": tool_calls,
        "usage": {
            "inputTokens": usage.input,
            "outputTokens": usage.output,
            "reasoningTokens": usage.reasoning,
            "cacheReadTokens": usage.cache_read,
            "cacheWriteTokens": usage.cache_write,
            "providerReportedCostMicrousd": usage.cost_microusd,
        },
        "selectiveRepair": {
            "sourceRunId": source_run_id,
            "repairRunId": repair_run_id,
            "replayRunId": replay_run_id,
            "upstreamReused": true,
            "replayFreshEffects": 0,
        },
        "examples": examples,
    });
    fs::write(
        directory.join("live-summary.json"),
        format!("{}\n", serde_json::to_string_pretty(&value)?),
    )?;
    Ok(())
}

fn write_live_container_summary(root: &Path, container: &LiveContainerSummary) -> Result<()> {
    let path = root.join(".release-evidence/selective-repair/live-summary.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&path)?)?;
    value
        .as_object_mut()
        .context("live summary must be an object")?
        .insert(
            "container".to_owned(),
            serde_json::json!({
                "sourceRunId": container.source_run_id,
                "repairRunId": container.repair_run_id,
                "replayRunId": container.replay_run_id,
                "requestCount": container.requests,
                "toolCallCount": container.tool_calls,
                "usage": {
                    "inputTokens": container.usage.input,
                    "outputTokens": container.usage.output,
                    "reasoningTokens": container.usage.reasoning,
                    "cacheReadTokens": container.usage.cache_read,
                    "cacheWriteTokens": container.usage.cache_write,
                    "providerReportedCostMicrousd": container.usage.cost_microusd,
                },
                "upstreamReused": true,
                "replayFreshEffects": 0,
            }),
        );
    fs::write(path, format!("{}\n", serde_json::to_string_pretty(&value)?))?;
    Ok(())
}

fn assert_error_metadata(value: &Value) -> Result<()> {
    ensure!(
        value
            .pointer("/error/runId")
            .and_then(Value::as_str)
            .is_some()
    );
    ensure!(
        value
            .pointer("/error/traceId")
            .and_then(Value::as_str)
            .is_some()
    );
    Ok(())
}

fn run_args(workflow: &Path, db: &Path, workspace: &Path, extra: &[String]) -> Vec<String> {
    let mut args = vec![
        "run".to_owned(),
        workflow.to_string_lossy().into_owned(),
        "--workspace".to_owned(),
        workspace.to_string_lossy().into_owned(),
        "--db".to_owned(),
        db.to_string_lossy().into_owned(),
        "--output".to_owned(),
        "json".to_owned(),
        "--color".to_owned(),
        "never".to_owned(),
    ];
    args.extend_from_slice(extra);
    args
}

#[derive(Default)]
struct ProtocolFixtureState {
    mcp_initializations: AtomicUsize,
    mcp_calls: AtomicUsize,
    a2a_sends: AtomicUsize,
    a2a_observations: AtomicUsize,
}

struct ProtocolFixtureServer {
    address: SocketAddr,
    state: Arc<ProtocolFixtureState>,
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl ProtocolFixtureServer {
    fn start() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let state = Arc::new(ProtocolFixtureState::default());
        let thread_state = Arc::clone(&state);
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_shutdown = Arc::clone(&shutdown);
        let handle = thread::spawn(move || {
            while !thread_shutdown.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = serve_protocol_request(&mut stream, address, &thread_state);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            address,
            state,
            shutdown,
            handle: Some(handle),
        })
    }

    const fn address(&self) -> SocketAddr {
        self.address
    }

    fn mcp_calls(&self) -> usize {
        self.state.mcp_calls.load(Ordering::SeqCst)
    }

    fn a2a_sends(&self) -> usize {
        self.state.a2a_sends.load(Ordering::SeqCst)
    }
}

impl Drop for ProtocolFixtureServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.address);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn serve_protocol_request(
    stream: &mut TcpStream,
    address: SocketAddr,
    state: &ProtocolFixtureState,
) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let request = read_http_request(stream)?;
    let header_end = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .context("fixture HTTP request has no header boundary")?;
    let headers = std::str::from_utf8(&request[..header_end])?;
    let request_line = headers.lines().next().context("fixture request line")?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().context("fixture request method")?;
    let path = request_parts.next().context("fixture request path")?;
    let body = &request[header_end + 4..];
    let body: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(body)?
    };
    let rpc_method = body.get("method").and_then(Value::as_str).unwrap_or("");
    let id = body.get("id").and_then(Value::as_u64).unwrap_or(0);
    let mut response_headers = Vec::new();
    let (status, response_body) = match (method, path, rpc_method) {
        ("POST", "/mcp", "initialize") => {
            let generation = state.mcp_initializations.fetch_add(1, Ordering::SeqCst) + 1;
            response_headers.push(("Mcp-Session-Id", format!("fixture-session-{generation}")));
            (
                200,
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": "2025-11-25",
                        "capabilities": {},
                        "serverInfo": {"name": "fixture", "version": "1"}
                    }
                })
                .to_string(),
            )
        }
        ("POST", "/mcp", "notifications/initialized") => (202, String::new()),
        ("POST", "/mcp", "tools/list") => (
            200,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [{
                        "name": "durable",
                        "inputSchema": {
                            "type": "object",
                            "properties": {"value": {"type": "string"}},
                            "required": ["value"]
                        }
                    }]
                }
            })
            .to_string(),
        ),
        ("POST", "/mcp", "tools/call") => {
            let call = state.mcp_calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call == 1 {
                (404, String::new())
            } else {
                (
                    200,
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "structuredContent": {"ok": true},
                            "isError": false
                        }
                    })
                    .to_string(),
                )
            }
        }
        ("GET", "/agent-card.json", _) => (
            200,
            serde_json::json!({
                "name": "fixture-agent",
                "description": "durable continuation fixture",
                "supportedInterfaces": [{
                    "url": format!("http://{address}/a2a"),
                    "protocolBinding": "JSONRPC",
                    "protocolVersion": "1.0"
                }],
                "capabilities": {"streaming": false},
                "skills": []
            })
            .to_string(),
        ),
        ("POST", "/a2a", "SendMessage") => {
            state.a2a_sends.fetch_add(1, Ordering::SeqCst);
            (
                200,
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "task": {
                            "id": "remote-task-1",
                            "status": {"state": "working"}
                        }
                    }
                })
                .to_string(),
            )
        }
        ("POST", "/a2a", "GetTask") => {
            let observation = state.a2a_observations.fetch_add(1, Ordering::SeqCst) + 1;
            let task = if observation == 1 {
                serde_json::json!({
                    "id": "remote-task-1",
                    "status": {"state": "working"}
                })
            } else {
                serde_json::json!({
                    "id": "remote-task-1",
                    "status": {"state": "completed"},
                    "artifacts": [{
                        "artifactId": "report",
                        "parts": [{
                            "text": "completed without resubmission",
                            "filename": "report.txt",
                            "mediaType": "text/plain"
                        }]
                    }]
                })
            };
            (
                200,
                serde_json::json!({"jsonrpc": "2.0", "id": id, "result": task}).to_string(),
            )
        }
        ("POST", "/a2a", "CancelTask") => (
            200,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "id": "remote-task-1",
                    "status": {"state": "canceled"}
                }
            })
            .to_string(),
        ),
        _ => (400, String::new()),
    };
    write_http_response(stream, status, &response_headers, &response_body)
}

fn read_http_request(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    let mut expected = None;
    loop {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..count]);
        ensure!(
            request.len() <= 1024 * 1024,
            "fixture request exceeded 1 MiB"
        );
        if expected.is_none()
            && let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n")
        {
            let headers = std::str::from_utf8(&request[..header_end])?;
            let content_length = headers.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            });
            expected = Some(header_end + 4 + content_length.unwrap_or(0));
        }
        if expected.is_some_and(|expected| request.len() >= expected) {
            break;
        }
    }
    Ok(request)
}

fn write_http_response(
    stream: &mut TcpStream,
    status: u16,
    extra_headers: &[(&str, String)],
    body: &str,
) -> Result<()> {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Error",
    };
    let mut response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in extra_headers {
        response.push_str(name);
        response.push_str(": ");
        response.push_str(value);
        response.push_str("\r\n");
    }
    response.push_str("\r\n");
    response.push_str(body);
    stream.write_all(response.as_bytes())?;
    stream.flush()?;
    Ok(())
}

fn successful_json(binary: &Path, cwd: &Path, args: &[String]) -> Result<Value> {
    json_with_code(binary, cwd, args, 0)
}

fn expect_code(binary: &Path, cwd: &Path, args: &[String], code: i32) -> Result<()> {
    output_with_code(command_for(binary, cwd, args), code, "agentctl")?;
    Ok(())
}

fn json_with_code(binary: &Path, cwd: &Path, args: &[String], code: i32) -> Result<Value> {
    let output = output_with_code(command_for(binary, cwd, args), code, "agentctl")?;
    parse_output(&output)
}

fn json_with_removed_env(
    binary: &Path,
    cwd: &Path,
    args: &[String],
    removed: &str,
    code: i32,
) -> Result<Value> {
    let mut command = command_for(binary, cwd, args);
    command.env_remove(removed);
    let output = output_with_code(command, code, "agentctl with removed credential")?;
    parse_output(&output)
}

fn json_with_path(
    binary: &Path,
    cwd: &Path,
    args: &[String],
    path: &Path,
    code: i32,
) -> Result<Value> {
    let mut command = command_for(binary, cwd, args);
    command.env("PATH", path);
    let output = output_with_code(command, code, "agentctl with isolated executable path")?;
    parse_output(&output)
}

fn json_with_env(
    binary: &Path,
    cwd: &Path,
    args: &[String],
    environment: &[(&str, &str)],
    code: i32,
) -> Result<Value> {
    let mut command = command_for(binary, cwd, args);
    for name in [
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "GOOGLE_API_KEY",
        "AZURE_OPENAI_API_KEY",
    ] {
        command.env_remove(name);
    }
    for (name, value) in environment {
        command.env(name, value);
    }
    let output = output_with_code(command, code, "agentctl with state key reference")?;
    parse_output(&output)
}

fn command_for(binary: &Path, cwd: &Path, args: &[String]) -> Command {
    let mut command = Command::new(binary);
    command.current_dir(cwd).args(args);
    command
}

fn output_with_code(command: Command, code: i32, label: &str) -> Result<Output> {
    let output = bounded_output(command, label).with_context(|| format!("run {label}"))?;
    if output.status.code() == Some(code) {
        Ok(output)
    } else {
        bail!(
            "{label} returned {:?}, expected {code}\n{}",
            output.status.code(),
            output_diagnostics(&output)
        )
    }
}

fn parse_output(output: &Output) -> Result<Value> {
    let bytes = if output.stdout.iter().any(|byte| !byte.is_ascii_whitespace()) {
        &output.stdout
    } else {
        &output.stderr
    };
    serde_json::from_slice(bytes)
        .with_context(|| format!("parse machine output: {}", String::from_utf8_lossy(bytes)))
}

fn ensure_eq<T>(value: &Value, pointer: &str, expected: T) -> Result<()>
where
    T: Into<Value>,
{
    let expected = expected.into();
    ensure!(
        value.pointer(pointer) == Some(&expected),
        "{pointer} was {:?}, expected {expected}",
        value.pointer(pointer)
    );
    Ok(())
}

fn string_at<'a>(value: &'a Value, pointer: &str) -> Result<&'a str> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .with_context(|| format!("missing string at {pointer}"))
}

fn array_len(value: &Value, pointer: &str) -> Result<usize> {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .map(Vec::len)
        .with_context(|| format!("missing array at {pointer}"))
}

fn path(value: &Path) -> Result<&str> {
    value.to_str().context("path is not UTF-8")
}

fn strings<const N: usize>(values: [&str; N]) -> Vec<String> {
    values.into_iter().map(ToOwned::to_owned).collect()
}

fn debug_binary(root: &Path) -> PathBuf {
    let target = env::var_os("CARGO_TARGET_DIR").map_or_else(
        || root.join("target"),
        |target| {
            let target = PathBuf::from(target);
            if target.is_absolute() {
                target
            } else {
                root.join(target)
            }
        },
    );
    target.join("debug").join(binary_name())
}

fn packaged_binary(root: &Path) -> Result<PathBuf> {
    let mut command = Command::new("rustc");
    command.arg("-vV");
    let output = bounded_output(command, "rustc -vV")?;
    ensure!(output.status.success(), "rustc -vV failed");
    let version = String::from_utf8(output.stdout)?;
    let host = version
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .context("rustc did not report a host target")?;
    Ok(root
        .join("dist")
        .join(format!("agentctl-{}-{host}", env!("CARGO_PKG_VERSION")))
        .join(binary_name()))
}

fn binary_name() -> &'static str {
    if cfg!(windows) {
        "agentctl.exe"
    } else {
        "agentctl"
    }
}

fn command(root: &Path, program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .current_dir(root)
        .args(args)
        .status()?;
    ensure!(
        status.success(),
        "{program} {} failed: {status}",
        args.join(" ")
    );
    Ok(())
}

fn scenario(number: usize, label: &str) {
    println!("[{number}/{ACCEPTANCE_SCENARIOS}] {label}");
}

fn write(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(())
}

fn copy_example(root: &Path, source: &str, destination: &Path) -> Result<()> {
    let source = root.join(source);
    fs::create_dir_all(destination.join("fixture"))?;
    fs::create_dir_all(destination.join("artifacts"))?;
    fs::copy(
        source.join("workflow.yaml"),
        destination.join("workflow.yaml"),
    )?;
    fs::copy(
        source.join("fixture/service.txt"),
        destination.join("fixture/service.txt"),
    )?;
    Ok(())
}

fn deterministic_workflow(name: &str, uses: &str, input: &str) -> String {
    format!(
        "apiVersion: agentctl.dev/v1alpha1\nkind: Workflow\nmetadata: {{ name: {name} }}\nspec:\n  tasks:\n    - id: task\n      uses: {uses}\n      with: {input}\n"
    )
}

fn agent_workflow(name: &str, option: &str, tool: &str, extra_option: &str) -> String {
    let tools = if tool.is_empty() {
        ""
    } else {
        "      tools: [echo]\n"
    };
    let provider_options = if option.is_empty() && extra_option.is_empty() {
        String::new()
    } else {
        format!("      providerOptions:\n        {option}\n        {extra_option}\n")
    };
    format!(
        "apiVersion: agentctl.dev/v1alpha1\nkind: Workflow\nmetadata: {{ name: {name} }}\nspec:\n  providers:\n    fake: {{ kind: fake }}\n{tool}  agents:\n    worker:\n      provider: fake\n      model: scripted\n      instructions: complete the fixture\n{tools}      maxTurns: 2\n      maxToolCalls: 1\n      timeoutSeconds: 1\n{provider_options}  tasks:\n    - id: work\n      uses: agent:worker\n      retry: {{ maxAttempts: 1 }}\n      with: {{ prompt: hello }}\n"
    )
}

fn echo_tool() -> &'static str {
    r#"  tools:
    echo:
      kind: builtin.echo
      description: Echo structured input.
      inputSchema:
        type: object
        properties: { text: { type: string } }
        required: [text]
        additionalProperties: false
      outputSchema:
        type: object
        properties: { text: { type: string } }
        required: [text]
        additionalProperties: false
      capability: internal
      risk: low
      effectClass: pure
      idempotency: pure
      retrySafe: true
      timeoutSeconds: 2
      approval: never
"#
}

fn mismatched_echo_tool() -> &'static str {
    r#"  tools:
    echo:
      kind: builtin.echo
      description: Deliberately incompatible output contract.
      inputSchema:
        type: object
        properties: { text: { type: string } }
        required: [text]
        additionalProperties: false
      outputSchema:
        type: object
        properties: { result: { type: string } }
        required: [result]
        additionalProperties: false
      capability: internal
      risk: low
      effectClass: pure
      idempotency: pure
      retrySafe: true
      timeoutSeconds: 2
      approval: never
"#
}

fn read_tool_workflow(name: &str, file: &str) -> String {
    format!(
        r#"apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: {{ name: {name} }}
spec:
  providers:
    fake: {{ kind: fake }}
  tools:
    read_fixture:
      kind: builtin.workspace.read
      description: Read a bounded UTF-8 fixture.
      inputSchema:
        type: object
        properties: {{ path: {{ type: string }} }}
        required: [path]
        additionalProperties: false
      outputSchema:
        type: object
        properties:
          path: {{ type: string }}
          content: {{ type: string }}
          bytes: {{ type: integer }}
          sha256: {{ type: string }}
        required: [path, content, bytes, sha256]
        additionalProperties: false
      capability: filesystem.read
      risk: low
      effectClass: observe
      idempotency: idempotent
      retrySafe: true
      timeoutSeconds: 2
      approval: never
  agents:
    reader:
      provider: fake
      model: scripted
      instructions: read once
      tools: [read_fixture]
      maxTurns: 2
      maxToolCalls: 1
      providerOptions:
        toolInput: {{ path: {file} }}
        finalText: unreachable
  tasks:
    - id: read
      uses: agent:reader
      with: {{ prompt: read }}
"#
    )
}

#[derive(Debug)]
struct ContainerLayout {
    config: PathBuf,
    workspace: PathBuf,
    state: PathBuf,
    artifacts: PathBuf,
}

fn container_layout(root: &Path, live: bool) -> Result<ContainerLayout> {
    let layout = ContainerLayout {
        config: root.join("config"),
        workspace: root.join("workspace"),
        state: root.join("state"),
        artifacts: root.join("artifacts"),
    };
    for path in [
        &layout.config,
        &layout.workspace,
        &layout.state,
        &layout.artifacts,
    ] {
        fs::create_dir_all(path)?;
    }
    write(
        &layout.workspace.join("fixture/service.txt"),
        if live {
            "service=agentctl\nmarker=OPENAI_TOOL_PATH_CONFIRMED\n"
        } else {
            "service=agentctl\nmarker=MOCK_TOOL_PATH_CONFIRMED\n"
        },
    )?;
    write(
        &layout.config.join("workflow.yaml"),
        if live {
            CONTAINER_LIVE_WORKFLOW
        } else {
            CONTAINER_MOCK_WORKFLOW
        },
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&layout.state, fs::Permissions::from_mode(0o777))?;
        fs::set_permissions(&layout.artifacts, fs::Permissions::from_mode(0o777))?;
    }
    Ok(layout)
}

fn container_engine() -> Result<PathBuf> {
    if let Some(engine) = env::var_os("AGENTCTL_CONTAINER_ENGINE") {
        return Ok(PathBuf::from(engine));
    }
    for candidate in ["docker", "podman"] {
        if executable_on_path(candidate) {
            return Ok(PathBuf::from(candidate));
        }
    }
    let podman = PathBuf::from("/opt/podman/bin/podman");
    if podman.is_file() {
        return Ok(podman);
    }
    bail!("Docker or Podman is required for OCI acceptance")
}

fn executable_on_path(name: &str) -> bool {
    env::var_os("PATH").is_some_and(|paths| {
        env::split_paths(&paths).any(|directory| directory.join(name).is_file())
    })
}

fn ensure_engine_ready(engine: &Path) -> Result<()> {
    let mut command = Command::new(engine);
    command.arg("info");
    let output = bounded_output(command, "container engine info")?;
    if output.status.success() {
        Ok(())
    } else {
        bail!(
            "container engine is installed but unavailable:\n{}",
            output_diagnostics(&output)
        )
    }
}

fn build_image(root: &Path, engine: &Path) -> Result<()> {
    let mut command = Command::new(engine);
    command.current_dir(root).arg("build");
    if let Some(ca_file) = env::var_os("AGENTCTL_BUILD_CA_FILE") {
        let ca_file = PathBuf::from(ca_file);
        ensure!(
            ca_file.is_file(),
            "AGENTCTL_BUILD_CA_FILE must refer to a readable certificate file"
        );
        let secret = format!("id=agentctl_ca,src={}", ca_file.display());
        command.args([OsStr::new("--secret"), OsStr::new(&secret)]);
    }
    command.args([
        OsStr::new("--file"),
        OsStr::new("Containerfile"),
        OsStr::new("--tag"),
        OsStr::new("agentctl-acceptance:local"),
        OsStr::new("."),
    ]);
    let output = bounded_output(command, "OCI image build")?;
    if output.status.success() {
        Ok(())
    } else {
        bail!("OCI image build failed:\n{}", output_diagnostics(&output))
    }
}

fn container_isolation_acceptance(root: &Path, engine: &Path) -> Result<()> {
    let mut inspect = Command::new(engine);
    inspect.args([
        "image",
        "inspect",
        "--format",
        "{{.Id}}",
        "agentctl-acceptance:local",
    ]);
    let output = bounded_output(inspect, "container isolation image inspection")?;
    ensure!(
        output.status.success(),
        "container isolation image inspection failed:\n{}",
        output_diagnostics(&output)
    );
    let image = normalize_container_image_id(&String::from_utf8(output.stdout)?)?;
    let runtime = engine
        .file_name()
        .and_then(OsStr::to_str)
        .filter(|name| name.contains("podman"))
        .map_or("docker", |_| "podman");
    let mut test = Command::new("cargo");
    test.current_dir(root)
        .args([
            "test",
            "-p",
            "agentctl-runtime",
            "container_isolation_live_executes_in_the_pinned_image",
            "--",
            "--ignored",
            "--nocapture",
        ])
        .env("AGENTCTL_TEST_CONTAINER_IMAGE", image)
        .env("AGENTCTL_TEST_CONTAINER_RUNTIME", runtime);
    let output = bounded_output(test, "container-isolated process acceptance")?;
    ensure!(
        output.status.success(),
        "container-isolated process acceptance failed:\n{}",
        output_diagnostics(&output)
    );
    Ok(())
}

fn normalize_container_image_id(image: &str) -> Result<String> {
    let image = image.trim();
    let digest = image.strip_prefix("sha256:").unwrap_or(image);
    ensure!(
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "container engine returned an invalid content-addressed image ID"
    );
    Ok(format!("sha256:{digest}"))
}

fn container_base(engine: &Path, layout: &ContainerLayout) -> Result<Command> {
    let mut command = Command::new(engine);
    command.args([
        "run",
        "--rm",
        "--read-only",
        "--user",
        "65532:65532",
        "--tmpfs",
        "/tmp:rw,noexec,nosuid,size=16m",
        "--volume",
        &format!("{}:/config:ro", path(&layout.config)?),
        "--volume",
        &format!("{}:/workspace:ro", path(&layout.workspace)?),
        "--volume",
        &format!("{}:/state:rw", path(&layout.state)?),
        "--volume",
        &format!("{}:/artifacts:rw", path(&layout.artifacts)?),
    ]);
    Ok(command)
}

fn run_container(
    engine: &Path,
    layout: &ContainerLayout,
    live: bool,
    credential: Option<&str>,
) -> Result<Value> {
    let mut command = container_base(engine, layout)?;
    if let Some(name) = credential {
        command.args(["--env", name]);
    }
    command.args([
        "agentctl-acceptance:local",
        "run",
        "/config/workflow.yaml",
        "--workspace",
        "/workspace",
        "--db",
        "/state/runtime.db",
        "--input",
        "reportPath=/artifacts/report.txt",
        "--output",
        "json",
        "--color",
        "never",
    ]);
    let output = output_with_code(command, 0, if live { "live OCI run" } else { "OCI run" })?;
    parse_output(&output)
}

fn run_container_with_code(
    engine: &Path,
    layout: &ContainerLayout,
    code: i32,
    label: &str,
) -> Result<Value> {
    let mut command = container_base(engine, layout)?;
    command.args([
        "agentctl-acceptance:local",
        "run",
        "/config/workflow.yaml",
        "--workspace",
        "/workspace",
        "--db",
        "/state/runtime.db",
        "--output",
        "json",
        "--color",
        "never",
    ]);
    parse_output(&output_with_code(command, code, label)?)
}

fn inspect_container(engine: &Path, layout: &ContainerLayout, run_id: &str) -> Result<Value> {
    let mut command = container_base(engine, layout)?;
    command.args([
        "agentctl-acceptance:local",
        "inspect",
        run_id,
        "--db",
        "/state/runtime.db",
        "--output",
        "json",
        "--color",
        "never",
    ]);
    parse_output(&output_with_code(command, 0, "OCI inspect")?)
}

fn replay_container(engine: &Path, layout: &ContainerLayout, run_id: &str) -> Result<Value> {
    let mut command = container_base(engine, layout)?;
    command.args(["--network", "none"]);
    command.args([
        "agentctl-acceptance:local",
        "replay",
        run_id,
        "--db",
        "/state/runtime.db",
        "--output",
        "json",
        "--color",
        "never",
    ]);
    parse_output(&output_with_code(command, 0, "keyless OCI replay")?)
}

fn container_agentctl(
    engine: &Path,
    layout: &ContainerLayout,
    args: &[&str],
    code: i32,
    label: &str,
) -> Result<Value> {
    let mut command = container_base(engine, layout)?;
    command.args(["--network", "none", "agentctl-acceptance:local"]);
    command.args(args);
    parse_output(&output_with_code(command, code, label)?)
}

fn container_agentctl_with_openai(
    engine: &Path,
    layout: &ContainerLayout,
    args: &[&str],
    code: i32,
    label: &str,
) -> Result<Value> {
    let mut command = container_base(engine, layout)?;
    command.args(["--env", "OPENAI_API_KEY", "agentctl-acceptance:local"]);
    command.args(args);
    parse_output(&output_with_code(command, code, label)?)
}

fn container_signal_acceptance(engine: &Path, root: &Path) -> Result<()> {
    let layout = container_layout(&root.join("container-signal"), false)?;
    write(
        &layout.config.join("workflow.yaml"),
        CONTAINER_SIGNAL_WORKFLOW,
    )?;
    let name = format!("agentctl-signal-{}", std::process::id());
    let mut command = container_base(engine, &layout)?;
    command.args([
        "--name",
        &name,
        "agentctl-acceptance:local",
        "run",
        "/config/workflow.yaml",
        "--workspace",
        "/workspace",
        "--db",
        "/state/runtime.db",
        "--output",
        "json",
        "--color",
        "never",
    ]);
    configure_piped_command(&mut command);
    let child = command.spawn()?;
    for _ in 0..100 {
        if layout.state.join("runtime.db").exists() {
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }
    ensure!(
        layout.state.join("runtime.db").exists(),
        "OCI run did not create durable state"
    );
    thread::sleep(Duration::from_millis(100));
    let mut stop = Command::new(engine);
    stop.args(["stop", "--time", "10", &name]);
    let stopped = bounded_output(stop, "stop OCI run")?;
    ensure!(
        stopped.status.success(),
        "failed to stop OCI run: {}",
        output_diagnostics(&stopped)
    );
    let output = bounded_wait(child, "SIGTERM OCI run")?;
    ensure!(
        output.status.code() == Some(130),
        "OCI SIGTERM exit was {:?}, expected 130; {}",
        output.status.code(),
        output_diagnostics(&output)
    );
    let value = parse_output(&output)?;
    ensure_eq(&value, "/data/state", "cancelled")?;
    Ok(())
}

const APPROVAL_WORKFLOW: &str = r#"apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: approval }
spec:
  policy:
    workspaceRoot: .
    writableRoots: [artifacts]
    approval: mutations
  actions:
    write: { kind: builtin.write }
  tasks:
    - id: write
      uses: action:write
      with: { path: artifacts/approved.txt, content: approved }
"#;

const CONFIRMED_BEFORE_APPROVAL_WORKFLOW: &str = r#"apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: confirmed-before-approval }
spec:
  providers:
    fake: { kind: fake }
  policy:
    workspaceRoot: .
    writableRoots: [artifacts]
    approval: mutations
  agents:
    worker:
      provider: fake
      model: scripted
      instructions: reply
  actions:
    write: { kind: builtin.write }
  tasks:
    - id: model
      uses: agent:worker
      with: { prompt: hello }
    - id: write
      uses: action:write
      needs: [model]
      with: { path: artifacts/confirmed.txt, content: confirmed }
"#;

const RETRY_WORKFLOW: &str = r#"apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: retry }
spec:
  providers:
    fake: { kind: fake }
  agents:
    worker:
      provider: fake
      model: scripted
      instructions: retry once
      providerOptions: { failFirst: 1, finalText: recovered }
  outputs:
    verdict: "${{ tasks.work.output.text }}"
  tasks:
    - id: work
      uses: agent:worker
      retry: { maxAttempts: 2, backoffMs: 1 }
      with: { prompt: hello }
"#;

const TERMINAL_RETRY_WORKFLOW: &str = r#"apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: terminal-retry }
spec:
  policy:
    processAllowlist: [sh]
    approval: never
  actions:
    assign: { kind: builtin.assign }
    recover:
      kind: builtin.shell.exec
      command: /bin/sh
      args: [-c, "if [ -f .terminal-retry-ready ]; then printf recovered; else touch .terminal-retry-ready; exit 1; fi"]
      timeoutSeconds: 5
  tasks:
    - id: first
      uses: action:assign
      with: { value: durable }
    - id: work
      uses: action:recover
      needs: [first]
    - id: third
      uses: action:assign
      needs: [work]
      with: { value: recovered }
"#;

const OPENAI_AUTH_WORKFLOW: &str = r#"apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: missing-auth }
spec:
  providers:
    openai:
      kind: openai
      credential: { env: OPENAI_API_KEY }
  agents:
    worker:
      provider: openai
      model: gpt-5.6
      instructions: reply
  tasks:
    - id: work
      uses: agent:worker
      with: { prompt: hello }
"#;

const OPENAI_RESOURCE_BUDGET_WORKFLOW: &str = r#"apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: live-resource-budget }
spec:
  runtime:
    budgets: { maxProviderRequests: 1 }
  providers:
    openai:
      kind: openai
      credential: { env: OPENAI_API_KEY }
  policy:
    networkAllowlist: [api.openai.com]
    approval: never
  agents:
    worker:
      provider: openai
      model: gpt-5.6
      instructions: Reply with exactly OK.
      maxTurns: 1
      maxToolCalls: 0
      maxOutputTokens: 64
      timeoutSeconds: 30
      reasoning: { effort: low }
  tasks:
    - { id: first, uses: "agent:worker", with: { prompt: Reply now. } }
    - { id: second, uses: "agent:worker", needs: [first], with: { prompt: Reply now. } }
"#;

const OPENAI_STATELESS_TOOL_WORKFLOW: &str = r#"apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: stateless-tools }
spec:
  providers: { openai: { kind: openai } }
  tools:
    echo:
      kind: builtin.echo
      description: echo
      inputSchema:
        type: object
        properties: {}
        required: []
        additionalProperties: false
      outputSchema:
        type: object
        properties: {}
        required: []
        additionalProperties: false
      capability: internal
      risk: low
      effectClass: pure
      idempotency: pure
      retrySafe: true
      timeoutSeconds: 5
      approval: never
  agents:
    worker:
      provider: openai
      model: gpt-5.6
      instructions: use echo
      tools: [echo]
      providerOptions: { store: false }
  tasks: [{ id: work, uses: "agent:worker" }]
"#;

const INPUTS_WORKFLOW: &str = r#"apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: inputs }
spec:
  inputs: { name: default, count: 1 }
  outputs:
    name: "${{ inputs.name }}"
    count: "${{ inputs.count }}"
  actions:
    assign: { kind: builtin.assign }
  tasks:
    - id: capture
      uses: action:assign
      with: { name: "${{ inputs.name }}", count: "${{ inputs.count }}" }
"#;

const TRAVERSAL_WORKFLOW: &str = r#"apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: traversal }
spec:
  policy:
    workspaceRoot: .
    writableRoots: [artifacts]
    approval: never
  actions:
    write: { kind: builtin.write }
  tasks:
    - id: write
      uses: action:write
      with: { path: ../escaped.txt, content: blocked }
"#;

const SELECTIVE_REPAIR_SOURCE_WORKFLOW: &str = r#"apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: selective-repair-acceptance }
spec:
  outputs:
    repaired: "${{ tasks.third.output.output.value }}"
  actions:
    assign: { kind: builtin.assign }
    assert: { kind: builtin.assert }
  tasks:
    - id: first
      uses: action:assign
      with: { value: durable }
    - id: second
      uses: action:assert
      needs: [first]
      with: { that: false, message: deliberately broken }
    - id: third
      uses: action:assign
      needs: [second]
      with: { value: repaired }
"#;

const SELECTIVE_REPAIR_TARGET_WORKFLOW: &str = r#"apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: selective-repair-acceptance }
spec:
  outputs:
    repaired: "${{ tasks.third.output.output.value }}"
  actions:
    assign: { kind: builtin.assign }
    assert: { kind: builtin.assert }
  tasks:
    - id: first
      uses: action:assign
      with: { value: durable }
    - id: second
      uses: action:assert
      needs: [first]
      with: { that: true, message: fixed }
    - id: third
      uses: action:assign
      needs: [second]
      with: { value: repaired }
"#;

const UNCERTAIN_REPAIR_SOURCE_WORKFLOW: &str = r#"apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: uncertain-repair-acceptance }
spec:
  policy:
    processAllowlist: [sh]
    approval: never
  actions:
    wait:
      kind: builtin.shell.exec
      command: /bin/sh
      args: [-c, "sleep 2"]
      timeoutSeconds: 1
  tasks:
    - { id: work, uses: "action:wait" }
"#;

const UNCERTAIN_REPAIR_TARGET_WORKFLOW: &str = r#"apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: uncertain-repair-acceptance }
spec:
  policy:
    processAllowlist: [sh]
    approval: never
  actions:
    assign: { kind: builtin.assign }
  tasks:
    - { id: work, uses: "action:assign", with: { recovered: true } }
"#;

const COMPENSATION_WORKFLOW: &str = r#"apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: compensation-acceptance }
spec:
  policy:
    workspaceRoot: .
    writableRoots: [artifacts]
    approval: never
  actions:
    write: { kind: builtin.write }
    assert: { kind: builtin.assert }
  tasks:
    - id: provision
      uses: action:write
      with:
        path: artifacts/compensation.txt
        content: provisioned
      compensate:
        uses: action:write
        with:
          path: artifacts/compensation.txt
          content: compensated
    - id: fail
      uses: action:assert
      needs: [provision]
      with: { that: false, message: expected acceptance failure }
"#;

const PROTOCOL_MCP_WORKFLOW: &str = r#"apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: mcp-resilience-acceptance }
spec:
  policy:
    networkAllowlist: [127.0.0.1]
    network: { allowPrivate: true }
    approval: never
  mcpServers:
    fixture:
      url: __MCP_URL__
      protocolVersion: 2025-11-25
      timeoutSeconds: 2
  actions:
    invoke:
      kind: mcp.call
      idempotency: idempotent
  tasks:
    - id: invoke
      uses: action:invoke
      outputSchema:
        type: object
        required: [ok]
        properties: { ok: { const: true } }
      with:
        server: fixture
        tool: durable
        arguments: { value: safe }
"#;

const PROTOCOL_A2A_WORKFLOW: &str = r#"apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: a2a-resilience-acceptance }
spec:
  outputs:
    result: "${{ tasks.consume.output.output.value }}"
  policy:
    networkAllowlist: [127.0.0.1]
    network: { allowPrivate: true }
    approval: never
  a2aPeers:
    fixture:
      cardUrl: __A2A_CARD_URL__
      protocolVersion: "1.0"
      timeoutSeconds: 2
      maxPolls: 1
      pollIntervalMs: 1
  actions:
    delegate: { kind: a2a.delegate }
    consume: { kind: builtin.assign }
  tasks:
    - id: delegate
      uses: action:delegate
      outputSchema:
        type: object
        required: [id, status]
        properties:
          id: { const: remote-task-1 }
          status:
            type: object
            required: [state]
            properties: { state: { const: completed } }
      with:
        peer: fixture
        messageId: message-1
        message: perform durable work
    - id: consume
      uses: action:consume
      needs: [delegate]
      with:
        value: "${{ tasks.delegate.output.id }}"
"#;

const NETWORK_PRIVATE_DENIED_WORKFLOW: &str = r#"apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: network-private-denied }
spec:
  policy:
    networkAllowlist: [127.0.0.1]
    approval: never
  mcpServers:
    fixture:
      url: http://127.0.0.1:9/mcp
      protocolVersion: 2025-11-25
      timeoutSeconds: 1
  actions:
    invoke:
      kind: mcp.call
      idempotency: idempotent
  tasks:
    - id: invoke
      uses: action:invoke
      with:
        server: fixture
        tool: unreachable
        arguments: {}
"#;

const CONTAINER_ISOLATION_UNAVAILABLE_WORKFLOW: &str = r#"apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: container-isolation-unavailable }
spec:
  policy:
    processAllowlist: [fixture]
    approval: never
  actions:
    isolated:
      kind: builtin.shell.exec
      command: fixture
      isolation: container
      container:
        image: sha256:0000000000000000000000000000000000000000000000000000000000000000
        runtime: docker
  tasks:
    - id: isolated
      uses: action:isolated
"#;

const RESOURCE_BUDGET_WORKFLOW: &str = r#"apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: resource-budget }
spec:
  runtime:
    budgets: { maxProviderRequests: 1 }
  policy: { approval: never }
  providers:
    fake: { kind: fake }
  agents:
    worker:
      provider: fake
      model: scripted
      instructions: return the prompt
      maxTurns: 1
      maxOutputTokens: 32
  tasks:
    - { id: first, uses: "agent:worker", with: { prompt: first } }
    - { id: second, uses: "agent:worker", needs: [first], with: { prompt: second } }
"#;

const CONTAINER_MOCK_WORKFLOW: &str = r#"apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: container-mock }
spec:
  inputs: { reportPath: /artifacts/report.txt }
  outputs:
    verdict: "${{ tasks.inspect.output.text }}"
  providers:
    fake: { kind: fake }
  policy:
    workspaceRoot: /workspace
    writableRoots: [/artifacts]
    approval: never
  tools:
    read_fixture:
      kind: builtin.workspace.read
      description: Read the mounted fixture.
      inputSchema:
        type: object
        properties: { path: { type: string } }
        required: [path]
        additionalProperties: false
      outputSchema:
        type: object
        properties:
          path: { type: string }
          content: { type: string }
          bytes: { type: integer }
          sha256: { type: string }
        required: [path, content, bytes, sha256]
        additionalProperties: false
      capability: filesystem.read
      risk: low
      effectClass: observe
      idempotency: idempotent
      retrySafe: true
      timeoutSeconds: 5
      approval: never
  agents:
    inspector:
      provider: fake
      model: scripted
      instructions: inspect fixture
      tools: [read_fixture]
      maxTurns: 2
      maxToolCalls: 1
      providerOptions:
        toolInput: { path: fixture/service.txt }
        finalText: AGENTCTL_MOCK_FIXTURE_VERIFIED
  actions:
    write: { kind: builtin.write }
  tasks:
    - id: inspect
      uses: agent:inspector
      with: { prompt: inspect }
    - id: report
      uses: action:write
      needs: [inspect]
      with: { path: "${{ inputs.reportPath }}", content: "${{ tasks.inspect.output.text }}" }
"#;

const CONTAINER_SIGNAL_WORKFLOW: &str = r#"apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: container-signal }
spec:
  providers: { fake: { kind: fake } }
  agents:
    worker:
      provider: fake
      model: scripted
      instructions: wait
      timeoutSeconds: 30
      providerOptions: { delayMs: 10000 }
  tasks:
    - id: wait
      uses: agent:worker
      with: { prompt: wait }
"#;

const CONTAINER_LIVE_WORKFLOW: &str = r#"apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: { name: container-openai-live }
spec:
  inputs: { reportPath: /artifacts/report.txt }
  outputs:
    verdict: "${{ tasks.inspect.output.text }}"
  providers:
    openai:
      kind: openai
      credential: { env: OPENAI_API_KEY }
  policy:
    workspaceRoot: /workspace
    writableRoots: [/artifacts]
    networkAllowlist: [api.openai.com]
    approval: never
  tools:
    read_fixture:
      kind: builtin.workspace.read
      description: Read one UTF-8 fixture file inside the mounted workspace.
      inputSchema:
        type: object
        properties: { path: { type: string } }
        required: [path]
        additionalProperties: false
      outputSchema:
        type: object
        properties:
          path: { type: string }
          content: { type: string }
          bytes: { type: integer }
          sha256: { type: string }
        required: [path, content, bytes, sha256]
        additionalProperties: false
      capability: filesystem.read
      risk: low
      effectClass: observe
      idempotency: idempotent
      retrySafe: true
      timeoutSeconds: 5
      approval: never
  agents:
    inspector:
      provider: openai
      model: gpt-5.6
      instructions: Call read_fixture exactly once with path fixture/service.txt. If the result contains marker=OPENAI_TOOL_PATH_CONFIRMED, reply exactly AGENTCTL_LIVE_FIXTURE_VERIFIED with no other text.
      tools: [read_fixture]
      maxTurns: 3
      maxToolCalls: 1
      maxOutputTokens: 64
      timeoutSeconds: 45
      reasoning: { effort: low }
      providerOptions:
        store: false
        reasoningContext: current_turn
        promptCacheMode: implicit
        promptCacheTtl: 30m
        parallelToolCalls: false
  actions:
    write: { kind: builtin.write }
  tasks:
    - id: inspect
      uses: agent:inspector
      with: { prompt: Perform the required fixture inspection now. }
    - id: report
      uses: action:write
      needs: [inspect]
      with: { path: "${{ inputs.reportPath }}", content: "${{ tasks.inspect.output.text }}" }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn stateless_live_evidence() -> Value {
        serde_json::json!({
            "data": {
                "effects": [
                    {
                        "request": {
                            "effectClass": "model",
                            "input": {"providerOptions": {"store": false}}
                        },
                        "result": {
                            "assistantContent": [
                                {"type": "tool_call", "id": "call-1"}
                            ]
                        }
                    },
                    {
                        "request": {
                            "effectClass": "model",
                            "input": {"providerOptions": {"store": false}}
                        },
                        "result": {
                            "assistantContent": [
                                {"type": "text", "text": "done"}
                            ]
                        }
                    }
                ],
                "toolCalls": [{"callId": "call-1", "status": "succeeded"}],
                "providerSessions": [{
                    "continuation": {
                        "kind": "conversation",
                        "value": [
                            {
                                "role": "assistant",
                                "content": [{"type": "tool_call", "id": "call-1"}]
                            },
                            {
                                "role": "user",
                                "content": [{"type": "tool_result", "id": "call-1"}]
                            }
                        ]
                    }
                }],
                "checkpoints": [{}],
                "traces": [{}]
            }
        })
    }

    #[test]
    fn stateless_live_evidence_allows_a_turn_without_reasoning_items() {
        assert_live_evidence(&stateless_live_evidence()).expect("valid stateless evidence");
    }

    #[test]
    fn stateless_live_evidence_rejects_unencrypted_reasoning_items() {
        let mut evidence = stateless_live_evidence();
        evidence["data"]["effects"][0]["result"]["assistantContent"]
            .as_array_mut()
            .expect("assistant content")
            .push(serde_json::json!({
                "type": "opaque_reasoning",
                "value": {"type": "reasoning"}
            }));
        assert!(assert_live_evidence(&evidence).is_err());
    }

    #[test]
    fn container_image_ids_are_normalized_across_engine_formats() {
        let digest = "a".repeat(64);
        assert_eq!(
            normalize_container_image_id(&digest).expect("Podman image ID"),
            format!("sha256:{digest}")
        );
        assert_eq!(
            normalize_container_image_id(&format!(" sha256:{digest}\n")).expect("Docker image ID"),
            format!("sha256:{digest}")
        );
    }

    #[test]
    fn malformed_container_image_ids_are_rejected() {
        assert!(normalize_container_image_id(&"a".repeat(63)).is_err());
        assert!(normalize_container_image_id(&format!("sha512:{}", "a".repeat(64))).is_err());
        assert!(normalize_container_image_id(&format!("sha256:{}", "g".repeat(64))).is_err());
    }

    #[test]
    fn live_request_reservations_fail_before_crossing_the_command_budget() {
        reserve_live_requests(33, LIVE_CONTAINER_MAX_REQUESTS).expect("remaining requests fit");
        assert!(reserve_live_requests(34, LIVE_CONTAINER_MAX_REQUESTS).is_err());
    }
}
