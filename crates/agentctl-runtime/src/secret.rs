use std::time::Duration;

use agentctl_core::dsl::{MAX_SECRET_OUTPUT_LIMIT_BYTES, SecretReference};
use agentctl_core::policy::{PolicyEngine, PolicyError};
use agentctl_core::secret::{SecretSourceResolver, SecretValue};
use async_trait::async_trait;
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::process::{ProcessOutputLimits, ProcessRunError, run_bounded_process};

const MAX_SECRET_FILE_BYTES: u64 = MAX_SECRET_OUTPUT_LIMIT_BYTES;
const SECRET_PROCESS_STDERR_LIMIT_BYTES: u64 = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvironmentSecretPolicy {
    RequireAllowlist,
    ProviderCredentialCompatibility,
}

#[derive(Debug, Clone)]
pub struct SecretResolver {
    policy: PolicyEngine,
    environment_policy: EnvironmentSecretPolicy,
}

impl SecretResolver {
    #[must_use]
    pub fn restricted(policy: PolicyEngine) -> Self {
        Self {
            policy,
            environment_policy: EnvironmentSecretPolicy::RequireAllowlist,
        }
    }

    #[must_use]
    pub fn provider_credentials(policy: PolicyEngine) -> Self {
        Self {
            policy,
            environment_policy: EnvironmentSecretPolicy::ProviderCredentialCompatibility,
        }
    }

    pub async fn resolve(
        &self,
        reference: &SecretReference,
        cancellation: &CancellationToken,
    ) -> Result<SecretValue, SecretResolutionError> {
        let value = match reference {
            SecretReference::Environment { env } => {
                if self.environment_policy == EnvironmentSecretPolicy::RequireAllowlist {
                    self.policy.authorize_environment(env)?;
                }
                std::env::var(env).map_err(|_| {
                    SecretResolutionError::Unavailable(reference.source_description())
                })?
            }
            SecretReference::File { file } => {
                let path = self.policy.resolve_secret_file(file)?;
                let input = tokio::fs::File::open(&path).await.map_err(|_| {
                    SecretResolutionError::Unavailable(reference.source_description())
                })?;
                let mut bytes = Vec::new();
                input
                    .take(MAX_SECRET_FILE_BYTES.saturating_add(1))
                    .read_to_end(&mut bytes)
                    .await
                    .map_err(|_| {
                        SecretResolutionError::Unavailable(reference.source_description())
                    })?;
                if bytes.len() as u64 > MAX_SECRET_FILE_BYTES {
                    bytes.fill(0);
                    return Err(SecretResolutionError::OutputLimit {
                        reference: reference.source_description(),
                        limit_bytes: MAX_SECRET_FILE_BYTES,
                    });
                }
                secret_utf8(bytes, reference)?
            }
            SecretReference::Process { process } => {
                if cancellation.is_cancelled() {
                    return Err(SecretResolutionError::Process(ProcessRunError::Cancelled));
                }
                self.policy.authorize_secret_process(&process.command)?;
                let mut command = Command::new(&process.command);
                command
                    .args(&process.args)
                    .current_dir(self.policy.workspace_root())
                    .env_clear()
                    .kill_on_drop(true);
                let mut output = match run_bounded_process(
                    command,
                    ProcessOutputLimits {
                        stdout_bytes: process.output_limit_bytes,
                        stderr_bytes: SECRET_PROCESS_STDERR_LIMIT_BYTES,
                        combined_bytes: process
                            .output_limit_bytes
                            .saturating_add(SECRET_PROCESS_STDERR_LIMIT_BYTES),
                    },
                    Duration::from_secs(process.timeout_seconds),
                    cancellation,
                )
                .await
                {
                    Ok(output) => output,
                    Err(mut error) => {
                        error.clear_captured_output();
                        return Err(error.into());
                    }
                };
                output.stderr.fill(0);
                if !output.status.success() {
                    output.stdout.fill(0);
                    return Err(SecretResolutionError::ProcessExit {
                        reference: reference.source_description(),
                        code: output.status.code(),
                    });
                }
                secret_utf8(output.stdout, reference)?
            }
        };
        let value = strip_one_line_ending(value);
        if value.is_empty() {
            return Err(SecretResolutionError::Empty(reference.source_description()));
        }
        Ok(SecretValue::new(value))
    }
}

