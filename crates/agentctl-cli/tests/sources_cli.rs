use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

fn cli(cwd: &Path, args: &[&str], expected: i32) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_agentctl"))
        .current_dir(cwd)
        .env_remove("OPENAI_API_KEY")
        .args(args)
        .args(["--output", "json"])
        .output()
        .expect("execute real CLI");
    assert_eq!(
        output.status.code(),
        Some(expected),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(if output.stdout.is_empty() {
        &output.stderr
    } else {
        &output.stdout
    })
    .expect("stable JSON envelope")
}

fn workflow() -> Value {
    json!({
        "apiVersion": "agentctl.dev/v1", "kind": "Workflow", "metadata": {"name":"source-cli"},
        "spec": {
            "varsFiles": ["defaults.yaml", "later.json"],
            "vars": {"winner":"workflow", "object":{"inline":true}},
            "actions": {"assign":{"kind":"builtin.assign"}},
            "tasks": [{"id":"result", "uses":"action:assign", "varsFiles":["task.yaml"],
                "vars":{"winner":"task"}, "with":{"winner":"${{ vars.winner }}", "object":"${{ vars.object }}", "nullable":"${{ vars.nullable }}"}}],
            "outputs": {"result":"${{ tasks.result.output.output }}"}
        }
    })
}

fn write_fixture(root: &Path, value: &Value) -> String {
    fs::write(
        root.join("defaults.yaml"),
        "winner: file\nobject: {old: true}\nnullable: old\n",
    )
    .unwrap();
    fs::write(
        root.join("later.json"),
        r#"{"winner":"later","nullable":null}"#,
    )
    .unwrap();
    fs::write(root.join("task.yaml"), "winner: task-file\n").unwrap();
    let path = root.join("workflow.json");
    fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
    path.to_str().unwrap().to_owned()
}

#[test]
fn ordered_files_cli_overrides_origins_and_replay_from_another_directory() {
    let root = tempfile::tempdir().unwrap();
    let caller = tempfile::tempdir().unwrap();
    let path = write_fixture(root.path(), &workflow());
    let db = root.path().join("state.db");
    let db = db.to_str().unwrap();
    let run = cli(
        caller.path(),
        &[
            "run",
            &path,
            "--db",
            db,
            "--var",
            "winner=\"first\"",
            "--var",
            "winner=\"invocation\"",
        ],
        0,
    );
    assert_eq!(run["data"]["output"]["result"]["winner"], "invocation");
    assert_eq!(
        run["data"]["output"]["result"]["object"],
        json!({"inline":true})
    );
    assert!(run["data"]["output"]["result"]["nullable"].is_null());
    let explanation = cli(
        caller.path(),
        &[
            "explain",
            &path,
            "--var",
            "winner=\"PRIVATE_VALUE_SENTINEL\"",
        ],
        0,
    );
    let serialized = explanation.to_string();
    assert!(serialized.contains("--var"));
    assert!(!serialized.contains("PRIVATE_VALUE_SENTINEL"));
    fs::remove_file(root.path().join("defaults.yaml")).unwrap();
    fs::remove_file(root.path().join("later.json")).unwrap();
    fs::remove_file(root.path().join("task.yaml")).unwrap();
    fs::remove_file(&path).unwrap();
    let run_id = run["data"]["runId"].as_str().unwrap();
    let replay = cli(caller.path(), &["replay", run_id, "--db", db], 0);
    assert_eq!(replay["data"]["output"], run["data"]["output"]);
    let replay_id = replay["data"]["runId"].as_str().unwrap();
    let inspection = cli(caller.path(), &["inspect", replay_id, "--db", db], 0);
    assert_eq!(inspection["data"]["effects"].as_array().unwrap().len(), 0);
}

#[test]
fn duplicate_and_reserved_variables_fail_before_creating_run_state() {
    let root = tempfile::tempdir().unwrap();
    let path = write_fixture(root.path(), &workflow());
    fs::write(
        root.path().join("defaults.yaml"),
        "winner: first\nwinner: second\n",
    )
    .unwrap();
    cli(root.path(), &["check", &path], 2);
    write_fixture(root.path(), &workflow());
    cli(root.path(), &["run", &path, "--var", "loopIndex=7"], 2);
    cli(root.path(), &["run", &path, "--var", "winner=not-json"], 2);
    assert!(!root.path().join(".agentctl/runtime.db").exists());
}

#[test]
fn check_reads_instruction_files_and_reports_missing_source() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("workflow.json");
    fs::write(&path, json!({
        "apiVersion":"agentctl.dev/v1", "kind":"Workflow", "metadata":{"name":"instructions-preflight"},
        "spec": {"providers":{"fake":{"kind":"fake"}}, "agents":{"a":{"provider":"fake", "model":"fake", "instructionsFile":"missing.md"}}, "tasks":[{"id":"a", "uses":"agent:a", "with":{"prompt":"hello"}}]}
    }).to_string()).unwrap();
    let result = cli(root.path(), &["check", path.to_str().unwrap()], 2);
    assert!(result.to_string().contains("instructionsFile"));
    fs::write(
        root.path().join("missing.md"),
        "Use the PRIVATE_INSTRUCTION_SENTINEL captured instructions.",
    )
    .unwrap();
    cli(root.path(), &["check", path.to_str().unwrap()], 0);
    let plan = cli(root.path(), &["plan", path.to_str().unwrap()], 0);
    assert!(!plan.to_string().contains("PRIVATE_INSTRUCTION_SENTINEL"));
    assert!(plan.to_string().contains("instructionsDigest"));
}

#[test]
fn doctor_missing_credentials_is_nonzero_and_does_not_create_state() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("workflow.json");
    fs::write(&path, json!({
        "apiVersion":"agentctl.dev/v1", "kind":"Workflow", "metadata":{"name":"doctor-missing"},
        "spec": {"providers":{"openai":{"kind":"openai"}}, "policy":{"networkAllowlist":["api.openai.com"]},
            "agents":{"a":{"provider":"openai", "model":"gpt-5-mini", "instructions":"hello"}},
            "tasks":[{"id":"a", "uses":"agent:a", "with":{"prompt":"hello"}}]}
    }).to_string()).unwrap();
    let doctor = cli(root.path(), &["doctor", path.to_str().unwrap()], 6);
    assert_eq!(doctor["kind"], "DoctorResult");
    assert_eq!(doctor["data"]["ready"], false);
    assert_eq!(doctor["data"]["checks"][0]["status"], "missing");
    assert!(!root.path().join(".agentctl").exists());
}
