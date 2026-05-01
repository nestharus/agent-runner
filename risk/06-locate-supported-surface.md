# 06-locate — Phase 4 Supported-Surface Risk Report (Rev 2)

**Termination signal:** `none`
**LOW / MEDIUM / HIGH:** **LOW**

All four Rev 1 advisory findings are closed. A1–A9 still hold; A4
keeps the narrowing it gained in Rev 1 and Rev 2 makes the Codex side
of that narrowing explicit by deferring `payload.cwd` to a Phase 5
hookpoint and fail-closing every Codex session to exit `12` in v1.
Net value remains positive: problem-map §6 #1–11 stay retired, with
#8 retired for Claude and informationally-equivalent-to-status-quo
for Codex (the harness has no Codex workspace-root answer today
either). Blast radius across the six adjacent paths is unchanged
from Rev 1 except that the §11.1 `migrate-db` claim is no longer
inaccurate. One new advisory: locate v1 provides zero support for
Codex sessions, so the harness's v1 Codex locator must stay in place
until Phase 5 closes the gap.

## R1 advisory closure

| R1 advisory | Status | Evidence |
| --- | --- | --- |
| F1 (Codex `payload.cwd` schema unverified) | closed | Rev 2 §4 step 8 Codex branch drops the `payload.cwd`/`payload.workspace_root` commitment entirely and treats all Codex sessions as exit `12`. §1.1 A4, §9.1 D7 row, and §12 residuals are aligned. The unverified schema is no longer load-bearing. |
| F2 (Claude path-hash tiebreaker) | closed | Rev 2 §4 step 8 Claude branch: "generate candidate decompositions in longest-prefix-of-existing-path-first order ... If two or more decompositions both yield existing paths, treat it as exit `12`." §9.1 adds a dedicated D7 ambiguity test row covering zero/one/multiple existing decompositions. |
| F3 (`migrate-db` overpromise) | closed | Rev 2 §11.1 replaces the prior overpromise with "Repair of partial-chain DBs is outside this PR's scope and is handled by the broader Initiative-05 backfill flow." §12 honestly records that `backfill_session_chains` skips on existing chain rows and partial sessions map to exit `10`. |
| F4 (Codex synthetic fixture flag) | closed | Rev 2 §9.1 D7 row no longer commits to a Codex success fixture; it now exercises the fail-closed branch only ("Codex provider fixture with located JSONL but no supported root derivation"). The synthetic-schema risk is moot because the Codex success path is deferred. |

## Concern 1 — Assumption invalidation check (Rev 2)

- **A1 — single-owner common case — HOLDS.** Resolver code unchanged
  by Rev 2; `candidate_chain_ids` + `choose_resume_chain` +
  `active_segment_for_chain` at `src-tauri/src/state/db.rs:2696-2764`
  still produce the single-owner answer.
- **A2 — ambiguity = `ResumeError::Ambiguous` — HOLDS.** §4 step 4
  D1a unchanged; only the multi-recent collapse case becomes exit
  `11`.
- **A3 — `transcript_locator` resolves canonical JSONL without
  provider spawn — HOLDS.** §4 step 7 still calls `locate_transcript`
  at `src-tauri/src/sessions/mod.rs:171-199`; bundled scripts still
  do filename + content fallback only.
- **A4 — workspace_root derivable (REPHRASED in Rev 2) — HOLDS.**
  Claude derivation: §4 step 8 commits to
  longest-prefix-of-existing-path-first inversion with
  multi-existing → exit `12`; encode direction exists at
  `src-tauri/src/migration/mod.rs:155-188`; §9.1 D7 ambiguity row
  tests zero/one/multiple-existing cases. Codex deferral: §1.1 A4,
  §4 step 8 Codex branch, §9.1 D7 row, and §12 all agree on exit
  `12` for v1. The new A4 invalidator — "Phase 5 proves a stable
  Codex workspace-root field and risk gates require folding it into
  v1 rather than a follow-up" — is falsifiable in two parts: Phase 5
  either samples real Codex rollout JSONL with a stable root field
  or it doesn't, and risk gates either do or don't insist on
  Phase-5-blocking inclusion. The fail-closed shape (exit `12`,
  never partial JSON) keeps derivation failure from producing wrong
  roots; harness `01-session-locate.md:35` accepts
  `unsupported-storage` over partial location.
