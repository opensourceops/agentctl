use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use agentctl_core::dsl::{
    ActionKind, PackReference, PackTrustDefinition, UnsignedPackPolicy, Workflow,
};
use agentctl_core::pack::{
    PACK_LOCK_API_VERSION, PackDependency, PackLock, PackLockEntry, PackManifest, PackSignature,
    PackSource, PackTrustRecord, digest_bytes, parse_pack,
};
use flate2::read::GzDecoder;
use reqwest::blocking::Client;
use reqwest::redirect::Policy;
use semver::{Version, VersionReq};
use sigstore_trust_root::{SIGSTORE_PRODUCTION_TRUSTED_ROOT, TrustedRoot};
use sigstore_types::Bundle;
use sigstore_verify::{VerificationPolicy, verify};
use tar::Archive;

const LOCK_FILE_NAME: &str = "agentctl.pack.lock";
const CACHE_DIRECTORY: &str = ".agentctl/pack-cache";
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_BUNDLE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ARCHIVE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_ARCHIVE_EXPANDED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 1024;

#[derive(Debug, Clone, Copy, Default)]
pub struct PackOptions {
    pub offline: bool,
    pub locked: bool,
}

pub struct LoadedPacks {
    pub packs: Vec<(PackLockEntry, PackManifest)>,
    pub warnings: Vec<String>,
}

pub fn lock_path(workflow_path: &Path) -> PathBuf {
    workflow_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join(LOCK_FILE_NAME)
}

pub fn generate_lock(
    workflow: &Workflow,
    workflow_path: &Path,
    offline: bool,
) -> Result<PackLock, String> {
    let root = canonical_workflow_root(workflow_path)?;
    let mut resolver = Resolver::new(root, offline);
    for reference in &workflow.spec.packs {
        let (source, signature) = root_requirement(reference)?;
        let constraint = root_constraint(reference)?;
        resolver.resolve(
            &reference.name,
            &constraint,
            &source,
            signature.as_ref(),
            &workflow.spec.pack_trust,
        )?;
        verify_legacy_integrity(reference, resolver.entries.get(&reference.name))?;
    }
    let lock = PackLock {
        api_version: PACK_LOCK_API_VERSION.to_owned(),
        agentctl: env!("CARGO_PKG_VERSION").to_owned(),
        packs: resolver.entries.into_values().collect(),
    }
    .canonicalized();
    lock.validate().map_err(|error| error.to_string())?;
    Ok(lock)
}

pub fn write_lock(workflow_path: &Path, lock: &PackLock) -> Result<PathBuf, String> {
    let path = lock_path(workflow_path);
    let yaml = serde_yaml_ng::to_string(lock).map_err(|error| error.to_string())?;
    let temporary = path.with_extension(format!("lock.tmp-{}", std::process::id()));
    fs::write(&temporary, yaml.as_bytes())
        .map_err(|error| format!("{}: {error}", temporary.display()))?;
    fs::rename(&temporary, &path).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(path)
}

pub fn load_for_workflow(
    workflow: &Workflow,
    workflow_path: &Path,
    options: PackOptions,
) -> Result<LoadedPacks, String> {
    if workflow.spec.packs.is_empty() {
        return Ok(LoadedPacks {
            packs: Vec::new(),
            warnings: Vec::new(),
        });
    }
    let path = lock_path(workflow_path);
    if path.exists() {
        let bytes = read_bounded(&path, MAX_MANIFEST_BYTES)?;
        let lock: PackLock = serde_yaml_ng::from_slice(&bytes)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        lock.validate().map_err(|error| error.to_string())?;
        return load_locked(workflow, workflow_path, &lock, options.offline);
    }
    if options.locked {
        return Err(format!(
            "{} is required by --locked; run `agentctl packs lock {}` and commit it",
            path.display(),
            workflow_path.display()
        ));
    }
    if workflow.spec.packs.iter().any(|reference| {
        reference.source.is_some()
            || reference.signature.is_some()
            || reference.path.is_none()
            || reference.integrity.is_none()
    }) {
        return Err(format!(
            "{} is required for source-based or signed packs; run `agentctl packs lock {}`",
            path.display(),
            workflow_path.display()
        ));
    }
    let lock = generate_lock(workflow, workflow_path, options.offline)?;
    let mut loaded = load_locked(workflow, workflow_path, &lock, options.offline)?;
    loaded.warnings.push(format!(
        "legacy unlocked pack references are deprecated; generate and commit {}",
        path.display()
    ));
    Ok(loaded)
}

fn load_locked(
    workflow: &Workflow,
    workflow_path: &Path,
    lock: &PackLock,
    offline: bool,
) -> Result<LoadedPacks, String> {
    let root = canonical_workflow_root(workflow_path)?;
    verify_root_requirements(workflow, lock)?;
    let resolver = Resolver::new(root.clone(), offline);
    let mut packs = Vec::with_capacity(lock.packs.len());
    let mut warnings = Vec::new();
    for entry in &lock.packs {
        let bytes = resolver.fetch(&entry.source)?;
        let actual = digest_bytes(&bytes);
        if actual != entry.integrity {
            return Err(format!(
                "pack `{}` lock digest mismatch: expected {}, got {actual}",
                entry.name, entry.integrity
            ));
        }
        let manifest = parse_pack(&bytes, &entry.name).map_err(|error| error.to_string())?;
        verify_locked_manifest(entry, &manifest, lock)?;
        let trust = verify_trust(
            &root,
            &bytes,
            entry.signature.as_ref(),
            &workflow.spec.pack_trust,
        )?;
        if trust != entry.trust {
            return Err(format!(
                "pack `{}` trust metadata drifted from the lockfile",
                entry.name
            ));
        }
        apply_unsigned_policy(
            &entry.name,
            &manifest,
            &trust,
            &workflow.spec.pack_trust,
            &mut warnings,
        )?;
        packs.push((entry.clone(), manifest));
    }
    Ok(LoadedPacks { packs, warnings })
}

