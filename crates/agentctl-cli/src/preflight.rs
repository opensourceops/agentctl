//! Non-dispatching prerequisite checks. Secret values are never loaded here.

use std::path::{Path, PathBuf};

use agentctl_core::dsl::{ActionDefinition, ProcessIsolation, ProviderKind, SecretReference};
use agentctl_core::policy::PolicyEngine;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::{
    CliError, EXIT_OK, EXIT_REMOTE, OutputFormat, WorkflowFile, default_credential_env,
    load_with_options, print_value, resolve_base_path,
};

pub async fn doctor(output: OutputFormat, args: &WorkflowFile) -> Result<u8, CliError> {
    let path = &args.file;
    let (workflow, plan, diagnostics) =
        load_with_options(path, args.workspace.as_deref(), &args.variables)?;
    let base = resolve_base_path(
        args.workspace
            .as_deref()
            .or_else(|| path.parent().filter(|path| !path.as_os_str().is_empty())),
    )?;
    let policy = PolicyEngine::new(workflow.spec.policy.clone(), &base)
        .map_err(|error| CliError::validation(error.to_string()))?;
    let mut checks = Vec::new();
    let mut unverified = Vec::new();
    for requirement in &plan.requirements.providers {
        let provider = &workflow.spec.providers[&requirement.name];
        if provider.kind == ProviderKind::Fake {
            continue;
        }
        let reference = provider.credential.clone().unwrap_or_else(|| {
            SecretReference::environment(default_credential_env(provider.kind.clone()))
        });
        let mut check = credential_check(&reference, &policy);
        check["check"] = json!(format!("provider:{}", requirement.name));
        checks.push(check);
    }
    for requirement in &plan.requirements.processes {
        let action = &workflow.spec.actions[&requirement.action];
        if requirement.isolation != ProcessIsolation::Container {
            host_process_checks(
                &requirement.action,
                action,
                &base,
                &policy,
                &mut checks,
                &mut unverified,
            );
            continue;
        }
        let probe =
            agentctl_runtime::prepare_process_isolation(action, &CancellationToken::new()).await;
        let (available, engine) = match probe {
            Ok(backend) => (true, backend.backend_name()),
            Err(_) => (false, None),
        };
        checks.push(json!({
            "check": format!("container:{}", requirement.action),
            "available": available,
            "status": if available { "present" } else { "unavailable" },
            "engine": engine,
            "detail": if available { "engine responded and the pinned image is present" }
                else { "start the configured Docker/Podman engine and load the declared pinned image" },
        }));
    }
    let ready = checks.iter().all(|check| check["available"] == true);
    let mut human = format!(
        "{}: {} (checked local prerequisites only; workflow execution readiness unverified)",
        if ready { "ready" } else { "not ready" },
        workflow.metadata.name
    );
    for check in checks.iter().chain(&unverified) {
        human.push_str(&format!(
            "\n{}: {} — {}",
            check["check"].as_str().unwrap_or_default(),
            check["status"].as_str().unwrap_or_default(),
            check["detail"].as_str().unwrap_or_default()
        ));
    }
    print_value(
        output,
        "DoctorResult",
        &json!({
            "ready": ready,
            "workflow": workflow.metadata.name,
            "planDigest": plan.plan_digest,
            "checks": checks,
            "unverified": unverified,
            "scope": "checked local prerequisites only; ready does not establish workflow execution readiness. Provider access, executable formats, helper dependencies and secret process results require separate validation",
        }),
        diagnostics,
        human,
    )?;
    Ok(if ready { EXIT_OK } else { EXIT_REMOTE })
}

