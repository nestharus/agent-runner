# agent-scratchpad

`agent-scratchpad` is a private, invocation-scoped artifact scratchpad for
dispatched agents. It stores exact bytes in the existing `oulipoly-agent-store`
SQLite database and publishes selected private artifacts into canonical
`agent-store` keys.

Every command that touches artifacts requires `--db <path>`. The database must
already be initialized with `agent-store init --db <path>`; scratchpad commands
do not auto-create or migrate schema.

## Invocation Scope

Private artifacts are scoped by invocation UUID. Commands that accept
`--invocation-uuid` resolve scope in this order:

1. explicit `--invocation-uuid <uuid>` wins.
2. otherwise parse `OULIPOLY_PARENT_INVOCATION` as JSON and use its `id` field.
3. otherwise fail with exit code `64`.

Internally, a private scratchpad artifact maps to
`workflow_run_id = scratchpad:<invocation_uuid>` and `artifact_name = <name>`.
Callers should not construct that private `scratchpad:` key themselves.

## Commands

```text
agent-scratchpad write --db <path> [--invocation-uuid <uuid>] --name <name> [--format <hint>] [--verdict-line <text>] [--content-file <path> | --content-stdin] [--json]
agent-scratchpad read --db <path> [--invocation-uuid <uuid>] --name <name> [--version <n>] [--out <path>]
agent-scratchpad list --db <path> [--invocation-uuid <uuid>] [--name <name>] [--include-tombstoned] [--json]
agent-scratchpad delete --db <path> [--invocation-uuid <uuid>] --name <name> [--version <n> | --all-versions] [--actor <actor>] [--reason <text>] [--json]
agent-scratchpad publish --db <path> [--invocation-uuid <uuid>] --name <name> --workflow-run-id <id> --artifact-name <name> [--version <n>] [--format <hint>] [--verdict-line <text>] [--predecessor-version <n>] [--json]
agent-scratchpad gc --db <path> (--invocation-uuid <uuid> | --expired-before <rfc3339>) [--dry-run] [--actor <actor>] [--reason <text>] [--json]
agent-scratchpad scope --invocation-uuid <uuid> [--json]
```

`read` writes raw bytes to stdout, with no JSON and no trailing newline. With
`--out <path>`, the same raw bytes are written to the file and stdout is empty.
All diagnostics go to stderr.

## JSON Output

`write`, `list`, `delete`, `publish`, `gc`, and `scope` emit JSON only when
`--json` is supplied. Timestamps are RFC3339 UTC strings and field names are
stable snake_case.

Write receipt fields include: `address`, `invocation_uuid`, `name`, `version`,
`producer_invocation_uuid`, `sha256`, `content_len`, `format_hint`,
`verdict_line`, `predecessor_version`, `created_at`.

List row fields include: `address`, `invocation_uuid`, `name`, `version`,
`sha256`, `content_len`, `producer_invocation_uuid`, `format_hint`,
`verdict_line`, `predecessor_version`, `created_at`, `tombstone`.

Delete receipt fields include: `address`, `selector`, `tombstoned_versions`,
`already_tombstoned_versions`, `actor`, `reason`, `tombstoned_at`.

Publish receipt fields include: `source`, `source_version`, `source_sha256`,
`destination`, `destination_version`, `destination_sha256`, `content_len`,
`producer_invocation_uuid`, `format_hint`, `verdict_line`,
`predecessor_version`, `created_at`.

GC report fields include: `selector`, `dry_run`, `tombstoned_rows`,
`already_tombstoned_rows`, `actor`, `reason`, `evaluated_at`.

## Publish And Cleanup

`publish` copies bytes from the caller's private scratchpad source to canonical
storage addressed by `--workflow-run-id` and `--artifact-name`. It preserves the
private source, sets canonical `producer_invocation_uuid` to the source
invocation UUID, and rejects canonical destinations whose workflow run starts
with `scratchpad:`.

`delete` and `gc` are logical tombstones through `agent-store`; they do not
physically purge SQLite BLOB rows. `gc --invocation-uuid` tombstones only that
private scope. `gc --expired-before` sweeps scratchpad rows whose derived expiry
is `created_at + 7 days <= cutoff`; canonical rows are never tombstoned by GC.

Existing load-bearing planning filesystem artifacts remain filesystem
artifacts until a later migration. Scratchpad is for private per-dispatch work
unless an artifact is explicitly published to canonical storage.

## Exit Codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 64 | CLI or caller misuse: clap parse error, malformed UUID, missing invocation scope, invalid name |
| 65 | Not found, tombstoned, or expired artifact |
| 66 | Backing store collision |
| 70 | JSON serialization error |
| 73 | Database, migration, schema, or metadata-decode error |
| 74 | I/O error such as `--content-file` read or `--out` write failure |
