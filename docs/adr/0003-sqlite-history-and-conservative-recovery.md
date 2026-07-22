# ADR 0003: SQLite history and conservative recovery

Status: accepted, 2026-07-22.

One versioned SQLite database is the local correctness store. Transactional task transitions/checkpoints/audits and a request-before-start effect ledger support resume and no-effect replay. Confirmed results are reused; started unconfirmed work becomes uncertain.

No exactly-once claim is made. Automatic retry of ambiguous external effects is rejected. Fork is the explicit operation for fresh effects. Distributed history services are outside this release.
