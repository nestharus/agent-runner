# 06-pause-handshake — Phase 4 Supported-Surface Risk Report (Rev 1)

**Termination signal:** `none`
**LOW / MEDIUM / HIGH:** **LOW**

Rev 1 is the first round on this proposal. Concerns are evaluated against
`research/06-pause-handshake-problem-map.md`, the harness spec at
`/home/nes/projects/agent-harness/tmp/scratch/agent-runner-feature-requests/04-session-pause-handshake.md`,
and the Initiative 06 cross-feature constraints in
`/home/nes/projects/agent-runner/worktrees/06-locate/initiatives/06-session-override-contract.md`.
Assumptions A1–A7 in §1.1 hold. Net value is positive but bounded: v1
ships a stable session-scoped lease primitive and a stable refusal API,
while sibling-writer lock observation (resume/repl/migration/balanced
one-shot/import-replace) is explicitly deferred to sibling PRs by D4b.
The supported-surface track in §11 documents that deferral honestly,
the migration/rollback story is trivially clean (no schema, no user
state migration, lockfiles inert to older binaries), and no adjacent
public path is BROKEN or DEGRADED. Findings below are advisory, not
blocking.

## Concern 1 — Assumption invalidation check (Rev 1)

| ID | Status | Evidence |
| --- | --- | --- |
| A1 — Pause/resume reuses `StateDb::resolve_resume` ownership semantics | **HOLDS** | Problem map §1.22 confirms `resolve_resume` is the existing ownership path; harness spec mandates same path as locate; cross-feature constraints (`initiatives/06-session-override-contract.md:112`) forbid a second resolver. |
| A2 — Lock key is resolved active provider session id (chain/provider in receipt) | **HOLDS** | `ResolvedResume` exposes `chain_id` + `active_provider` + `active_session_id` (problem map §1.19). No competing public mutable-target key exists. (See R1-F03 below for a v1 consequence.) |
| A3 — Existing `running` invocation rows are not a safe active-writer lock | **HOLDS** | Problem map §1.26-27 confirms `running` rows lack token/TTL semantics, can survive hard crash, are per-invocation not per-session. `FinalizerGuard` only runs on Rust drop, not after kill. |
| A4 — Lease must outlive the `pause-handshake` process | **HOLDS** | Harness spec returns a token and exits; an fd-held `flock` would release on `exit()`. D1a in §4 step 6 makes lockfile metadata the lease and uses `flock` only around acquire/release/read critical sections. Internally consistent. |
| A5 — TTL-based crash recovery is sufficient for v1 | **HOLDS** | Problem map §1.18 + README confirm no daemon, no background process. D5 (lazy stale removal on next acquire) is feasible without a reaper. |
| A6 — v1 adds primitive first; sibling writer paths observe in their own PRs | **HOLDS (partial-by-design)** | D4b owns this trade-off; cross-feature constraints in `06-session-override-contract.md:114-117` commit migration/repl/resume/balanced one-shot/import-replace to wire observation when each lands. See Concern 5. |
| A7 — File-backed receipts acceptable if files are owner-private | **HOLDS** | Runner state already lives in `dirs::data_dir()` per problem map §1.16. §8 mandates `0700` directory and `0600` files on Unix; permission failure is exit `1`, not silent downgrade. |

**Termination signal #1 (`invalidated-assumption`) does not fire.**

## Concern 2 — Net-positive value (Rev 1)

### Risk reduced (problem map §3 entries Rev 1 demonstrably retires)

| §3 entry | Retired by | Rev 1 status |
| --- | --- | --- |
| §3.1 No way to block a second writer for a resolved session | §4 atomic-acquire flock + lockfile metadata | Retired **for second-pause callers**; partial for non-pause sibling writers (D4b) |
| §3.3 No token identity / no `lock-token-invalid` error | §6 `token_hash` + §5 exit `16` | Retired |
| §3.4 No `expires_at` / TTL field anywhere | §6 metadata + §4 step 8 + D3 bounds | Retired |
| §3.5 No crash-recovery path for lock state | §4 step 8-9 lazy stale removal under flock | Retired |
| §3.16 No structured session-busy signal | §5 exit `13` + stderr JSON shape | Retired |
| §3.17 No stable `lock_path` value | §3 receipt `lock_path` field + §4 step 4 | Retired |
| §6 obs-gaps #1-4 (no paused-state surface, no JSON receipt precedent for leases, no JSON error mapping for busy/expired/token-invalid, no audit trail) | §3 receipts + §5 exit codes | Retired #1-3; #4 explicitly **not** retired (no audit table by design — §11) |

