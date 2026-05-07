# agent-store

`agent-store` is the v1 durable artifact store for agent-produced outputs. It stores inline BLOB content in SQLite and addresses every artifact version by:

```text
(workflow_run_id, artifact_name, version)
```

`workflow_run_id` and `artifact_name` identify the artifact stream. `version` is assigned by the store, starts at `1`, and increases per `(workflow_run_id, artifact_name)` on each `put`.

## Schema And Migrations

Initialize a database explicitly:

```bash
agent-store init --db ./agent-store.sqlite --json
```

JSON:

```json
{"db_path":"./agent-store.sqlite","schema_version":1,"status":"initialized"}
```

The schema is tracked in `schema_meta` with `schema_meta.schema_version = 1`. Migrations are monotonic forward: `Store::open` accepts only version `1`, reports migration required when the schema marker is missing, and reports incompatible schema when the marker has another value. `init` is idempotent and returns `"already_current"` when the database is already at schema version `1`.

## Writing Artifacts

```bash
agent-store put --db ./agent-store.sqlite \
  --workflow-run-id R1 \
  --artifact-name report.md \
  --producer-uuid 550e8400-e29b-41d4-a716-446655440000 \
  --format text/markdown \
  --verdict-line "APPROVED: ready" \
  --content-file ./report.md \
  --json
```

JSON:

```json
{
  "workflow_run_id": "R1",
  "artifact_name": "report.md",
  "version": 1,
  "producer_invocation_uuid": "550e8400-e29b-41d4-a716-446655440000",
  "sha256": "64_lowercase_hex_characters",
  "content_len": 1234,
  "format_hint": "text/markdown",
  "verdict_line": "APPROVED: ready",
  "predecessor_version": null,
  "created_at": "2026-05-07T00:00:00Z"
}
```

Content can also come from stdin with `--content-stdin`. The `sha256` field is computed over the exact bytes supplied.

## Reading Content

```bash
agent-store get --db ./agent-store.sqlite \
  --workflow-run-id R1 \
  --artifact-name report.md
```

`get` writes raw artifact bytes to stdout. It never writes JSON, prefixes, or diagnostics to stdout. Without `--version`, it returns the latest non-tombstoned version. Tombstoned versions are hidden from content reads and return exit code 65, the same as other not-found reads. Use `--out <path>` to write the same bytes to a file instead of stdout:

```bash
agent-store get --db ./agent-store.sqlite \
  --workflow-run-id R1 \
  --artifact-name report.md \
  --version 1 \
  --out ./report-copy.md
```

## Reading Metadata

```bash
agent-store get-meta --db ./agent-store.sqlite \
  --workflow-run-id R1 \
  --artifact-name report.md \
  --version 1 \
  --json
```

JSON:

```json
{
  "workflow_run_id": "R1",
  "artifact_name": "report.md",
  "version": 1,
  "producer_invocation_uuid": "550e8400-e29b-41d4-a716-446655440000",
  "sha256": "64_lowercase_hex_characters",
  "content_len": 1234,
  "format_hint": "text/markdown",
  "verdict_line": "APPROVED: ready",
  "predecessor_version": null,
  "created_at": "2026-05-07T00:00:00Z",
  "tombstone": null
}
```

## Listing

```bash
agent-store list --db ./agent-store.sqlite --workflow-run-id R1 --json
```

`list --json` returns an array of metadata objects with the same field names as `get-meta --json`, ordered by `(workflow_run_id, artifact_name, version)` ascending. `--workflow-run-id` is optional; omit it to list artifacts across all workflow runs.

## Tombstones

Deletes are soft deletes. Tombstoning preserves the row and content hash metadata, records audit fields, and hides tombstoned content from `get` and latest metadata reads. Explicit `get-meta --version <n>` can still return tombstoned metadata. Tombstoning a missing `(workflow_run_id, artifact_name, version)` tuple returns exit code 65.

```bash
agent-store tombstone --db ./agent-store.sqlite \
  --workflow-run-id R1 \
  --artifact-name report.md \
  --version 1 \
  --actor operator \
  --reason "no longer canonical" \
  --json
```

JSON:

```json
{
  "workflow_run_id": "R1",
  "artifact_name": "report.md",
  "version": 1,
  "tombstoned_at": "2026-05-07T00:00:00Z",
  "actor": "operator",
  "reason": "no longer canonical",
  "status": "tombstoned"
}
```

Repeating the same tombstone command is idempotent. It returns the original actor, reason, timestamp, and `"status": "already_tombstoned"`.

Include tombstoned rows in history with:

```bash
agent-store list --db ./agent-store.sqlite --workflow-run-id R1 --include-tombstoned --json
```

The row shape includes:

```json
{
  "tombstone": {
    "tombstoned_at": "2026-05-07T00:00:00Z",
    "actor": "operator",
    "reason": "no longer canonical"
  }
}
```

## Exit Codes

`agent-store` uses stable exit codes for shell and cross-language consumers:

| Code | Meaning |
| ---- | ------- |
| 0 | Success |
| 64 | Clap-validated misuse, such as missing required flags or invalid values |
| 65 | Not found |
| 66 | Version collision |
| 70 | Internal serialization error |
| 73 | Database, migration-required, or incompatible-schema error |
| 74 | I/O error |

Version collision is reserved for a SQLite uniqueness conflict on the `(workflow_run_id, artifact_name, version)` tuple. Normal writers receive monotonically assigned versions; callers that hit 66 should retry the write after reopening or treat repeated collisions as database contention/corruption.

Errors are written to stderr. Machine-readable success output is written to stdout only when `--json` is supplied, except `get`, whose stdout is always raw content bytes.