fn verify_root_requirements(workflow: &Workflow, lock: &PackLock) -> Result<(), String> {
    let mut reachable = BTreeSet::new();
    let mut pending = workflow
        .spec
        .packs
        .iter()
        .map(|reference| reference.name.clone())
        .collect::<Vec<_>>();
    for reference in &workflow.spec.packs {
        let entry = lock
            .packs
            .iter()
            .find(|entry| entry.name == reference.name)
            .ok_or_else(|| {
                format!(
                    "workflow pack `{}` is missing from the lockfile",
                    reference.name
                )
            })?;
        let requirement = root_version_requirement(reference)?;
        let locked_version = Version::parse(&entry.version).map_err(|error| error.to_string())?;
        if !requirement.matches(&locked_version) {
            return Err(format!(
                "workflow constraint `{}` for `{}` does not match locked version {}",
                reference.version, reference.name, entry.version
            ));
        }
        let (source, signature) = root_requirement(reference)?;
        if normalize_source(&source)? != entry.source {
            return Err(format!(
                "workflow source for `{}` drifted from the lockfile",
                reference.name
            ));
        }
        if signature != entry.signature {
            return Err(format!(
                "workflow signature metadata for `{}` drifted from the lockfile",
                reference.name
            ));
        }
        verify_legacy_integrity(reference, Some(entry))?;
    }
    while let Some(name) = pending.pop() {
        if !reachable.insert(name.clone()) {
            continue;
        }
        let entry = lock
            .packs
            .iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| format!("lockfile is missing reachable pack `{name}`"))?;
        pending.extend(entry.dependencies.keys().cloned());
    }
    if reachable.len() != lock.packs.len() {
        let extras = lock
            .packs
            .iter()
            .filter(|entry| !reachable.contains(&entry.name))
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "lockfile contains packs that are not reachable from workflow roots: {extras}"
        ));
    }
    Ok(())
}

fn verify_locked_manifest(
    entry: &PackLockEntry,
    manifest: &PackManifest,
    lock: &PackLock,
) -> Result<(), String> {
    if manifest.name != entry.name
        || manifest.version != entry.version
        || manifest.agentctl != entry.compatibility
    {
        return Err(format!(
            "locked pack `{}@{}` does not match manifest `{}@{}`",
            entry.name, entry.version, manifest.name, manifest.version
        ));
    }
    if manifest.dependencies.len() != entry.dependencies.len() {
        return Err(format!(
            "locked dependency graph for `{}` does not match its manifest",
            entry.name
        ));
    }
    for (name, dependency) in &manifest.dependencies {
        let locked_version = entry.dependencies.get(name).ok_or_else(|| {
            format!(
                "locked dependency graph for `{}` is missing `{name}`",
                entry.name
            )
        })?;
        let target = lock
            .packs
            .iter()
            .find(|target| target.name == *name)
            .ok_or_else(|| format!("lockfile is missing dependency `{name}`"))?;
        if &target.version != locked_version {
            return Err(format!(
                "locked dependency `{name}` version does not match its graph edge"
            ));
        }
        let requirement =
            VersionReq::parse(&dependency.version).map_err(|error| error.to_string())?;
        let version = Version::parse(locked_version).map_err(|error| error.to_string())?;
        if !requirement.matches(&version) || normalize_source(&dependency.source)? != target.source
        {
            return Err(format!(
                "dependency `{name}` from `{}` drifted from the lockfile",
                entry.name
            ));
        }
        if dependency.signature != target.signature {
            return Err(format!(
                "dependency `{name}` signature metadata drifted from the lockfile"
            ));
        }
    }
    Ok(())
}

fn apply_unsigned_policy(
    name: &str,
    manifest: &PackManifest,
    trust: &PackTrustRecord,
    policy: &PackTrustDefinition,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    if !matches!(trust, PackTrustRecord::Unsigned) {
        return Ok(());
    }
    match policy.unsigned {
        UnsignedPackPolicy::Deny => {
            return Err(format!(
                "unsigned pack `{name}` is denied by spec.packTrust.unsigned"
            ));
        }
        UnsignedPackPolicy::Warn => {
            warnings.push(format!("pack `{name}` is unsigned"));
        }
        UnsignedPackPolicy::Allow => {}
    }
    let process_capable = manifest.actions.values().any(|action| {
        matches!(
            action.kind,
            ActionKind::ShellExec | ActionKind::ProcessExtension
        )
    });
    if process_capable && !policy.allow_unsigned_process {
        return Err(format!(
            "unsigned pack `{name}` declares process execution; set spec.packTrust.allowUnsignedProcess only after reviewing the pack"
        ));
    }
    Ok(())
}

