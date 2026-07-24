use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::process::{bounded_output, output_diagnostics};

mod acceptance;
mod process;

fn main() -> Result<()> {
    let command = env::args().nth(1).unwrap_or_else(|| "help".to_owned());
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("xtask must be inside the workspace")?
        .to_path_buf();
    match command.as_str() {
        "verify" => verify(&root),
        "docs-verify" => docs_verify(&root),
        "migration-verify" => migration_verify(&root),
        "acceptance" => acceptance::run(&root),
        "acceptance-container" => acceptance::container(&root),
        "acceptance-live-openai" => acceptance::live_openai(&root),
        "examples-verify" => examples_verify(&root),
        "examples-verify-live-openai" => acceptance::examples_live_openai(&root),
        "generate" => generate(&root),
        "package" => package(&root),
        "secret-scan" => {
            verify_no_secrets(&root)?;
            verify_workflow_action_pins(&root)
        }
        "help" | "--help" | "-h" => {
            println!(
                "cargo xtask verify\ncargo xtask docs-verify\ncargo xtask migration-verify\ncargo xtask acceptance\ncargo xtask acceptance-container\ncargo xtask acceptance-live-openai\ncargo xtask examples-verify\ncargo xtask examples-verify-live-openai\ncargo xtask generate\ncargo xtask package\ncargo xtask secret-scan"
            );
            Ok(())
        }
        other => bail!("unknown xtask command `{other}`"),
    }
}

fn migration_verify(root: &Path) -> Result<()> {
    run(
        root,
        "cargo",
        &[
            "test",
            "-p",
            "agentctl-store",
            "upgrades_every_retained_database_schema_fixture",
            "--locked",
        ],
    )
}

fn docs_verify(root: &Path) -> Result<()> {
    println!("[1/6] build documentation test binary");
    run(root, "cargo", &["build", "-p", "agentctl-cli", "--locked"])?;

    println!("[2/6] generated CLI and schema freshness");
    verify_generated(root)?;

    println!("[3/6] canonical v1 examples");
    verify_examples(root)?;

    println!("[4/6] documentation journey examples");
    verify_docs_examples(root)?;

    println!("[5/6] public writing and source inclusion");
    verify_public_documentation(root)?;

    println!("[6/6] local Markdown links");
    verify_markdown_links(root)?;

    println!("agentctl documentation verification passed");
    Ok(())
}

pub(crate) fn package(root: &Path) -> Result<()> {
    run(
        root,
        "cargo",
        &["build", "--release", "-p", "agentctl-cli", "--locked"],
    )?;
    let mut rustc = Command::new("rustc");
    rustc.arg("-vV");
    let host_output = bounded_output(rustc, "rustc -vV")?;
    ensure_success(&host_output, "rustc -vV")?;
    let version = String::from_utf8_lossy(&host_output.stdout);
    let host = version
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .context("rustc did not report a host target")?;
    let package = root
        .join("dist")
        .join(format!("agentctl-{}-{host}", env!("CARGO_PKG_VERSION")));
    fs::create_dir_all(&package)?;
    let source = root.join("target").join("release").join(if cfg!(windows) {
        "agentctl.exe"
    } else {
        "agentctl"
    });
    let binary = package.join(source.file_name().context("release binary name")?);
    fs::copy(&source, &binary)?;
    fs::copy(root.join("LICENSE"), package.join("LICENSE"))?;
    fs::copy(root.join("README.md"), package.join("README.md"))?;
    for (shell, name) in [
        ("bash", "agentctl.bash"),
        ("zsh", "_agentctl"),
        ("fish", "agentctl.fish"),
        ("powershell", "_agentctl.ps1"),
    ] {
        let mut completion = Command::new(&binary);
        completion.args(["completion", shell]);
        let output = bounded_output(completion, "agentctl completion")?;
        ensure_success(&output, "agentctl completion")?;
        fs::write(package.join(name), output.stdout)?;
    }
    let digest = hex::encode(Sha256::digest(fs::read(&binary)?));
    fs::write(
        package.join("SHA256SUMS"),
        format!(
            "{digest}  {}\n",
            binary.file_name().context("binary name")?.to_string_lossy()
        ),
    )?;
    println!("packaged {}", package.display());
    Ok(())
}

