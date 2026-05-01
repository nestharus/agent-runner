# 06-locate — Phase 4 Supported-Surface Risk Report (Rev 1)

**Termination signal:** `none`
**LOW / MEDIUM / HIGH:** **LOW**

Termination signals do not fire: A1–A9 hold (one HOLDS-with-rephrasing
on A4 for Codex, but the proposal already routes that case fail-closed
to exit `12 unsupported-storage` rather than depending on a Codex
schema it has not verified). Net value is positive — locate retires
problem-map §6 #1–11 with a single stable JSON contract on the
existing supported CLI surface, and is additive enough that "uninstall
the binary" is a complete rollback. Blast radius across the six
adjacent supported paths is bounded: locate shares only the read-only
resolver with `resume`/`repl --resume`/top-level `--resume`, diverges
deliberately from `trace --json`'s graceful degradation, and does not
couple to `migrate-config`, `migrate-db`, hidden `resume-list`, or
direct CLI ingestion. Findings are advisory documentation/test-fixture
items, not blockers.

## Concern 1 — Assumption invalidation check

Walked A1–A9 from proposal §1.1 against current code.

### A1 — single-owner common case — HOLDS

`StateDb::resolve_resume` returns one chain via `candidate_chain_ids`
+ `choose_resume_chain` with the single-match early return at
`src-tauri/src/state/db.rs:2718-2719`, then reads exactly one active
segment via `active_segment_for_chain` ordered by `started_at DESC, id
DESC` at `src-tauri/src/state/db.rs:2751-2764`. Resolver's
single-match path is unchanged by this proposal. ✓

### A2 — ambiguity = `ResumeError::Ambiguous`, not multi-row — HOLDS

`choose_resume_chain` at `src-tauri/src/state/db.rs:2713-2748`
collapses multi-row inputs to a single chain when exactly one is
recent (24h cutoff, line 2741) or when none are recent (latest by
`last_used_at`, lines 2744-2747); only the multi-recent case returns
`Ok(None)`, which then becomes `ResumeError::Ambiguous` at
`src-tauri/src/state/db.rs:2599-2607`. Initiative constraint
"reuse `StateDb::resolve_resume`; no second ownership path"
(`initiatives/06-session-override-contract.md:112-113`) forecloses
strict multi-row enumeration. D1a in §4 step 4 mirrors this. ✓

### A3 — `transcript_locator` resolves canonical JSONL without provider spawn — HOLDS

`locate_transcript` at `src-tauri/src/sessions/mod.rs:171-199` runs
the configured locator script with `SESSION_ID`/`STATE_DIR` only —
no provider command invocation. The bundled scripts `claude-code-locate-transcript:33-45`
and `codex-locate-transcript:27-39` do filename-suffix match plus
content-fallback against local JSONL stores, never spawning the
provider CLI. ✓

### A4 — `workspace_root` derivable from path/metadata — HOLDS, REPHRASED

This is the proposal's most fragile assumption. Verified in pieces:

- **Claude direction**: migration writes `target_dir =
  projects_dir.join(cwd_hash)` where `cwd_hash` is `source_path
  .parent().file_name()` (`src-tauri/src/migration/mod.rs:155-188`).
  This is the *encoding* direction (cwd → hash dir name as Claude's
  `-`-substitution); the *decoding* direction (hash dir → absolute
  cwd) is not exercised anywhere in the codebase today. The proposal
  §4 step 8 introduces it as new work. Hash decoding is heuristic for
  paths whose components contain `-` (`-home-foo-bar-baz` admits
  multiple interpretations), and the proposal's "require that path
  to exist" guard is the only disambiguator — for a pathological
  case where two interpretations both exist, the proposal does not
  define a tiebreaker. Rephrased: "Claude path-hash inversion
  succeeds for the well-behaved common case; ambiguous cases are
  documented as exit `12`."
- **Codex direction**: §4 step 8 cites `payload.cwd`/
  `payload.workspace_root` in `session_meta` and claims the precedent
  is `codex-locate-transcript:45-60`. That script reads only
  `payload.id` (`scripts/codex-locate-transcript:45-60` confirmed) —
  there is no evidence in the agent-runner repo that real Codex
  rollout JSONL contains `payload.cwd` or `payload.workspace_root`.
  The proposal extends the precedent of "JSON-inspecting Codex
  rollout files" but does not show real-world Codex schema evidence
  for the cwd field. Rephrased: "Codex workspace-root derivation
  depends on real `session_meta.payload.cwd` presence; absent it,
  every Codex session exits `12`."

