# ADR 0005: Narrow v1 scheduling and extension surface

Status: superseded for scheduling by ADR 0008, 2026-07-24.

V1alpha1 schedules a sequential DAG in declaration order and integrates typed actions/tools, local packs, MCP 2025-11-25, and A2A 1.0. `maxConcurrency` greater than one is rejected.

Parallel groups, loops, routing, sub-workflows, teams/handoffs, automatic reconnection/resubmission, executable plugin ABIs, and registries are deferred. Each needs deterministic merge, cancellation, policy, version, and recovery semantics before it can enter the stable contract.
