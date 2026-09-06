//! Bounded, policy-checked configuration capture before compilation.
//!
//! YAML cannot supply SourceSnapshot. Captured values are persisted with the
//! workflow, under the store's existing selected-field encryption policy.
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::diagnostic::{Diagnostic, DiagnosticCode};
use crate::dsl::{EffectClass, JsonMap, Risk, TaskDefinition, Workflow};
use crate::policy::{PolicyContext, PolicyDecision, PolicyEngine};

pub const MAX_SOURCE_BYTES: u64 = 1_048_576;
pub const MAX_SOURCE_FILES: usize = 256;
pub const MAX_CAPTURE_BYTES: u64 = 16 * MAX_SOURCE_BYTES;

#[derive(Debug, Clone, Default)]
pub struct SourceOrigins {
    pub agents: BTreeMap<String, PathBuf>,
    pub subworkflows: BTreeMap<String, PathBuf>,
    pub files: BTreeMap<PathBuf, String>,
}

#[derive(Debug, Clone, Default)]
pub struct VariableOverrides {
    /// CLI callers resolve these relative to the invoking directory.
    pub files: Vec<PathBuf>,
    pub values: JsonMap,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceResolution {
    /// Scope -> variable name -> winning source. Values are deliberately absent.
    pub origins: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    pub effective_origins: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapturedFile {
    pub source: String,
    pub digest: String,
    pub content: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceSnapshot {
    pub format_version: u32,
    pub instructions: BTreeMap<String, CapturedFile>,
    pub file_digests: BTreeMap<String, String>,
    pub invocation_vars: JsonMap,
    pub resolution: SourceResolution,
}

/// Resolve once, before compilation. Already captured workflows remain immutable.
/// Ordinary config reads obey filesystem policy denies; approval requirements are
/// retained on runtime instruction observation/model effects, bound to captured bytes.
pub fn resolve_workflow_sources(
    workflow: &mut Workflow,
    origin: &Path,
    policy_base: &Path,
    overrides: &VariableOverrides,
    origins: &SourceOrigins,
) -> Result<SourceResolution, Vec<Diagnostic>> {
    if let Some(snapshot) = &workflow.spec.source_snapshot {
        if !overrides.files.is_empty() || !overrides.values.is_empty() {
            return Err(vec![*error(
                origin,
                "spec.sourceSnapshot",
                "captured variables cannot be overridden; load the workflow YAML for a fresh invocation",
            )]);
        }
        return Ok(snapshot.resolution.clone());
    }
    // Work on a clone so any error leaves the caller's workflow unchanged.
    let mut resolved = workflow.clone();
    let policy = PolicyEngine::new(workflow.spec.policy.clone(), policy_base)
        .map_err(|e| vec![*error(origin, "spec.policy.workspaceRoot", &e.to_string())])?;
    let mut capture = Capture {
        policy,
        expected_digests: &origins.files,
        cache: BTreeMap::new(),
        files: 0,
        bytes: 0,
        snapshot: SourceSnapshot {
            format_version: 1,
            ..SourceSnapshot::default()
        },
    };
    let outcome = (|| {
        capture.merge_vars(
            &mut resolved.spec.vars,
            &resolved.spec.vars_files,
            origin,
            "spec",
        )?;
        for (name, agent) in &mut resolved.spec.agents {
            let agent_origin = origins.agents.get(name).map_or(origin, PathBuf::as_path);
            let scope = format!("spec.agents.{name}");
            capture.merge_vars(&mut agent.vars, &agent.vars_files, agent_origin, &scope)?;
            if let Some(path) = &agent.instructions_file {
                let content =
                    capture.read(path, agent_origin, &format!("{scope}.instructionsFile"))?;
                capture.snapshot.instructions.insert(name.clone(), content);
            }
        }
        capture.tasks(&mut resolved.spec.tasks, origin, "spec.tasks")?;
        for (name, definition) in &mut resolved.spec.subworkflows {
            let definition_origin = origins
                .subworkflows
                .get(name)
                .map_or(origin, PathBuf::as_path);
            capture.tasks(
                &mut definition.tasks,
                definition_origin,
                &format!("spec.subworkflows.{name}.tasks"),
            )?;
        }
        let mut invocation = JsonMap::new();
        let files = overrides
            .files
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>();
        capture.merge_vars(&mut invocation, &files, origin, "invocation")?;
        validate_variables(&overrides.values, origin, "invocation.values")?;
        for (key, value) in &overrides.values {
            invocation.insert(key.clone(), value.clone());
            capture
                .snapshot
                .resolution
                .origins
                .entry("invocation".to_owned())
                .or_default()
                .insert(key.clone(), "invocation:--var".to_owned());
        }
        capture.snapshot.invocation_vars = invocation;
        Ok::<(), Box<Diagnostic>>(())
    })();
    outcome.map_err(|e| vec![*e])?;
    populate_effective_origins(&resolved, &mut capture.snapshot.resolution);
    let resolution = capture.snapshot.resolution.clone();
    // Avoid changing digests for existing inline-only workflows with no new vars.
    if capture.files != 0 || !overrides.values.is_empty() || !resolved.spec.vars.is_empty() {
        resolved.spec.source_snapshot = Some(capture.snapshot);
    }
    *workflow = resolved;
    Ok(resolution)
}

/// Redacted source information, derived solely from the captured workflow.
pub fn explain_sources(workflow: &Workflow) -> SourceResolution {
    if let Some(snapshot) = &workflow.spec.source_snapshot {
        return snapshot.resolution.clone();
    }
    let mut origins = BTreeMap::new();
    let mut add = |scope: String, vars: &JsonMap| {
        if !vars.is_empty() {
            origins.insert(
                scope.clone(),
                vars.keys()
                    .map(|k| (k.clone(), format!("{scope}.vars")))
                    .collect(),
            );
        }
    };
    add("spec".to_owned(), &workflow.spec.vars);
    for (name, agent) in &workflow.spec.agents {
        add(format!("spec.agents.{name}"), &agent.vars);
    }
    for task in &workflow.spec.tasks {
        add(format!("spec.tasks.{}", task.id), &task.vars);
    }
    for (name, definition) in &workflow.spec.subworkflows {
        for task in &definition.tasks {
            add(
                format!("spec.subworkflows.{name}.tasks.{}", task.id),
                &task.vars,
            );
        }
    }
    let mut resolution = SourceResolution {
        origins,
        ..SourceResolution::default()
    };
    populate_effective_origins(workflow, &mut resolution);
    resolution
}

fn populate_effective_origins(workflow: &Workflow, resolution: &mut SourceResolution) {
    let mut task_origins = |task: &TaskDefinition, scope: String| {
        let mut result = resolution.origins.get("spec").cloned().unwrap_or_default();
        if let Some(agent) = task.uses.strip_prefix("agent:") {
            result.extend(
                resolution
                    .origins
                    .get(&format!("spec.agents.{agent}"))
                    .cloned()
                    .unwrap_or_default(),
            );
        }
        result.extend(resolution.origins.get(&scope).cloned().unwrap_or_default());
        result.extend(
            resolution
                .origins
                .get("invocation")
                .cloned()
                .unwrap_or_default(),
        );
        resolution.effective_origins.insert(scope, result);
    };
    for task in &workflow.spec.tasks {
        task_origins(task, format!("spec.tasks.{}", task.id));
    }
    for (name, definition) in &workflow.spec.subworkflows {
        for task in &definition.tasks {
            task_origins(task, format!("spec.subworkflows.{name}.tasks.{}", task.id));
        }
    }
}

pub fn instruction_text<'a>(workflow: &'a Workflow, name: &str) -> Option<&'a str> {
    let agent = workflow.spec.agents.get(name)?;
    agent.instructions.as_deref().or_else(|| {
        workflow
            .spec
            .source_snapshot
            .as_ref()?
            .instructions
            .get(name)
            .map(|file| file.content.as_str())
    })
}

pub fn needs_capture(workflow: &Workflow) -> bool {
    workflow.spec.source_snapshot.is_none()
        && (!workflow.spec.vars_files.is_empty()
            || workflow
                .spec
                .agents
                .values()
                .any(|a| a.instructions_file.is_some() || !a.vars_files.is_empty())
            || workflow.spec.tasks.iter().any(|t| !t.vars_files.is_empty())
            || workflow
                .spec
                .subworkflows
                .values()
                .any(|s| s.tasks.iter().any(|t| !t.vars_files.is_empty())))
}

/// Called on unexpanded declarations, before compiler-owned bindings are inserted.
pub fn validate_workflow_variables(workflow: &Workflow, file: &str) -> Vec<Diagnostic> {
    let origin = Path::new(file);
    let mut diagnostics = Vec::new();
    let mut check = |values: &JsonMap, scope: &str| {
        if let Err(error) = validate_variables(values, origin, scope) {
            diagnostics.push(*error);
        }
    };
    check(&workflow.spec.vars, "spec.vars");
    for (name, agent) in &workflow.spec.agents {
        check(&agent.vars, &format!("spec.agents.{name}.vars"));
    }
    for task in &workflow.spec.tasks {
        check(&task.vars, &format!("spec.tasks.{}.vars", task.id));
    }
    for (name, definition) in &workflow.spec.subworkflows {
        for task in &definition.tasks {
            check(
                &task.vars,
                &format!("spec.subworkflows.{name}.tasks.{}.vars", task.id),
            );
        }
    }
    for task in workflow.spec.tasks.iter().chain(
        workflow
            .spec
            .subworkflows
            .values()
            .flat_map(|definition| definition.tasks.iter()),
    ) {
        if let Some(foreach) = &task.foreach {
            let agent = task
                .uses
                .strip_prefix("agent:")
                .and_then(|name| workflow.spec.agents.get(name));
            let invocation = workflow
                .spec
                .source_snapshot
                .as_ref()
                .map(|snapshot| &snapshot.invocation_vars);
            if workflow.spec.vars.contains_key(&foreach.binding)
                || task.vars.contains_key(&foreach.binding)
                || agent.is_some_and(|agent| agent.vars.contains_key(&foreach.binding))
                || invocation.is_some_and(|vars| vars.contains_key(&foreach.binding))
            {
                diagnostics.push(*error(origin, &format!("spec.tasks.{}.vars", task.id),
                    &format!("task `{}` foreach bindings conflict with existing task vars or higher/lower precedence sources", task.id)));
            }
        }
    }
    diagnostics
}

fn validate_variables(values: &JsonMap, origin: &Path, scope: &str) -> Result<(), Box<Diagnostic>> {
    for key in values.keys() {
        if matches!(
            key.as_str(),
            "loopIndex" | "loopPrevious" | "foreachIndex" | "matrix" | "matrixIndex"
        ) {
            return Err(error(
                origin,
                scope,
                &format!(
                    "bindings conflict with task vars: variable `{key}` is reserved for engine bindings"
                ),
            ));
        }
    }
    Ok(())
}

struct Capture<'a> {
    cache: BTreeMap<PathBuf, CapturedFile>,
    expected_digests: &'a BTreeMap<PathBuf, String>,
    policy: PolicyEngine,
    files: usize,
    bytes: u64,
    snapshot: SourceSnapshot,
}

