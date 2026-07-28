use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::dsl::{
    ActionDefinition, AgentDefinition, PolicyDefinition, SubworkflowDefinition, ToolDefinition,
};

pub const PACK_API_VERSION: &str = "agentctl.dev/pack/v1alpha1";
pub const PACK_LOCK_API_VERSION: &str = "agentctl.dev/pack-lock/v1";
pub const DEFAULT_PACK_MANIFEST: &str = "agentctl.pack.yaml";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackManifest {
    pub api_version: String,
    pub name: String,
    pub version: String,
    pub agentctl: String,
    #[serde(default)]
    pub dependencies: BTreeMap<String, PackDependency>,
    #[serde(default)]
    pub actions: BTreeMap<String, ActionDefinition>,
    #[serde(default)]
    pub agents: BTreeMap<String, AgentDefinition>,
    #[serde(default)]
    pub tools: BTreeMap<String, ToolDefinition>,
    #[serde(default)]
    pub workflows: BTreeMap<String, SubworkflowDefinition>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub providers: Vec<String>,
    #[serde(default)]
    pub policy_defaults: Option<PolicyDefinition>,
}

impl PackManifest {
    pub fn validate(&self) -> Result<(), PackError> {
        if self.api_version != PACK_API_VERSION {
            return Err(PackError::Invalid(format!(
                "unsupported apiVersion `{}`; expected `{PACK_API_VERSION}`",
                self.api_version
            )));
        }
        if !self.name.contains('.') || self.name.split('.').any(str::is_empty) {
            return Err(PackError::Invalid(
                "name must be a fully qualified dotted name".to_owned(),
            ));
        }
        Version::parse(&self.version)
            .map_err(|error| PackError::Invalid(format!("version is not semver: {error}")))?;
        let requirement = VersionReq::parse(&self.agentctl).map_err(|error| {
            PackError::Invalid(format!("agentctl constraint is not valid semver: {error}"))
        })?;
        let current = Version::parse(env!("CARGO_PKG_VERSION")).map_err(|error| {
            PackError::Invalid(format!("agentctl build version is invalid: {error}"))
        })?;
        if !requirement.matches(&current) {
            return Err(PackError::Invalid(format!(
                "agentctl {current} does not satisfy `{requirement}`"
            )));
        }
        for (name, dependency) in &self.dependencies {
            validate_pack_name(name)?;
            VersionReq::parse(&dependency.version).map_err(|error| {
                PackError::Invalid(format!(
                    "dependency `{name}` constraint is not valid semver: {error}"
                ))
            })?;
            dependency.source.validate()?;
            if let Some(signature) = &dependency.signature {
                signature.validate()?;
            }
        }
        for (name, action) in &self.actions {
            action.validate_process_bounds().map_err(|message| {
                PackError::Invalid(format!(
                    "action `{name}` has invalid process bounds: {message}"
                ))
            })?;
        }
        for (name, workflow) in &self.workflows {
            Version::parse(&workflow.version).map_err(|error| {
                PackError::Invalid(format!("workflow `{name}` version is not semver: {error}"))
            })?;
            for (label, schema) in [
                ("inputSchema", &workflow.input_schema),
                ("outputSchema", &workflow.output_schema),
            ] {
                jsonschema::validator_for(schema).map_err(|error| {
                    PackError::Invalid(format!(
                        "workflow `{name}` {label} is not valid JSON Schema: {error}"
                    ))
                })?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackDependency {
    pub version: String,
    pub source: PackSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<PackSignature>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
pub enum PackSource {
    Path {
        path: PathBuf,
    },
    Git {
        git: String,
        rev: String,
        #[serde(default = "default_pack_manifest")]
        manifest: PathBuf,
    },
    Archive {
        url: String,
        integrity: String,
        #[serde(default = "default_pack_manifest")]
        manifest: PathBuf,
    },
}

impl PackSource {
    pub fn validate(&self) -> Result<(), PackError> {
        match self {
            Self::Path { path } => validate_relative_path(path, "pack path"),
            Self::Git { git, rev, manifest } => {
                let parsed = url::Url::parse(git).map_err(|error| {
                    PackError::Invalid(format!("Git pack source URL is invalid: {error}"))
                })?;
                if !matches!(parsed.scheme(), "https" | "file")
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                    || parsed.query().is_some()
                    || parsed.fragment().is_some()
                {
                    return Err(PackError::Invalid(
                        "Git pack source must use a credential-free, query-free https or file URL"
                            .to_owned(),
                    ));
                }
                if rev.len() != 40 || !rev.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(PackError::Invalid(
                        "Git pack source rev must be a full 40-character commit".to_owned(),
                    ));
                }
                validate_relative_path(manifest, "Git pack manifest")
            }
            Self::Archive {
                url,
                integrity,
                manifest,
            } => {
                let parsed = url::Url::parse(url).map_err(|error| {
                    PackError::Invalid(format!("archive URL is invalid: {error}"))
                })?;
                if !parsed.username().is_empty()
                    || parsed.password().is_some()
                    || parsed.query().is_some()
                    || parsed.fragment().is_some()
                {
                    return Err(PackError::Invalid(
                        "archive URL must not contain credentials, query parameters, or fragments"
                            .to_owned(),
                    ));
                }
                let local_http = parsed.scheme() == "http"
                    && parsed.host_str().is_some_and(|host| {
                        host == "localhost"
                            || host
                                .parse::<std::net::IpAddr>()
                                .is_ok_and(|ip| ip.is_loopback())
                    });
                if parsed.scheme() != "https" && !local_http {
                    return Err(PackError::Invalid(
                        "archive URL scheme must be https; loopback http is accepted only for local fixtures".to_owned(),
                    ));
                }
                validate_integrity(integrity)?;
                validate_relative_path(manifest, "archive pack manifest")
            }
        }
    }
}

fn default_pack_manifest() -> PathBuf {
    PathBuf::from(DEFAULT_PACK_MANIFEST)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackSignature {
    pub bundle: PathBuf,
    pub identity: String,
    pub issuer: String,
}

impl PackSignature {
    pub fn validate(&self) -> Result<(), PackError> {
        validate_relative_path(&self.bundle, "Sigstore bundle")?;
        if self.identity.trim().is_empty() || self.issuer.trim().is_empty() {
            return Err(PackError::Invalid(
                "Sigstore identity and issuer must not be empty".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackLock {
    pub api_version: String,
    pub agentctl: String,
    pub packs: Vec<PackLockEntry>,
}

impl PackLock {
    pub fn validate(&self) -> Result<(), PackError> {
        if self.api_version != PACK_LOCK_API_VERSION {
            return Err(PackError::Invalid(format!(
                "unsupported lock apiVersion `{}`; expected `{PACK_LOCK_API_VERSION}`",
                self.api_version
            )));
        }
        let locked_agentctl = Version::parse(&self.agentctl).map_err(|error| {
            PackError::Invalid(format!("locked agentctl version is not semver: {error}"))
        })?;
        let current = Version::parse(env!("CARGO_PKG_VERSION")).map_err(|error| {
            PackError::Invalid(format!("agentctl build version is invalid: {error}"))
        })?;
        if locked_agentctl != current {
            return Err(PackError::Invalid(format!(
                "lockfile was generated for agentctl {locked_agentctl}, current version is {current}"
            )));
        }
        let mut names = BTreeSet::new();
        for entry in &self.packs {
            entry.validate()?;
            if !names.insert(entry.name.clone()) {
                return Err(PackError::Invalid(format!(
                    "lockfile contains duplicate pack `{}`",
                    entry.name
                )));
            }
        }
        for entry in &self.packs {
            for (dependency, version) in &entry.dependencies {
                let Some(target) = self.packs.iter().find(|pack| pack.name == *dependency) else {
                    return Err(PackError::Invalid(format!(
                        "locked pack `{}` references missing dependency `{dependency}`",
                        entry.name
                    )));
                };
                if target.version != *version {
                    return Err(PackError::Invalid(format!(
                        "locked dependency `{dependency}` from `{}` expects version `{version}`, found `{}`",
                        entry.name, target.version
                    )));
                }
            }
        }
        detect_lock_cycle(self)?;
        Ok(())
    }

    #[must_use]
    pub fn canonicalized(mut self) -> Self {
        self.packs.sort_by(|left, right| left.name.cmp(&right.name));
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackLockEntry {
    pub name: String,
    pub version: String,
    pub source: PackSource,
    pub integrity: String,
    pub compatibility: String,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<PackSignature>,
    pub trust: PackTrustRecord,
}

impl PackLockEntry {
    fn validate(&self) -> Result<(), PackError> {
        validate_pack_name(&self.name)?;
        Version::parse(&self.version).map_err(|error| {
            PackError::Invalid(format!(
                "locked pack `{}` version is not semver: {error}",
                self.name
            ))
        })?;
        VersionReq::parse(&self.compatibility).map_err(|error| {
            PackError::Invalid(format!(
                "locked pack `{}` compatibility is not semver: {error}",
                self.name
            ))
        })?;
        self.source.validate()?;
        validate_integrity(&self.integrity)?;
        for (name, version) in &self.dependencies {
            validate_pack_name(name)?;
            Version::parse(version).map_err(|error| {
                PackError::Invalid(format!(
                    "locked dependency `{name}` version is not semver: {error}"
                ))
            })?;
        }
        if let Some(signature) = &self.signature {
            signature.validate()?;
        }
        self.trust.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum PackTrustRecord {
    Unsigned,
    Sigstore {
        identity: String,
        issuer: String,
        bundle_integrity: String,
    },
}

impl PackTrustRecord {
    fn validate(&self) -> Result<(), PackError> {
        match self {
            Self::Unsigned => Ok(()),
            Self::Sigstore {
                identity,
                issuer,
                bundle_integrity,
            } => {
                if identity.trim().is_empty() || issuer.trim().is_empty() {
                    return Err(PackError::Invalid(
                        "locked Sigstore identity and issuer must not be empty".to_owned(),
                    ));
                }
                validate_integrity(bundle_integrity)
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum PackError {
    #[error("pack manifest is invalid: {0}")]
    Invalid(String),
    #[error("pack integrity mismatch for {path}: expected {expected}, got {actual}")]
    Integrity {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    #[error("pack input/output error: {0}")]
    Io(#[from] std::io::Error),
}

pub fn verify_pack(path: &Path, expected: &str) -> Result<String, PackError> {
    let bytes = fs::read(path)?;
    let actual = format!("sha256:{}", hex::encode(Sha256::digest(bytes)));
    if actual == expected {
        Ok(actual)
    } else {
        Err(PackError::Integrity {
            path: path.to_path_buf(),
            expected: expected.to_owned(),
            actual,
        })
    }
}

#[must_use]
pub fn digest_bytes(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

pub fn parse_pack(bytes: &[u8], display: &str) -> Result<PackManifest, PackError> {
    let manifest: PackManifest = serde_yaml_ng::from_slice(bytes)
        .map_err(|error| PackError::Invalid(format!("{display}: {error}")))?;
    manifest.validate()?;
    Ok(manifest)
}

fn validate_pack_name(name: &str) -> Result<(), PackError> {
    if !name.contains('.') || name.split('.').any(str::is_empty) {
        Err(PackError::Invalid(format!(
            "pack name `{name}` must be a fully qualified dotted name"
        )))
    } else {
        Ok(())
    }
}

fn validate_integrity(integrity: &str) -> Result<(), PackError> {
    let Some(value) = integrity.strip_prefix("sha256:") else {
        return Err(PackError::Invalid(
            "integrity must use a sha256:<hex> digest".to_owned(),
        ));
    };
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(PackError::Invalid(
            "integrity must contain exactly 64 hexadecimal SHA-256 characters".to_owned(),
        ));
    }
    Ok(())
}

fn validate_relative_path(path: &Path, label: &str) -> Result<(), PackError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::RootDir
            )
        })
    {
        Err(PackError::Invalid(format!(
            "{label} must be a contained relative path"
        )))
    } else {
        Ok(())
    }
}

fn detect_lock_cycle(lock: &PackLock) -> Result<(), PackError> {
    fn visit(
        name: &str,
        lock: &PackLock,
        visiting: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
    ) -> Result<(), PackError> {
        if visited.contains(name) {
            return Ok(());
        }
        if !visiting.insert(name.to_owned()) {
            return Err(PackError::Invalid(format!(
                "pack dependency cycle includes `{name}`"
            )));
        }
        let entry = lock
            .packs
            .iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| PackError::Invalid(format!("missing locked pack `{name}`")))?;
        for dependency in entry.dependencies.keys() {
            visit(dependency, lock, visiting, visited)?;
        }
        visiting.remove(name);
        visited.insert(name.to_owned());
        Ok(())
    }

    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for entry in &lock.packs {
        visit(&entry.name, lock, &mut visiting, &mut visited)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn detects_pack_tampering() {
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        file.write_all(b"trusted").expect("write");
        let integrity = verify_pack(
            file.path(),
            "sha256:a9a089195c68d2adeee23beaa2c3a93b1d4cdf09046e7a9e520b3b166dff3e6a",
        )
        .expect("matches");
        assert!(integrity.starts_with("sha256:"));
        file.write_all(b"tampered").expect("tamper");
        assert!(matches!(
            verify_pack(file.path(), &integrity),
            Err(PackError::Integrity { .. })
        ));
    }

    #[test]
    fn validates_manifest_identity_and_compatibility() {
        let manifest: PackManifest = serde_yaml_ng::from_str(
            "apiVersion: agentctl.dev/pack/v1alpha1\nname: example.utility\nversion: 1.0.0\nagentctl: '>=0.3.0, <1.0.0'\n",
        )
        .expect("manifest");
        manifest.validate().expect("valid manifest");

        let mut invalid = manifest;
        invalid.name = "local".to_owned();
        assert!(matches!(invalid.validate(), Err(PackError::Invalid(_))));
    }

    #[test]
    fn rejects_unreasonable_pack_process_output_limit() {
        let manifest: PackManifest = serde_yaml_ng::from_str(
            "apiVersion: agentctl.dev/pack/v1alpha1\nname: example.utility\nversion: 1.0.0\nagentctl: '>=0.3.0, <1.0.0'\nactions:\n  noisy:\n    kind: builtin.shell.exec\n    command: sh\n    stdoutLimitBytes: 16777217\n",
        )
        .expect("manifest");
        assert!(matches!(manifest.validate(), Err(PackError::Invalid(_))));
    }
}