fn host_process_checks(
    name: &str,
    action: &ActionDefinition,
    base: &Path,
    policy: &PolicyEngine,
    checks: &mut Vec<Value>,
    unverified: &mut Vec<Value>,
) {
    let cwd = action
        .cwd
        .as_deref()
        .map(|path| policy.resolve_read_path(path))
        .transpose()
        .map(|path| path.unwrap_or_else(|| base.to_path_buf()));
    let cwd = match cwd {
        Ok(path) if path.is_dir() => path,
        _ => {
            checks.push(json!({"check":format!("process:{name}:cwd"), "available":false,
                "status":"missing_or_denied", "detail":"declare an existing working directory allowed by the workspace policy"}));
            return;
        }
    };
    let command = action.command.as_deref().unwrap_or_default();
    let command_path = Path::new(command);
    let explicit_path = command_path.is_absolute() || command_path.components().count() > 1;
    let (available, status, detail) = if policy.authorize_process(command).is_err() {
        (
            false,
            "denied",
            "the declared command is not allowed by processAllowlist",
        )
    } else if explicit_path {
        if executable_present(&cwd.join(command_path)) {
            (
                true,
                "present",
                "regular file with executable metadata found; effective OS access, format, loader and transitive dependencies are unverified",
            )
        } else {
            (
                false,
                "missing_or_not_executable",
                "install the declared executable or correct its path and executable permissions",
            )
        }
    } else {
        // Runtime clears the environment and may resolve PATH through a secret
        // reference. The invoking shell's PATH is not evidence of that lookup.
        (
            false,
            "unchecked",
            "bare command lookup depends on the runtime platform and cleared environment; use an explicit executable path or verify it during an authorized run. Doctor does not read a secret-derived PATH",
        )
    };
    checks.push(
        json!({"check":format!("process:{name}:command"), "available":available,
        "status":status, "detail":detail}),
    );
    for (variable, reference) in &action.env {
        let mut check = credential_check(reference, policy);
        check["check"] = json!(format!("process:{name}:env:{variable}"));
        if policy.authorize_environment(variable).is_err() {
            check["available"] = json!(false);
            check["status"] = json!("denied");
            check["detail"] = json!("the variable is not allowed by environmentAllowlist");
        }
        checks.push(check);
    }
    if let Some(script) = interpreter_script(command_path, &action.args) {
        let present = std::fs::metadata(cwd.join(script)).is_ok_and(|metadata| metadata.is_file());
        checks.push(json!({"check":format!("process:{name}:script"), "available":present,
            "status":if present {"present"} else {"missing_or_not_file"},
            "detail":"direct interpreter script argument checked as a regular file relative to the action working directory; contents, imports and child commands are not inspected"}));
    }
    unverified.push(json!({"check":format!("process:{name}:dependencies"), "status":"unchecked",
        "detail":"no command or helper was executed. Executable format, interpreter/loader, modules, command strings, further argument paths and helper-internal dependencies are unverified; validate them separately before relying on workflow execution"}));
}

fn executable_present(path: &Path) -> bool {
    let candidates = executable_candidates(path);
    candidates.iter().any(|path| {
        std::fs::metadata(path).is_ok_and(|metadata| {
            if !metadata.is_file() {
                return false;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                metadata.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            {
                true
            }
        })
    })
}

fn executable_candidates(path: &Path) -> Vec<PathBuf> {
    let paths = vec![path.to_path_buf()];
    #[cfg(windows)]
    let paths = {
        let mut paths = paths;
        if path.extension().is_none() {
            paths.push(path.with_extension("exe"));
        }
        paths
    };
    paths
}

fn interpreter_script<'a>(command: &Path, args: &'a [String]) -> Option<&'a str> {
    let name = command.file_stem()?.to_str()?.to_ascii_lowercase();
    // Deliberately recognize only an unambiguous direct script invocation.
    // Options, modules, stdin and command strings remain unverified above.
    if !matches!(
        name.as_str(),
        "python" | "python3" | "node" | "ruby" | "perl" | "sh" | "bash" | "dash" | "zsh"
    ) {
        return None;
    }
    args.first()
        .filter(|argument| !argument.is_empty() && !argument.starts_with('-'))
        .map(String::as_str)
}

fn credential_check(reference: &SecretReference, policy: &PolicyEngine) -> Value {
    let (available, status, detail) = match reference {
        SecretReference::Environment { env } => {
            let present = std::env::var_os(env).is_some_and(|value| !value.is_empty());
            (
                present,
                if present { "present" } else { "missing" },
                "environment reference checked without displaying its value",
            )
        }
        SecretReference::File { file } => {
            let present = policy
                .resolve_secret_file(file)
                .ok()
                .and_then(|path| agentctl_core::sources::regular_file_metadata(&path).ok())
                .is_some_and(|metadata| {
                    metadata.is_file()
                        && metadata.len() > 0
                        && metadata.len() <= agentctl_core::dsl::MAX_SECRET_OUTPUT_LIMIT_BYTES
                });
            (
                present,
                if present {
                    "present"
                } else {
                    "missing_or_denied"
                },
                "secret file policy, regular file type, readability and size checked without reading content",
            )
        }
        SecretReference::Process { process } => {
            let permitted = policy.authorize_secret_process(&process.command).is_ok();
            (
                false,
                if permitted { "unchecked" } else { "denied" },
                "secret process is not executed by doctor; resolve it during an authorized run",
            )
        }
    };
    json!({"available": available, "status": status, "source": reference.source_description(), "detail": detail})
}