- **A5 — `[providers.session_storage]` is the source of storage
  discrimination — HOLDS.** D2b unchanged; serde tag remains `codex`,
  output vocabulary remains `codex_session`.
- **A6 — logically read-only despite physical open side effects —
  HOLDS.** §8 caveat unchanged; physical read-only deferred to
  06-schema-probe per `initiatives/06-session-override-contract.md:118-120`.
- **A7 — chain membership for direct CLI sessions after Initiative
  05 — HOLDS.** §4 step 4 D4a maps segmentless `session_turns` to
  exit `10`; §11.1 + §12 now honestly record the partial-DB residual
  rather than overpromising migrate-db repair.
- **A8 — `mutable` is composite, not stored — HOLDS.** §3 D3
  five-condition definition unchanged.
- **A9 — `mutable` excludes `exhausted_at` — HOLDS.** §3 D3 + §8
  forbid quota reads.

**Termination signal #1 (`invalidated-assumption`) does not fire.**

## Concern 2 — Net value (Rev 2)

Rev 2 retires the same eleven §6 entries Rev 1 retired:

| §6 entry | Rev 2 retirement |
| --- | --- |
| #1 owner discovery via SQL/trace/resume | §3 + §4 single subcommand |
| #2 SQL exposes tables, not contract | §3 schema; §10 README |
| #3 trace --json invocation-tree scoped | §4 arbitrary-session input |
| #4 resume requires spawn | §8 forbids provider commands |
| #5 resume-list is human text | §3 single-line JSON |
| #6 mutable is not single observable | §3 D3 boolean; §9.1 mutable row |
| #7 storage-type implicit in TOML | §3 storage_type enum |
| #8 workspace-root unobservable | §3 workspace_root for Claude; Codex fail-closed (informationally equivalent to today's no-answer state for Codex) |
| #9 locator failures non-durable | §3 fail-closed exit 12 |
| #10 no chain_id + storage in one output | §3 emits both |
| #11 no persisted last located transcript | per-call but stable |

#8 deserves a closer look in Rev 2: locate exits `12` for every
Codex session in v1, so the §6 #8 gap remains open on the Codex side.
But the harness has no Codex workspace_root answer today either —
locate's fail-closed Codex outcome is informationally equivalent to
status quo on the Codex side. Net value still positive because the
Claude side moves from "ad-hoc / unobservable" to "stable JSON
contract", while Codex moves from "ad-hoc / unobservable" to
"explicit unsupported-storage refusal". An explicit refusal is a
strictly better signal than silence for the harness's fallback
logic.

**Termination signal #2 (`non-positive-value`) does not fire.**

## Concern 3 — Adjacent paths blast-radius

| Path | Verdict | Evidence |
| --- | --- | --- |
| `agents resume`, `agents repl --resume`, top-level `--resume` | PRESERVED | Locate is a new caller of the same `StateDb::resolve_resume`; existing callers unchanged. |
| `trace --json` | PRESERVED + DIVERGENT | `trace --json` keeps four-state graceful degradation; locate refuses every state except `available`. §10 documents the divergence. |
| `migrate-config` | UNCOUPLED | §7 + §11.1 explicit. |
| `migrate-db` | UNCOUPLED | **F3 resolved.** §11.1 no longer claims `migrate-db` is the user remediation for partial-chain DBs; §12 records the gap honestly. |
| Hidden `resume-list` | PRESERVED | Unchanged. |
| Direct CLI ingestion | PRESERVED | Locate is downstream-read-only. |

No path is BROKEN or DEGRADED.

## Concern 4 — Migration / rollback / observability

- **No user state one-shot**: VERIFIED. Locate adds no schema, no
  migration step.
- **Uninstall/revert rollback**: VERIFIED. §6 pins
  `TranscriptState`'s serde repr through the move into
  `src-tauri/src/session_metadata/`; trace JSON shape unchanged.
  The committed module path (R1-F08 closure) makes the rollback
  surface explicit.