### Risk NOT retired (D4b — partial-by-design, documented)

| §3 entry | Why deferred |
| --- | --- |
| §3.1 (sibling-writer side) — second `agents resume`, `repl --resume`, `migrate-db`, `migrate_chain_segment`, ingestion, backfill can still race a pause holder | D4b sequesters observation in sibling PRs; `import-replace` lands after pause-handshake and consumes the primitive natively (cross-feature constraint). |
| §3.6-9 — provider-child opacity, balanced-one-shot post-hoc session writes, migration mid-sequence crash, segmentless raw `session_turns` | These are sibling-feature concerns; v1 primitive does not pretend to address them. §7 anti-scope is explicit. |
| §3.13-14 — backfill-during-`StateDb::open` side effects, multiple-active-segment tolerance | Inherited from existing `StateDb::open`; pause-handshake keeps the inheritance per §13 "Read-only `StateDb` open belongs to schema-probe. Yes / inherited." |

### Blast radius added

| Failure mode | Guard |
| --- | --- |
| Lockfile permissions wrong on Unix | `0700` dir / `0600` file or exit `1` (§8); pinned by §9 `Permissions` test |
| Lockfile orphaned by holder crash | TTL + lazy stale removal under flock (§4 step 8-9, D5); pinned by §9 `Stale acquire` test |
| Stolen-token release | `token_hash` in lockfile not raw token (§6); pinned by §9 `Wrong token` test |
| Two concurrent pauses double-grant | `flock` + metadata-validity check (§4 step 7-8); pinned by §9 `Atomic acquire` test |
| Active session id changes under lock holder (sibling migration writes during hold) | TTL bounds the orphaned lockfile; resume returns `lock-token-invalid` because path differs. See R1-F01. |
| Idempotent-replay marker file accumulation | Phase 5 picks (a) replace lockfile or (b) `locks/releases/` sibling marker; both bounded by next acquire / TTL. See R1-F02. |
| Windows lacks POSIX flock | Documented residual in §12 ("Windows semantics are not designed"); README in §10 should note Linux/macOS support. See R1-F04. |

### Net-value verdict — POSITIVE

The harness primary use case (coordinate a pause→import-replace→resume
cycle) lands fully when import-replace adopts the primitive natively in
its own PR, which is the next initiative-06 feature in technical order
(`initiatives/06-session-override-contract.md:48-56`). Pause-handshake
in isolation establishes a stable refusal API, two-arbiter pause
coordination, token-mediated release, TTL-bounded crash recovery, and
exit-code namespace coverage for `13` / `16` / `17`. None of those
exist today (problem map §3.1-17, §6). The primitive blast radius is
contained to a new lock subdirectory and two new subcommands; no schema
migration, no DB write, no provider control, no transcript mutation
(§7 anti-scope, §8 side-effect contract). Honest about partial sibling
adoption (§1.2 last paragraph, §11 adjacent paths, §13 compliance
table). Termination signal #2 does NOT fire. Net value positive.

## Concern 3 — Adjacent paths blast radius

| Path | Verdict | Rev 1 evidence |
| --- | --- | --- |
| `agents session locate` | PRESERVED | Same resolver; metadata receipt unchanged. §11. |
| `agents session schema-probe` | PRESERVED | Read-only DB open is schema-probe's surface; pause-handshake does not change it. §13. |
| `agents session export` | PRESERVED | Read-only and lock-blind in v1 by §11. |
| Future `agents session import-replace` | PRE-WIRED | Will consume `SessionLockManager::acquire`/`release`/`observe` in its own PR. §6. |
| `agents resume` | UNCOUPLED IN V1 | D4b: sibling PR adds observer call. Adjacent today; not blocked by pause-handshake. Documented in §11. |
| `agents repl <model> --resume` | UNCOUPLED IN V1 | Same as above. |
| Top-level `agents -m <model> --resume <UUID>` | UNCOUPLED IN V1 | Same — both `run_repl` and `run_resume` are sibling observers per cross-feature constraint, not v1 consumers. |
| Balanced one-shot (`run_with_balancing`) | UNCOUPLED IN V1 | Same; §12 records the future fail-closed point for post-hoc session discovery. |
| `migrate_chain_segment` | UNCOUPLED IN V1 | Same; sibling PR will call `observe` before file rename + segment rotation. |
| `agents migrate-db` (chain backfill, compaction backfill) | UNCOUPLED IN V1 | Same; per §13 not coupled to migrate-config. |
| `StateDb::open` (open-path backfill) | INHERITED | Pause-handshake opens via `StateDb::open` per A1; backfill side effect inherited from existing behavior, idempotent across resume/repl callers. |
| `sessions.toml` adapter scripts | UNCOUPLED | Pause-handshake does not invoke locator/scan scripts. |
| Provider config (`[providers.session_storage]`, `[providers.resume]`) | UNCOUPLED | Pause-handshake reuses model/provider validation through resolver only; no config mutation. |
| GUI/Tauri commands (`test_model_with_db_path`, etc.) | UNCOUPLED | §1 explicit "No GUI/Tauri frontend surface." DB-location divergence between GUI and CLI (problem map §4.12) is not introduced or aggravated. |
| Hidden `resume-list` | PRESERVED | Lock-blind by §11 adjacent paths. No coupling. |
| `trace --json` | PRESERVED | §11 explicit: no trace event added. The "no audit table, no telemetry" stance is an intentional non-extension of trace. |