impl Capture<'_> {
    fn tasks(
        &mut self,
        tasks: &mut [TaskDefinition],
        origin: &Path,
        scope: &str,
    ) -> Result<(), Box<Diagnostic>> {
        for task in tasks {
            self.merge_vars(
                &mut task.vars,
                &task.vars_files,
                origin,
                &format!("{scope}.{}", task.id),
            )?;
        }
        Ok(())
    }

    fn merge_vars(
        &mut self,
        inline: &mut JsonMap,
        files: &[String],
        origin: &Path,
        scope: &str,
    ) -> Result<(), Box<Diagnostic>> {
        validate_variables(inline, origin, scope)?;
        let mut merged = JsonMap::new();
        let mut winners = BTreeMap::new();
        for (index, path) in files.iter().enumerate() {
            let label = format!("{scope}.varsFiles[{index}]");
            let captured = self.read(path, origin, &label)?;
            let document: serde_yaml_ng::Value = serde_yaml_ng::from_str(&captured.content)
                .map_err(|e| {
                    let mut diagnostic = error(
                        Path::new(&captured.source),
                        &label,
                        "variable file must be a valid mapping with unique keys",
                    );
                    if let Some(location) = e.location() {
                        diagnostic = Box::new(
                            (*diagnostic).with_location(location.line(), location.column()),
                        );
                    }
                    diagnostic
                })?;
            let json = serde_json::to_value(document).map_err(|_| {
                error(
                    Path::new(&captured.source),
                    &label,
                    "variable file must contain JSON-compatible values and string keys",
                )
            })?;
            let values: JsonMap = json
                .as_object()
                .ok_or_else(|| {
                    error(
                        Path::new(&captured.source),
                        &label,
                        "variable file root must be a mapping",
                    )
                })?
                .clone()
                .into_iter()
                .collect();
            if values.contains_key("varsFiles")
                || values.contains_key("include")
                || values.contains_key("includes")
            {
                return Err(error(
                    Path::new(&captured.source),
                    &label,
                    "variable files cannot include other files; list every file in varsFiles",
                ));
            }
            validate_variables(&values, Path::new(&captured.source), &label)?;
            for (key, value) in values {
                winners.insert(key.clone(), format!("{label}:{}", captured.source));
                merged.insert(key, value);
            }
        }
        for (key, value) in inline.iter() {
            winners.insert(key.clone(), format!("{scope}.vars"));
            merged.insert(key.clone(), value.clone());
        }
        *inline = merged;
        if !winners.is_empty() {
            self.snapshot
                .resolution
                .origins
                .insert(scope.to_owned(), winners);
        }
        Ok(())
    }

    fn read(
        &mut self,
        requested: &str,
        origin: &Path,
        scope: &str,
    ) -> Result<CapturedFile, Box<Diagnostic>> {
        if self.files >= MAX_SOURCE_FILES {
            return Err(error(
                origin,
                scope,
                "configuration exceeds 256 source-file references",
            ));
        }
        let path = Path::new(requested);
        if path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
        {
            return Err(error(
                origin,
                scope,
                "source paths must not contain parent traversal (`..`)",
            ));
        }
        if requested.contains("${{") || requested.trim().is_empty() {
            return Err(error(
                origin,
                scope,
                "source paths must be nonempty literal paths",
            ));
        }
        let parent = origin
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let candidate = if path.is_absolute() {
            path.to_path_buf()
        } else {
            parent.join(path)
        };
        let absolute = if candidate.is_absolute() {
            candidate
        } else {
            std::env::current_dir()
                .map_err(|_| error(origin, scope, "cannot resolve source origin"))?
                .join(candidate)
        };
        if let Some(cached) = self.cache.get(&absolute) {
            self.files += 1;
            self.bytes = self.bytes.saturating_add(cached.content.len() as u64);
            if self.bytes > MAX_CAPTURE_BYTES {
                return Err(error(
                    origin,
                    scope,
                    "configuration source capture exceeds 16 MiB",
                ));
            }
            self.snapshot
                .file_digests
                .insert(scope.to_owned(), cached.digest.clone());
            return Ok(CapturedFile {
                source: requested.to_owned(),
                ..cached.clone()
            });
        }
        let resolved = self
            .policy
            .resolve_read_path(&absolute.display().to_string())
            .map_err(|e| {
                error(
                    origin,
                    scope,
                    &format!("source `{requested}` is not readable within policy: {e}"),
                )
            })?;
        let context = PolicyContext {
            run_id: "configuration".to_owned(),
            trace_id: "configuration".to_owned(),
            task_id: scope.to_owned(),
            agent: None,
            tool: "filesystem.read".to_owned(),
            capability: "observe".to_owned(),
            effect_class: EffectClass::Observe,
            risk: Risk::Low,
            resource: Some(resolved.display().to_string()),
            provider: None,
            input: serde_json::json!({"path": requested}),
            interactive: true,
        };
        if let PolicyDecision::Deny { reason } = self.policy.decide(&context) {
            return Err(error(
                origin,
                scope,
                &format!("source `{requested}` denied by read policy: {reason}"),
            ));
        }
        let mut file = open_regular(&resolved).map_err(|e| {
            error(
                origin,
                scope,
                &format!("source `{requested}` cannot be opened as a regular file: {e}"),
            )
        })?;
        let before = file
            .metadata()
            .map_err(|_| error(origin, scope, "cannot inspect opened source"))?;
        if !before.is_file() || before.len() > MAX_SOURCE_BYTES {
            return Err(error(
                origin,
                scope,
                &format!(
                    "source `{requested}` must be a regular UTF-8 file of at most 1048576 bytes"
                ),
            ));
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(MAX_SOURCE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| {
                error(
                    origin,
                    scope,
                    &format!("source `{requested}` cannot be read"),
                )
            })?;
        let after = file
            .metadata()
            .map_err(|_| error(origin, scope, "cannot recheck opened source"))?;
        if bytes.len() as u64 > MAX_SOURCE_BYTES
            || before.len() != after.len()
            || before.modified().ok() != after.modified().ok()
            || before.len() != bytes.len() as u64
        {
            return Err(error(
                origin,
                scope,
                &format!(
                    "source `{requested}` changed while being captured or exceeds 1048576 bytes; retry with a stable file"
                ),
            ));
        }
        self.bytes = self.bytes.saturating_add(bytes.len() as u64);
        if self.bytes > MAX_CAPTURE_BYTES {
            return Err(error(
                origin,
                scope,
                "configuration source capture exceeds 16 MiB",
            ));
        }
        self.files += 1;
        let digest = format!("sha256:{}", hex::encode(Sha256::digest(&bytes)));
        if let Some(expected) = self
            .expected_digests
            .get(&absolute)
            .or_else(|| self.expected_digests.get(&resolved))
        {
            if &digest != expected {
                return Err(error(
                    origin,
                    scope,
                    &format!(
                        "source `{requested}` integrity digest does not match its pack manifest"
                    ),
                ));
            }
        }
        let content = String::from_utf8(bytes).map_err(|_| {
            error(
                origin,
                scope,
                &format!("source `{requested}` must use UTF-8 encoding"),
            )
        })?;
        self.snapshot
            .file_digests
            .insert(scope.to_owned(), digest.clone());
        let captured = CapturedFile {
            source: requested.to_owned(),
            digest,
            content,
        };
        self.cache.insert(absolute, captured.clone());
        Ok(captured)
    }
}

