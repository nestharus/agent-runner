# 06-locate — Phase 4 Supported-Surface Risk Report (Rev 3)

**Termination signal:** `none`
**LOW / MEDIUM / HIGH:** **LOW**

Rev 3 folds Codex `payload.cwd` derivation into v1, supported by
Phase 5 sampling of 25 real Codex rollout files (Codex 0.46.0 +
0.58.0) where `session_meta.payload.cwd` was present in every
sampled file (`research/06-locate-hookpoints.md` §I.WS1). A1–A9
all hold; A4's Rev 3 form retains the fail-closed shape (missing
`session_meta` or absent `payload.cwd` → exit `12`) and replaces
the now-fired Phase-5-conditional invalidator with three
forward-looking clauses (Claude path-hash ambiguity, Codex schema
drift, path-less storage). Net value rises: problem-map §6 #8 is
now retired for both providers (was retired-for-Claude /
informationally-equivalent for Codex in Rev 2). Harness coverage
for "Codex storage" flips from partial-by-design to covered.
Adjacent-path blast radius unchanged. R2-F01's advisory
("locate gives zero Codex support") is superseded by Rev 3. One
cosmetic R3 finding below.

## Round 1/2 finding closure regression

| Finding | Rev 3 status |
| --- | --- |
| R1-F01, F03, F04, F06, F07, F08, F09 | still closed (Rev 3 untouched) |
| R1-F02 (Codex schema speculation) | now closed via empirical verification + folding into v1; §1.1 A4 evidence cites the Phase 5 sample |
| R1-F05 (Claude path-hash tiebreaker) | strengthened — Rev 3 §4 step 8 prose enumerates all decompositions, succeeds iff exactly one exists, else exit `12` |
| R2-F01 (path-hash prose ambiguity) | closed by Rev 3 §4 step 8 tightening |
| R2-F02 (resume-parity malformed config) | still inherited limitation (not a locate concern) |

No closure regressed.

## Concern 1 — Assumption invalidation check (Rev 3)

A1, A2, A3, A5, A6, A7, A9 — **HOLDS.** Resolver/locator/storage
code paths untouched by Rev 3; §4 / §8 sections unchanged.

A8 — **HOLDS.** §3 D3 unchanged. Rev 3 makes Codex sessions
mutable-eligible (always `false` in Rev 2 because workspace_root
failed); this is a positive expansion, not a contract change, and
§10 still frames `mutable` as a read-time eligibility hint.

A4 — **HOLDS in Rev 3 form.** Evidence is concrete: §1.1 A4
evidence cell cites Phase 5's 25-file sample across Codex 0.46.0
and 0.58.0 where `session_meta.payload.cwd` was present in every
file. New invalidator clauses are forward-looking and falsifiable:
(i) Claude path hashes yielding multiple existing decoded roots —
covered by §9.1 D7 ambiguity row → exit `12`; (ii) Codex schema
drift relocating/removing `payload.cwd` — covered by §4 step 8
Codex branch's missing-`session_meta` / absent-`payload.cwd`
fail-closed clauses; (iii) future storage types lacking
path/config provenance — covered by `other` exit-`12` rule. The
fail-closed shape is preserved everywhere — no partial JSON
emitted for derivation failure.

**Termination signal #1 (`invalidated-assumption`) does not fire.**

## Concern 2 — Net value (Rev 3)

§6 #1–#7 and #9–#11 retired same as Rev 2. The Rev 3 flip:

| §6 entry | Rev 3 retirement |
| --- | --- |
| **#8 workspace-root unobservable** | **now retired for BOTH providers (Rev 3 flip): Claude via path-hash inversion; Codex via `session_meta.payload.cwd` parsing. Was retired-for-Claude / informationally-equivalent for Codex in Rev 2.** |

Net value strictly increased over Rev 2: the Codex side of #8
moves from "explicit unsupported-storage refusal" (Rev 2) to
"stable canonical absolute UTF-8 workspace_root" (Rev 3) for the
sampled-and-confirmed common case, with fail-closed exit `12`
retained for any rollout where the field is absent or malformed.

**Termination signal #2 (`non-positive-value`) does not fire.**

## Concern 3 — Adjacent paths blast-radius

| Path | Verdict |
| --- | --- |
| `agents resume`, `repl --resume`, top-level `--resume` | PRESERVED |
| `trace --json` (four-state graceful degradation) | PRESERVED + DIVERGENT |
| `migrate-config` | UNCOUPLED |
| `migrate-db` | UNCOUPLED |
| Hidden `resume-list` | PRESERVED |
| Direct CLI ingestion | PRESERVED |

No path is BROKEN or DEGRADED. The new Codex JSONL line-walk uses
the same read-only pattern trace and migration already use; §8's
"no copy / create / rename / truncate / rewrite of JSONL files"
still holds.

## Concern 4 — Migration / rollback / observability

§11.1 claims hold under Rev 3:

- **No user state one-shot**: VERIFIED (no schema, no migration).
- **Uninstall/revert rollback**: VERIFIED (Rev 3 changes contained
  in `session_metadata` module + §4 step 8 Codex branch; trace
  JSON shape unchanged).
- **No telemetry**: VERIFIED via §8.

§11.1 does not need a Codex-specific update. The "agent-harness is
the primary consumer, replacing its v1 direct `state.db`/JSONL
locator" framing is provider-neutral and now factually true for
both providers.

## Concern 5 — Harness acceptance criteria coverage

Rev 2 marked the "Tests cover ... Codex storage" bullet
**partial-by-design**. Rev 3 §9.1 D7 row now exercises both Codex
success ("Codex provider fixture with a located rollout JSONL whose
`session_meta` line contains valid `payload.cwd`") and Codex
failure ("missing `session_meta`, absent `payload.cwd`, and
invalid paths") fixtures. Verdict flips to **covered**. All other
harness bullets remain covered.

## Concern 6 — Initiative-06 sequencing forward-compat

`SessionMetadata` field set unchanged from Rev 2. Downstream:

- **06-export** needs `jsonl_path` + `provider_name` + `chain_id`
  (+ optionally `workspace_root`) — now available for BOTH
  providers in v1.
- **06-import-replace** needs `chain_id` + `mutable` +
  `jsonl_path` + `workspace_root` — now available for BOTH
  providers in v1. **This does not promise Codex support in
  06-import-replace's MVP.** Locate's role is to expose stable
  metadata; whether import-replace operates on Codex is a
  sibling-feature scope decision. §13 makes no claim about which
  providers import-replace must support.
- **06-pause-handshake** unchanged; §12 still records the future
  sixth `mutable` condition.
- **06-schema-probe** unchanged; §11.1 + §12 still anticipate the
  read-only open variant.

`MetadataError` reserved siblings 13–17 unchanged; forward-compat
preserved.

## Findings

- **R3-F01 (cosmetic)** — §4 step 8 Codex branch says "read the
  located rollout JSONL line-by-line until a `session_meta` record
  is found", but does not specify how a `session_meta` record is
  identified on a JSONL line (e.g., `type == "session_meta"` or
  another discriminator). Phase 5 sampling confirms the convention
  is stable, and §9.1 D7 row pins behavior via success + failure
  test obligations, so the Phase 6 implementer is bounded by
  tests. Not a contract problem; the implementer can match the
  existing `scripts/codex-locate-transcript` line-walk pattern that
  the proposal cites.