No adjacent path is BROKEN or DEGRADED. The "UNCOUPLED IN V1" rows are
intentional per D4b and matched by the supported-surface track.

## Concern 4 — Migration / rollback / observability

### Migration

§11 claim: "no user state migration beyond on-demand lock directory
creation." VERIFIED — proposal adds no schema, no DDL, no DB column,
no chain/segment changes. The lock directory is created lazily on
first `pause-handshake` per §4 step 5 with `0700` permissions.
Existing sessions are unlocked because no lockfile exists (§11).

### Rollback

§11 claim: "remove or stop invoking the subcommands. Existing lockfiles
are inert to older binaries." VERIFIED — older `agents` binaries do not
read `~/.local/share/oulipoly-agent-runner/locks/`; lockfiles do not
appear in the SQLite schema and are not consulted by any pre-Rev-1
command path. Operators can `rm -rf locks/` after confirming no Rev-1+
binary is observing them. No schema downgrade required.

### Observability

§11 claim: "stdout receipts, stderr JSON errors, and lockfiles are the
entire v1 surface. No invocation row, trace event, audit table, or
telemetry is added." VERIFIED — explicit in §8 side-effect contract.
This is a deliberate non-gain over problem map §6 obs-gap #4 (no audit
trail). Acceptable for v1 because:

- Lockfile metadata (`session_id`, `chain_id`, `provider_name`,
  `created_at`, `expires_at`, `owner_pid`, `token_hash`) is itself an
  observability artifact: `ls`, `cat`, or a future `agents session
  observe` reveals current state.
- Failure paths emit structured JSON on stderr (§3.3) — first JSON
  error precedent in `agents` (problem map §6 obs-gap #3 retired).
- Receipt JSON is a stable harness consumption surface (§3.1, §3.2).

Two advisory items for the implementer (non-blocking) are recorded
under Findings.

## Concern 5 — Harness acceptance criteria coverage

Re-checked against
`/home/nes/projects/agent-harness/tmp/scratch/agent-runner-feature-requests/04-session-pause-handshake.md`
acceptance section:

| Acceptance criterion | Coverage | Evidence |
| --- | --- | --- |
| Idle pause returns unique token | **covered** | §3.1 receipt + §4 acquire path + §9 "Atomic acquire" / "Token format" tests. |
| Resume releases the lock and permits subsequent writes | **covered** | §4 step 12-16 + §9 "Correct release" test. |
| Wrong token, expired token, missing session, ambiguous session map to stable error codes | **covered** | §5 (10/11/16/17) + §9 wrong-token / not-found / ambiguous / expired tests. |
| Second pause while valid lock returns `session-busy` | **covered** | §4 step 8 + §9 "Busy lock" test. |
| Lock acquisition is crash-safe with TTL cleanup | **covered** | D5 lazy stale removal + §9 "Stale acquire" test. |
| Pause prevents concurrent `import-replace` or migration for the same session | **partial-by-design** | D4b: import-replace lands AFTER this PR and adopts the primitive natively; migration becomes an observer in a sibling PR per cross-feature constraint. v1 ships the primitive; v1 does not block sibling write paths. Documented in §1.2 residual + §11 adjacent paths + §13 compliance table. |
| `agents resume` / `repl --resume` check the lock before write path | **deferred** | D4b: sibling PR. Cross-feature constraint commits the change. Same documentation honesty as above. |
| Tests cover concurrent pause/import/resume processes, lease expiry, crash recovery | **partial** | §9 covers concurrent pause/resume, lease expiry (`Expired release`), crash recovery (`Stale acquire`). The "concurrent import" leg lands when import-replace lands; pause-handshake's primitive cannot test against a non-existent sibling. |

Two acceptance criteria are partial-by-design (D4b). This is honest,
documented in three independent places (§1.2, §11, §13), and resolved
by sibling PRs in the technical-dependency-ordered initiative
sequence. The harness's primary end-to-end story (pause → import-replace
→ resume) coheres once import-replace lands. **Not a termination signal**:
the v1 primitive's contract is sound for harness intra-pause coordination
and for the sibling adoption that follows. **Verdict: ADEQUATE for v1
scope, with sibling-PR continuation explicitly committed by initiative
constraints.**