fn verify(root: &Path) -> Result<()> {
    println!("[1/12] rustfmt");
    run(root, "cargo", &["fmt", "--all", "--", "--check"])?;
    run(
        root,
        "cargo",
        &["fmt", "--manifest-path", "fuzz/Cargo.toml", "--", "--check"],
    )?;

    println!("[2/12] clippy (warnings denied)");
    run(
        root,
        "cargo",
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ],
    )?;

    println!("[3/12] production workspace build");
    run(
        root,
        "cargo",
        &[
            "build",
            "--workspace",
            "--exclude",
            "xtask",
            "--all-features",
            "--locked",
        ],
    )?;

    println!(
        "[4/12] unit, integration, compatibility, provider, protocol, persistence, and security tests"
    );
    run(
        root,
        "cargo",
        &["test", "--workspace", "--all-features", "--locked"],
    )?;
    run(
        root,
        "cargo",
        &[
            "check",
            "--manifest-path",
            "fuzz/Cargo.toml",
            "--bins",
            "--locked",
        ],
    )?;

    println!("[5/12] documentation tests and docs");
    run_with_env(
        root,
        "cargo",
        &["test", "--doc", "--workspace", "--locked"],
        &[("RUSTDOCFLAGS", "-D warnings")],
    )?;
    run_with_env(
        root,
        "cargo",
        &[
            "doc",
            "--workspace",
            "--no-deps",
            "--all-features",
            "--locked",
        ],
        &[("RUSTDOCFLAGS", "-D warnings")],
    )?;

    println!("[6/12] generated schema and CLI reference consistency");
    verify_generated(root)?;

    println!("[7/12] examples, inventory, and negative contracts");
    verify_examples(root)?;
    verify_example_matrix(root)?;

    println!("[8/12] dependency sources and license metadata");
    verify_metadata(root)?;

    println!("[9/12] dependency advisories and policy");
    verify_supply_chain(root)?;

    println!("[10/12] deterministic secret and workflow action-pin scan");
    verify_no_secrets(root)?;
    verify_workflow_action_pins(root)?;

    println!("[11/12] source installation smoke");
    verify_install(root)?;

    println!("[12/12] repository production boundary");
    verify_production_boundary(root)?;

    println!("agentctl verification passed");
    Ok(())
}

fn examples_verify(root: &Path) -> Result<()> {
    run(root, "cargo", &["build", "-p", "agentctl-cli", "--locked"])?;
    verify_example_matrix(root)?;
    verify_examples(root)?;
    verify_docs_examples(root)?;
    verify_markdown_links(root)?;
    println!("agentctl credential-free example verification passed");
    Ok(())
}

fn generate(root: &Path) -> Result<()> {
    run(root, "cargo", &["build", "-p", "agentctl-cli", "--locked"])?;
    let binary = binary_path(root);
    let schema_path = root.join("schemas/workflow.schema.json");
    let schema_text = generated_schema(&binary)?;
    write(&schema_path, &schema_text)?;
    let cli_path = root.join("docs/generated/CLI.md");
    let cli_text = generated_cli_reference(&binary)?;
    write(&cli_path, &cli_text)?;
    println!(
        "generated {} and {}",
        schema_path.display(),
        cli_path.display()
    );
    Ok(())
}

fn verify_generated(root: &Path) -> Result<()> {
    let binary = binary_path(root);
    compare_generated(
        &root.join("schemas/workflow.schema.json"),
        &generated_schema(&binary)?,
        "cargo xtask generate",
    )?;
    compare_generated(
        &root.join("docs/generated/CLI.md"),
        &generated_cli_reference(&binary)?,
        "cargo xtask generate",
    )
}

