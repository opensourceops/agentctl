# Archived TypeScript reference

The root `src/`, `test/`, legacy examples, `package.json`, and `package-lock.json` are retained only as the behavioral oracle used during the Rust migration. They have no production binary entry point, are excluded from `cargo xtask verify`, and are not part of installation, packaging, CI, or runtime support. Superseded prototype documentation was removed to avoid presenting two product contracts.

The final passing prototype baseline was Node.js 26.0.0 with `NODE_OPTIONS=--no-deprecation`: 16 test files and 139 tests passed. Compatibility-derived behavior lives in `fixtures/compat`; intentional changes are recorded in `docs/COMPATIBILITY.md`.

New behavior changes must target the Rust workspace. Do not add product features to the TypeScript reference.