## Concern 6 — Initiative-06 sequencing forward-compat

| Concern | Status |
| --- | --- |
| `SessionLockManager` reusable by sibling PRs | YES — §6 names `acquire` / `release` / `observe` as public methods. `observe` is the read-only path that migration / repl / resume / balanced-one-shot will call. |
| Lock metadata schema versioned | YES — §6 `version: 1` field. Sibling PRs reading existing locks know the schema. |
| Lock path stable for harness pinning | YES — §3 lock_path field; §4 step 4 path formula. |
| Token format committed (`pause_<32 hex>`) | YES — §3.1 D2; sibling PRs distinguish their own token formats by prefix if needed. |
| Exit-code namespace consistent with shared 10/11/12/13/14/15/16/17 | YES — §5 uses 10/11/13/16/17; reserves 12/14/15 for sibling features (initiative `:107-111`). |
| Resolver inherited from locate when stacked, falls back to `StateDb::resolve_resume` standalone | YES — §4 step 2 names both paths. §9 fixture note accommodates temporary unstacking. |
| Read-only `StateDb` open belongs to schema-probe | YES / inherited — §13 explicit; pause-handshake does not duplicate that surface. Open-path backfill side effect inherits existing `StateDb::open` behavior. |
| `MetadataError`/`ResumeError`/`SessionLockError` namespaces non-overlapping | YES — `SessionLockError` is a new module; §6 defines `Busy`, `TokenInvalid`, `Expired`, `Operational`. No collision with existing `ResumeError` or future `MetadataError`. |

Forward-compat: PRESERVED. The downstream import-replace PR consumes
this primitive cleanly; migration / repl / resume / balanced-one-shot
adopt `observe` calls in their own PRs without contract churn here.

## Findings

- **R1-F01 (advisory, supported-surface UX)** — When v1 sibling
  migration / ingestion runs concurrently with a held pause lock,
  the chain's active segment can flip from S1 to S2 mid-hold. Resume
  with the holder's chain-id then resolves to S2 and looks for a
  lockfile at the S2 path; the original lockfile at the S1 path is
  orphaned and the holder gets `lock-token-invalid` despite holding a
  valid token. The orphan is reaped by TTL. This is a direct
  consequence of A2 (session-id key) plus D4b (sibling writers do
  not yet observe). It disappears once sibling PRs adopt observation
  per cross-feature constraint. Recommended advisory: `README.md`
  §10 should explicitly state that v1 does not block sibling
  resume/migrate/ingest from changing the active segment under a
  held lock, so users do not assume end-to-end coverage. The
  Phase 5 implementer can also consider including the resolved
  `session_id` in the receipt and recommending callers pass the
  resolved value (already done — §3.1) **and** that callers should
  not initiate sibling write commands during the hold. **Not
  blocking** — the primitive contract is sound; the gap is in
  documentation phrasing, not contract correctness.

- **R1-F02 (advisory, deferred-decision)** — §6 says "Phase 5 chooses
  one shape" between (a) replace lockfile with release marker (overwritten
  by next acquire) and (b) sibling marker under
  `locks/releases/session-<uuid>.json`. Both are equivalent for the
  contract surface, but the §9 "Idempotent replay" test fixture
  depends on which is chosen. Phase 5 must select before Step 6b. **Not
  blocking** — the deferral is bounded and documented; both options
  preserve idempotency semantics.