fn verify_trust(
    root: &Path,
    bytes: &[u8],
    signature: Option<&PackSignature>,
    policy: &PackTrustDefinition,
) -> Result<PackTrustRecord, String> {
    let Some(signature) = signature else {
        return Ok(PackTrustRecord::Unsigned);
    };
    if !policy
        .identities
        .iter()
        .any(|allowed| allowed.identity == signature.identity && allowed.issuer == signature.issuer)
    {
        return Err(format!(
            "Sigstore identity `{}` from issuer `{}` is not allowlisted by spec.packTrust.identities",
            signature.identity, signature.issuer
        ));
    }
    let bundle_path = contained_path(root, &signature.bundle, "Sigstore bundle")?;
    let bundle_bytes = read_bounded(&bundle_path, MAX_BUNDLE_BYTES)?;
    let bundle_json = std::str::from_utf8(&bundle_bytes)
        .map_err(|error| format!("{}: {error}", bundle_path.display()))?;
    let bundle = Bundle::from_json(bundle_json).map_err(|error| {
        format!(
            "{}: malformed Sigstore bundle: {error}",
            bundle_path.display()
        )
    })?;
    let trusted_root = TrustedRoot::from_json(SIGSTORE_PRODUCTION_TRUSTED_ROOT)
        .map_err(|error| format!("embedded Sigstore trust root is invalid: {error}"))?;
    let verification_policy = VerificationPolicy::default()
        .require_identity(&signature.identity)
        .require_issuer(&signature.issuer);
    let result = verify(bytes, &bundle, &verification_policy, &trusted_root)
        .map_err(|error| format!("Sigstore verification failed: {error}"))?;
    if !result.success {
        return Err("Sigstore verification did not produce a successful result".to_owned());
    }
    Ok(PackTrustRecord::Sigstore {
        identity: signature.identity.clone(),
        issuer: signature.issuer.clone(),
        bundle_integrity: digest_bytes(&bundle_bytes),
    })
}

struct Resolver {
    root: PathBuf,
    cache: PathBuf,
    offline: bool,
    entries: BTreeMap<String, PackLockEntry>,
    visiting: BTreeSet<String>,
}

impl Resolver {
    fn new(root: PathBuf, offline: bool) -> Self {
        Self {
            cache: root.join(CACHE_DIRECTORY),
            root,
            offline,
            entries: BTreeMap::new(),
            visiting: BTreeSet::new(),
        }
    }

    fn resolve(
        &mut self,
        expected_name: &str,
        constraint: &str,
        source: &PackSource,
        signature: Option<&PackSignature>,
        trust_policy: &PackTrustDefinition,
    ) -> Result<String, String> {
        let requirement = VersionReq::parse(constraint)
            .map_err(|error| format!("pack `{expected_name}` constraint is invalid: {error}"))?;
        if self.visiting.contains(expected_name) {
            return Err(format!("pack dependency cycle includes `{expected_name}`"));
        }
        if let Some(existing) = self.entries.get(expected_name) {
            let version = Version::parse(&existing.version).map_err(|error| error.to_string())?;
            if !requirement.matches(&version)
                || existing.source != normalize_source(source)?
                || existing.signature.as_ref() != signature
            {
                return Err(format!(
                    "conflicting requirements for pack `{expected_name}`"
                ));
            }
            return Ok(existing.version.clone());
        }
        self.visiting.insert(expected_name.to_owned());
        let source = normalize_source(source)?;
        source.validate().map_err(|error| error.to_string())?;
        let bytes = self.fetch(&source)?;
        let manifest = parse_pack(&bytes, expected_name).map_err(|error| error.to_string())?;
        if manifest.name != expected_name {
            return Err(format!(
                "pack requirement `{expected_name}` resolved manifest `{}`",
                manifest.name
            ));
        }
        let version = Version::parse(&manifest.version).map_err(|error| error.to_string())?;
        if !requirement.matches(&version) {
            return Err(format!(
                "pack `{expected_name}` version {} does not satisfy `{constraint}`",
                manifest.version
            ));
        }
        let trust = verify_trust(&self.root, &bytes, signature, trust_policy)?;
        let mut dependencies = BTreeMap::new();
        for (
            dependency_name,
            PackDependency {
                version,
                source,
                signature,
            },
        ) in &manifest.dependencies
        {
            let resolved = self.resolve(
                dependency_name,
                version,
                source,
                signature.as_ref(),
                trust_policy,
            )?;
            dependencies.insert(dependency_name.clone(), resolved);
        }
        self.visiting.remove(expected_name);
        self.entries.insert(
            expected_name.to_owned(),
            PackLockEntry {
                name: manifest.name.clone(),
                version: manifest.version.clone(),
                source,
                integrity: digest_bytes(&bytes),
                compatibility: manifest.agentctl.clone(),
                dependencies,
                signature: signature.cloned(),
                trust,
            },
        );
        Ok(manifest.version)
    }