fn generated_schema(binary: &Path) -> Result<String> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("workflow.schema.json");
    let mut command = Command::new(binary);
    command
        .args(["schema", "--write"])
        .arg(&path)
        .arg("--output")
        .arg("json");
    let output = bounded_output(command, "generate schema").context("generate schema")?;
    ensure_success(&output, "agentctl schema")?;
    fs::read_to_string(path).context("read generated schema")
}

fn generated_cli_reference(binary: &Path) -> Result<String> {
    let commands: &[&[&str]] = &[
        &[],
        &["check"],
        &["plan"],
        &["run"],
        &["resume"],
        &["replay"],
        &["fork"],
        &["repair"],
        &["retry"],
        &["runs"],
        &["runs", "analyze"],
        &["runs", "upgrade"],
        &["cancel"],
        &["inspect"],
        &["effects"],
        &["effects", "list"],
        &["effects", "inspect"],
        &["effects", "reconcile"],
        &["approvals"],
        &["approvals", "list"],
        &["approvals", "approve"],
        &["approvals", "reject"],
        &["providers"],
        &["providers", "inspect"],
        &["providers", "smoke-openai"],
        &["auth"],
        &["schema"],
        &["migrate"],
        &["packs"],
        &["artifacts"],
        &["artifacts", "list"],
        &["artifacts", "inspect"],
        &["artifacts", "verify"],
        &["artifacts", "export"],
        &["artifacts", "gc"],
        &["db"],
        &["memory"],
        &["gc"],
        &["completion"],
        &["version"],
        &["update"],
    ];
    let mut markdown = String::from(
        "# CLI reference\n\nGenerated from the Rust CLI by `cargo xtask generate`. Do not edit by hand.\n\n",
    );
    for command in commands {
        let mut help_command = Command::new(binary);
        help_command.args(*command).arg("--help");
        let output = bounded_output(help_command, "agentctl --help")
            .with_context(|| format!("render help for {}", command.join(" ")))?;
        ensure_success(&output, "agentctl --help")?;
        let title = if command.is_empty() {
            "agentctl".to_owned()
        } else {
            format!("agentctl {}", command.join(" "))
        };
        let help = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n");
        write!(markdown, "## `{title}`\n\n```text\n{help}\n```\n\n",)?;
    }
    Ok(format!("{}\n", markdown.trim_end()))
}

fn verify_examples(root: &Path) -> Result<()> {
    let binary = binary_path(root);
    let examples = root.join("examples/v1");
    for entry in fs::read_dir(&examples)? {
        let path = entry?.path();
        if path.extension() != Some(OsStr::new("yaml"))
            || path.file_name() == Some(OsStr::new("example.pack.yaml"))
        {
            continue;
        }
        let expected_failure = path.file_name() == Some(OsStr::new("capability-failure.yaml"));
        let mut command = Command::new(&binary);
        command.arg("check").arg(&path).args(["--output", "json"]);
        let output = bounded_output(command, "agentctl example check")
            .with_context(|| format!("check example {}", path.display()))?;
        if expected_failure {
            if output.status.code() != Some(2) {
                bail!(
                    "negative capability fixture returned {:?}: {}",
                    output.status.code(),
                    output_diagnostics(&output)
                );
            }
        } else {
            ensure_success(&output, &format!("check {}", path.display()))?;
        }
    }

    let directory = tempfile::tempdir()?;
    for name in [
        "hello.yaml",
        "dataflow.yaml",
        "condition.yaml",
        "working-memory.yaml",
        "long-term-memory.yaml",
        "fake-provider.yaml",
        "reusable-pack.yaml",
    ] {
        let db = directory.path().join(format!("{name}.db"));
        run_binary(
            root,
            &binary,
            &[
                "run",
                examples.join(name).to_str().context("example path")?,
                "--db",
                db.to_str().context("db path")?,
                "--output",
                "json",
            ],
            Some(0),
        )?;
    }
    let check_db = directory.path().join("check.db");
    run_binary(
        root,
        &binary,
        &[
            "run",
            examples
                .join("check-diff.yaml")
                .to_str()
                .context("example path")?,
            "--db",
            check_db.to_str().context("db path")?,
            "--check",
            "--diff",
            "--output",
            "json",
        ],
        Some(0),
    )?;
    if examples.join("artifacts/report.txt").exists() {
        bail!("check mode mutated examples/v1/artifacts/report.txt");
    }

    let denied_db = directory.path().join("denied.db");
    let mut denied_command = Command::new(&binary);
    denied_command.current_dir(root).args([
        "run",
        examples
            .join("policy-denial.yaml")
            .to_str()
            .context("example path")?,
        "--db",
        denied_db.to_str().context("db path")?,
        "--output",
        "json",
    ]);
    let denied = bounded_output(denied_command, "agentctl policy denial")?;
    if denied.status.success() {
        bail!("policy-denial example unexpectedly succeeded");
    }
    if examples.join("artifacts/denied.txt").exists() {
        bail!("policy-denial example mutated the workspace");
    }
    Ok(())
}

