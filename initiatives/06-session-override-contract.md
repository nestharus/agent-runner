# Initiative 06 — Session Override Contract (v2 surface for agent-harness)

**Status:** proposing (feature 06-locate Phase 2.5 dispatched)
**Depends on:** Initiative 05 (session migration — uses `session_chains`, `session_chain_segments`, `StateDb::resolve_resume`, `[providers.session_storage]`, transcript locator)
**Blocks:** —

## Problem (user framing, verbatim)

Captured from the conversation that opened this initiative on 2026-04-30:

> Another product is requesting these features.
>
> [local agent-harness scratch directory]/agent-runner-feature-requests/

The companion product `agent-harness` has authored five `agents`-CLI
feature requests under the external `agent-harness`
`tmp/scratch/agent-runner-feature-requests/` directory.
Together they form an upstream replacement for the v1
`SessionOverrideContract` adapter that today reads `state.db` and
provider JSONL files directly. The v2 contract goes through stable CLI
seams instead so the harness can pin a public surface and refuse-rather-
than-corrupt on drift.

Five harness specs (see `INDEX.md` in that directory):

| Spec | Surface | Mutates? |
|---|---|---|
| `01-session-locate.md` | `agents session locate <id>` → JSON metadata | No |
| `02-session-export.md` | `agents session export <id>` → canonical JSONL | No |
| `03-session-import-replace.md` | `agents session import-replace <id>` ← canonical JSONL (atomic, two-phase) | Yes |
| `04-session-pause-handshake.md` | `agents session pause-handshake` / `resume-handshake` (lease lock) | Lock state |
| `05-session-schema-probe.md` | `agents session schema-probe` → JSON probe | No |

## Scope

**In scope (this initiative):**

Five separate PRs against `agent-runner`, each its own proposal/risk/
hookpoint/implementation cycle, sequenced in technical-dependency order:

1. **06-locate** — `agents session locate <session-id>`. Factors out
   reusable `SessionMetadata` API consumed by 06-export and
   06-import-replace.
2. **06-schema-probe** — `agents session schema-probe`. Read-only
   `StateDb` open variant; explicit public schema_version; advertises
   feature flags so harness can gate on us. Sequenced second so
   harness can pin from day one.
3. **06-export** — `agents session export <session-id>`. Builds the
   canonical-transcript reader that 06-import-replace round-trips
   against.
4. **06-pause-handshake** — `agents session pause-handshake` /
   `resume-handshake`. Session-scoped exclusive lease lock observed by
   import-replace, migration, resume/repl write paths.
5. **06-import-replace** — `agents session import-replace
   <session-id>`. Two-phase atomic replace with crash recovery,
   composes locate + canonical reader + lock.

Each feature follows the full implementation pipeline:
Phase 2.5 (problem-map) → Phase 3 (proposal) → Phase 4 (4 risk
gates) → Phase 5 (hookpoints) → Phase 6 (contract / tests / code)
→ Phase 7 (CodeRabbit) → Phase 8 (PR review gates) → Phase 9
(draft PR) → Phase 10 (promote).

**Out of scope:**

- Mid-process provider migration during a single `repl`.
- Cross-CLI migration (Claude → Codex, etc.).
- `.zst` ingestion (deferred from Initiative 05).
- Cross-org / cross-workspace cache policy.
- Frontend visibility of session locate/export/replace.
- Alternate transcript export formats beyond canonical JSONL.
- Auto-resume semantics on `import-replace` (the harness drives resume
  separately).

## Sequencing rationale

Numerical (1→2→3→4→5 as the harness numbered them) would force
retrofitting lock observation into 03 once 04 lands, and would ship
01–04 before harness can pin a schema_version. The technical order
(1 → 5 → 2 → 4 → 3) ships clean: each feature lands with its
dependencies already in place.

- `locate` first establishes `SessionMetadata` reusable API + read-only
  state surface needed by `schema-probe`.
- `schema-probe` second exposes feature flags so the harness can adopt
  features incrementally as each lands.