    fn fetch(&self, source: &PackSource) -> Result<Vec<u8>, String> {
        match source {
            PackSource::Path { path } => {
                let path = contained_path(&self.root, path, "pack path")?;
                read_bounded(&path, MAX_MANIFEST_BYTES)
            }
            PackSource::Git { git, rev, manifest } => self.fetch_git(git, rev, manifest),
            PackSource::Archive {
                url,
                integrity,
                manifest,
            } => self.fetch_archive(url, integrity, manifest),
        }
    }

    fn fetch_git(&self, git: &str, rev: &str, manifest: &Path) -> Result<Vec<u8>, String> {
        validate_git_source(&self.root, git)?;
        let key = digest_bytes(format!("{git}\n{rev}").as_bytes())
            .trim_start_matches("sha256:")
            .to_owned();
        let directory = self.cache.join("git").join(&key);
        if !directory.exists() {
            if self.offline {
                return Err(format!(
                    "offline pack cache miss for Git source `{git}` at `{rev}`"
                ));
            }
            fs::create_dir_all(directory.parent().expect("Git cache parent"))
                .map_err(|error| error.to_string())?;
            let temporary = directory.with_extension(format!("tmp-{}", std::process::id()));
            if temporary.exists() {
                fs::remove_dir_all(&temporary).map_err(|error| error.to_string())?;
            }
            fs::create_dir_all(&temporary).map_err(|error| error.to_string())?;
            git_status(["init", "-q"], &temporary)?;
            git_status(["remote", "add", "origin", git], &temporary)?;
            git_status(
                ["fetch", "-q", "--depth", "1", "--no-tags", "origin", rev],
                &temporary,
            )?;
            git_status(["checkout", "-q", "--detach", "FETCH_HEAD"], &temporary)?;
            let actual = git_output(["rev-parse", "HEAD"], &temporary)?;
            if actual.trim() != rev {
                return Err(format!(
                    "Git source `{git}` resolved `{}`, expected `{rev}`",
                    actual.trim()
                ));
            }
            fs::rename(&temporary, &directory).map_err(|error| error.to_string())?;
        }
        let actual = git_output(["rev-parse", "HEAD"], &directory)?;
        if actual.trim() != rev {
            return Err(format!(
                "cached Git source `{git}` has commit `{}`, expected `{rev}`",
                actual.trim()
            ));
        }
        let manifest = contained_path(&directory, manifest, "Git pack manifest")?;
        read_bounded(&manifest, MAX_MANIFEST_BYTES)
    }

    fn fetch_archive(
        &self,
        url: &str,
        integrity: &str,
        manifest: &Path,
    ) -> Result<Vec<u8>, String> {
        let key = integrity
            .strip_prefix("sha256:")
            .ok_or_else(|| "archive integrity must use sha256".to_owned())?;
        let archive_path = self.cache.join("archives").join(format!("{key}.tar.gz"));
        let directory = self.cache.join("archives").join(key);
        if !archive_path.exists() {
            if self.offline {
                return Err(format!("offline pack cache miss for archive `{url}`"));
            }
            let bytes = download_archive(url)?;
            let actual = digest_bytes(&bytes);
            if actual != integrity {
                return Err(format!(
                    "archive `{url}` integrity mismatch: expected {integrity}, got {actual}"
                ));
            }
            fs::create_dir_all(archive_path.parent().expect("archive cache parent"))
                .map_err(|error| error.to_string())?;
            fs::write(&archive_path, bytes).map_err(|error| error.to_string())?;
        } else {
            let bytes = read_bounded(&archive_path, MAX_ARCHIVE_BYTES)?;
            let actual = digest_bytes(&bytes);
            if actual != integrity {
                return Err(format!(
                    "cached archive `{url}` integrity mismatch: expected {integrity}, got {actual}"
                ));
            }
        }
        if !directory.exists() {
            let temporary = directory.with_extension(format!("tmp-{}", std::process::id()));
            if temporary.exists() {
                fs::remove_dir_all(&temporary).map_err(|error| error.to_string())?;
            }
            fs::create_dir_all(&temporary).map_err(|error| error.to_string())?;
            extract_archive(&archive_path, &temporary)?;
            fs::rename(&temporary, &directory).map_err(|error| error.to_string())?;
        }
        let manifest = contained_path(&directory, manifest, "archive pack manifest")?;
        read_bounded(&manifest, MAX_MANIFEST_BYTES)
    }
}

fn root_requirement(
    reference: &PackReference,
) -> Result<(PackSource, Option<PackSignature>), String> {
    if let Some(source) = &reference.source {
        if reference.path.is_some() || reference.integrity.is_some() {
            return Err(format!(
                "pack `{}` cannot combine source with legacy path/integrity fields",
                reference.name
            ));
        }
        return Ok((source.clone(), reference.signature.clone()));
    }
    let path = reference.path.as_ref().ok_or_else(|| {
        format!(
            "pack `{}` must define source or legacy path/integrity",
            reference.name
        )
    })?;
    reference.integrity.as_ref().ok_or_else(|| {
        format!(
            "legacy pack `{}` must define both path and integrity",
            reference.name
        )
    })?;
    if !reference.version.starts_with('=') {
        Version::parse(&reference.version).map_err(|error| {
            format!(
                "legacy pack `{}` version must be exact semver: {error}",
                reference.name
            )
        })?;
    }
    let source = PackSource::Path {
        path: PathBuf::from(path),
    };
    Ok((source, reference.signature.clone()))
}