#[derive(Debug)]
struct ExampleMatrixRow {
    path: String,
    check_code: i32,
    plan_code: i32,
}

fn verify_example_matrix(root: &Path) -> Result<()> {
    let matrix_path = root.join("docs/execution/EXAMPLE_VERIFICATION_MATRIX.md");
    let matrix = fs::read_to_string(&matrix_path)
        .with_context(|| format!("read {}", matrix_path.display()))?;
    let mut rows = BTreeMap::new();
    for line in matrix.lines().filter(|line| line.starts_with("| `")) {
        let columns = line.split('|').skip(1).map(str::trim).collect::<Vec<_>>();
        if columns.len() < 13 {
            bail!(
                "{} has an incomplete example row: {line}",
                matrix_path.display()
            );
        }
        let path = columns[0]
            .strip_prefix('`')
            .and_then(|value| value.strip_suffix('`'))
            .context("matrix path must be wrapped in backticks")?
            .to_owned();
        let check_code = columns[4]
            .parse::<i32>()
            .with_context(|| format!("matrix check code for {path}"))?;
        let plan_code = columns[5]
            .parse::<i32>()
            .with_context(|| format!("matrix plan code for {path}"))?;
        if rows
            .insert(
                path.clone(),
                ExampleMatrixRow {
                    path,
                    check_code,
                    plan_code,
                },
            )
            .is_some()
        {
            bail!("duplicate example matrix row");
        }
    }

    let mut discovered = Vec::new();
    collect_yaml_files(&root.join("examples"), root, &mut discovered)?;
    collect_yaml_files(&root.join("fixtures/compat"), root, &mut discovered)?;
    let discovered = discovered.into_iter().collect::<BTreeSet<_>>();
    let documented = rows.keys().cloned().collect::<BTreeSet<_>>();
    if discovered != documented {
        let missing = discovered.difference(&documented).collect::<Vec<_>>();
        let stale = documented.difference(&discovered).collect::<Vec<_>>();
        bail!("example matrix inventory mismatch; missing={missing:?}, stale={stale:?}");
    }

    let binary = binary_path(root);
    for row in rows.values() {
        if row.path.ends_with(".pack.yaml") {
            continue;
        }
        let workflow = root.join(&row.path);
        for (command_name, expected_code) in [("check", row.check_code), ("plan", row.plan_code)] {
            let mut command = Command::new(&binary);
            command
                .current_dir(workflow.parent().context("workflow parent")?)
                .arg(command_name)
                .arg(&workflow)
                .args(["--output", "json", "--color", "never"]);
            let output = bounded_output(command, "example matrix validation")
                .with_context(|| format!("{command_name} {}", row.path))?;
            if output.status.code() != Some(expected_code) {
                bail!(
                    "{command_name} {} returned {:?}, expected {expected_code}\n{}",
                    row.path,
                    output.status.code(),
                    output_diagnostics(&output)
                );
            }
            let machine = if output.stdout.iter().any(|byte| !byte.is_ascii_whitespace()) {
                &output.stdout
            } else {
                &output.stderr
            };
            serde_json::from_slice::<Value>(machine)
                .with_context(|| format!("{command_name} {} did not emit JSON", row.path))?;
        }
    }
    Ok(())
}

