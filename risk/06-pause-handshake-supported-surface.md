# 06-pause-handshake — Phase 4 Supported-Surface Risk Report (Rev 2)

**Termination signal:** `none`
**Verdict:** **LOW**

Rev 2 closes the four R1 audit findings called out in the proposal's
Rev 2 changelog (R1-F01..R1-F04). No new supported-surface regression
introduced. Two prior Rev 1 advisory findings (R1-F01-supported user-
facing orphan UX, R1-F06 README v1-vs-eventual phrasing) are partially
or fully addressed by §10's expanded README mandate. Two remaining
advisories (Windows residual, CLI `observe` ergonomics) are unchanged
non-blockers.

## Closure check on R1-F01..R1-F04 (audit-only, cross-track)

| Finding | Rev 2 evidence | Status |
| --- | --- | --- |
| **R1-F01** idempotent release marker shape deferred | Rev 2 picks the concrete sibling-file shape `<lock_dir>/session-<uuid>.released`. Defined in §6 (versioned JSON, `token_hash`, `released_at`); receipt field `release_marker_path` in §3.2; acquire removes prior marker under flock in §4 step 11; resume reads marker as idempotency evidence in §4 steps 13–17; §8 mandates create/update on release and stale-marker removal during fresh acquire; §12 explicit "no future marker-shape deferral". | **CLOSED** |
| **R1-F02** writer-path observer wiring deferred without explicit harness-surface narrowing | Rev 2 narrows in §1 ("v1 lock enforcement is advisory until sibling PRs wire observers"; "harness consumer should treat the lock as advisory in v1"), §12 residual ("harness acceptance surface is narrowed"), §13 compliance row marked "Partial by design (deferred to sibling PRs)" with each sibling PR named. README §10 mandate now requires the advisory-scope sentence. | **CLOSED** |
| **R1-F03** `StateDb::open` mutation exception unpinned | Rev 2 §8 adds: "open the state DB via `StateDb::open_default()` for resolver-only access. Inherent `StateDb::open` side effects (parent dir creation, WAL enable, schema-ensure, chain backfill) are accepted, matching 06-locate and 06-export's §8 contracts. No DDL, no row mutation, no `session_turns`/`session_chains`/`session_chain_segments` writes." §12 residual reiterates inheritance until 06-schema-probe's read-only open lands. | **CLOSED** |
| **R1-F04** §9.1 missing `assumption_link` + `residual_risk` columns | Rev 2 §9.1 matrix now exposes both columns on every row; assumption ids reference §1.1 A1–A7. | **CLOSED** |

All four R1 audit findings are addressed by structural proposal changes,
not by deferral or hand-wave. No closure is asserted without
corresponding section text.

## Fresh assessment of Rev 2 changes

### Marker shape decision (R1-F01) — supported-surface impact

The chosen sibling-file shape (`session-<uuid>.released` next to
`session-<uuid>.lock` in the same `0700` lock dir) is supported-surface
clean:

- Marker accumulation bounded: §4 step 11 removes any prior marker
  during fresh acquire under the same flock, so per-session marker count
  is at most one between release and next acquire.
- Owner-private permissions: §8 contract covers lock state generally;
  marker file inherits the `0600` files / `0700` dir mandate (§8
  "Permissions are contract surface" + Rev 2 explicit `0700`/`0600`
  language). Permission failure remains exit `1`, not silent downgrade.
- Rollback: marker files are inert to older binaries (older binaries do
  not read `~/.local/share/oulipoly-agent-runner/locks/` per Rev 1
  Concern 4); operators may delete after confirming no Rev-2 binary is
  observing. Verified — the marker shape change does not alter the
  rollback story established in Rev 1.
- Receipt observability: §3.2 exposes `release_marker_path` in the
  release receipt, giving harness/operator a stable inspection surface
  beyond `cat`/`ls`.

No new blast-radius paths added. The §6 release marker schema is
versioned (`version: 1`), preserving forward-compat for sibling-PR
consumers.

### Harness-surface narrowing (R1-F02) — supported-surface impact

Rev 2's explicit advisory-lock framing improves the supported-surface
posture by making honest scope claims load-bearing in three places (§1,
§12, §13) and one user-facing place (§10 README mandate). The Rev 1
Concern 5 row that read "partial-by-design" now has crisp language for
the harness consumer to plan around. This **partly closes prior Rev 1
advisory R1-F06** (README v1-vs-eventual sentence) by §10's new mandate:
"State that sibling writer-path enforcement is advisory until
06-import-replace, migration, resume/repl, and balanced one-shot wire
observers in their own PRs." Same §10 sentence also partly addresses
prior Rev 1 advisory R1-F01-supported (orphaned-lockfile UX from sibling
writes during a held lock) by communicating the v1 boundary to users
before they assume coverage.