fn root_version_requirement(reference: &PackReference) -> Result<VersionReq, String> {
    if reference.source.is_none() && !reference.version.starts_with('=') {
        let version = Version::parse(&reference.version).map_err(|error| error.to_string())?;
        VersionReq::parse(&format!("={version}")).map_err(|error| error.to_string())
    } else {
        VersionReq::parse(&reference.version).map_err(|error| error.to_string())
    }
}

fn root_constraint(reference: &PackReference) -> Result<String, String> {
    if reference.source.is_none() && !reference.version.starts_with('=') {
        let version = Version::parse(&reference.version).map_err(|error| error.to_string())?;
        Ok(format!("={version}"))
    } else {
        Ok(reference.version.clone())
    }
}

fn verify_legacy_integrity(
    reference: &PackReference,
    entry: Option<&PackLockEntry>,
) -> Result<(), String> {
    let Some(expected) = reference.integrity.as_ref() else {
        return Ok(());
    };
    let entry = entry.ok_or_else(|| format!("pack `{}` did not resolve", reference.name))?;
    if entry.integrity == *expected {
        Ok(())
    } else {
        Err(format!(
            "legacy pack `{}` integrity mismatch: expected {expected}, got {}",
            reference.name, entry.integrity
        ))
    }
}

fn normalize_source(source: &PackSource) -> Result<PackSource, String> {
    source.validate().map_err(|error| error.to_string())?;
    Ok(source.clone())
}

fn canonical_workflow_root(workflow_path: &Path) -> Result<PathBuf, String> {
    let base = workflow_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::canonicalize(base).map_err(|error| format!("{}: {error}", base.display()))
}

fn contained_path(root: &Path, relative: &Path, label: &str) -> Result<PathBuf, String> {
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!("{label} must be a contained relative path"));
    }
    let joined = root.join(relative);
    let canonical =
        fs::canonicalize(&joined).map_err(|error| format!("{}: {error}", joined.display()))?;
    if !canonical.starts_with(root) {
        return Err(format!("{label} resolves outside {}", root.display()));
    }
    Ok(canonical)
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let metadata = fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if metadata.len() > limit {
        return Err(format!("{} exceeds the {limit}-byte limit", path.display()));
    }
    fs::read(path).map_err(|error| format!("{}: {error}", path.display()))
}

fn validate_git_source(root: &Path, source: &str) -> Result<(), String> {
    if let Ok(url) = url::Url::parse(source) {
        return match url.scheme() {
            "https" => Ok(()),
            "file" => {
                let path = url
                    .to_file_path()
                    .map_err(|()| format!("invalid file Git URL `{source}`"))?;
                let canonical = fs::canonicalize(&path)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                if canonical.starts_with(root) {
                    Ok(())
                } else {
                    Err("file Git sources must remain under the workflow directory".to_owned())
                }
            }
            _ => Err("Git pack sources must use https or a contained file URL".to_owned()),
        };
    }
    Err("Git pack source must be an absolute URL".to_owned())
}

fn git_status<const N: usize>(args: [&str; N], directory: &Path) -> Result<(), String> {
    let status = Command::new("git")
        .args(args)
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("failed to execute Git: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Git command failed with {status}"))
    }
}

fn git_output<const N: usize>(args: [&str; N], directory: &Path) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(directory)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("failed to execute Git: {error}"))?;
    if !output.status.success() || output.stdout.len() > 128 {
        return Err(format!("Git command failed with {}", output.status));
    }
    String::from_utf8(output.stdout).map_err(|error| format!("Git output was not UTF-8: {error}"))
}