The proposal's fail-closed contract — on derivation failure, exit
`12 unsupported-storage`, never partial JSON — keeps this rephrasing
from invalidating the assumption: a wrong workspace root is never
emitted, only refusal. The harness spec explicitly accepts
`unsupported-storage` as a stable error code. A4 holds in its
narrowed form. (See Findings F1, F2.)

### A5 — `[providers.session_storage]` is the source of storage discrimination — HOLDS

`SessionStorage` is a serde-tagged enum with exactly two variants
`ClaudeCode { projects_dir }` and `Codex { sessions_dir }` at
`src-tauri/src/config/model.rs:195-229`, with serde tags `claude_code`
and `codex`. `ProvidersConfig::effective_provider`/`runtime_provider`
at `src-tauri/src/config/providers.rs:116-134,157-190` propagate this
into the runtime provider config. D2b's choice — keep internal
`Codex`, emit `codex_session` only at the `locate` JSON boundary —
matches existing precedent for vocabulary translation. ✓

### A6 — logically read-only despite physical open side effects — HOLDS

`StateDb::open` at `src-tauri/src/state/db.rs:431-608` (verified
lines 431-490+) creates parent dirs (line 432-435), enables WAL
(line 439-440), ensures invocations/providers/quotas/memory/sessions
schemas (lines 441-608), and Phase 1 of the open path also runs
backfill. These are physical side effects on every open. The
initiative explicitly assigns the read-only open variant to
06-schema-probe (`initiatives/06-session-override-contract.md:118-120`).
The proposal's §8 contract ("logically read-only after open;
physical read-only deferred to 06-schema-probe") and §12 residuals
both name this gap honestly. Sequencing is consistent with
initiative §75-89: schema-probe lands second exactly to ship the
read-only variant. A6 holds as a sequencing claim, not as a
behavioral claim about v1. ✓

### A7 — chain membership for direct CLI sessions after Initiative 05 — HOLDS

`scan_provider` calls `mint_imported_chain_if_absent` immediately
after `ingest_session_turns_batch` for every freshly-ingested turn
at `src-tauri/src/sessions/mod.rs:125-141`. The known
counterexample — `backfill_session_chains` skips entirely when any
chain row exists at `src-tauri/src/state/db.rs:2256-2271` — produces
"segmentless" `session_turns` rows that the resolver maps to
`NoChainFound`, which the proposal (D4a, §4 step 4) explicitly maps
to exit `10 session-not-found`. Locate does not reach into
`session_turns` directly. ✓

### A8 — `mutable` is composite, not stored — HOLDS

`session_turns`/`session_chains`/`session_chain_segments` schemas at
`src-tauri/src/state/db.rs:559-592` have no `mutable` column.
Active-segment existence is read at line 2751-2764 (`ended_at IS
NULL`); resume-block presence is checked downstream of resume at
`src-tauri/src/main.rs:1154-1162`; storage block is the
`session_storage` field already covered under A5. The §3 D3
five-condition definition (active segment ∧ first-class storage ∧
resume block ∧ available `jsonl_path` ∧ available `workspace_root`)
is composable from these existing reads alone. ✓

### A9 — `mutable` excludes `exhausted_at` — HOLDS

`exhausted_at` is on `provider_quotas` keyed by `provider_name`,
not on any session table (`src-tauri/src/state/db.rs:455-463`),
making it provider-account global rather than session-scoped.
Including it in `mutable` would conflate routing policy with
identity metadata. The harness's defensive contract in
`01-session-locate.md:71` ("Does not expose ... quota windows")
endorses this exclusion. Defensible. ✓

**Termination signal #1 (`invalidated-assumption`) does not fire.**
A4 is properly rephrased into a fail-closed shape; the proposal's
§3 D3 and §5 exit-12 guard depend on the rephrased form, not on
the unverified Codex schema.

## Concern 2 — Net value on the supported surface

### Risk reduced

| problem-map §6 entry | Retired by |
| --- | --- |
| #1 — "where is this session" requires SQL/trace-with-uuid/resume-attempt | §3 stable JSON contract; §4 single subcommand |
| #2 — SQL exposes tables, not stable contract | §3 schema; §10 README documents fields as stable |
| #3 — `trace --json` is invocation-tree scoped | §4 arbitrary-session input via resolver |
| #4 — `resume` exposes owner only by attempting spawn | §8 forbids provider commands |
| #5 — `resume-list` is human text | §3 single-line JSON object |
| #6 — `mutable` is not a single observable | §3 D3 boolean, §9.1 mutable test row |
| #7 — storage-type is implicit in TOML | §3 `storage_type` enum |
| #8 — workspace-root is unobservable | §3 `workspace_root` (subject to A4 fail-closed) |
| #9 — locator failures are non-durable warnings | §3 fail-closed exit `12`; not durable but observable |
| #10 — no output combines `chain_id` + storage | §3 emits both |
| #11 — no persisted "last located transcript" | locate is per-call but stable; same as today |