/// Inspect a normalized absolute regular-file path without reading its contents.
/// Symlinks and special files are rejected by the same handle-relative walk
/// used for captured workflow sources (nonblocking descriptors on Unix).
pub fn regular_file_metadata(path: &Path) -> std::io::Result<std::fs::Metadata> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "expected a normalized absolute path",
        ));
    }
    open_regular(path)?.metadata()
}

#[cfg(unix)]
pub(crate) fn open_regular(path: &Path) -> std::io::Result<File> {
    use nix::fcntl::{OFlag, openat};
    use nix::sys::stat::Mode;
    let mut directory = File::open("/")?;
    let parts = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part),
            _ => None,
        })
        .collect::<Vec<_>>();
    for (index, part) in parts.iter().enumerate() {
        let mut flags = OFlag::O_RDONLY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK;
        if index + 1 != parts.len() {
            flags |= OFlag::O_DIRECTORY;
        }
        directory = File::from(
            openat(&directory, Path::new(part), flags, Mode::empty())
                .map_err(std::io::Error::from)?,
        );
    }
    if !directory.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    Ok(directory)
}

#[cfg(windows)]
pub(crate) fn open_regular(path: &Path) -> std::io::Result<File> {
    open_regular_windows(path, |_, _, _| {})
}

#[cfg(windows)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceOpenCheckpoint {
    BeforeChild,
    AfterChild,
}

