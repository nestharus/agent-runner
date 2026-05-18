# AGE-144 invocation-output manifest schema (v1)

## Purpose

This document is the canonical common-interface schema for the per-invocation
manifest exposed under each `TraceNode.invocation` object in
`agents trace --json`.

Schema v1 documents only the invocation fields that the runtime already emits:
the invocation UUID, parent invocation UUID, completion status, and completion
timestamp. The canonical names below are consumer-facing schema aliases for the
existing runtime JSON shape; this document does not add or rename runtime fields.

## Declared roles

`conventions/agents-trace-output.md` declares `[formatter, validator]`.

```yaml
declared_roles:
  - file: conventions/agents-trace-output.md
    roles: [formatter, validator]
    rationale: canonical-doc-as-schema artifact; formatter for the field-catalog / status enum / parse-semantics tables; validator for the v2-deferred non-closure language and consumer pull rules
```

## Schema version

`schema_version: "1"`

Version 1 covers the four required v1 canonical field names in the field
catalog and the parse semantics in this document.

## Source

The runtime source anchor is
`crates/oulipoly-runtime/src/trace/mod.rs::TraceInvocation`.

The v1 canonical schema maps to these existing producer fields:
`TraceInvocation.id`, `TraceInvocation.parent_id`, `TraceInvocation.status`,
and `TraceInvocation.finished_at`. The manifest is read recursively from each
`TraceNode.invocation` object in the `agents trace --json` tree.

## Field catalog

| Canonical name | Runtime source | Required | Semantics |
|---|---|---|---|
| `invocation_uuid` | `TraceInvocation.id` | required | UUID string; the child invocation's stable identifier. |
| `parent_invocation_uuid` | `TraceInvocation.parent_id` | optional | UUID string; absent on the root invocation. |
| `completion_status` | `TraceInvocation.status` | required | Enum: `running`, `succeeded`, `failed`, `legacy`. See § Parse semantics for stale-running JSON lift behavior. |
| `completion_timestamp` | `TraceInvocation.finished_at` | optional | ISO-8601 UTC timestamp; `null`/absent while running or for stale-running rows. |

## Parse semantics

Consumers read the v1 manifest from every `TraceNode.invocation` object emitted
by `agents trace --json`. The canonical names are aliases for the runtime fields
listed in the field catalog; consumers should not require the runtime JSON keys
to be renamed.

`completion_status` uses the v1 vocabulary `running`, `succeeded`, `failed`,
and `legacy`. A stale-running JSON lift preserves `completion_status` as
`running` while exposing stale-running details through the runtime's existing
adjacent fields; it does not create an additional v1 completion status.

`returned_artifacts.name` is artifact display metadata, not a canonical output
path declaration. Consumers MUST NOT infer `declared_output_paths` from `returned_artifacts.name` or from any other existing trace field.

## Stability guarantee

Schema v1 is stable for consumers of `agents trace --json`. Forward-compatible
changes to this schema are additive.

Removing a v1 field, changing the meaning of a v1 field, changing the required
status of a v1 field, or changing the `completion_status` vocabulary is a
breaking change. Field renames require a `schema_version` bump; specifically,
renames require a `schema_version` bump.

## Consumer registry

The known consumers for this common-interface schema are informational and do
not change the v1 field catalog:

- orchestrator phase-join logic
- process-tree-auditor trace validation
- decision-encoder audit-history and closure evidence

Consumers should pull the four v1 canonical values from this document's schema
instead of depending on private storage details or ad hoc trace parsing rules.

## Pull-site closure mechanism

AGE-144 closure uses common-interface proof. The agent-runner runtime owns the
producer surface for `agents trace --json`, and this repo-root document is the
declared consumer-facing contract for the v1 invocation manifest.

The canonical-doc-as-schema recipe is NOT the closure mechanism. This document
lives at repo-root in `conventions/agents-trace-output.md`, not `~/ai/`, so it
does not satisfy any recipe that specifically requires a canonical file under
`~/ai/`.

## Schema future / deferred

`declared_output_paths` is reserved for a future schema v2 and a future
producer-side work unit. v1 does NOT declare `declared_output_paths`, does not
promise that the runtime emits it, and does not close the output
path-to-invocation join gap.

until then, consumers needing path-to-invocation joins MUST continue using their existing topology + canonical-output-path matching mechanisms

## Intrinsic-surface declarations

```yaml
intrinsic_surface_declarations:
  - component: conventions/agents-trace-output.md
    role: intrinsic-surface
    Domain: agents trace --json invocation manifest contract
    Owns:
      - agents trace --json
      - TraceNode.invocation
      - TraceInvocation
      - id
      - parent_id
      - status
      - finished_at
      - returned_artifacts.name
      - InvocationStatus running/succeeded/failed/legacy vocabulary
      - stale_running JSON lift behavior
  - component: crates/oulipoly-runtime/tests/age144_trace_output_schema_doc.rs
    role: intrinsic-surface
    Domain: conventions/agents-trace-output.md doc-alignment validation
    Owns:
      - section heading set (Purpose, Schema version, Source, Field catalog, Parse semantics, Stability guarantee, Consumer registry, Pull-site closure mechanism, Schema future / deferred, Declared roles, Intrinsic-surface declarations, Out-of-scope)
      - v1 field catalog rows (invocation_uuid, parent_invocation_uuid, completion_status, completion_timestamp)
      - completion_status enum vocabulary (running, succeeded, failed, legacy)
      - schema future / deferred language for declared_output_paths v2 deferral
      - common-interface proof / closure-mechanism phrases
      - stability guarantee additive-vs-breaking language
      - returned_artifacts.name non-inference rule
```

## Out-of-scope

AGE-144 v1 does not change runtime code, SQLite state, executor behavior,
messenger behavior, CLI behavior, or trace serialization.

This work does not emit output-path declarations, edit orchestrator/operator
files, edit files under `~/ai/`, add a JSON Schema sidecar, update README.md,
update AGENTS.md, or edit `docs/architecture/`.

`BLOCKED` is not part of the v1 `completion_status` enum.
