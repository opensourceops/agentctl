use serde::{Deserialize, Serialize};

/// Stable category for a user-facing diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCode {
    YamlSyntax,
    SchemaViolation,
    UnsupportedVersion,
    MigrationRequired,
    DuplicateTask,
    MissingReference,
    DependencyCycle,
    InvalidTemplate,
    UnsupportedCapability,
    InvalidSecretReference,
    PolicyDenied,
    IncompatibleState,
}

/// Diagnostic importance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

/// Source-aware, stable diagnostic shape used by both humans and machines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub severity: Severity,
    pub message: String,
    pub file: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub path: Option<String>,
    pub help: Option<String>,
}

impl Diagnostic {
    #[must_use]
    pub fn error(code: DiagnosticCode, file: &str, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: Severity::Error,
            message: message.into(),
            file: file.to_owned(),
            line: None,
            column: None,
            path: None,
            help: None,
        }
    }

    #[must_use]
    pub fn with_location(mut self, line: usize, column: usize) -> Self {
        self.line = Some(line);
        self.column = Some(column);
        self
    }

    #[must_use]
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }
}
