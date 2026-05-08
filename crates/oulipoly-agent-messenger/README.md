# agent-messenger

`agent-messenger` lets a child agent return a durable artifact reference to the parent invocation. It stores bytes in `agent-store` under the internal return namespace and appends one JSONL receipt to the parent-owned `OULIPOLY_RETURN_CHANNEL`.

The database must already be initialized by `agent-store`; `--db <path>` is required on every command that reads or writes artifacts.

## Commands

```bash
agent-messenger return --db <path> [--invocation-uuid <uuid>] --name <return-name> --scratchpad <scratchpad-name> [--scratchpad-version <n>] [--format <hint>] [--verdict-line <text>] [--return-channel <path>] [--json]
agent-messenger return --db <path> [--invocation-uuid <uuid>] --name <return-name> (--body <text> | --content-file <path> | --content-stdin) [--format <hint>] [--verdict-line <text>] [--return-channel <path>] [--json]
agent-messenger list-returned --db <path> [--invocation-uuid <uuid>] [--name <return-name>] [--json]
agent-messenger show --db <path> (--version-id <id> | [--invocation-uuid <uuid>] --name <return-name> [--version <n>]) [--out <path>]
agent-messenger version --json
```

`--body` uses the UTF-8 bytes of the argument exactly. `--content-file` and `--content-stdin` preserve raw bytes. `show` writes raw bytes to stdout with no JSON and no trailing newline; with `--out`, stdout stays empty and the file receives the exact bytes.

## Invocation Scope

For `return`, `list-returned`, and name-based `show`, `--invocation-uuid` wins over `OULIPOLY_PARENT_INVOCATION`. Without the flag, `OULIPOLY_PARENT_INVOCATION` must be JSON containing an `id` UUID. Missing, malformed, or non-UUID scope exits `64`.

For `return`, `--return-channel` wins over `OULIPOLY_RETURN_CHANNEL`. In dispatched use, missing `OULIPOLY_RETURN_CHANNEL` exits `64`; library callers may omit a channel for store-only behavior.

## Addressing

Returned artifacts are stored in `agent-store` as:

```text
workflow_run_id = return:<invocation_uuid>
artifact_name = <name>
version = <store-assigned version>
```

The caller-facing `version_id` shape is:

```text
store://return/<invocation_uuid>/<percent-encoded artifact_name>/<version>
```

Returned receipts never expose private `scratchpad:` workflow IDs. A scratchpad return copies bytes through the scratchpad API into a new `return:` store version.

## JSON

`return --json` emits one receipt object:

```json
{
  "schema_version": 1,
  "version_id": "store://return/<uuid>/proposal.md/1",
  "name": "proposal.md",
  "store_address": {
    "workflow_run_id": "return:<uuid>",
    "artifact_name": "proposal.md",
    "version": 1
  },
  "sha256": "64 lowercase hex characters",
  "content_len": 123,
  "format_hint": "text/markdown",
  "verdict_line": "APPROVED: ready",
  "source": { "kind": "inline_bytes" },
  "producer_invocation_uuid": "<uuid>",
  "returned_at": "2026-05-07T12:00:00Z"
}
```

Scratchpad source receipts use `source.kind = "scratchpad"` and include the public scratchpad name/version only. `list-returned --json` emits an array with the same fields except `schema_version`. `version --json` emits `package`, `version`, and `receipt_schema_version`.

The parent runner reads channel receipts and projects them as terminal `returned_artifacts` in state-backed trace JSON. A returned artifact does not imply provider success; success and failure still come from the child exit code and terminal reason.

## Exit Codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 64 | CLI or caller misuse: clap parse error, malformed UUID, missing invocation scope, missing/invalid return channel, invalid name, conflicting content flags |
| 65 | Source, returned artifact, or version not found or tombstoned |
| 66 | Backing store collision |
| 70 | JSON serialization error |
| 73 | Database, migration, schema, or metadata-decode error |
| 74 | I/O error for content file, output file, or channel append |
