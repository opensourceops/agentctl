# ADR 0002: Versioned strict workflow envelope

Status: accepted, 2026-07-22.

Workflows use `apiVersion`, `kind`, `metadata`, and `spec`. The envelope is adopted because identity/version and evolution need unambiguous locations, not because another system uses it. All typed objects deny unknown fields and a generated JSON Schema is checked in.

A narrow legacy translator provides actionable migration. General aliases and silent coercion are rejected because they make security review and durable reproduction ambiguous.
