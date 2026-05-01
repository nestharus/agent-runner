# 06-pause-handshake — Phase 4 Scope Risk Assessment (Rev 2)

**Assessor:** scope reviewer
**Verdict:** **LOW** — Rev 2 closes Round 1 audit findings R1-F01..R1-F04
without expanding scope. The proposal still stays inside the harness ask
(`04-session-pause-handshake.md`) and the Initiative 06 cross-feature
constraints (`initiatives/06-session-override-contract.md:106-122`).
Every §2–§6 design hunk traces back to a harness behavior, an initiative
constraint, or a problem-map gap; every §7 anti-scope clause holds
across §2–§6. The only structural decomposition decision (D4b — defer
sibling write-path observation to later PRs) is sanctioned by the
initiative and is now disclosed *more* honestly via Rev 2's "advisory
in v1" framing.

## Round 1 closure check (audit only)

| ID | Round 1 ask | Rev 2 close | Closed |
| --- | --- | --- | --- |
| R1-F01 | Idempotent-release marker storage was a Phase 5 deferral with two enumerated options. | Rev 2 §6 picks the sibling-file shape concretely (`session-<uuid>.released`, JSON with `version`, `token_hash`, `released_at`); §3.2 surfaces `release_marker_path` in the resume receipt; §4 step 11 removes a stale marker on acquire and step 17 writes one on release; §8 lists marker creation/removal as the only new file mutations; §12 deletes the "marker-shape deferral" residual. | yes |
| R1-F02 | Writer-path observers were deferred without explicit harness-AC narrowing. | Rev 2 §1 names every deferred sibling path (`import-replace`, `migrate_chain_segment`, `run_repl`, `run_resume`, balanced one-shot), states the v1 lock is **advisory** until those land, and points at 06-import-replace specifically as the "first observer" PR. §12 D4b mirrors the same narrowing. §13 row 3 ("Partial by design") spells out the cross-PR completion path. | yes |
| R1-F03 | `StateDb::open` mutation exception was unpinned. | Rev 2 §8 adds an explicit `StateDb::open_default()` clause that enumerates accepted side effects (parent dir creation, WAL enable, schema-ensure, chain backfill) and pins them to the same shape as 06-locate / 06-export §8. §12 keeps the read-only-open follow-up tied to 06-schema-probe. | yes |
| R1-F04 | §9.1 lacked `assumption_link` / `residual_risk` columns. | Rev 2 §9.1 now has both columns on every row; assumption ids reference §1.1 (A1..A7), and each row carries a residual-risk note bounding what the test does *not* prove. | yes |

All four R1 findings are closed without producing follow-on scope creep
(see "Fresh assessment" below).

## Fresh assessment of Rev 2 deltas

| Rev 2 change | Direction | Magnitude | Scope verdict |
| --- | --- | --- | --- |
| Sibling marker file `session-<uuid>.released` with versioned JSON containing `token_hash` + `released_at` | adds one bounded file artifact and one new receipt field (`release_marker_path`) | small | In-scope. The harness ask requires same-token idempotent replay; Rev 1 already committed the behavior. Rev 2 only fixes the storage shape, which is mechanism, not behavior. The marker is data-only (no DB rows, no transcript, no provider IO) and lives next to the lockfile under the existing `locks/` dir. §8 bounds the file mutations precisely. |
| `release_marker_path` field in `resume-handshake` stdout | new receipt field beyond harness's named fields | tiny | In-scope. Same justification class as `chain_id` (Rev 1 LOW): the field surfaces a path the implementer already commits to creating, and observability gap §6.1 of the problem map asked for a structured release receipt. Harness contract specifies required fields, not forbidden ones. |
| §1 / §12 / §13 explicit narrowing of v1 harness acceptance surface to "advisory" | disclosure of existing D4b decomposition | none (clarification) | In-scope and **scope-positive**: Rev 2 makes the cross-PR completion path explicit instead of leaving R1 readers to infer it from D4b alone. The actual decomposition shape is unchanged — initiative §115 already names the deferred observers — Rev 2 just stops papering over it. Honest disclosure improves the harness consumer's mental model without enlarging the build. |
| §8 explicit `StateDb::open_default()` side-effect clause aligned with 06-locate / 06-export | side-effect bound, written down | none (consistency) | In-scope. This is alignment with sibling proposals' contracts, not new behavior. Inheriting `StateDb::open`'s open-time effects is the same posture every Initiative 06 feature takes. |
| §9.1 `assumption_link` + `residual_risk` columns | test-track metadata | none (process) | Documentation completeness, not scope. |
| §9.1 added test row "Writer-path advisory scope" + "Marker token mismatch" + "Missing lock wrong token" | new test rows tied to Rev 2's concretized behaviors | bounded | In-scope. Each row maps to a §4/§6 behavior Rev 2 commits to (advisory v1 disclosure, marker-vs-token mismatch on `16`, missing-lock + missing-marker on `16`). No row tests anti-scope behavior. |

**Net direction:** Rev 2 narrows v1 acceptance (advisory framing),
concretizes one previously deferred mechanism (marker shape), and pins
one previously implicit side-effect contract (`StateDb::open`). All
three moves *shrink* uncertainty without growing the build envelope.
The Rev 1 expansions (chain_id receipt, TTL bounds, lock module,
`observe()` API, hex-32 token) are unchanged in Rev 2 and still inside
scope.