#[cfg(windows)]
fn open_regular_windows(
    path: &Path,
    mut checkpoint: impl FnMut(SourceOpenCheckpoint, &Path, &File),
) -> std::io::Result<File> {
    use cap_fs_ext::OpenOptionsFollowExt;
    use cap_primitives::fs::{FollowSymlinks, OpenOptions, open, open_dir_nofollow};
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    let invalid = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "source must be a normalized absolute regular-file path without reparse points",
        )
    };
    if !path.is_absolute() {
        return Err(invalid());
    }
    let mut root = PathBuf::new();
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir if parts.is_empty() => root.push(component),
            Component::Normal(part) => parts.push(part),
            _ => return Err(invalid()),
        }
    }
    if parts.is_empty() {
        return Err(invalid());
    }

    // Only the volume/share root uses an ambient pathname. Every descendant
    // opens one component relative to an already opened directory handle.
    // Do not replace this with full-path opens plus canonicalize checks:
    // intermediate junctions can be swapped and restored between those calls.
    // Sharing restrictions alone are also insufficient: Windows permits some
    // reparse mutations through attribute-only handles.
    let root_handle =
        cap_primitives::fs::open_ambient_dir(&root, cap_primitives::ambient_authority())?;
    let metadata = root_handle.metadata()?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(invalid());
    }
    // Retain ancestors until the final handle is obtained. cap-primitives
    // disallows directory delete sharing; NtCreateFile's RootDirectory is the
    // opened parent handle, so an in-place junction mutation cannot redirect
    // the following operation through a freshly resolved absolute pathname.
    let mut directories = vec![root_handle];
    let mut current_path = root;
    for (index, part) in parts.iter().enumerate() {
        let parent = directories.last().expect("root handle is retained");
        let is_last = index + 1 == parts.len();
        current_path.push(part);
        checkpoint(SourceOpenCheckpoint::BeforeChild, &current_path, parent);
        let result = if is_last {
            let mut options = OpenOptions::new();
            options.read(true).follow(FollowSymlinks::No);
            open(parent, Path::new(part), &options)
        } else {
            open_dir_nofollow(parent, Path::new(part))
        };
        // The no-op production callback lets platform tests stage and restore
        // mutations around exactly the open operation, without timing loops.
        checkpoint(SourceOpenCheckpoint::AfterChild, &current_path, parent);
        let child = result?;
        let metadata = child.metadata()?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || (is_last && !metadata.is_file())
            || (!is_last && !metadata.is_dir())
        {
            return Err(invalid());
        }
        if is_last {
            return Ok(child);
        }
        directories.push(child);
    }
    Err(invalid())
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn open_regular(_path: &Path) -> std::io::Result<File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "race-safe configuration source capture is unsupported on this platform",
    ))
}

