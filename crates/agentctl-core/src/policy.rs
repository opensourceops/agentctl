use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use url::Url;

use crate::dsl::{ApprovalMode, EffectClass, NonInteractiveMode, PolicyDefinition, Risk};

#[derive(Debug, Clone)]
pub struct PolicyEngine {
    policy: PolicyDefinition,
    workspace_root: PathBuf,
    writable_roots: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyContext {
    pub run_id: String,
    pub trace_id: String,
    pub task_id: String,
    pub agent: Option<String>,
    pub tool: String,
    pub capability: String,
    pub effect_class: EffectClass,
    pub risk: Risk,
    pub resource: Option<String>,
    pub provider: Option<String>,
    pub input: Value,
    pub interactive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow { reason: String },
    RequireApproval { reason: String },
    Deny { reason: String },
}

#[derive(Debug, Error)]
pub enum PolicyError {
    #[error("workspace root is invalid: {0}")]
    Workspace(String),
    #[error("resource path escapes the authorized root: {0}")]
    PathEscape(String),
    #[error("network destination is not authorized: {0}")]
    NetworkDenied(String),
    #[error("environment variable is not authorized: {0}")]
    EnvironmentDenied(String),
    #[error("process is not authorized: {0}")]
    ProcessDenied(String),
}

impl PolicyEngine {
    pub fn new(policy: PolicyDefinition, base: &Path) -> Result<Self, PolicyError> {
        let workspace_candidate = if Path::new(&policy.workspace_root).is_absolute() {
            PathBuf::from(&policy.workspace_root)
        } else {
            base.join(&policy.workspace_root)
        };
        let workspace_root = canonicalize_existing_or_parent(&workspace_candidate)?;
        let writable_roots = policy
            .writable_roots
            .iter()
            .map(|root| {
                let candidate = if Path::new(root).is_absolute() {
                    PathBuf::from(root)
                } else {
                    workspace_root.join(root)
                };
                canonicalize_existing_or_parent(&candidate)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            policy,
            workspace_root,
            writable_roots,
        })
    }

    #[must_use]
    pub fn decide(&self, context: &PolicyContext) -> PolicyDecision {
        if self
            .policy
            .tools_deny
            .iter()
            .any(|tool| tool == &context.tool)
        {
            return PolicyDecision::Deny {
                reason: "tool is explicitly denied".to_owned(),
            };
        }
        if !self.policy.tools_allow.is_empty()
            && !self
                .policy
                .tools_allow
                .iter()
                .any(|tool| tool == &context.tool)
        {
            return PolicyDecision::Deny {
                reason: "tool is not in the allowlist".to_owned(),
            };
        }
        if let Some(provider) = &context.provider
            && !self.policy.providers.is_empty()
            && !self.policy.providers.contains(provider)
        {
            return PolicyDecision::Deny {
                reason: "provider is not in the allowlist".to_owned(),
            };
        }
        let approval = match self.policy.approval {
            ApprovalMode::Never => false,
            ApprovalMode::Always => true,
            ApprovalMode::HighRisk => matches!(context.risk, Risk::High | Risk::Critical),
            ApprovalMode::Mutations => matches!(
                context.effect_class,
                EffectClass::WorkspaceMutate
                    | EffectClass::ExternalMutate
                    | EffectClass::ProcessExecution
                    | EffectClass::RemoteAgent
            ),
        };
        if approval && !context.interactive {
            match self.policy.non_interactive {
                NonInteractiveMode::Pause => PolicyDecision::RequireApproval {
                    reason: "approval is required; the non-interactive run will pause durably"
                        .to_owned(),
                },
                NonInteractiveMode::DenyApproval => PolicyDecision::Deny {
                    reason: "approval is required and non-interactive policy denies approval"
                        .to_owned(),
                },
                NonInteractiveMode::Fail => PolicyDecision::Deny {
                    reason: "approval is required and non-interactive policy is fail".to_owned(),
                },
            }
        } else if approval {
            PolicyDecision::RequireApproval {
                reason: format!(
                    "policy requires approval for {:?} / {:?}",
                    context.effect_class, context.risk
                ),
            }
        } else {
            PolicyDecision::Allow {
                reason: "request satisfies policy".to_owned(),
            }
        }
    }

    pub fn resolve_read_path(&self, requested: &str) -> Result<PathBuf, PolicyError> {
        let candidate = self.join_workspace(requested)?;
        let canonical = fs::canonicalize(&candidate)
            .map_err(|error| PolicyError::PathEscape(format!("{requested}: {error}")))?;
        if canonical.starts_with(&self.workspace_root) {
            Ok(canonical)
        } else {
            Err(PolicyError::PathEscape(requested.to_owned()))
        }
    }

    pub fn resolve_write_path(&self, requested: &str) -> Result<PathBuf, PolicyError> {
        let candidate = self.join_workspace(requested)?;
        let canonical = canonicalize_existing_or_parent(&candidate)?;
        if self
            .writable_roots
            .iter()
            .any(|root| canonical.starts_with(root))
        {
            Ok(candidate)
        } else {
            Err(PolicyError::PathEscape(requested.to_owned()))
        }
    }

    pub fn authorize_network(&self, target: &Url) -> Result<(), PolicyError> {
        if target.scheme() != "https" && target.scheme() != "http" {
            return Err(PolicyError::NetworkDenied(target.to_string()));
        }
        let host = target
            .host_str()
            .ok_or_else(|| PolicyError::NetworkDenied(target.to_string()))?;
        if self
            .policy
            .network_allowlist
            .iter()
            .any(|rule| host_matches(host, rule))
        {
            Ok(())
        } else {
            Err(PolicyError::NetworkDenied(host.to_owned()))
        }
    }