fn collect_yaml_files(directory: &Path, root: &Path, output: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_yaml_files(&path, root, output)?;
        } else if matches!(
            path.extension().and_then(OsStr::to_str),
            Some("yaml" | "yml")
        ) {
            output.push(
                path.strip_prefix(root)
                    .context("example path outside repository")?
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

fn verify_docs_examples(root: &Path) -> Result<()> {
    let binary = binary_path(root);
    let examples = root.join("examples/docs");
    let mut files = Vec::new();
    collect_files(&examples, &mut files)?;
    files.sort();

    for path in files
        .iter()
        .filter(|path| path.extension() == Some(OsStr::new("yaml")))
    {
        let mut command = Command::new(&binary);
        command.arg("check").arg(path).args(["--output", "json"]);
        let output = bounded_output(command, "agentctl documentation example check")
            .with_context(|| format!("check documentation example {}", path.display()))?;
        ensure_success(&output, &format!("check {}", path.display()))?;
    }

    for (directory, expected) in [
        ("release-readiness", "RELEASE_EVIDENCE_REVIEWED"),
        ("scheduled-review", "operational-review.txt"),
        ("ci-quality-gate", "\"verdict\":\"pass\""),
        ("provider-portability", "PORTABLE_SUMMARY_VERIFIED"),
    ] {
        let source_name = if directory == "provider-portability" {
            "fake.yaml"
        } else {
            "workflow.yaml"
        };
        let temporary = tempfile::tempdir()?;
        fs::create_dir_all(temporary.path().join("artifacts"))?;
        fs::copy(
            examples.join(directory).join(source_name),
            temporary.path().join("workflow.yaml"),
        )?;
        let mut command = Command::new(&binary);
        command.current_dir(temporary.path()).args([
            "run",
            "workflow.yaml",
            "--db",
            "runtime.db",
            "--output",
            "json",
        ]);
        let output = bounded_output(command, "agentctl documentation example run")
            .with_context(|| format!("run documentation example {directory}"))?;
        ensure_success(&output, &format!("run documentation example {directory}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !stdout.contains(expected) && !temporary.path().join("artifacts").join(expected).exists()
        {
            bail!("documentation example `{directory}` did not produce `{expected}`");
        }
    }

    let temporary = tempfile::tempdir()?;
    fs::copy(
        examples.join("ci-quality-gate/workflow.yaml"),
        temporary.path().join("workflow.yaml"),
    )?;
    let mut failed_gate = Command::new(&binary);
    failed_gate.current_dir(temporary.path()).args([
        "run",
        "workflow.yaml",
        "--db",
        "runtime.db",
        "--input",
        "checksPassed=false",
        "--output",
        "json",
    ]);
    let output = bounded_output(failed_gate, "agentctl failed documentation quality gate")?;
    if output.status.code() != Some(4) {
        bail!(
            "failed documentation quality gate returned {:?}: {}",
            output.status.code(),
            output_diagnostics(&output)
        );
    }
    Ok(())
}

fn public_documentation_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = vec![
        root.join("README.md"),
        root.join("CODE_OF_CONDUCT.md"),
        root.join("SECURITY.md"),
        root.join("examples/docs/README.md"),
    ];
    let mut docs = Vec::new();
    collect_files(&root.join("docs"), &mut docs)?;
    files.extend(docs.into_iter().filter(|path| {
        path.extension() == Some(OsStr::new("md"))
            && !path.components().any(|component| {
                matches!(
                    component.as_os_str().to_str(),
                    Some("execution" | "research")
                )
            })
    }));
    files.sort();
    files.dedup();
    Ok(files)
}

fn verify_public_documentation(root: &Path) -> Result<()> {
    let required = [
        "docs/guides/INSTALLATION.md",
        "docs/guides/GETTING_STARTED.md",
        "docs/guides/FIRST_AGENT_WORKFLOW.md",
        "docs/guides/WORKFLOW_AUTHORING.md",
        "docs/guides/LOCAL_OPERATION.md",
        "docs/guides/TERMINAL_RETRY.md",
        "docs/guides/repair-a-failed-workflow.md",
        "docs/guides/LEGACY_RUN_UPGRADE.md",
        "docs/guides/EFFECT_RECONCILIATION.md",
        "docs/guides/CI_CD.md",
        "docs/guides/TROUBLESHOOTING.md",
        "docs/reference/YAML.md",
        "docs/reference/CLI_OUTPUT.md",
        "docs/reference/ENVIRONMENT_AND_PATHS.md",
        "docs/reference/MATRICES.md",
        "docs/reference/DATABASE.md",
        "docs/reference/TERMINOLOGY.md",
        "docs/architecture/DIAGRAMS.md",
        "docs/development/REPOSITORY.md",
        "docs/development/ADD_ACTION.md",
        "docs/development/ADD_PROVIDER.md",
        "docs/development/ADD_MIGRATION.md",
        "docs/development/DOCUMENTATION.md",
        "docs/execution/EXAMPLE_VERIFICATION_MATRIX.md",
        "docs/research/selective-repair.md",
    ];
    for relative in required {
        if !root.join(relative).is_file() {
            bail!("required canonical documentation is missing: {relative}");
        }
    }

    let discouraged = [
        "unlock",
        "unleash",
        "revolutionize",
        "supercharge",
        "seamlessly",
        "effortlessly",
        "cutting-edge",
        "game-changing",
        "next-generation",
        "robust and scalable",
        "powerful solution",
        "in today's fast-paced world",
    ];
    for path in public_documentation_files(root)? {
        let source = fs::read_to_string(&path)?;
        if source.contains('—') {
            bail!("em dash in public documentation: {}", path.display());
        }
        if source.contains("/Users/") {
            bail!(
                "absolute local path in public documentation: {}",
                path.display()
            );
        }
        let lower = source.to_lowercase();
        if let Some(phrase) = discouraged.iter().find(|phrase| lower.contains(**phrase)) {
            bail!(
                "discouraged marketing phrase `{phrase}` in {}",
                path.display()
            );
        }
        for line in source.lines() {
            let Some(include) = line
                .trim()
                .strip_prefix("<!-- agentctl-include: ")
                .and_then(|line| line.strip_suffix(" -->"))
            else {
                continue;
            };
            let relative = include
                .split_whitespace()
                .next()
                .context("documentation include requires a source path")?;
            if !root.join(relative).is_file() {
                bail!(
                    "documentation include in {} references missing `{relative}`",
                    path.display()
                );
            }
        }
    }
    Ok(())
}

fn verify_markdown_links(root: &Path) -> Result<()> {
    for path in public_documentation_files(root)? {
        let source = fs::read_to_string(&path)?;
        let mut remaining = source.as_str();
        while let Some(start) = remaining.find("](") {
            remaining = &remaining[start + 2..];
            let Some(end) = remaining.find(')') else {
                bail!("unterminated Markdown link in {}", path.display());
            };
            let destination = &remaining[..end];
            remaining = &remaining[end + 1..];
            if destination.is_empty()
                || destination.starts_with('#')
                || destination.starts_with('/')
                || destination.starts_with("http://")
                || destination.starts_with("https://")
                || destination.starts_with("mailto:")
            {
                continue;
            }
            let local = destination
                .split('#')
                .next()
                .context("Markdown destination")?;
            let resolved = path.parent().context("Markdown source parent")?.join(local);
            if !resolved.exists() {
                bail!(
                    "broken local link in {}: `{destination}` resolves to {}",
                    path.display(),
                    resolved.display()
                );
            }
        }
    }
    Ok(())
}

fn verify_metadata(root: &Path) -> Result<()> {
    let mut metadata = Command::new("cargo");
    metadata
        .current_dir(root)
        .args(["metadata", "--format-version", "1", "--locked"]);
    let output = bounded_output(metadata, "cargo metadata")?;
    ensure_success(&output, "cargo metadata")?;
    let metadata: Value = serde_json::from_slice(&output.stdout)?;
    let packages = metadata["packages"]
        .as_array()
        .context("cargo metadata packages")?;
    let accepted = [
        "Apache-2.0",
        "MIT",
        "MIT-0",
        "BSD-2-Clause",
        "BSD-3-Clause",
        "ISC",
        "Unicode-3.0",
        "Zlib",
        "MPL-2.0",
        "OpenSSL",
        "CDLA-Permissive-2.0",
    ];
    for package in packages {
        let name = package["name"].as_str().unwrap_or("unknown");
        if package["source"]
            .as_str()
            .is_some_and(|source| source.starts_with("git+"))
        {
            bail!("git dependency `{name}` is not allowed");
        }
        let license = package["license"]
            .as_str()
            .ok_or_else(|| anyhow!("dependency `{name}` does not declare a license"))?;
        if !accepted.iter().any(|accepted| license.contains(accepted)) {
            bail!("dependency `{name}` has unreviewed license expression `{license}`");
        }
    }
    Ok(())
}

fn verify_supply_chain(root: &Path) -> Result<()> {
    if !command_exists("cargo-deny") {
        bail!("cargo-deny is required for the supply-chain verification gate");
    }
    run(root, "cargo", &["deny", "check"])
}

fn verify_no_secrets(root: &Path) -> Result<()> {
    let forbidden = forbidden_secret_patterns();
    let mut files = Vec::new();
    collect_files(root, &mut files)?;
    for path in files {
        let bytes = fs::read(&path)?;
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        if let Some(pattern) = forbidden
            .iter()
            .find(|pattern| text.contains(pattern.as_str()))
        {
            bail!("possible secret pattern `{pattern}` in {}", path.display());
        }
    }
    Ok(())
}

fn forbidden_secret_patterns() -> [String; 4] {
    [
        ["sk-", "proj-"].concat(),
        ["sk-", "ant-api"].concat(),
        ["AI", "zaSy"].concat(),
        ["-----BEGIN ", "PRIVATE KEY-----"].concat(),
    ]
}

fn verify_workflow_action_pins(root: &Path) -> Result<()> {
    let workflows = root.join(".github/workflows");
    for entry in fs::read_dir(&workflows)? {
        let path = entry?.path();
        if !matches!(
            path.extension().and_then(OsStr::to_str),
            Some("yml" | "yaml")
        ) {
            continue;
        }
        let source = fs::read_to_string(&path)?;
        for (index, line) in source.lines().enumerate() {
            let Some(reference) = line.trim_start().strip_prefix("- uses: ") else {
                continue;
            };
            if reference.starts_with("./") {
                continue;
            }
            let (reference, version) = reference.split_once(" # ").ok_or_else(|| {
                anyhow!(
                    "{}:{} action reference requires an exact-version comment",
                    path.display(),
                    index + 1
                )
            })?;
            let (_, revision) = reference.rsplit_once('@').ok_or_else(|| {
                anyhow!(
                    "{}:{} malformed action reference",
                    path.display(),
                    index + 1
                )
            })?;
            if revision.len() != 40 || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                bail!(
                    "{}:{} action `{reference}` is not pinned to a full commit SHA",
                    path.display(),
                    index + 1
                );
            }
            if version.trim().is_empty() {
                bail!(
                    "{}:{} action `{reference}` has an empty version comment",
                    path.display(),
                    index + 1
                );
            }
        }
    }
    Ok(())
}

fn collect_files(directory: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    let ignored = [
        ".git",
        "target",
        "node_modules",
        "dist",
        ".runtime",
        ".agentctl",
        ".release-evidence",
    ];
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if !ignored
                .iter()
                .any(|name| entry.file_name() == OsStr::new(name))
            {
                collect_files(&path, output)?;
            }
        } else {
            output.push(path);
        }
    }
    Ok(())
}