fn error(origin: &Path, scope: &str, message: &str) -> Box<Diagnostic> {
    Box::new(Diagnostic::error(DiagnosticCode::SchemaViolation, &origin.display().to_string(), message)
        .with_path(scope).with_help("source files resolve from their declaring workflow or pack; keep them within policy.workspaceRoot and grant filesystem.read"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{compile, parse_workflow};
    use serde_json::{Value, json};
    use tempfile::tempdir;

    fn workflow(extra: &str) -> Workflow {
        parse_workflow(&format!("apiVersion: agentctl.dev/v1\nkind: Workflow\nmetadata: {{ name: sources }}\nspec:\n{extra}\n"), "workflow.yaml").expect("workflow parses").workflow
    }

    fn resolve(workflow: &mut Workflow, root: &Path) -> Result<SourceResolution, Vec<Diagnostic>> {
        resolve_workflow_sources(
            workflow,
            &root.join("workflow.yaml"),
            root,
            &VariableOverrides::default(),
            &SourceOrigins::default(),
        )
    }

    #[test]
    fn regular_metadata_requires_absolute_regular_file() {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = std::fs::canonicalize(directory.path()).expect("canonical root");
        let file = root.join("secret.txt");
        std::fs::write(&file, "metadata only").expect("file");
        assert_eq!(regular_file_metadata(&file).expect("metadata").len(), 13);
        assert!(regular_file_metadata(Path::new("relative.txt")).is_err());
        assert!(regular_file_metadata(&root.join("../secret.txt")).is_err());
        assert!(regular_file_metadata(&root).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(&file, root.join("link")).expect("symlink");
            assert!(regular_file_metadata(&root.join("link")).is_err());
            nix::unistd::mkfifo(&root.join("fifo"), nix::sys::stat::Mode::S_IRUSR).expect("fifo");
            assert!(regular_file_metadata(&root.join("fifo")).is_err());
        }
    }

    #[test]
    fn ordered_files_replace_whole_keys_and_keep_input_namespace_separate() {
        let dir = tempdir().unwrap();
        for (name, content) in [
            (
                "base.yaml",
                "winner: base\nobject: { old: true, keep: false }\narray: [1,2]\nnullable: old\nretained: true\n",
            ),
            (
                "later.yaml",
                "winner: later\nobject: { new: true }\narray: [3]\nnullable: null\n",
            ),
            ("agent.yaml", "winner: agent-file\nagentOnly: yes\n"),
            ("task.yaml", "winner: task-file\n"),
            ("override.yaml", "winner: invocation-file\n"),
        ] {
            std::fs::write(dir.path().join(name), content).unwrap();
        }
        let mut w = workflow(
            "  varsFiles: [base.yaml, later.yaml]\n  vars: { winner: workflow-inline }\n  inputs: { winner: input-default }\n  providers: { fake: { kind: fake } }\n  agents:\n    review:\n      provider: fake\n      model: fake\n      instructions: '${{ vars.winner }} / ${{ inputs.winner }}'\n      varsFiles: [agent.yaml]\n      vars: { winner: agent-inline }\n  tasks:\n    - id: first\n      uses: agent:review\n      varsFiles: [task.yaml]\n      vars: { winner: task-inline }\n",
        );
        let overrides = VariableOverrides {
            files: vec![dir.path().join("override.yaml")],
            values: BTreeMap::from([("winner".to_owned(), json!("invocation-inline"))]),
        };
        let result = resolve_workflow_sources(
            &mut w,
            &dir.path().join("workflow.yaml"),
            dir.path(),
            &overrides,
            &SourceOrigins::default(),
        )
        .unwrap();
        let plan = compile(&w, "workflow.yaml").unwrap();
        let vars = &plan.tasks["first"].vars;
        assert_eq!(vars["winner"], json!("invocation-inline"));
        assert_eq!(vars["object"], json!({"new":true}));
        assert_eq!(vars["array"], json!([3]));
        assert_eq!(vars["nullable"], Value::Null);
        assert_eq!(vars["retained"], json!(true));
        assert_eq!(w.spec.inputs["winner"], json!("input-default"));
        let diagnostics = serde_json::to_string(&result).unwrap();
        assert_eq!(
            result.effective_origins["spec.tasks.first"]["winner"],
            "invocation:--var"
        );
        assert!(diagnostics.contains("invocation:--var"));
        assert!(!diagnostics.contains("invocation-inline"));
        assert!(!diagnostics.contains("input-default"));
    }

    #[test]
    fn captured_instruction_bytes_are_immutable_and_change_plan_identity() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("instructions.txt");
        std::fs::write(&path, "review ${{ inputs.name }}").unwrap();
        let original = workflow(
            "  providers: { fake: { kind: fake } }\n  agents:\n    review: { provider: fake, model: fake, instructionsFile: instructions.txt }\n  tasks: [{ id: first, uses: 'agent:review' }]",
        );
        let mut first = original.clone();
        resolve(&mut first, dir.path()).unwrap();
        let first_plan = compile(&first, "workflow.yaml").unwrap();
        std::fs::write(&path, "changed bytes").unwrap();
        resolve(&mut first, dir.path()).unwrap();
        assert_eq!(
            instruction_text(&first, "review"),
            Some("review ${{ inputs.name }}")
        );
        let mut changed = original;
        resolve(&mut changed, dir.path()).unwrap();
        assert_ne!(
            first_plan.workflow_digest,
            compile(&changed, "workflow.yaml").unwrap().workflow_digest
        );
        std::fs::remove_file(&path).unwrap();
        let persisted: Workflow =
            serde_json::from_value(serde_json::to_value(&first).unwrap()).unwrap();
        assert_eq!(compile(&persisted, "workflow.yaml").unwrap(), first_plan);
        assert!(parse_workflow(&serde_yaml_ng::to_string(&first).unwrap(), "user.yaml").is_err());
        let legacy =
            "playbook: forbidden-snapshot\nsourceSnapshot: { formatVersion: 1 }\ntasks: []\n";
        assert!(
            parse_workflow(legacy, "legacy.yaml").unwrap_err()[0]
                .message
                .contains("reserved")
        );
    }

    #[test]
    fn source_origins_are_independent_from_workspace_and_pack_integrity_binds_bytes() {
        let dir = tempdir().unwrap();
        let config = dir.path().join("config");
        let pack = dir.path().join("pack");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(&pack).unwrap();
        std::fs::write(config.join("prompt.txt"), "caller").unwrap();
        std::fs::write(pack.join("prompt.txt"), "pack").unwrap();
        let mut w = workflow(
            "  providers: { fake: { kind: fake } }\n  agents:\n    packed: { provider: fake, model: fake, instructionsFile: prompt.txt }\n  tasks: [{ id: first, uses: 'agent:packed' }]",
        );
        let mut origins = SourceOrigins::default();
        origins
            .agents
            .insert("packed".to_owned(), pack.join("pack.yaml"));
        origins.files.insert(
            pack.join("prompt.txt"),
            format!("sha256:{}", hex::encode(Sha256::digest(b"pack"))),
        );
        resolve_workflow_sources(
            &mut w,
            &config.join("workflow.yaml"),
            dir.path(),
            &VariableOverrides::default(),
            &origins,
        )
        .unwrap();
        assert_eq!(instruction_text(&w, "packed"), Some("pack"));
        w.spec.source_snapshot = None;
        std::fs::write(pack.join("prompt.txt"), "tampered").unwrap();
        assert!(
            resolve_workflow_sources(
                &mut w,
                &config.join("workflow.yaml"),
                dir.path(),
                &VariableOverrides::default(),
                &origins
            )
            .unwrap_err()[0]
                .message
                .contains("integrity digest")
        );
    }

    #[test]
    fn source_denials_fail_before_capture_and_do_not_mutate_input_workflow() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("vars.yaml"),
            "secretLookingData: never-print-this-value",
        )
        .unwrap();
        let mut w = workflow(
            "  varsFiles: [vars.yaml]\n  policy: { toolsDeny: [filesystem.read] }\n  actions: { assign: { kind: builtin.assign } }\n  tasks: [{ id: first, uses: 'action:assign' }]",
        );
        let original = w.clone();
        let errors = resolve(&mut w, dir.path()).unwrap_err();
        assert!(errors[0].message.contains("denied by read policy"));
        assert!(!format!("{errors:?}").contains("never-print-this-value"));
        assert_eq!(w, original);
    }

    #[test]
    fn source_types_encoding_size_duplicate_keys_and_includes_fail_closed() {
        let dir = tempdir().unwrap();
        let base = workflow(
            "  varsFiles: [vars.yaml]\n  actions: { assign: { kind: builtin.assign } }\n  tasks: [{ id: first, uses: 'action:assign' }]",
        );
        for content in [
            b"[1,2]".to_vec(),
            b"a: 1\na: 2".to_vec(),
            b"nested: { a: 1, a: 2 }".to_vec(),
            b"include: next.yaml".to_vec(),
            vec![0xff],
            vec![b'a'; MAX_SOURCE_BYTES as usize + 1],
        ] {
            std::fs::write(dir.path().join("vars.yaml"), content).unwrap();
            assert!(resolve(&mut base.clone(), dir.path()).is_err());
        }
        std::fs::remove_file(dir.path().join("vars.yaml")).unwrap();
        assert!(resolve(&mut base.clone(), dir.path()).is_err());
        std::fs::create_dir(dir.path().join("vars.yaml")).unwrap();
        assert!(resolve(&mut base.clone(), dir.path()).is_err());
    }

    #[cfg(unix)]
    fn assert_unreadable_source_rejected(filename: &str, config: &str, scope: &str) {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempdir().expect("workspace");
        let source = directory.path().join(filename);
        std::fs::write(&source, "value: PRIVATE_SOURCE_CONTENT").expect("regular source");
        let original = workflow(config);
        resolve(&mut original.clone(), directory.path()).expect("readable fixture resolves");
        let permissions = std::fs::metadata(&source).expect("metadata").permissions();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o000))
            .expect("remove file access permissions");
        let permission_probe = File::open(&source);
        if permission_probe.is_ok() {
            std::fs::set_permissions(&source, permissions).expect("restore permissions");
            eprintln!(
                "SKIPPED OS permission-denial assertion for {filename}: this identity can open a mode-000 file (for example root or CAP_DAC_OVERRIDE); this run provides no unreadable-file evidence"
            );
            return;
        }
        let mut attempted = original.clone();
        let result = resolve(&mut attempted, directory.path());
        std::fs::set_permissions(&source, permissions).expect("restore permissions");
        assert_eq!(
            permission_probe
                .expect_err("unreadable regular file")
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );
        let diagnostics = result.expect_err("permission denial must prevent capture");
        assert_eq!(diagnostics[0].path.as_deref(), Some(scope));
        assert!(diagnostics[0].message.contains(filename));
        assert!(diagnostics[0].message.contains("cannot be opened"));
        assert!(diagnostics[0].help.is_some());
        assert!(!format!("{diagnostics:?}").contains("PRIVATE_SOURCE_CONTENT"));
        assert_eq!(attempted, original, "failed capture must be atomic");
    }

    #[cfg(unix)]
    #[test]
    fn source_unreadable_instruction_file_fails_before_capture() {
        assert_unreadable_source_rejected(
            "instructions.txt",
            "  providers: { fake: { kind: fake } }\n  agents:\n    review: { provider: fake, model: fake, instructionsFile: instructions.txt }\n  tasks: [{ id: first, uses: 'agent:review' }]",
            "spec.agents.review.instructionsFile",
        );
    }

    #[cfg(unix)]
    #[test]
    fn source_unreadable_variable_file_fails_before_capture() {
        assert_unreadable_source_rejected(
            "vars.yaml",
            "  varsFiles: [vars.yaml]\n  actions: { assign: { kind: builtin.assign } }\n  tasks: [{ id: first, uses: 'action:assign' }]",
            "spec.varsFiles[0]",
        );
    }

    #[cfg(windows)]
    #[test]
    fn source_windows_opened_parent_cannot_be_renamed_during_capture() {
        let directory = tempdir().expect("workspace");
        let parent = directory.path().join("parent");
        let moved = directory.path().join("moved");
        std::fs::create_dir(&parent).expect("parent");
        let source = parent.join("source.txt");
        std::fs::write(&source, "authorized source").expect("source");
        let normalized = std::fs::canonicalize(&source).expect("canonical source");
        let mut attempted = false;
        let mut opened = open_regular_windows(&normalized, |phase, child, _| {
            if phase == SourceOpenCheckpoint::BeforeChild && child == normalized {
                let error =
                    std::fs::rename(&parent, &moved).expect_err("opened ancestor must deny rename");
                assert!(matches!(error.raw_os_error(), Some(5 | 32)));
                attempted = true;
            }
        })
        .expect("original source remains readable");
        let mut content = String::new();
        opened.read_to_string(&mut content).expect("captured bytes");
        assert!(attempted);
        assert_eq!(content, "authorized source");
        drop(opened);
        std::fs::rename(&parent, &moved).expect("fixture can rename after handles are released");
    }

    #[cfg(windows)]
    #[test]
    fn source_windows_junction_swap_and_restore_cannot_redirect_component_open() {
        let directory = tempdir().expect("workspace");
        let outside = tempdir().expect("outside workspace");
        let parent = directory.path().join("parent");
        let saved = directory.path().join("saved");
        std::fs::create_dir(&parent).expect("parent");
        std::fs::write(parent.join("source.txt"), "authorized source").expect("source");
        std::fs::write(outside.path().join("source.txt"), "PRIVATE_OUTSIDE_CONTENT")
            .expect("outside source");
        let normalized =
            std::fs::canonicalize(parent.join("source.txt")).expect("canonical source");
        let normalized_parent = normalized.parent().expect("parent");
        let mut replaced = false;
        let mut restored = false;
        let result = open_regular_windows(&normalized, |phase, child, _| {
            if child != normalized_parent {
                return;
            }
            if phase == SourceOpenCheckpoint::BeforeChild {
                std::fs::rename(&parent, &saved).expect("swap directory before it is opened");
                let output = std::process::Command::new("cmd")
                    .args(["/d", "/c", "mklink", "/J"])
                    .arg(&parent)
                    .arg(outside.path())
                    .output()
                    .expect("junction fixture");
                assert!(
                    output.status.success(),
                    "junction fixture failed: {output:?}"
                );
                assert_eq!(
                    std::fs::read(parent.join("source.txt")).expect("junction is active"),
                    b"PRIVATE_OUTSIDE_CONTENT"
                );
                replaced = true;
            } else {
                std::fs::remove_dir(&parent).expect("remove junction only");
                std::fs::rename(&saved, &parent).expect("restore original path before postcheck");
                restored = true;
            }
        });
        assert!(
            replaced && restored,
            "both deterministic race checkpoints ran"
        );
        assert!(
            result.is_err(),
            "the intermediate junction must never be followed"
        );
        assert_eq!(
            std::fs::read(parent.join("source.txt")).unwrap(),
            b"authorized source"
        );
        assert_eq!(
            std::fs::canonicalize(parent.join("source.txt")).unwrap(),
            normalized
        );
    }

    #[cfg(windows)]
    #[test]
    fn source_windows_in_place_reparse_mutation_cannot_redirect_opened_parent() {
        use std::os::windows::fs::MetadataExt;

        let directory = tempdir().expect("workspace");
        let outside = tempdir().expect("outside workspace");
        let parent = directory.path().join("parent");
        std::fs::create_dir(&parent).expect("parent");
        let source = parent.join("source.txt");
        std::fs::write(&source, "authorized source").expect("source");
        std::fs::write(outside.path().join("source.txt"), "PRIVATE_OUTSIDE_CONTENT")
            .expect("outside source");
        let script = directory.path().join("mutate-junction.ps1");
        std::fs::write(&script, WINDOWS_REPARSE_FIXTURE).expect("fixture helper");
        let normalized = std::fs::canonicalize(&source).expect("canonical source");
        let mut mutated = false;
        let mut restored = false;
        let result = open_regular_windows(&normalized, |phase, child, opened_parent| {
            if child != normalized {
                return;
            }
            let operation = if phase == SourceOpenCheckpoint::BeforeChild {
                // NTFS permits setting a junction on an existing empty
                // directory. Keep its handle open and mutate that same object.
                std::fs::remove_file(&source).expect("empty the held directory");
                "set"
            } else {
                "delete"
            };
            let output = std::process::Command::new("powershell.exe")
                .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-File"])
                .arg(&script)
                .arg(operation)
                .arg(&parent)
                .arg(outside.path())
                .output()
                .expect("attribute-only reparse fixture helper");
            assert!(
                output.status.success(),
                "in-place reparse {operation} fixture failed; no race evidence: {output:?}"
            );
            let attributes = opened_parent
                .metadata()
                .expect("same opened directory")
                .file_attributes();
            if phase == SourceOpenCheckpoint::BeforeChild {
                assert_ne!(
                    attributes & 0x0000_0400,
                    0,
                    "the opened object itself became a junction"
                );
                assert_eq!(std::fs::read(&source).unwrap(), b"PRIVATE_OUTSIDE_CONTENT");
                mutated = true;
            } else {
                assert_eq!(attributes & 0x0000_0400, 0, "the same object was restored");
                std::fs::write(&source, "authorized source").expect("restore original file");
                restored = true;
            }
        });
        assert!(
            mutated && restored,
            "both deterministic mutation checkpoints ran"
        );
        assert!(
            result.is_err(),
            "a held parent must not redirect to junction target bytes"
        );
        assert_eq!(std::fs::read(&source).unwrap(), b"authorized source");
        assert_eq!(std::fs::canonicalize(&source).unwrap(), normalized);
    }

    // Only used on disposable test directories. FILE_WRITE_ATTRIBUTES (0x100)
    // deliberately proves why deny-write sharing is not a reparse defense.
    // No Rust unsafe-code exception or privileged symlink creation is needed.
    #[cfg(windows)]
    const WINDOWS_REPARSE_FIXTURE: &str = r#"