- **R1-F03 (advisory, A2 consequence)** — A2 keys the lockfile path
  on resolved active provider session id. If a chain has multiple
  candidate active segments (problem map §1.21 — "Multiple active
  rows are tolerated by selecting one"), pause and resume in close
  succession could in principle resolve to different active segments
  if a sibling migration runs in between. This is the same scenario
  as R1-F01 from a different angle; the mitigation is the same
  (sibling adoption of observe in their own PRs). **Not blocking**.

- **R1-F04 (advisory, platform residual)** — §12 says "Windows
  semantics are not designed." The proposal uses POSIX `flock` (§4
  step 7) and Unix mode bits (§8). README §10 lists permission docs
  but does not name supported platforms. Recommended advisory:
  README §10 should state pause-handshake is supported on Linux/macOS
  in v1, with Windows behavior undefined, so a Windows user does not
  silently get a downgraded or non-portable lock. **Not blocking** —
  agent-runner deployment posture (CLI/no-daemon, problem map §1.18)
  has no documented Windows production support today.

- **R1-F05 (advisory, observability ergonomics)** — §11 declines to
  add an `agents session observe <id>` read-only inspection command.
  Operators today must `cat` the lockfile or run `ls
  ~/.local/share/oulipoly-agent-runner/locks/` to see lock state. The
  `observe` method exists in §6 as a library API; exposing it as a
  CLI subcommand is a small addition that future PRs (likely the
  sibling adoption PR for resume/migrate) could land. **Not
  blocking** — the lockfile-as-observability surface is sufficient
  for the harness consumer.

- **R1-F06 (advisory, README scope)** — §10 does not mandate that
  README explicitly note the v1 partial-coverage stance ("v1 does
  not yet block sibling resume/migrate/ingest"). §11 documents this
  as an adjacent-path note but the README synopsis section is what
  most users read. Recommended advisory: README's pause-handshake
  paragraph should include one sentence on the v1-vs-eventual scope.
  **Not blocking** — the primitive contract is honestly framed; this
  is a docs-clarity nudge.

## Verdict rationale

**Termination signal #1 (`invalidated-assumption`) does not fire.**
A1–A7 hold against problem map evidence. No assumption depends on a
fact contradicted by current state.

**Termination signal #2 (`non-positive-value`) does not fire.** The
v1 primitive concretely retires problem map §3.1 (second-pause
arbitration), §3.3 (token identity), §3.4 (TTL/expires_at), §3.5
(crash-recovery), §3.16 (structured session-busy signal), §3.17
(stable lock_path), and obs-gaps #1-3 (paused-state surface, JSON
receipt for leases, JSON error mapping). Sibling-writer side of
§3.1 is partial-by-design via D4b and resolved by sibling PRs in the
initiative's technical-dependency order. Net value positive on the
harness consumer surface and on the future-sibling consumer surface.

**Standard verdict: LOW.** Adjacent paths preserved or intentionally
uncoupled (Concern 3); migration/rollback trivially clean (Concern 4
— no schema, lockfiles inert to older binaries); observability
deliberately bounded to receipts + stderr JSON + lockfile metadata
(Concern 4); harness acceptance criteria covered with two
partial-by-design rows that resolve in sibling PRs (Concern 5);
forward-compat preserved across the remaining Initiative-06 features
(Concern 6).

**Recommendation:** Phase 4 supported-surface gate **passes**. Phase 5
hookpoint research and Phase 6 implementation may proceed once the
other three risk reports also clear LOW. The six advisory findings are
implementer guidance, not blockers.

## Advisory items for the implementer (non-blocking)

1. README §10: add one sentence on v1-vs-eventual scope (R1-F06) and
   one sentence naming Linux/macOS as supported with Windows undefined
   (R1-F04).
2. README §10: state explicitly that v1 does not yet block sibling
   resume / repl --resume / migrate-db / ingestion / backfill from
   changing the active segment of a held session, so users avoid the
   orphaned-lockfile UX described in R1-F01.
3. Phase 5 must pick the idempotent-release marker shape from §6 (a)
   vs (b) before Step 6b encodes the "Idempotent replay" test.
4. Consider exposing `SessionLockManager::observe` as a CLI subcommand
   (`agents session observe <id>`) in the sibling adoption PR (R1-F05),
   so operators have a first-class read surface beyond `cat
   ~/.local/share/oulipoly-agent-runner/locks/session-<uuid>.lock`.
5. When sibling PRs (resume / repl / migrate / balanced one-shot)
   wire `observe`, ensure they refuse-and-emit a structured stderr
   JSON `session-busy` rather than waiting silently — this preserves
   the "stable refusal surface" framing of §1.2 net value.
