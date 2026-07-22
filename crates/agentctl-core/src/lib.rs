//! Deterministic, provider-independent contracts for `agentctl`.

pub mod compiler;
pub mod diagnostic;
pub mod dsl;
pub mod effect;
pub mod pack;
pub mod policy;
pub mod provider;
pub mod state;
pub mod template;
pub mod tool;

pub use compiler::{CompiledPlan, CompiledTask, PlanPredictability, compile};
pub use diagnostic::{Diagnostic, DiagnosticCode, Severity};
pub use dsl::{ParseOutcome, Workflow, parse_workflow, schema_json};

/// Version of every machine-readable CLI envelope emitted by this release.
pub const MACHINE_OUTPUT_VERSION: &str = "agentctl.dev/cli/v1";
/// Version of the durable compiled-plan representation.
pub const PLAN_FORMAT_VERSION: u32 = 1;
/// Version of the durable effect representation.
pub const EFFECT_FORMAT_VERSION: u32 = 1;