#[async_trait]
impl SecretSourceResolver for SecretResolver {
    async fn resolve_secret(
        &self,
        reference: &SecretReference,
        cancellation: &CancellationToken,
    ) -> Result<SecretValue, String> {
        self.resolve(reference, cancellation)
            .await
            .map_err(|error| error.to_string())
    }
}

fn secret_utf8(
    bytes: Vec<u8>,
    reference: &SecretReference,
) -> Result<String, SecretResolutionError> {
    String::from_utf8(bytes).map_err(|error| {
        let mut bytes = error.into_bytes();
        bytes.fill(0);
        SecretResolutionError::InvalidUtf8(reference.source_description())
    })
}

fn strip_one_line_ending(mut value: String) -> String {
    if value.ends_with("\r\n") {
        value.truncate(value.len() - 2);
    } else if value.ends_with('\n') {
        value.truncate(value.len() - 1);
    }
    value
}

#[derive(Debug, Error)]
pub enum SecretResolutionError {
    #[error("{0}")]
    Policy(#[from] PolicyError),
    #[error("{0} is unavailable")]
    Unavailable(String),
    #[error("{0} resolved to an empty value")]
    Empty(String),
    #[error("{0} did not contain UTF-8")]
    InvalidUtf8(String),
    #[error("{reference} exceeded the {limit_bytes}-byte output limit")]
    OutputLimit { reference: String, limit_bytes: u64 },
    #[error("{reference} exited unsuccessfully with code {code:?}")]
    ProcessExit {
        reference: String,
        code: Option<i32>,
    },
    #[error("secret process failed: {0}")]
    Process(#[from] ProcessRunError),
}

#[cfg(test)]
mod tests {
    use std::fs;

    use agentctl_core::dsl::{PolicyDefinition, SecretProcessReference};
    use tempfile::tempdir;

    use super::*;

    fn policy(root: &std::path::Path) -> PolicyEngine {
        PolicyEngine::new(
            PolicyDefinition {
                workspace_root: root.display().to_string(),
                secret_file_roots: vec!["secrets".to_owned()],
                secret_process_allowlist: vec!["sh".to_owned()],
                ..PolicyDefinition::default()
            },
            root,
        )
        .expect("secret policy")
    }

    #[tokio::test]
    async fn mounted_file_is_bounded_and_strips_one_line_ending() {
        let directory = tempdir().expect("tempdir");
        fs::create_dir(directory.path().join("secrets")).expect("secret root");
        fs::write(directory.path().join("secrets/token"), b"file-secret\r\n").expect("secret file");
        let resolver = SecretResolver::restricted(policy(directory.path()));
        let value = resolver
            .resolve(
                &SecretReference::File {
                    file: "secrets/token".to_owned(),
                },
                &CancellationToken::new(),
            )
            .await
            .expect("resolved file");
        assert_eq!(value.expose(), "file-secret");

        assert!(matches!(
            resolver
                .resolve(
                    &SecretReference::File {
                        file: "secrets/missing".to_owned()
                    },
                    &CancellationToken::new()
                )
                .await,
            Err(SecretResolutionError::Policy(
                PolicyError::SecretFileDenied(_)
            ))
        ));

        fs::write(
            directory.path().join("secrets/oversized"),
            vec![b'x'; usize::try_from(MAX_SECRET_FILE_BYTES + 1).expect("size")],
        )
        .expect("oversized file");
        assert!(matches!(
            resolver
                .resolve(
                    &SecretReference::File {
                        file: "secrets/oversized".to_owned()
                    },
                    &CancellationToken::new()
                )
                .await,
            Err(SecretResolutionError::OutputLimit { .. })
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn mounted_file_rejects_a_symlink_escape() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().expect("tempdir");
        let outside = tempdir().expect("outside");
        fs::create_dir(directory.path().join("secrets")).expect("secret root");
        fs::write(outside.path().join("token"), b"outside-secret").expect("outside file");
        symlink(
            outside.path().join("token"),
            directory.path().join("secrets/token"),
        )
        .expect("symlink");
        let resolver = SecretResolver::restricted(policy(directory.path()));
        assert!(matches!(
            resolver
                .resolve(
                    &SecretReference::File {
                        file: "secrets/token".to_owned()
                    },
                    &CancellationToken::new()
                )
                .await,
            Err(SecretResolutionError::Policy(
                PolicyError::SecretFileDenied(_)
            ))
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn process_provider_is_allowlisted_bounded_and_timed_out() {
        let directory = tempdir().expect("tempdir");
        fs::create_dir(directory.path().join("secrets")).expect("secret root");
        let resolver = SecretResolver::restricted(policy(directory.path()));
        let value = resolver
            .resolve(
                &SecretReference::Process {
                    process: SecretProcessReference {
                        command: "/bin/sh".to_owned(),
                        args: vec!["-c".to_owned(), "printf process-secret".to_owned()],
                        timeout_seconds: 5,
                        output_limit_bytes: 64,
                    },
                },
                &CancellationToken::new(),
            )
            .await
            .expect("resolved process");
        assert_eq!(value.expose(), "process-secret");

        let timeout = resolver
            .resolve(
                &SecretReference::Process {
                    process: SecretProcessReference {
                        command: "/bin/sh".to_owned(),
                        args: vec!["-c".to_owned(), "sleep 2".to_owned()],
                        timeout_seconds: 1,
                        output_limit_bytes: 64,
                    },
                },
                &CancellationToken::new(),
            )
            .await
            .expect_err("process timeout");
        assert!(matches!(
            timeout,
            SecretResolutionError::Process(ProcessRunError::Timeout { seconds: 1 })
        ));

        let output_limit = resolver
            .resolve(
                &SecretReference::Process {
                    process: SecretProcessReference {
                        command: "/bin/sh".to_owned(),
                        args: vec!["-c".to_owned(), "printf 123456789".to_owned()],
                        timeout_seconds: 5,
                        output_limit_bytes: 4,
                    },
                },
                &CancellationToken::new(),
            )
            .await
            .expect_err("process output limit");
        assert!(matches!(
            output_limit,
            SecretResolutionError::Process(ProcessRunError::OutputLimitExceeded { .. })
        ));

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let cancelled = resolver
            .resolve(
                &SecretReference::Process {
                    process: SecretProcessReference {
                        command: "/bin/sh".to_owned(),
                        args: vec!["-c".to_owned(), "sleep 2".to_owned()],
                        timeout_seconds: 5,
                        output_limit_bytes: 64,
                    },
                },
                &cancellation,
            )
            .await
            .expect_err("process cancellation");
        assert!(matches!(
            cancelled,
            SecretResolutionError::Process(ProcessRunError::Cancelled)
        ));

        let denied = SecretResolver::restricted(
            PolicyEngine::new(PolicyDefinition::default(), directory.path())
                .expect("denied policy"),
        )
        .resolve(
            &SecretReference::Process {
                process: SecretProcessReference {
                    command: "/bin/sh".to_owned(),
                    args: Vec::new(),
                    timeout_seconds: 1,
                    output_limit_bytes: 64,
                },
            },
            &CancellationToken::new(),
        )
        .await;
        assert!(matches!(
            denied,
            Err(SecretResolutionError::Policy(
                PolicyError::SecretProcessDenied(_)
            ))
        ));
    }
}
