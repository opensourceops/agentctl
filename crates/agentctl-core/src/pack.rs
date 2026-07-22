use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::dsl::{ActionDefinition, AgentDefinition, PolicyDefinition, ToolDefinition};

pub const PACK_API_VERSION: &str = "agentctl.dev/pack/v1alpha1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackManifest {
    pub api_version: String,
    pub name: String,
    pub version: String,
    pub agentctl: String,
    #[serde(default)]
    pub actions: BTreeMap<String, ActionDefinition>,
    #[serde(default)]
    pub agents: BTreeMap<String, AgentDefinition>,
    #[serde(default)]
    pub tools: BTreeMap<String, ToolDefinition>,
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
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackLockEntry {
    pub name: String,
    pub version: String,
    pub source: PathBuf,
    pub integrity: String,
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
            "apiVersion: agentctl.dev/pack/v1alpha1\nname: example.utility\nversion: 1.0.0\nagentctl: '>=0.2.0, <1.0.0'\n",
        )
        .expect("manifest");
        manifest.validate().expect("valid manifest");

        let mut invalid = manifest;
        invalid.name = "local".to_owned();
        assert!(matches!(invalid.validate(), Err(PackError::Invalid(_))));
    }
}