## Anti-scope §7 audit vs Rev 2 §2–§6

Re-checked each §7 clause against Rev 2's marker-file additions and
advisory-narrowing language:

| §7 clause | Rev 2 leakage check | Result |
| --- | --- | --- |
| No transcript content mutation or import-replace implementation | Marker file is JSON-on-disk under `locks/`; not a transcript, not a `session_turns` row, not import-replace | honored |
| No provider spawn / signal / suspend / resume / kill | Marker write happens inside `release()`, not via any executor | honored |
| No proof of safety for provider CLIs launched outside agent-runner | Advisory-v1 framing explicitly accepts this gap until sibling observers land | honored |
| No global runner lock | Marker path is per-session, same scope as lockfile | honored |
| No DB lock table in v1 | Marker is filesystem JSON, not SQLite | honored |
| No strict ambiguity query outside the shared resolver | §4 unchanged in Rev 2 on this dimension | honored |
| No fallback to raw `session_turns` | §4 unchanged | honored |
| No GUI / frontend lock indicator | No frontend file added | honored |
| No quota/auth refresh, provider selection, config edit, `migrate-config` coupling | §4/§8 do not invoke quota, balancer, or any provider-config writer | honored |

D4b's wording in Rev 2 §7 is unchanged. The advisory-v1 framing in §1
*describes* the same decomposition but does not weaken any anti-scope
clause; the v1 lock primitive still does not silently spawn, drain, or
write transcripts.

## Decomposition assessment (unchanged from Rev 1)

Pause + resume must ship as one PR (Rev 1 analysis stands). D4b's
sibling-observer deferral is the only meaningful split, and Rev 2 makes
it more discoverable to readers. Rev 2 introduces no new merge surface
that would invite further decomposition: the marker file is owned by
the same `acquire`/`release` pair and cannot be split off without
duplicating the lock metadata format.

## Coverage matrix delta vs Rev 1

| Source ask / constraint | Rev 2 status |
| --- | --- |
| Harness same-token idempotent replay | Now backed by a concrete sibling marker (`release_marker_path` in §3.2; §6 marker schema; §4 steps 13–17) — was abstract behavior in Rev 1. |
| Initiative §112 reuse `StateDb::resolve_resume` | Unchanged; §4 step 2–3. |
| Initiative §115 deferred observers | Now explicitly named in §1 with the v1-advisory consequence, plus the four-PR follow-up sequence in §12 and §13. |
| Initiative §118 read-only `StateDb` open belongs to schema-probe | §8 explicitly inherits mutating open and pins follow-up to schema-probe. |
| Problem-map §3.1 risk: no token identity | §6 token + sha256 hash, plus marker `token_hash` for replay attestation. |
| Problem-map §6.2 gap: no JSON pause/release receipts | §3.2 release receipt now includes the marker path, closing the gap further. |

No previously covered row regresses; three rows tighten.

## Cross-feature consistency (no regression)

- Shared error namespace (`10`/`11`/`13`/`16`/`17`; `12`/`14`/`15`
  reserved): aligned with initiative §107–§110. Rev 2 unchanged.
- Ownership through `resolve_resume`: aligned with initiative §112.
  Rev 2 unchanged.
- Lock observation by sibling features: Rev 2 §13 row 3 still reads
  "Partial by design" but now names each deferred observer PR;
  aligned with initiative §115 ("once 06-pause-handshake lands").
- Read-only `StateDb` open belongs to schema-probe: Rev 2 §8 pins this
  explicitly; aligned with initiative §118.
- No auto-resume / spawn / quota / config-edit / migrate-config:
  aligned with initiative §121–§122; honored throughout §7/§8.

## Findings (severity ≥ MEDIUM)

None.

## Findings (LOW)

- **F1 (carried) — Harness AC bullet #6 satisfied across multiple PRs.**
  Rev 2 explicitly narrows v1 to "advisory" and names the four sibling
  observer PRs (§1, §12, §13). The decomposition matches initiative
  §115 ("once 06-pause-handshake lands"); Phase 5 hookpoint research
  will pin the exact sibling hookpoints. No Phase 4 remediation.
- **F2 (carried) — `observe()` API exposed but not consumed in this PR.**
  §6 defines the read shape; no §4 flow calls it; consumer set is the
  four sibling PRs named in §12. One shared read shape beats per-PR
  copy-paste. Cosmetic nit unchanged: §6 itself does not name the
  consumers (§1 and §12 do).
- **F3 (carried) — TTL bounds (1s / 30m / 5m default) without harness
  mandate.** Reasonable guardrails; out-of-range exits `2`.
- **F4 (carried) — Token format diverges from harness's ULID-shaped
  example.** `pause_<32 hex>` keeps prefix; ULID/UUID rejection
  defensible on entropy grounds.
- **F5 (closed by Rev 2) — Idempotent-release marker shape was
  deferred to Phase 5.** §6 commits to sibling-file shape with explicit
  JSON schema; §3.2 surfaces `release_marker_path`; §12 deletes the
  "marker-shape deferral" residual. R1-F01 closure verified.

## Recommended revisions

None that change scope. Round 1 findings are closed; the proposal as
written is correctly scoped. Optional cosmetic nit (carried from Rev 1):
§6 could name the deferred consumer set inline next to the API
definition, mirroring §1/§12. Not a scope concern.
