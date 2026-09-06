use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

fn workflow(command: &Path, args: &[&str]) -> Value {
    json!({
        "apiVersion":"agentctl.dev/v1", "kind":"Workflow", "metadata":{"name":"doctor-host"},
        "spec": {
            "policy":{"processAllowlist":[command.file_name().unwrap().to_str().unwrap()], "approval":"never"},
            "actions":{"host":{"kind":"builtin.shell.exec", "command":command, "args":args}},
            "tasks":[{"id":"host", "uses":"action:host"}]
        }
    })
}

fn doctor(root: &Path, workflow: &Value, expected: i32) -> Value {
    let path = root.join("workflow.json");
    fs::write(&path, serde_json::to_vec(workflow).unwrap()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_agentctl"))
        .current_dir(root)
        .env_remove("AGENTCTL_DOCTOR_TEST_SECRET")
        .args(["doctor", path.to_str().unwrap(), "--output", "json"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(expected),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["apiVersion"], "agentctl.dev/cli/v1");
    assert_eq!(result["kind"], "DoctorResult");
    assert_eq!(result["data"]["ready"], expected == 0);
    assert!(
        !root.join(".agentctl").exists(),
        "preflight must not create run state"
    );
    result["data"].clone()
}

fn check<'a>(result: &'a Value, name: &str) -> &'a Value {
    result["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["check"] == name)
        .unwrap()
}

#[test]
fn doctor_rejects_missing_explicit_host_executable_without_dispatch() {
    let root = tempfile::tempdir().unwrap();
    let result = doctor(
        root.path(),
        &workflow(&root.path().join("missing-python"), &[]),
        6,
    );
    assert_eq!(
        check(&result, "process:host:command")["status"],
        "missing_or_not_executable"
    );
    let human = Command::new(env!("CARGO_BIN_EXE_agentctl"))
        .current_dir(root.path())
        .args(["doctor", "workflow.json"])
        .output()
        .unwrap();
    assert_eq!(human.status.code(), Some(6));
    let text = String::from_utf8(human.stdout).unwrap();
    assert!(text.contains("process:host:command: missing_or_not_executable"));
    assert!(text.contains("execution readiness unverified"));
}

#[test]
fn doctor_does_not_claim_bare_command_lookup_or_helper_dependencies_are_verified() {
    let root = tempfile::tempdir().unwrap();
    let result = doctor(
        root.path(),
        &workflow(Path::new("missing-host-command"), &["helper.py"]),
        6,
    );
    assert_eq!(
        check(&result, "process:host:command")["status"],
        "unchecked"
    );
    assert!(
        result["unverified"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("helper-internal")
    );
    assert!(
        result["scope"]
            .as_str()
            .unwrap()
            .contains("does not establish workflow execution readiness")
    );
}

#[test]
fn doctor_checks_extension_interpreter_script_in_action_working_directory() {
    let root = tempfile::tempdir().unwrap();
    let script_dir = root.path().join("helpers");
    fs::create_dir(&script_dir).unwrap();
    // Copy a native binary under an interpreter name; doctor must only inspect
    // metadata, so it need not actually be Python or support its arguments.
    let interpreter = root.path().join(if cfg!(windows) {
        "python3.exe"
    } else {
        "python3"
    });
    fs::copy(env!("CARGO_BIN_EXE_agentctl"), &interpreter).unwrap();
    let mut fixture = workflow(&interpreter, &["analyze.py"]);
    fixture["spec"]["actions"]["host"]["kind"] = json!("extension.process");
    fixture["spec"]["actions"]["host"]["idempotency"] = json!("unknown");
    fixture["spec"]["actions"]["host"]["protocolVersion"] =
        json!("agentctl.dev/process-extension/v1");
    fixture["spec"]["actions"]["host"]["inputSchema"] = json!({"type":"object"});
    fixture["spec"]["actions"]["host"]["outputSchema"] = json!({"type":"object"});
    fixture["spec"]["actions"]["host"]["capabilities"] = json!(["doctor.fixture"]);
    fixture["spec"]["actions"]["host"]["cwd"] = json!("helpers");
    let missing = doctor(root.path(), &fixture, 6);
    assert_eq!(check(&missing, "process:host:command")["status"], "present");
    assert_eq!(
        check(&missing, "process:host:script")["status"],
        "missing_or_not_file"
    );
    fs::write(
        script_dir.join("analyze.py"),
        "PRIVATE_HELPER_CONTENT_SENTINEL",
    )
    .unwrap();
    let present = doctor(root.path(), &fixture, 0);
    assert_eq!(check(&present, "process:host:script")["status"], "present");
    assert_eq!(present["unverified"][0]["status"], "unchecked");
    assert!(
        !present
            .to_string()
            .contains("PRIVATE_HELPER_CONTENT_SENTINEL")
    );
}

#[test]
fn doctor_reports_module_dependencies_as_unverified_without_importing_them() {
    let root = tempfile::tempdir().unwrap();
    let interpreter = root.path().join(if cfg!(windows) {
        "python3.exe"
    } else {
        "python3"
    });
    fs::copy(env!("CARGO_BIN_EXE_agentctl"), &interpreter).unwrap();
    let result = doctor(
        root.path(),
        &workflow(&interpreter, &["-m", "missing.module"]),
        0,
    );
    assert_eq!(result["checks"].as_array().unwrap().len(), 1);
    assert!(
        result["unverified"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("modules")
    );
}

#[test]
fn doctor_rejects_missing_cwd_and_missing_process_environment_reference() {
    let root = tempfile::tempdir().unwrap();
    let mut fixture = workflow(Path::new(env!("CARGO_BIN_EXE_agentctl")), &[]);
    fixture["spec"]["actions"]["host"]["cwd"] = json!("missing");
    let missing_cwd = doctor(root.path(), &fixture, 6);
    assert_eq!(
        check(&missing_cwd, "process:host:cwd")["status"],
        "missing_or_denied"
    );
    fixture["spec"]["actions"]["host"]
        .as_object_mut()
        .unwrap()
        .remove("cwd");
    fixture["spec"]["actions"]["host"]["env"] =
        json!({"TOKEN":{"env":"AGENTCTL_DOCTOR_TEST_SECRET"}});
    fixture["spec"]["policy"]["environmentAllowlist"] = json!(["TOKEN"]);
    let missing_env = doctor(root.path(), &fixture, 6);
    assert_eq!(
        check(&missing_env, "process:host:env:TOKEN")["status"],
        "missing"
    );
}

#[test]
fn doctor_preserves_ready_for_workflows_without_external_prerequisites() {
    let root = tempfile::tempdir().unwrap();
    let fixture = json!({"apiVersion":"agentctl.dev/v1", "kind":"Workflow", "metadata":{"name":"pure"},
        "spec":{"actions":{"assign":{"kind":"builtin.assign"}}, "tasks":[{"id":"assign", "uses":"action:assign", "with":{"value":1}}]}});
    let result = doctor(root.path(), &fixture, 0);
    assert_eq!(result["checks"], json!([]));
}

#[cfg(unix)]
#[test]
fn doctor_checks_executable_permissions_without_running_a_host_helper() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let helper = root.path().join("helper");
    fs::write(&helper, "#!/bin/sh\nprintf dispatched > marker\n").unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o600)).unwrap();
    let fixture = workflow(&helper, &[]);
    let blocked = doctor(root.path(), &fixture, 6);
    assert_eq!(
        check(&blocked, "process:host:command")["status"],
        "missing_or_not_executable"
    );
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
    doctor(root.path(), &fixture, 0);
    assert!(!root.path().join("marker").exists());
}