- **No telemetry / no invocation rows / no quota reads**: VERIFIED
  via §8 enumerated forbidden writes and the explicit `STATE_DIR`
  mkdir clause (R1-F03 closure).

## Concern 5 — Harness acceptance criteria coverage

| Harness bullet | Coverage |
| --- | --- |
| Exactly one JSON object + exit `0` for known session | covered (§3 + §4 step 10) |
| Provider/account ownership via same chain/segment logic | covered (§4 step 4: `StateDb::resolve_resume`) |
| `storage_type` distinguishes `claude_code`, `codex_session`, `other` | covered (§3 D2b) |
| Missing/ambiguous/unsupported return stable error codes; no partial JSON | covered (§5; §3 fail-closed) |
| No transcript mutation, no quota/provider spawn | covered (§7 + §8) |
| Tests cover `transcript_locator`, no-locator, missing-file, Claude storage, Codex storage, ambiguous | **partial-covered.** Codex storage row in §9.1 D7 now exercises only the fail-closed branch by design. The harness bullet "Tests cover ... Codex storage" is satisfied as negative-only coverage; positive-path Codex coverage is deferred to Phase 5 follow-up. (See R2-F01.) |
| README documents command + JSON shape | covered (§10) |

The Codex coverage row is partial-by-design rather than partial-by-
oversight; the partial coverage is a direct, intentional consequence
of the Rev 2 deferral. Recording it here so Phase 6b sees the
fixture is fail-closed only.

## Concern 6 — Initiative-06 sequencing forward-compat

`SessionMetadata` field set unchanged from Rev 1; downstream needs:

- **06-export** needs `jsonl_path` + `provider_name` + `chain_id` —
  available for both providers (only `workspace_root` fail-closes
  for Codex in v1). If export needs `workspace_root` for Codex, it
  inherits the Phase-5 dependency.
- **06-import-replace** needs `chain_id` + `mutable` + `jsonl_path`
  + `workspace_root`. For Codex in v1, locate exits `12`, so
  import-replace cannot operate on Codex sessions through the
  supported metadata API until Phase 5 closes the gap. This is a
  sequencing constraint, not a locate Rev 2 contract defect.
- **06-pause-handshake** observes locks; not a metadata consumer.
  §12 records the future sixth `mutable` condition (R1-F07 closure).
- **06-schema-probe** introduces the read-only open variant; §11.1 +
  §12 anticipate the follow-up.

`MetadataError` reserved siblings 13–17 can still be added without
breaking external consumers; forward-compat preserved.

## Findings

- **R2-F01 (advisory)** — Locate v1 provides zero support for Codex
  sessions because §4 step 8 Codex branch fails closed unconditionally.
  This means the harness's v1 direct-locator code path for Codex
  sessions cannot be retired by 06-locate; the harness must keep its
  Codex fallback in place until Phase 5 closes the workspace_root
  gap. §11.1's "Customer cohort: `agent-harness` is the primary
  consumer, replacing its v1 direct `state.db`/JSONL locator" should
  optionally clarify "for Claude sessions; Codex coverage follows
  Phase 5 hookpoint research." Not blocking — the harness spec
  accepts `unsupported-storage` as a stable refusal, and downstream
  Initiative 06 features (notably 06-import-replace) inherit the
  same Codex sequencing dependency.

- **R2-F02 (cosmetic)** — §4 step 8 Claude branch wording: "generate
  candidate decompositions in longest-prefix-of-existing-path-first
  order and pick the first interpretation whose decoded path exists
  on the filesystem. If two or more decompositions both yield
  existing paths, treat it as exit `12`." The "pick the first ...
  exists" clause is operationally inert because the next clause
  requires enumerating all candidates to detect the multi-existing
  case. The §9.1 D7 ambiguity row's phrasing — "longest-prefix-
  existing is deterministic only when it yields a single existing
  decoded path" — is the cleaner statement of the rule. Not a
  contract problem; just a wording overlap that the implementer can
  resolve by treating §9.1 as authoritative.