- `export` third builds the canonical transcript reader.
- `pause-handshake` fourth establishes the lock primitive.
- `import-replace` last because it composes all four prior surfaces.

## Reference framework

- Harness feature request files in the external `agent-harness`
  `tmp/scratch/agent-runner-feature-requests/` directory
  (01-session-locate.md, 02-session-export.md, 03-session-import-replace.md,
  04-session-pause-handshake.md, 05-session-schema-probe.md, INDEX.md).
- Workflow convention `no-backwards-compatibility.md` — new subcommand
  surface; no shims.
- Workflow convention `no-deferred-stubs.md` — each feature lands fully
  functional or not at all.
- Initiative 05 artifacts establishing the resume/chain/segment
  vocabulary that `locate` consumes:
  `proposals/05-session-migration.md`,
  `research/05-session-migration-hookpoints.md`.

## Cross-feature design constraints (carried into every proposal)

- **Error code namespace.** All five features share: `10`
  session-not-found, `11` ambiguous-session, `12` unsupported-storage,
  `13` session-busy, `14` schema-incompatible, `15` invalid-input or
  preimage mismatch, `16` lock-token-invalid, `17` lock-expired.
- **Ownership resolution.** Reuse `StateDb::resolve_resume`
  (`src-tauri/src/state/db.rs:2577-2635`). No second ownership path.
- **Lock observation.** `import-replace`, migration's
  `migrate_chain_segment`, `run_repl`, `run_resume`, and balanced
  one-shot must observe `pause-handshake` locks once 06-pause-handshake
  lands.
- **Read-only `StateDb` open.** `schema-probe` requires a read-only
  variant; current `StateDb::open` creates dirs, enables WAL, ensures
  schemas, and backfills chains. The variant lands in 06-schema-probe.
- **Anti-scope, every feature.** No auto-resume. No provider spawn. No
  quota refresh. No config edits. No coupling to `migrate-config`.

## Artifacts (filled per feature as each phase completes)

| Feature | Problem map | Proposal | Risk (audit/scope/shortcut/supported-surface) | Hookpoints | Review (test-audit/multi-concern/justification/commit-hygiene) | PR |
|---|---|---|---|---|---|---|
| 06-locate | `research/06-locate-problem-map.md` | `proposals/06-locate.md` | `risk/06-locate-*.md` | `research/06-locate-hookpoints.md` | `review/06-locate-*.md` | TBD |
| 06-schema-probe | `research/06-schema-probe-problem-map.md` | `proposals/06-schema-probe.md` | `risk/06-schema-probe-*.md` | `research/06-schema-probe-hookpoints.md` | `review/06-schema-probe-*.md` | TBD |
| 06-export | `research/06-export-problem-map.md` | `proposals/06-export.md` | `risk/06-export-*.md` | `research/06-export-hookpoints.md` | `review/06-export-*.md` | TBD |
| 06-pause-handshake | `research/06-pause-handshake-problem-map.md` | `proposals/06-pause-handshake.md` | `risk/06-pause-handshake-*.md` | `research/06-pause-handshake-hookpoints.md` | `review/06-pause-handshake-*.md` | TBD |
| 06-import-replace | `research/06-import-replace-problem-map.md` | `proposals/06-import-replace.md` | `risk/06-import-replace-*.md` | `research/06-import-replace-hookpoints.md` | `review/06-import-replace-*.md` | TBD |

## Decision gate

Per feature, the user reads the risk-cleared proposal at Phase 4
exit and confirms hookpoints at Phase 5 exit. Phase 10 promotion
is human-owned per the pipeline.

## Log

- **2026-04-30** — Initiative opened. User pointed at five harness
  feature request docs. Sequencing confirmed as 1 → 5 → 2 → 4 → 3
  (technical-dependency order). Single initiative with five separate
  proposals/risks/PRs. Worktree `worktrees/06-locate` created off
  `main`; feature 06-locate Phase 2.5 prompt dispatched.