param([string]$Operation, [string]$SourceDirectory, [string]$Destination)
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32.SafeHandles;
public static class ReparseFixture {
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern SafeFileHandle CreateFileW(string path, uint access, uint share,
        IntPtr security, uint disposition, uint flags, IntPtr template);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool DeviceIoControl(SafeFileHandle handle, uint code,
        byte[] input, uint length, IntPtr output, uint outputLength,
        out uint returned, IntPtr overlapped);
    public static void Change(string operation, string directory, string target) {
        byte[] buffer;
        uint control;
        using (var stream = new MemoryStream()) {
            using (var writer = new BinaryWriter(stream, Encoding.Unicode, true)) {
                writer.Write((uint)0xA0000003);
                if (operation == "set") {
                    string full = Path.GetFullPath(target);
                    string substitute = full.StartsWith(@"\\?\")
                        ? @"\??\" + full.Substring(4) : @"\??\" + full;
                    byte[] sub = Encoding.Unicode.GetBytes(substitute);
                    byte[] print = Encoding.Unicode.GetBytes(full);
                    writer.Write((ushort)(8 + sub.Length + 2 + print.Length + 2));
                    writer.Write((ushort)0);
                    writer.Write((ushort)0);
                    writer.Write((ushort)sub.Length);
                    writer.Write((ushort)(sub.Length + 2));
                    writer.Write((ushort)print.Length);
                    writer.Write(sub); writer.Write((ushort)0);
                    writer.Write(print); writer.Write((ushort)0);
                    control = 0x000900A4;
                } else if (operation == "delete") {
                    writer.Write((ushort)0); writer.Write((ushort)0);
                    control = 0x000900AC;
                } else { throw new ArgumentException("unknown fixture operation"); }
            }
            buffer = stream.ToArray();
        }
        using (var handle = CreateFileW(directory, 0x100, 7, IntPtr.Zero, 3,
                                       0x02200000, IntPtr.Zero)) {
            if (handle.IsInvalid) throw new Win32Exception(Marshal.GetLastWin32Error());
            uint returned;
            if (!DeviceIoControl(handle, control, buffer, (uint)buffer.Length,
                                 IntPtr.Zero, 0, out returned, IntPtr.Zero))
                throw new Win32Exception(Marshal.GetLastWin32Error());
        }
    }
}
'@
[ReparseFixture]::Change($Operation, $SourceDirectory, $Destination)
"#;