fn download_archive(url: &str) -> Result<Vec<u8>, String> {
    let parsed = url::Url::parse(url).map_err(|error| format!("invalid archive URL: {error}"))?;
    let local_http = parsed.scheme() == "http"
        && parsed.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
    if parsed.scheme() != "https" && !local_http {
        return Err("archive URL must use https; loopback http is fixture-only".to_owned());
    }
    let client = Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .get(parsed)
        .send()
        .map_err(|error| format!("archive download failed: {error}"))?;
    if response.status().is_redirection() {
        return Err("archive redirects are not followed; lock the final immutable URL".to_owned());
    }
    if !response.status().is_success() {
        return Err(format!(
            "archive download returned HTTP {}",
            response.status()
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_ARCHIVE_BYTES)
    {
        return Err(format!(
            "archive exceeds the {MAX_ARCHIVE_BYTES}-byte compressed limit"
        ));
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_ARCHIVE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("archive download failed: {error}"))?;
    if bytes.len() as u64 > MAX_ARCHIVE_BYTES {
        return Err(format!(
            "archive exceeds the {MAX_ARCHIVE_BYTES}-byte compressed limit"
        ));
    }
    Ok(bytes)
}

fn extract_archive(archive_path: &Path, destination: &Path) -> Result<(), String> {
    let file = fs::File::open(archive_path)
        .map_err(|error| format!("{}: {error}", archive_path.display()))?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    let entries = archive.entries().map_err(|error| error.to_string())?;
    let mut count = 0_usize;
    let mut expanded = 0_u64;
    for entry in entries {
        let mut entry = entry.map_err(|error| error.to_string())?;
        count += 1;
        if count > MAX_ARCHIVE_ENTRIES {
            return Err(format!(
                "archive exceeds the {MAX_ARCHIVE_ENTRIES}-entry limit"
            ));
        }
        let path = entry.path().map_err(|error| error.to_string())?;
        if path.as_os_str().is_empty()
            || path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err("archive contains a path outside its extraction root".to_owned());
        }
        let entry_type = entry.header().entry_type();
        if !entry_type.is_file() && !entry_type.is_dir() {
            return Err(
                "archive symlinks, hardlinks, and special files are not allowed".to_owned(),
            );
        }
        expanded = expanded
            .checked_add(entry.header().size().map_err(|error| error.to_string())?)
            .ok_or_else(|| "archive expanded size overflowed".to_owned())?;
        if expanded > MAX_ARCHIVE_EXPANDED_BYTES {
            return Err(format!(
                "archive exceeds the {MAX_ARCHIVE_EXPANDED_BYTES}-byte expanded limit"
            ));
        }
        let output = destination.join(path.as_ref());
        if entry_type.is_dir() {
            fs::create_dir_all(&output).map_err(|error| error.to_string())?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let mut file = fs::File::create(&output).map_err(|error| error.to_string())?;
        std::io::copy(&mut entry, &mut file).map_err(|error| error.to_string())?;
        file.flush().map_err(|error| error.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = entry.header().mode().map_err(|error| error.to_string())? & 0o777;
            fs::set_permissions(&output, fs::Permissions::from_mode(mode))
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentctl_core::dsl::parse_workflow;
    use tempfile::tempdir;

    fn write(path: &Path, value: &str) {
        fs::create_dir_all(path.parent().expect("fixture parent")).expect("fixture directory");
        fs::write(path, value).expect("fixture");
    }

    fn workflow(directory: &Path, roots: &str, trust: &str) -> (Workflow, PathBuf) {
        let path = directory.join("workflow.yaml");
        let source = format!(
            r#"
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata: {{ name: pack-test }}
spec:
  packTrust:
{trust}
  packs:
{roots}
  tasks:
    - id: done
      uses: action:example.root.assign
      with: {{ value: ok }}
"#
        );
        write(&path, &source);
        let parsed = parse_workflow(&source, "workflow.yaml")
            .expect("workflow")
            .workflow;
        (parsed, path)
    }

    #[test]
    fn resolves_transitive_local_graph_deterministically_and_offline() {
        let directory = tempdir().expect("tempdir");
        write(
            &directory.path().join("dependency.pack.yaml"),
            r#"
apiVersion: agentctl.dev/pack/v1alpha1
name: example.dependency
version: 1.2.0
agentctl: ">=0.2.0, <1.0.0"
actions:
  assign: { kind: builtin.assign }
"#,
        );
        write(
            &directory.path().join("root.pack.yaml"),
            r#"
apiVersion: agentctl.dev/pack/v1alpha1
name: example.root
version: 2.0.0
agentctl: ">=0.2.0, <1.0.0"
dependencies:
  example.dependency:
    version: "^1.0"
    source: { path: dependency.pack.yaml }
actions:
  assign: { kind: builtin.assign }
"#,
        );
        let (workflow, path) = workflow(
            directory.path(),
            "    - name: example.root\n      version: \"^2.0\"\n      source: { path: root.pack.yaml }",
            "    unsigned: allow",
        );
        let first = generate_lock(&workflow, &path, true).expect("first lock");
        let second = generate_lock(&workflow, &path, true).expect("second lock");
        assert_eq!(first, second);
        assert_eq!(
            first
                .packs
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["example.dependency", "example.root"]
        );
        write_lock(&path, &first).expect("write lock");
        let loaded = load_for_workflow(
            &workflow,
            &path,
            PackOptions {
                offline: true,
                locked: true,
            },
        )
        .expect("offline locked load");
        assert_eq!(loaded.packs.len(), 2);
    }

    #[test]
    fn reports_dependency_cycles_and_conflicts() {
        let directory = tempdir().expect("tempdir");
        write(
            &directory.path().join("a.pack.yaml"),
            r#"
apiVersion: agentctl.dev/pack/v1alpha1
name: example.a
version: 1.0.0
agentctl: ">=0.2.0, <1.0.0"
dependencies:
  example.b:
    version: "1"
    source: { path: b.pack.yaml }
actions:
  assign: { kind: builtin.assign }
"#,
        );
        write(
            &directory.path().join("b.pack.yaml"),
            r#"
apiVersion: agentctl.dev/pack/v1alpha1
name: example.b
version: 1.0.0
agentctl: ">=0.2.0, <1.0.0"
dependencies:
  example.a:
    version: "1"
    source: { path: a.pack.yaml }
"#,
        );
        let (cycle_workflow, path) = workflow(
            directory.path(),
            "    - name: example.a\n      version: \"1\"\n      source: { path: a.pack.yaml }",
            "    unsigned: allow",
        );
        let cycle = generate_lock(&cycle_workflow, &path, true).expect_err("cycle");
        assert!(cycle.contains("cycle"), "{cycle}");

        write(
            &directory.path().join("b.pack.yaml"),
            r#"
apiVersion: agentctl.dev/pack/v1alpha1
name: example.b
version: 1.0.0
agentctl: ">=0.2.0, <1.0.0"
dependencies:
  example.shared:
    version: "1"
    source: { path: shared-one.pack.yaml }
"#,
        );
        write(
            &directory.path().join("shared-one.pack.yaml"),
            "apiVersion: agentctl.dev/pack/v1alpha1\nname: example.shared\nversion: 1.0.0\nagentctl: \">=0.2.0, <1.0.0\"\n",
        );
        write(
            &directory.path().join("shared-two.pack.yaml"),
            "apiVersion: agentctl.dev/pack/v1alpha1\nname: example.shared\nversion: 2.0.0\nagentctl: \">=0.2.0, <1.0.0\"\n",
        );
        write(
            &directory.path().join("c.pack.yaml"),
            r#"
apiVersion: agentctl.dev/pack/v1alpha1
name: example.c
version: 1.0.0
agentctl: ">=0.2.0, <1.0.0"
dependencies:
  example.shared:
    version: "2"
    source: { path: shared-two.pack.yaml }
"#,
        );
        let (conflicting, conflicting_path) = workflow(
            directory.path(),
            "    - name: example.b\n      version: \"1\"\n      source: { path: b.pack.yaml }\n    - name: example.c\n      version: \"1\"\n      source: { path: c.pack.yaml }",
            "    unsigned: allow",
        );
        let conflict = generate_lock(&conflicting, &conflicting_path, true).expect_err("conflict");
        assert!(conflict.contains("conflicting requirements"), "{conflict}");
    }

    #[test]
    fn locked_mode_rejects_tamper_and_unsigned_process_by_default() {
        let directory = tempdir().expect("tempdir");
        let manifest = directory.path().join("root.pack.yaml");
        write(
            &manifest,
            r#"
apiVersion: agentctl.dev/pack/v1alpha1
name: example.root
version: 1.0.0
agentctl: ">=0.2.0, <1.0.0"
actions:
  assign: { kind: builtin.assign }
"#,
        );
        let (workflow, path) = workflow(
            directory.path(),
            "    - name: example.root\n      version: \"1\"\n      source: { path: root.pack.yaml }",
            "    unsigned: warn",
        );
        let lock = generate_lock(&workflow, &path, true).expect("lock");
        write_lock(&path, &lock).expect("write lock");
        let mut extra_lock = lock.clone();
        let mut extra = extra_lock.packs[0].clone();
        extra.name = "example.extra".to_owned();
        extra_lock.packs.push(extra);
        write_lock(&path, &extra_lock).expect("write extra lock");
        let unreachable = load_for_workflow(
            &workflow,
            &path,
            PackOptions {
                offline: true,
                locked: true,
            },
        )
        .err()
        .expect("unreachable lock entry");
        assert!(unreachable.contains("not reachable"), "{unreachable}");
        write_lock(&path, &lock).expect("restore lock");
        write(&manifest, "tampered");
        let tamper = load_for_workflow(
            &workflow,
            &path,
            PackOptions {
                offline: true,
                locked: true,
            },
        )
        .err()
        .expect("tamper");
        assert!(tamper.contains("digest mismatch"), "{tamper}");

        write(
            &manifest,
            r#"
apiVersion: agentctl.dev/pack/v1alpha1
name: example.root
version: 1.0.0
agentctl: ">=0.2.0, <1.0.0"
actions:
  assign:
    kind: builtin.shell.exec
    command: sh
"#,
        );
        let process_lock = generate_lock(&workflow, &path, true).expect("process lock");
        write_lock(&path, &process_lock).expect("write process lock");
        let denial = load_for_workflow(
            &workflow,
            &path,
            PackOptions {
                offline: true,
                locked: true,
            },
        )
        .err()
        .expect("unsigned process denied");
        assert!(denial.contains("declares process execution"), "{denial}");
    }

    #[test]
    fn pinned_git_cache_is_reusable_offline() {
        let directory = tempdir().expect("tempdir");
        let repository = directory.path().join("repository");
        fs::create_dir(&repository).expect("repository");
        git_status(["init", "-q"], &repository).expect("git init");
        write(
            &repository.join("agentctl.pack.yaml"),
            "apiVersion: agentctl.dev/pack/v1alpha1\nname: example.root\nversion: 1.0.0\nagentctl: \">=0.2.0, <1.0.0\"\nactions:\n  assign: { kind: builtin.assign }\n",
        );
        git_status(["add", "agentctl.pack.yaml"], &repository).expect("git add");
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=agentctl",
                "-c",
                "user.email=agentctl@example.invalid",
                "commit",
                "-q",
                "-m",
                "fixture",
            ])
            .current_dir(&repository)
            .status()
            .expect("git commit");
        assert!(status.success());
        let rev = git_output(["rev-parse", "HEAD"], &repository)
            .expect("rev")
            .trim()
            .to_owned();
        let url = url::Url::from_file_path(&repository)
            .expect("file URL")
            .to_string();
        let roots = format!(
            "    - name: example.root\n      version: \"1\"\n      source:\n        git: {url}\n        rev: {rev}\n        manifest: agentctl.pack.yaml"
        );
        let (workflow, path) = workflow(directory.path(), &roots, "    unsigned: allow");
        let online = generate_lock(&workflow, &path, false).expect("online lock");
        let offline = generate_lock(&workflow, &path, true).expect("offline lock");
        assert_eq!(online, offline);
    }

    #[test]
    fn verifies_sigstore_bundle_identity_and_rejects_tamper() {
        const BUNDLE: &str =
            include_str!("../tests/fixtures/sigstore/cosign-v3-blob.sigstore.json");
        const ARTIFACT: &[u8] = include_bytes!("../tests/fixtures/sigstore/cosign-v3-blob.txt");

        let directory = tempdir().expect("tempdir");
        write(&directory.path().join("bundle.sigstore.json"), BUNDLE);
        let signature = PackSignature {
            bundle: PathBuf::from("bundle.sigstore.json"),
            identity: "w.vollprecht@gmail.com".to_owned(),
            issuer: "https://github.com/login/oauth".to_owned(),
        };
        let policy = PackTrustDefinition {
            identities: vec![agentctl_core::dsl::PackIdentity {
                identity: signature.identity.clone(),
                issuer: signature.issuer.clone(),
            }],
            ..PackTrustDefinition::default()
        };
        let root = fs::canonicalize(directory.path()).expect("root");
        let trust = verify_trust(&root, ARTIFACT, Some(&signature), &policy).expect("valid bundle");
        assert!(matches!(trust, PackTrustRecord::Sigstore { .. }));

        let tamper = verify_trust(&root, b"tampered", Some(&signature), &policy)
            .expect_err("tampered artifact");
        assert!(tamper.contains("Sigstore verification failed"), "{tamper}");

        let untrusted = verify_trust(
            &root,
            ARTIFACT,
            Some(&signature),
            &PackTrustDefinition::default(),
        )
        .expect_err("untrusted identity");
        assert!(untrusted.contains("not allowlisted"), "{untrusted}");

        let mut invalid_time: serde_json::Value =
            serde_json::from_str(BUNDLE).expect("fixture bundle");
        *invalid_time
            .pointer_mut("/verificationMaterial/tlogEntries/0/integratedTime")
            .expect("integrated time") = serde_json::Value::String("0".to_owned());
        write(
            &directory.path().join("bundle.sigstore.json"),
            &serde_json::to_string(&invalid_time).expect("invalid-time bundle"),
        );
        let invalid_time = verify_trust(&root, ARTIFACT, Some(&signature), &policy)
            .expect_err("invalid timing evidence");
        assert!(
            invalid_time.contains("Sigstore verification failed"),
            "{invalid_time}"
        );

        write(&directory.path().join("bundle.sigstore.json"), "{broken");
        let malformed =
            verify_trust(&root, ARTIFACT, Some(&signature), &policy).expect_err("malformed bundle");
        assert!(
            malformed.contains("malformed Sigstore bundle"),
            "{malformed}"
        );
    }

    #[test]
    fn immutable_archive_is_bounded_cached_and_rejects_links() {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::net::TcpListener;
        use std::thread;

        fn archive_with_manifest(manifest: &[u8]) -> Vec<u8> {
            let encoder = GzEncoder::new(Vec::new(), Compression::default());
            let mut builder = tar::Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            header.set_path("agentctl.pack.yaml").expect("path");
            header.set_size(manifest.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, manifest).expect("manifest archive");
            builder
                .into_inner()
                .expect("encoder")
                .finish()
                .expect("gzip")
        }

        let manifest = b"apiVersion: agentctl.dev/pack/v1alpha1\nname: example.root\nversion: 1.0.0\nagentctl: \">=0.2.0, <1.0.0\"\nactions:\n  assign: { kind: builtin.assign }\n";
        let archive = archive_with_manifest(manifest);
        let integrity = digest_bytes(&archive);
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let served = archive.clone();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).expect("request");
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                served.len()
            )
            .expect("headers");
            stream.write_all(&served).expect("archive");
        });
        let directory = tempdir().expect("tempdir");
        let roots = format!(
            "    - name: example.root\n      version: \"1\"\n      source:\n        url: http://{address}/pack.tar.gz\n        integrity: {integrity}\n        manifest: agentctl.pack.yaml"
        );
        let (workflow, path) = workflow(directory.path(), &roots, "    unsigned: allow");
        let online = generate_lock(&workflow, &path, false).expect("online archive");
        server.join().expect("server");
        let offline = generate_lock(&workflow, &path, true).expect("offline archive");
        assert_eq!(online, offline);

        let malicious = directory.path().join("malicious.tar.gz");
        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_path("link").expect("link path");
        header.set_entry_type(tar::EntryType::Symlink);
        header
            .set_link_name("agentctl.pack.yaml")
            .expect("link name");
        header.set_size(0);
        header.set_mode(0o777);
        header.set_cksum();
        builder
            .append(&header, std::io::empty())
            .expect("link archive");
        let malicious_bytes = builder
            .into_inner()
            .expect("encoder")
            .finish()
            .expect("gzip");
        fs::write(&malicious, malicious_bytes).expect("malicious fixture");
        let extraction = directory.path().join("extraction");
        fs::create_dir(&extraction).expect("extraction");
        let error = extract_archive(&malicious, &extraction).expect_err("link rejected");
        assert!(error.contains("symlinks"), "{error}");
    }
}