    pub fn authorize_redirect(&self, from: &Url, to: &Url) -> Result<(), PolicyError> {
        self.authorize_network(from)?;
        self.authorize_network(to)
    }

    pub fn authorize_environment(&self, name: &str) -> Result<(), PolicyError> {
        if self
            .policy
            .environment_allowlist
            .iter()
            .any(|allowed| allowed == name)
        {
            Ok(())
        } else {
            Err(PolicyError::EnvironmentDenied(name.to_owned()))
        }
    }

    pub fn authorize_process(&self, command: &str) -> Result<(), PolicyError> {
        let basename = Path::new(command)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(command);
        if self
            .policy
            .process_allowlist
            .iter()
            .any(|allowed| allowed == basename)
        {
            Ok(())
        } else {
            Err(PolicyError::ProcessDenied(command.to_owned()))
        }
    }

    #[must_use]
    pub fn filtered_environment(
        &self,
        source: &BTreeMap<String, String>,
    ) -> BTreeMap<String, String> {
        source
            .iter()
            .filter(|(name, _)| self.policy.environment_allowlist.contains(name))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
    }

    fn join_workspace(&self, requested: &str) -> Result<PathBuf, PolicyError> {
        let path = Path::new(requested);
        if path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(PolicyError::PathEscape(requested.to_owned()));
        }
        Ok(if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workspace_root.join(path)
        })
    }
}

fn canonicalize_existing_or_parent(path: &Path) -> Result<PathBuf, PolicyError> {
    if path.exists() {
        return fs::canonicalize(path)
            .map_err(|error| PolicyError::Workspace(format!("{}: {error}", path.display())));
    }
    let parent = path
        .parent()
        .ok_or_else(|| PolicyError::Workspace(path.display().to_string()))?;
    let canonical_parent = fs::canonicalize(parent)
        .map_err(|error| PolicyError::Workspace(format!("{}: {error}", parent.display())))?;
    let name = path
        .file_name()
        .ok_or_else(|| PolicyError::Workspace(path.display().to_string()))?;
    Ok(canonical_parent.join(name))
}

fn host_matches(host: &str, rule: &str) -> bool {
    rule.strip_prefix("*.").map_or(host == rule, |suffix| {
        host != suffix && host.ends_with(&format!(".{suffix}"))
    })
}

/// Replace sensitive values before audit, persistence, or tracing.
#[must_use]
pub fn redact(value: &Value, secret_values: &[String]) -> Value {
    match value {
        Value::String(text) => {
            let mut redacted = text.clone();
            for secret in secret_values.iter().filter(|secret| !secret.is_empty()) {
                redacted = redacted.replace(secret, "[REDACTED]");
            }
            Value::String(redacted)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| redact(item, secret_values))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| {
                    let sensitive_key = matches!(
                        key.to_ascii_lowercase().as_str(),
                        "authorization" | "api_key" | "apikey" | "token" | "secret" | "password"
                    );
                    (
                        key.clone(),
                        if sensitive_key {
                            Value::String("[REDACTED]".to_owned())
                        } else {
                            redact(item, secret_values)
                        },
                    )
                })
                .collect(),
        ),
        primitive => primitive.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn policy(root: &Path) -> PolicyEngine {
        let policy = PolicyDefinition {
            workspace_root: root.display().to_string(),
            writable_roots: vec!["safe".to_owned()],
            network_allowlist: vec!["api.example.com".to_owned(), "*.tools.example".to_owned()],
            environment_allowlist: vec!["SAFE_VALUE".to_owned()],
            process_allowlist: vec!["git".to_owned()],
            ..PolicyDefinition::default()
        };
        PolicyEngine::new(policy, root).expect("valid policy")
    }

    #[test]
    fn rejects_parent_traversal() {
        let root = tempdir().expect("temp dir");
        fs::create_dir(root.path().join("safe")).expect("safe dir");
        assert!(matches!(
            policy(root.path()).resolve_write_path("safe/../../escape"),
            Err(PolicyError::PathEscape(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape() {
        use std::os::unix::fs::symlink;
        let root = tempdir().expect("temp dir");
        let outside = tempdir().expect("outside");
        fs::create_dir(root.path().join("safe")).expect("safe dir");
        symlink(outside.path(), root.path().join("safe/link")).expect("symlink");
        assert!(
            policy(root.path())
                .resolve_write_path("safe/link/secret")
                .is_err()
        );
    }

    #[test]
    fn network_wildcard_does_not_match_apex_or_suffix_attack() {
        let root = tempdir().expect("temp dir");
        fs::create_dir(root.path().join("safe")).expect("safe dir");
        let engine = policy(root.path());
        assert!(
            engine
                .authorize_network(&Url::parse("https://x.tools.example/a").expect("url"))
                .is_ok()
        );
        assert!(
            engine
                .authorize_network(&Url::parse("https://tools.example/a").expect("url"))
                .is_err()
        );
        assert!(
            engine
                .authorize_network(&Url::parse("https://api.example.com.evil/a").expect("url"))
                .is_err()
        );
    }

    #[test]
    fn redacts_keys_and_embedded_secret_values() {
        let value = serde_json::json!({
            "authorization": "Bearer secret-value",
            "message": "found secret-value in output"
        });
        let redacted = redact(&value, &["secret-value".to_owned()]);
        let text = serde_json::to_string(&redacted).expect("json");
        assert!(!text.contains("secret-value"));
        assert!(text.contains("[REDACTED]"));
    }
}
