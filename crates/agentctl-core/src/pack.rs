use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
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
    /// Manifest-bound digests for instruction and variable files, relative to this manifest.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, String>,
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
        if self.files.len() > crate::sources::MAX_SOURCE_FILES {
            return Err(PackError::Invalid(
                "pack files exceed the 256-source-file limit".to_owned(),
            ));
        }
        for (path, integrity) in &self.files {
            validate_relative_path(Path::new(path), "pack file")?;
            if path.contains(['\\', ':'])
                || path.split('/').any(|part| part.is_empty() || part == ".")
            {
                return Err(PackError::Invalid(
                    "pack files must use normalized relative paths".to_owned(),
                ));
            }
            validate_integrity(integrity)?;
            if integrity != &integrity.to_ascii_lowercase() {
                return Err(PackError::Invalid(
                    "pack files must use lowercase sha256 digests".to_owned(),
                ));
            }
        }
        let referenced_files = self
            .agents
            .values()
            .flat_map(|agent| {
                agent
                    .instructions_file
                    .iter()
                    .chain(agent.vars_files.iter())
            })
            .chain(self.workflows.values().flat_map(|workflow| {
                workflow
                    .tasks
                    .iter()
                    .flat_map(|task| task.vars_files.iter())
            }));
        for path in referenced_files {
            if !self.files.contains_key(path) {
                return Err(PackError::Invalid(format!(
                    "pack source file `{path}` requires a sha256 digest in files"
                )));
            }
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

/// Verify the manifest and every declared ancillary asset without dispatching
/// providers or executable pack content. The manifest directory is the read root.
pub fn verify_pack(path: &Path, expected: &str) -> Result<String, PackError> {
    validate_integrity(expected)?;
    let canonical = fs::canonicalize(path)?;
    let bytes = read_pack_file(&canonical)?;
    let actual = digest_bytes(&bytes);
    if actual != expected {
        return Err(PackError::Integrity {
            path: path.to_path_buf(),
            expected: expected.to_owned(),
            actual,
        });
    }
    let manifest = parse_pack(&bytes, &path.display().to_string())?;
    let root = canonical
        .parent()
        .ok_or_else(|| PackError::Invalid("pack manifest has no parent".to_owned()))?;
    let mut total = 0_u64;
    for (relative, expected) in &manifest.files {
        let source = fs::canonicalize(root.join(relative))?;
        if !source.starts_with(root) {
            return Err(PackError::Invalid(format!(
                "pack file `{relative}` resolves outside the manifest directory"
            )));
        }
        let bytes = read_pack_file(&source)?;
        total = total.saturating_add(bytes.len() as u64);
        if total > crate::sources::MAX_CAPTURE_BYTES {
            return Err(PackError::Invalid(
                "pack files exceed the 16 MiB source capture limit".to_owned(),
            ));
        }
        if std::str::from_utf8(&bytes).is_err() {
            return Err(PackError::Invalid(format!(
                "pack file `{relative}` must use UTF-8 encoding"
            )));
        }
        let digest = digest_bytes(&bytes);
        if &digest != expected {
            return Err(PackError::Integrity {
                path: source,
                expected: expected.clone(),
                actual: digest,
            });
        }
    }
    Ok(actual)
}

fn read_pack_file(path: &Path) -> Result<Vec<u8>, PackError> {
    read_pack_input(path, crate::sources::MAX_SOURCE_BYTES)
}

/// Read one bounded regular pack input from a no-follow descriptor. The caller
/// authorizes and checks path containment before invoking this helper. Binary
/// lock/signature/archive inputs remain supported; text assets are checked above.
pub fn read_pack_input(path: &Path, limit: u64) -> Result<Vec<u8>, PackError> {
    if !path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err(PackError::Invalid(
            "pack input requires a normalized absolute path".to_owned(),
        ));
    }
    let mut file = crate::sources::open_regular(path)?;
    let before = file.metadata()?;
    if before.len() > limit {
        return Err(PackError::Invalid(format!(
            "{} exceeds the {limit}-byte limit",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    if bytes.len() as u64 > limit
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
        || before.len() != bytes.len() as u64
    {
        return Err(PackError::Invalid(format!(
            "{} changed during verification or exceeds the {limit}-byte limit",
            path.display()
        )));
    }
    Ok(bytes)
}

#[must_use]
pub fn digest_bytes(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

pub fn parse_pack(bytes: &[u8], display: &str) -> Result<PackManifest, PackError> {
    if bytes.len() as u64 > crate::sources::MAX_SOURCE_BYTES {
        return Err(PackError::Invalid(format!(
            "{display}: pack manifest exceeds the 1 MiB source limit"
        )));
    }
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
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
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
        let trusted = b"apiVersion: agentctl.dev/pack/v1alpha1\nname: example.trusted\nversion: 1.0.0\nagentctl: '>=0.3.0, <1.0.0'\n";
        file.write_all(trusted).expect("write");
        let integrity = verify_pack(file.path(), &digest_bytes(trusted)).expect("matches");
        assert!(integrity.starts_with("sha256:"));
        file.write_all(b"tampered").expect("tamper");
        assert!(matches!(
            verify_pack(file.path(), &integrity),
            Err(PackError::Integrity { .. })
        ));
    }

    #[test]
    fn verifies_declared_assets_and_rejects_tamper_missing_and_invalid_text() {
        let directory = tempfile::tempdir().unwrap();
        let manifest_path = directory.path().join("agentctl.pack.yaml");
        let asset = directory.path().join("review.txt");
        fs::write(&asset, "review this fixture").unwrap();
        let manifest = format!(
            "apiVersion: agentctl.dev/pack/v1alpha1\nname: example.assets\nversion: 1.0.0\nagentctl: '>=0.3.0, <1.0.0'\nfiles:\n  review.txt: {}\n",
            digest_bytes(b"review this fixture")
        );
        fs::write(&manifest_path, &manifest).unwrap();
        let integrity = digest_bytes(manifest.as_bytes());
        assert_eq!(verify_pack(&manifest_path, &integrity).unwrap(), integrity);
        // Even a declared asset not referenced by an agent is verified.
        fs::write(&asset, "tampered").unwrap();
        assert!(
            matches!(verify_pack(&manifest_path, &integrity), Err(PackError::Integrity { path, .. }) if path == fs::canonicalize(&asset).unwrap())
        );
        fs::remove_file(&asset).unwrap();
        assert!(verify_pack(&manifest_path, &integrity).is_err());
        fs::write(&asset, [0xff]).unwrap();
        assert!(
            verify_pack(&manifest_path, &integrity)
                .unwrap_err()
                .to_string()
                .contains("UTF-8")
        );
        fs::write(
            &asset,
            vec![b'a'; crate::sources::MAX_SOURCE_BYTES as usize + 1],
        )
        .unwrap();
        assert!(
            verify_pack(&manifest_path, &integrity)
                .unwrap_err()
                .to_string()
                .contains("1048576-byte")
        );
        fs::remove_file(&asset).unwrap();
        fs::create_dir(&asset).unwrap();
        assert!(verify_pack(&manifest_path, &integrity).is_err());
    }

    #[test]
    fn asset_paths_are_portable_normalized_and_all_source_references_are_bound() {
        let base = "apiVersion: agentctl.dev/pack/v1alpha1\nname: example.assets\nversion: 1.0.0\nagentctl: '>=0.3.0, <1.0.0'\n";
        let digest = digest_bytes(b"fixture");
        for path in [
            "../escape",
            "/absolute",
            "a/../escape",
            "a//b",
            "./file",
            "a/./b",
            "a\\b",
            "C:drive-relative",
            "",
        ] {
            let file_map = serde_json::json!({path:digest});
            let source = format!("{base}files: {file_map}\n");
            assert!(
                parse_pack(source.as_bytes(), "fixture.pack.yaml").is_err(),
                "accepted {path}"
            );
        }
        let source = format!(
            "{base}agents:\n  worker: {{ provider: fake, model: fake, instructionsFile: missing.txt }}\n"
        );
        assert!(
            parse_pack(source.as_bytes(), "fixture.pack.yaml")
                .unwrap_err()
                .to_string()
                .contains("requires a sha256 digest")
        );
        let valid = format!(
            "{base}files: {{ 'review.txt': '{digest}' }}\nagents:\n  worker: {{ provider: fake, model: fake, instructionsFile: review.txt }}\n"
        );
        assert!(parse_pack(valid.as_bytes(), "fixture.pack.yaml").is_ok());
        assert!(
            parse_pack(
                vec![b' '; crate::sources::MAX_SOURCE_BYTES as usize + 1].as_slice(),
                "too-large"
            )
            .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn asset_symlink_escape_and_fifo_fail_without_blocking() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("review.txt"), "private").unwrap();
        let manifest = format!(
            "apiVersion: agentctl.dev/pack/v1alpha1\nname: example.assets\nversion: 1.0.0\nagentctl: '>=0.3.0, <1.0.0'\nfiles: {{ review.txt: '{}' }}\n",
            digest_bytes(b"private")
        );
        let path = directory.path().join("agentctl.pack.yaml");
        fs::write(&path, &manifest).unwrap();
        symlink(
            outside.path().join("review.txt"),
            directory.path().join("review.txt"),
        )
        .unwrap();
        assert!(
            verify_pack(&path, &digest_bytes(manifest.as_bytes()))
                .unwrap_err()
                .to_string()
                .contains("outside")
        );
        fs::remove_file(directory.path().join("review.txt")).unwrap();
        nix::unistd::mkfifo(
            &directory.path().join("review.txt"),
            nix::sys::stat::Mode::S_IRUSR,
        )
        .unwrap();
        assert!(verify_pack(&path, &digest_bytes(manifest.as_bytes())).is_err());
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
