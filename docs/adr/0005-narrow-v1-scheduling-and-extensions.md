# ADR 0005: Narrow v1 scheduling and extension surface

Status: superseded for scheduling and control flow by ADRs 0008 through 0011,
2026-07-24.

V1alpha1 schedules a sequential DAG in declaration order and integrates typed actions/tools, local packs, MCP 2025-11-25, and A2A 1.0. `maxConcurrency` greater than one is rejected.

The original decision excluded parallel execution, loops, and routing until
their deterministic semantics existed. ADRs 0008, 0010, and 0011 now define
those contracts. Sub-workflows, teams/handoffs, automatic
reconnection/resubmission, executable plugin ABIs, and registries remain
separate product decisions.
