# Sensitive-state encryption

`agentctl` can protect identified confidential SQLite fields with versioned AES-256-GCM envelopes. Encryption is explicit, application-level selected-field protection. It is not full-database encryption and does not encrypt artifact blob bytes or operational metadata.

## Prepare the key reference

Supply a base64-encoded 32-byte key through an environment variable. The CLI receives only the environment-variable name:

```console
export AGENTCTL_STATE_KEY="$(openssl rand -base64 32)"
agentctl db --db .agentctl/runtime.db encryption inventory \
  --output json --color never
```

Protect the key with the platform's normal secret injection and backup controls. Do not put its value in YAML, CLI arguments, logs, shell history, runtime inputs, or repository files. The database stores the key ID and reference name, never the value.

## Inventory and enable

Preview the migration:

```console
agentctl db --db .agentctl/runtime.db encryption enable \
  --key-id production-2026-01 \
  --key-env AGENTCTL_STATE_KEY \
  --dry-run \
  --output json \
  --color never
```

The report contains counts and key metadata, not protected values. Back up the SQLite database and WAL state, then run the same command without `--dry-run`.

Enablement rewrites every non-null protected field inside one immediate SQLite transaction and updates checkpoint checksums. After commit, database triggers reject plaintext and envelopes from stale key IDs. Opening the database requires the referenced key and authenticates every protected value. There is no plaintext fallback.

Protected fields include workflow definitions and plans, inputs, working memory, run/task output and errors, state deltas and reuse decisions, effect input/results/errors, approval content, checkpoints, audit and trace payloads, provider continuations, reconciliation evidence/results, run-upgrade records, and long-term-memory values.

## Rotate

Keep the current key reference available while planning and performing rotation:

```console
export AGENTCTL_STATE_KEY_NEXT="$(openssl rand -base64 32)"
agentctl db --db .agentctl/runtime.db encryption rotate \
  --key-id production-2026-07 \
  --key-env AGENTCTL_STATE_KEY_NEXT \
  --dry-run

agentctl db --db .agentctl/runtime.db encryption rotate \
  --key-id production-2026-07 \
  --key-env AGENTCTL_STATE_KEY_NEXT
```

Rotation decrypts with the current key and re-encrypts every protected value with fresh nonces and the new key in one transaction. Any authentication, write, or injected storage failure rolls the entire rotation back. After success, only the new reference is required.

## Backup and restore

Back up SQLite, its WAL state, and the sibling artifact root as one consistency set. Preserve the current key outside that backup; without it, protected content is intentionally unrecoverable. Retire plaintext pre-migration backups and old snapshots under the same confidentiality policy as their original workflow content.

Visible residual metadata includes run/task/effect IDs, state and mode, timestamps, effect class and operation, digests, schema versions, artifact paths/names/media types/sizes/digests, and the encryption key ID/reference. Artifact blob bytes remain governed by filesystem or volume encryption.

If the key is missing, wrong, or malformed, or an envelope was changed, every normal open fails with persistence exit code `5`. Restore the correct key or an internally consistent database and key backup; do not edit encrypted fields or try to bypass the write guards.