    #[cfg(windows)]
    #[test]
    fn source_windows_junction_escape_is_rejected_before_capture() {
        let directory = tempdir().expect("workspace");
        let outside = tempdir().expect("outside workspace");
        for filename in ["vars.yaml", "instructions.txt"] {
            std::fs::write(
                outside.path().join(filename),
                "value: PRIVATE_OUTSIDE_CONTENT",
            )
            .expect("outside regular file");
        }
        let junction = directory.path().join("escape");
        // Directory junctions on the local NTFS test volume do not require the
        // symlink privilege or Developer Mode. Failure to create the fixture is
        // a failed prerequisite, never evidence of successful escape rejection.
        let created = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&junction)
            .arg(outside.path())
            .output()
            .expect("Windows cmd creates local NTFS junction fixture");
        assert!(
            created.status.success(),
            "junction fixture prerequisite failed; no escape coverage: {}",
            String::from_utf8_lossy(&created.stderr)
        );
        let normalized_root = std::fs::canonicalize(directory.path()).expect("canonical workspace");
        for (filename, config, scope) in [
            (
                "vars.yaml",
                "  varsFiles: [escape/vars.yaml]\n  actions: { assign: { kind: builtin.assign } }\n  tasks: [{ id: first, uses: 'action:assign' }]",
                "spec.varsFiles[0]",
            ),
            (
                "instructions.txt",
                "  providers: { fake: { kind: fake } }\n  agents:\n    review: { provider: fake, model: fake, instructionsFile: escape/instructions.txt }\n  tasks: [{ id: first, uses: 'agent:review' }]",
                "spec.agents.review.instructionsFile",
            ),
        ] {
            assert_eq!(
                std::fs::read(junction.join(filename)).expect("junction reaches real outside file"),
                b"value: PRIVATE_OUTSIDE_CONTENT"
            );
            assert!(regular_file_metadata(&normalized_root.join("escape").join(filename)).is_err());
            let original = workflow(config);
            let mut attempted = original.clone();
            let diagnostics = resolve(&mut attempted, directory.path())
                .expect_err("junction must not grant access outside the workspace");
            assert_eq!(diagnostics[0].path.as_deref(), Some(scope));
            assert!(
                diagnostics[0]
                    .message
                    .contains("escapes the authorized root")
            );
            assert!(!format!("{diagnostics:?}").contains("PRIVATE_OUTSIDE_CONTENT"));
            assert_eq!(attempted, original);
        }
        std::fs::remove_dir(&junction).expect("remove junction without removing its target");
        assert!(outside.path().join("vars.yaml").is_file());
    }

    #[test]
    fn reserved_bindings_are_rejected_but_ordinary_data_does_not_dereference_secrets() {
        let dir = tempdir().unwrap();
        for source in [
            "loopIndex: 3",
            "loopPrevious: old",
            "matrix: other",
            "foreachIndex: 1",
            "matrixIndex: 1",
        ] {
            std::fs::write(dir.path().join("vars.yaml"), source).unwrap();
            let mut w = workflow(
                "  varsFiles: [vars.yaml]\n  actions: { assign: { kind: builtin.assign } }\n  tasks: [{ id: first, uses: 'action:assign' }]",
            );
            assert!(resolve(&mut w, dir.path()).is_err());
        }
        let w = workflow(
            "  vars: { data: { env: MISSING_EXPLICITLY_NOT_IMPORTED }, fixture: { file: build.log } }\n  actions: { assign: { kind: builtin.assign } }\n  tasks: [{ id: first, uses: 'action:assign', with: { value: '${{ vars.data }}' } }]",
        );
        assert_eq!(
            compile(&w, "workflow.yaml").unwrap().tasks["first"].vars["data"],
            json!({"env":"MISSING_EXPLICITLY_NOT_IMPORTED"})
        );
    }

    #[test]
    fn compiler_rejects_uncaptured_vars_and_instruction_dependency_escalation() {
        let w = workflow(
            "  varsFiles: [vars.yaml]\n  actions: { assign: { kind: builtin.assign } }\n  tasks: [{ id: first, uses: 'action:assign' }]",
        );
        assert!(
            compile(&w, "workflow.yaml").unwrap_err()[0]
                .message
                .contains("must be captured")
        );
        let w = workflow(
            "  providers: { fake: { kind: fake } }\n  agents:\n    review: { provider: fake, model: fake, instructions: '${{ tasks.other.output }}' }\n  tasks: [{ id: first, uses: 'agent:review' }]",
        );
        assert!(compile(&w, "workflow.yaml").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn source_symlink_escape_and_fifo_are_rejected_without_blocking() {
        use std::os::unix::fs::symlink;
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        std::fs::write(outside.path().join("vars.yaml"), "value: outside").unwrap();
        symlink(
            outside.path().join("vars.yaml"),
            dir.path().join("vars.yaml"),
        )
        .unwrap();
        let w = workflow(
            "  varsFiles: [vars.yaml]\n  actions: { assign: { kind: builtin.assign } }\n  tasks: [{ id: first, uses: 'action:assign' }]",
        );
        assert!(resolve(&mut w.clone(), dir.path()).is_err());
        std::fs::remove_file(dir.path().join("vars.yaml")).unwrap();
        nix::unistd::mkfifo(&dir.path().join("vars.yaml"), nix::sys::stat::Mode::S_IRUSR).unwrap();
        assert!(resolve(&mut w.clone(), dir.path()).is_err());
    }
}