### `StateDb::open` clause (R1-F03) — supported-surface impact

§8's explicit "matching 06-locate and 06-export's §8 contracts" pins
this proposal to the same inheritance posture as its sibling Initiative
06 commands. This is the right call for supported-surface consistency:
operators reading any of the three §8 contracts get the same set of
accepted open-time effects. No new schema, no new DDL, no new row
mutation surface. Rollback story unchanged — DB schema is unmodified by
this PR.

### §9.1 columns (R1-F04) — supported-surface impact

The new `assumption_link` and `residual_risk` columns let the test
matrix carry honest deferral language inline with each track. Notable
residual_risk entries that matter for supported-surface evidence:

- "Writer-path advisory scope" track residual_risk: "Full mutual
  exclusion remains cross-PR work, not validated in this PR." Matches
  the §1.2 / §12 / §13 narrowing.
- "Side effects" track residual_risk: "Cannot enforce future
  `StateDb::open` internals; read-only open is a follow-up." Matches §8.
- "Idempotent replay" track residual_risk: "Does not preserve
  idempotency after a later acquire removes the marker." This is a
  contract-correct statement (§4 step 11 removes prior marker
  pre-acquire by design); harness retries past the next acquire are not
  intended to replay.

Test matrix now self-documents what this PR does and does not prove.

## No-regression check vs Rev 1 supported-surface findings

| Rev 1 advisory | Rev 2 status |
| --- | --- |
| R1-F01-supported (orphaned-lockfile UX during sibling writes) | **partly addressed** by §10 advisory-scope mandate; root-cause fix still belongs to sibling PRs (D4b). |
| R1-F02-supported (Phase 5 marker shape) | **closed** — Rev 2 §6/§12 commit the sibling-file shape; no Phase 5 deferral remains. |
| R1-F03-supported (A2 multi-active-segment edge) | **unchanged** — same contract; same mitigation (sibling adoption). |
| R1-F04-supported (Windows residual in README) | **unchanged** — §12 still says "Windows semantics are not designed"; §10 README mandate still does not name supported platforms. Non-blocking. |
| R1-F05-supported (CLI `observe` ergonomics) | **unchanged** — `SessionLockManager::observe` still library-only in §6; future sibling-adoption PR can expose. Non-blocking. |
| R1-F06-supported (README v1-vs-eventual sentence) | **closed** — §10 now mandates the advisory-scope sentence. |

No Rev 2 change degrades Rev 1's adjacent-paths verdicts (locate /
schema-probe / export / migrate-db / resume / repl / trace / GUI /
sessions.toml all preserved or intentionally uncoupled per D4b).
Migration / rollback story unchanged: no schema, no DDL, no chain/
segment changes; lock state and marker state both inert to older
binaries. Observability deliberately bounded to receipts + stderr JSON
+ lockfile/marker metadata.

## Verdict rationale

- **Termination signal #1 (`invalidated-assumption`)** — does not fire.
  A1–A7 hold against problem map evidence; Rev 2 does not introduce a
  new assumption that contradicts current state.
- **Termination signal #2 (`non-positive-value`)** — does not fire.
  The Rev 1 retired-risk table (§3.1 second-pause arbitration, §3.3
  token identity, §3.4 TTL/expires_at, §3.5 crash recovery, §3.16
  structured session-busy, §3.17 stable lock_path, obs-gaps #1–3) is
  preserved; Rev 2 strengthens the partial-coverage honesty without
  retracting any retired-risk claim.

**Standard verdict: LOW.** Phase 4 supported-surface gate **passes** at
Rev 2. Remaining advisories (Windows residual, CLI `observe`) are
implementer guidance, not blockers, and are documented in §12 / §6.
Phase 5 hookpoint research and Phase 6 implementation may proceed once
the other Phase 4 reports also clear LOW at Rev 2.

## Advisory items carried forward (non-blocking)

1. README §10 should still name Linux/macOS as v1-supported with
   Windows behavior undefined (R1-F04-supported), to match §12's
   "Windows semantics are not designed" residual.
2. Sibling adoption PR (resume / repl / migrate / balanced one-shot)
   should consider exposing `SessionLockManager::observe` as
   `agents session observe <id>` for first-class read inspection
   (R1-F05-supported).
3. Sibling-PR observers should refuse-and-emit structured stderr JSON
   `session-busy` rather than waiting silently, preserving the §1.2
   "stable refusal surface" framing.