fn verify_install(root: &Path) -> Result<()> {
    let directory = tempfile::tempdir()?;
    run(
        root,
        "cargo",
        &[
            "install",
            "--path",
            "crates/agentctl-cli",
            "--root",
            directory.path().to_str().context("install root")?,
            "--locked",
            "--force",
        ],
    )?;
    let binary = directory.path().join("bin").join(if cfg!(windows) {
        "agentctl.exe"
    } else {
        "agentctl"
    });
    let mut version = Command::new(binary);
    version.arg("version");
    let output = bounded_output(version, "installed agentctl version")?;
    ensure_success(&output, "installed agentctl version")
}

fn verify_production_boundary(root: &Path) -> Result<()> {
    let package: Value = serde_json::from_str(&fs::read_to_string(root.join("package.json"))?)?;
    if package.get("bin").is_some() || package.get("main").is_some() {
        bail!("archived TypeScript package must not expose a production bin or main entry point");
    }
    if !root.join("archive/TYPESCRIPT_REFERENCE.md").exists() {
        bail!("TypeScript archive marker is missing");
    }
    Ok(())
}

fn compare_generated(path: &Path, generated: &str, command: &str) -> Result<()> {
    let committed = fs::read_to_string(path)
        .with_context(|| format!("read committed generated file {}", path.display()))?;
    if committed == generated {
        Ok(())
    } else {
        bail!("{} is stale; run `{command}`", path.display())
    }
}