### Blast radius added

| New failure mode | Guard |
| --- | --- |
| Locate over-refuses for paths with `-` in components | A4 rephrased; exit `12`, never partial |
| Locate over-refuses every Codex session if no `payload.cwd` | A4 rephrased; same fail-closed shape |
| `locate` output drifts from `trace --json` semantics | §10 README explicitly distinguishes |
| Resolver behavioral change leaks to locate | None — locate explicitly inherits, doesn't extend |

The fail-closed shape is harness-aligned: the spec at
`01-session-locate.md:35` literally requires `unsupported-storage`
rather than partial location. Worst-case A4 collapse (every Codex
session exits `12`) is still no regression on the current state,
where the harness has *no* answer for Codex workspace-root today
(problem-map §6 #8); locate would force the harness to fall back
to Codex-via-`-m` resume-only paths, which is what the harness
already does.

### Migration / rollback claim accuracy

- "no user state one-shot is required" — accurate; locate adds no
  schema, no migration step, and reads only existing tables.
- "uninstall/revert the binary or avoid the new subcommand" — accurate
  with one nuance: §6 says `TranscriptState` moves out of `trace`
  into `session_metadata`. The serde tag/values stay snake-case
  (`unresolved`/`no_locator`/`missing`/`available`) and the JSON
  output of `trace --json` is unchanged. Rust-level downstream
  consumers don't exist outside agent-runner, so a binary uninstall
  is a complete rollback for both subcommands. ✓
- "no telemetry, no invocation rows, no trace records, no quota reads"
  — accurate per §8 side-effect contract; the only durable I/O is
  reads (state DB, configs) plus the locator script's optional
  `STATE_DIR` mkdir at `src-tauri/src/sessions/mod.rs:184-185`,
  which is the same I/O `trace --json` already performs.

### Net-value verdict — POSITIVE

Eleven distinct §6 entries retired; four blast-radius items, all
fail-closed and within harness-defined error codes. **Termination
signal #2 (`non-positive-value`) does not fire.**

## Concern 3 — Adjacent paths blast-radius

| Path | Verdict | Evidence |
| --- | --- | --- |
| `agents resume`, `agents repl --resume`, top-level `--resume` | PRESERVED | All call `StateDb::resolve_resume` (`src-tauri/src/main.rs:341-389`, `:1056-1199`); locate adds a new caller of the same function — read-only, no behavioral change to existing callers. |
| `trace --json` | PRESERVED + DIVERGENT | `trace --json` keeps `transcript_state ∈ {unresolved, no_locator, missing, available}` graceful degradation (`src-tauri/src/trace/mod.rs:300-382`); locate refuses every state except `available` with exit `12`. Divergence is documented in §10 README and is a feature: trace inspects, locate contracts. ✓ |
| `migrate-config` | UNCOUPLED | §7 anti-scope and §11.1 explicit; no shared state. |
| `migrate-db` | UNCOUPLED, but §11.1 claim partly inaccurate | `migrate-db` runs `backfill_session_chains` (skips on existing chain rows) + `run_compaction_backfill` (uses `locate_transcript` for compaction-boundary flagging at `src-tauri/src/main.rs:1909-1966`). For partial DBs that already have any chain rows, `migrate-db` will *not* repair segmentless `session_turns` (problem-map §2.3). §11.1's claim "users can run existing `agents migrate-db` when they need backfill repair" overpromises: it covers compaction repair, not chain-segment repair. Documentation accuracy issue, not behavioral risk. (See Finding F3.) |
| Hidden `resume-list` | PRESERVED | `src-tauri/src/main.rs:1887-1900`; unchanged. |
| Direct CLI ingestion | PRESERVED | `scan_provider` and adapters at `src-tauri/src/sessions/mod.rs:55-141`; locate is a downstream reader of the chain state ingestion already mints. |

No path is BROKEN or DEGRADED by locate.

## Concern 4 — Migration / rollback / observability

- **No user state one-shot**: VERIFIED. Locate adds no schema, no
  migration; reads chain/segment tables already present after
  Initiative 05.
- **Uninstall/revert rollback**: VERIFIED with the
  `TranscriptState`-move caveat addressed. The enum's serde
  representation is the only public surface, and §6 pins it. Trace
  JSON output (`src-tauri/src/trace/mod.rs:73-91`) is unchanged.
- **No telemetry / no invocation rows / no quota reads**: VERIFIED
  by §8 enumerated forbidden writes. The only "soft" read beyond
  pure SQL is `locate_transcript`'s `STATE_DIR` mkdir, which trace
  already performs and which the proposal does not categorize as a
  state mutation.

## Concern 5 — Harness acceptance criteria coverage

| Harness bullet (`01-session-locate.md:48-56`) | Coverage |
| --- | --- |
| Exactly one JSON object + exit `0` for known session | covered (§3 + §4 step 10) |
| Provider/account ownership via same chain/segment logic as `agents resume` | covered (§4 step 4: `StateDb::resolve_resume`; D1a; D4a) |
| `storage_type` distinguishes `claude_code`, `codex_session`, `other` | covered (§3 D2b table) |
| Missing/ambiguous/unsupported return stable error codes; no partial JSON | covered (§5 exit codes; §3 fail-closed contract) |
| No transcript mutation, no quota/provider spawn | covered (§7 anti-scope; §8 side-effect contract) |
| Tests cover `transcript_locator`, no-locator, missing-file, Claude storage, Codex storage, ambiguous | covered (§9.1 rows: D2 mapping, D2 unsupported, D6 transcript state, D7 derivation, D1 ambiguity) — partial coverage caveat: §9.1 D7 row uses synthetic Codex `session_meta` cwd fixtures whose schema is not validated against real Codex output (Finding F4). |
| README documents command + JSON shape | covered (§10) |

All seven bullets are at minimum partial-covered; one (Codex storage
test fixture) carries a fixture-realism caveat that is implementer-
visible and not a proposal-level gap.

## Concern 6 — Initiative-06 sequencing forward-compat

The §6 `SessionMetadata` API has eight fields covering all metadata
that the harness defines for `locate`. Forward-compat for the
initiative's other features:

- **06-export** needs `jsonl_path` + `provider_name` + `chain_id` —
  all present.
- **06-import-replace** needs `chain_id` + `mutable` + `jsonl_path`
  + `workspace_root` (for cwd-aware writes) — all present, and
  `mutable: false` correctly tells import-replace to refuse.
- **06-pause-handshake** observes locks; not a SessionMetadata
  consumer.
- **06-schema-probe** introduces the read-only open variant; locate
  will need to consume that, which §11.1 and §12 anticipate as a
  follow-up.

`MetadataError` has variants matching shared error codes 10/11/12
plus `Operational` and `InvalidSessionId`. Reserved siblings 13/14/
15/16/17 from `initiatives/06-session-override-contract.md:106-111`
can be added as new variants without breaking existing consumers
because Rust's enum exhaustiveness for `MetadataError` is internal
to the API — external consumers see only the JSON error code on
stderr. Forward-compat: ✓.

One soft constraint: Phase 5 hookpoint research will need to verify
that `TranscriptState`'s serde repr is unchanged after the move out
of `trace::mod`, so trace JSON output is bit-stable. §6 names this
explicitly: "If Phase 5 hookpoint research proves that move would
materially change trace behavior, stop and revise this proposal."
This is the right escape hatch.

## Findings

- **F1 (advisory)** — A4 Codex `payload.cwd`/`payload.workspace_root`
  derivation has no evidence in the repo that real Codex
  `session_meta` payloads carry these fields. The cited precedent
  at `scripts/codex-locate-transcript:45-60` reads only `payload.id`.
  Proposal's fail-closed exit `12` keeps this from becoming a
  correctness risk, but the harness's "Codex storage" test bullet
  may end up exercising a synthetic schema rather than a real one.
  Phase 6 should pin against a real Codex rollout sample or document
  the synthetic-fixture limitation.

- **F2 (advisory)** — A4 Claude project-hash inversion (decode
  `<project-dir>` to absolute cwd) is heuristic for paths with `-`
  in components. §4 step 8's "require that path to exist" guard
  does not define a tiebreaker for cases where multiple
  interpretations both exist. Phase 6 should either pick a
  deterministic interpretation (longest-prefix-existing, leftmost-
  exists, etc.) or treat ambiguity as exit `12`.

- **F3 (advisory)** — §11.1 claim "users can run existing `agents
  migrate-db` when they need backfill repair" overpromises.
  `backfill_session_chains` skips entirely when any chain row exists
  (`src-tauri/src/state/db.rs:2256-2271`), so partial chain DBs are
  not user-fixable through the existing path. README/release notes
  should phrase the remediation more carefully, or 06-locate should
  exit `10 session-not-found` with a hint pointing at the actual
  remediation (which today is unspecified).

- **F4 (advisory)** — §9.1 D7 row's Codex `session_meta` cwd
  fixtures will be synthetic. Implementer should call this out in
  Phase 6b's index alongside the existing flag for new CLI
  integration fixture infrastructure.
