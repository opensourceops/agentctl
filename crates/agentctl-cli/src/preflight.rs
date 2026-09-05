//! Non-dispatching prerequisite checks. Secret values are never loaded here.

use agentctl_core::dsl::{ProcessIsolation, ProviderKind, SecretReference};
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
        if requirement.isolation != ProcessIsolation::Container {
            continue;
        }
        let action = &workflow.spec.actions[&requirement.action];
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
    print_value(
        output,
        "DoctorResult",
        &json!({
            "ready": ready,
            "workflow": workflow.metadata.name,
            "planDigest": plan.plan_digest,
            "checks": checks,
            "scope": "local prerequisites; provider access and secret process results require execution",
        }),
        diagnostics,
        format!(
            "{}: {}",
            if ready { "ready" } else { "not ready" },
            workflow.metadata.name
        ),
    )?;
    Ok(if ready { EXIT_OK } else { EXIT_REMOTE })
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