fn write(path: &Path, value: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, value)?;
    Ok(())
}

fn binary_path(root: &Path) -> PathBuf {
    root.join("target").join("debug").join(if cfg!(windows) {
        "agentctl.exe"
    } else {
        "agentctl"
    })
}

fn run(root: &Path, program: &str, args: &[&str]) -> Result<()> {
    run_with_env(root, program, args, &[])
}

fn run_with_env(root: &Path, program: &str, args: &[&str], vars: &[(&str, &str)]) -> Result<()> {
    let mut command = Command::new(program);
    command.current_dir(root).args(args);
    for (name, value) in vars {
        command.env(name, value);
    }
    let status = command
        .status()
        .with_context(|| format!("run {program} {}", args.join(" ")))?;
    if status.success() {
        Ok(())
    } else {
        bail!("{program} {} exited with {status}", args.join(" "))
    }
}

fn run_binary(root: &Path, binary: &Path, args: &[&str], expected_code: Option<i32>) -> Result<()> {
    let mut command = Command::new(binary);
    command.current_dir(root).args(args);
    let output = bounded_output(command, "agentctl verification command")?;
    if output.status.code() == expected_code {
        Ok(())
    } else {
        bail!(
            "{} {} returned {:?}\n{}",
            binary.display(),
            args.join(" "),
            output.status.code(),
            output_diagnostics(&output)
        )
    }
}

fn ensure_success(output: &Output, label: &str) -> Result<()> {
    if output.status.success() {
        Ok(())
    } else {
        bail!(
            "{label} failed with {:?}\n{}",
            output.status.code(),
            output_diagnostics(output)
        )
    }
}

fn command_exists(name: &str) -> bool {
    env::var_os("PATH").is_some_and(|paths| {
        env::split_paths(&paths).any(|directory| {
            let candidate = directory.join(name);
            candidate.is_file()
                || (cfg!(windows) && directory.join(format!("{name}.exe")).is_file())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::forbidden_secret_patterns;

    #[test]
    fn deterministic_secret_detector_recognizes_a_synthetic_credential() {
        let fake = ["sk-", "proj-", "synthetic-not-a-real-credential"].concat();
        assert!(
            forbidden_secret_patterns()
                .iter()
                .any(|pattern| fake.contains(pattern))
        );
    }
}
