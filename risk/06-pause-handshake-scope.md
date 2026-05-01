# 06-pause-handshake — Phase 4 Scope Risk Assessment (Rev 1)

**Assessor:** scope reviewer
**Verdict:** **LOW** — The proposal stays inside the harness ask
(`04-session-pause-handshake.md`) and the Initiative 06 cross-feature
constraints (`initiatives/06-session-override-contract.md:106-122`).
Every §2–§6 design hunk traces back to a harness behavior, an
initiative constraint, or a problem-map gap; every §7 anti-scope clause
is consistent with what §2–§6 actually build. The only structural
decomposition decision (D4b — defer sibling write-path observation to
later PRs) is the exact decomposition the initiative artifact already
sanctions in its PR-by-PR sequencing. No scope creep, no missing
in-scope work, no useful further decomposition available.

## Coverage matrix

| Source ask / constraint | Proposal section | Coverage |
| --- | --- | --- |
| Harness: `agents session pause-handshake <id> [--ttl-ms <ms>]` | §2 clap shape | complete |
| Harness: `agents session resume-handshake <id> --token <T>` | §2 clap shape | complete |
| Harness: pause stdout JSON with `session_id`, `provider_name`, `token`, `expires_at`, `lock_path` | §3.1 schema | complete — adds `chain_id` (justified: initiative §13 names chain id as part of resolver result) |
| Harness: resume releases only matching token; idempotent same-token replay | §4 steps 12–16; §3.2 `already_released` | complete |
| Harness: refuse wrong-token release | §4 step 15; §5.2 exit `16` | complete |
| Harness: exit codes `0`/`1`/`2`/`10`/`11`/`13`/`16`/`17` | §5.1, §5.2 | complete |
| Harness: side-effect contract — lock state only, no transcript mutation | §7 anti-scope, §8 side-effect contract | complete |
| Harness: crash-safe with TTL cleanup | §4 D3/D5; §1.1 A5 | complete (lazy-on-acquire, no daemon) |
| Harness AC: `agents resume`/`repl --resume` check lock before write | §13 row "Partial by design" via D4b | **deferred to sibling PRs** — see F1 |
| Harness: tests cover concurrent pause/import/resume, lease expiry, crash recovery | §9 test track rows "Atomic acquire", "Busy lock", "Stale acquire", "Expired release", "Idempotent replay" | complete (sibling-write concurrency tests appropriately deferred with D4b) |
| Initiative §106 shared error-code namespace | §5 uses only `0`/`1`/`2`/`10`/`11`/`13`/`16`/`17`; reserves `12`/`14`/`15` | complete |
| Initiative §112 reuse `StateDb::resolve_resume`, no second ownership path | §4 step 2–3, §1.1 A1, §13 row | complete |
| Initiative §115 deferred lock observation by import-replace, migration, repl/resume, balanced one-shot | §7 D4b; §13 "Partial by design" | complete (per-PR sequencing) |
| Initiative §118 read-only `StateDb` open belongs to schema-probe | §12 inherits mutating open; does not introduce read-only here | complete (correct deferral) |
| Initiative §121 anti-scope: no auto-resume / spawn / quota / config-edit / migrate-config | §7, §8 | complete |
| Problem-map §3 risk: no token identity for releaser | §6 token + sha256 hash, single-print of raw token | complete |
| Problem-map §3 risk: no `expires_at`/TTL field | §6 metadata `expires_at`; §4 D3 bounds | complete |
| Problem-map §3 risk: no crash-recovery for lock state | §4 D5 lazy stale removal; §1.1 A5 | complete |
| Problem-map §3 risk: no stable `lock_path` value | §3.1, §4 step 4 deterministic path | complete |
| Problem-map §6 observability gaps: no JSON pause receipt; no structured stderr for `session-busy` / `lock-token-invalid` / `lock-expired` | §3.1, §3.2, §3.3 | complete |

Every harness behavior maps to a §2–§6 hunk; every §2–§6 hunk traces
back to either the harness ask, an initiative constraint, or a
problem-map gap. No orphaned design surface.

## Scope-direction analysis

| Direction | Where | Magnitude | Justification |
| --- | --- | --- | --- |
| Add `chain_id` to receipts beyond harness's named fields | §3.1, §3.2 | tiny | Initiative §112 fixes ownership through `resolve_resume`, which returns `(chain_id, active_provider, active_session_id)`. Surfacing `chain_id` lets the harness disambiguate without a second resolver call. Consistent with `06-locate`'s `SessionMetadata` shape. |
| TTL bounds (1s min / 30m max / 5m default) | §4 D3 | small | Guardrail not in harness ask; default value is the conservative middle of the harness's "lease + retry" envelope. Bounds prevent abuse without restricting the documented use case. |
| Hex-32 token format with `pause_` prefix; reject ULID / UUIDv4 / UUIDv7 | §3.1 D2 | small | Harness example shows `pause_01HV...` (ULID-shaped), but the harness ask does not require ULID. D2 rejects ULID/UUIDv7 because their time bits cut entropy and rejects UUIDv4 because version/variant bits do the same. The `pause_` prefix is preserved. Defensible on security grounds; harness contract is not broken because it never specified an inner format. |
| New `src-tauri/src/session_lock/` module with `acquire`/`release`/`observe` | §6 | bounded | Initiative §53 explicitly says this PR establishes the "session-scoped exclusive lease lock observed by import-replace, migration, resume/repl write paths." Providing the module + reusable types is the feature itself, not a separable concern. |
| `observe()` exposed but unused in this PR | §6 | bounded | Sibling PRs (`run_repl`, `run_resume`, balanced one-shot, migration, import-replace) will consume `observe()`. Initiative §115 explicitly defers those wirings to later PRs. Exposing the read shape now keeps sibling PRs from introducing a parallel API. |
| Idempotent-release marker shape deferred to Phase 5 | §6 final paragraph | clarification | Behavior is committed (same-token replay returns `0` with `already_released: true`); the storage shape (in-place marker vs sibling marker file) is left to Phase 5 with two enumerated options. Appropriate behavior-vs-mechanism split. |

**Net direction:** in-scope build with three small expansions (chain_id
in receipt, TTL bounds, new lock module), all justified by either
problem-map gaps or initiative cross-feature constraints. No expansion
crosses into anti-scope territory.

## Anti-scope §7 audit vs §2–§6

I cross-checked each §7 clause against every clap arg, JSON field, and
behavior in §2–§6 for leakage:

| §7 clause | Check | Result |
| --- | --- | --- |
| No transcript content mutation or import-replace implementation | §6 `acquire`/`release`/`observe` touch only lockfile metadata; §8 enumerates allowed FS effects (lockfile + release marker only) | honored |
| No provider spawn / signal / suspend / resume / kill | §4 has no executor invocation; §6 API has no process handle; §8 names "no provider commands" | honored |
| No proof of safety for provider CLIs launched outside agent-runner | §4 makes no claim about external CLI processes; A3 explicitly notes invocation rows are not safe writer leases | honored |
| No global runner lock | Lock path is `session-<session_id>.lock`, keyed off resolved active session — per-session scope | honored |
| No DB lock table in v1 | D1 chose lockfile; §6 metadata is JSON-on-disk, not SQLite | honored |
| No strict ambiguity query outside the shared resolver | §4 step 3 calls shared resolver only; ambiguity propagates through `Ambiguous` | honored |
| No fallback to raw `session_turns` | §4 only consults resolver path | honored |
| No GUI / frontend lock indicator | No Tauri command, no frontend file mentioned | honored |
| No quota/auth refresh, provider selection, config edit, `migrate-config` coupling | §4 does not touch `quota`, `balancer`, `migrate_config`, or any provider-config writer; §8 explicit about not running quota scripts | honored |

All nine anti-scope clauses hold across §2–§6. The classic creep smell
(an anti-scope item silently violated by a §4 step or a §6 method) is
absent.

## Decomposition assessment

Pause + resume must ship as one PR. Splitting them would either:

1. **Pause-only first** — leaves users with no release path except TTL
   expiry. Anti-scope D5 explicitly disallows a background reaper, so
   pause-without-resume strands lockfiles for the full TTL on every
   acquisition. The lock primitive cannot be validated end-to-end by
   tests without `release`. Dead intermediate state.
2. **Resume-only first** — releases nothing because no acquirer exists.
   The §9 "Correct release" / "Wrong token" / "Idempotent replay" tests
   would all be unreachable. Worse than a no-op.

Splitting would also fork the `src-tauri/src/session_lock/` module API
into two separate landings (acquire-only, then release added later),
which contradicts the initiative §53 framing of this PR as
"the lock primitive."

D4b's deferral of sibling observers is the *only* meaningful
decomposition this PR can offer, and the proposal already takes it.
Each sibling PR (import-replace, migration, run_repl, run_resume,
balanced one-shot) gets its own `observe()` wiring in its own
proposal/risk/hookpoint cycle. That mirrors `06-locate`'s established
pattern of "primitive lands first, observers wire in later PRs"
(initiative artifact §76–§89 sequencing rationale).

No further useful split exists.

## Findings (severity ≥ MEDIUM)

None.

## Findings (LOW)

### F1 — LOW — Harness AC bullet #6 satisfied only across multiple PRs

§13 row 3 ("Lock observation by import-replace, migration, repl/resume,
balanced one-shot once pause lands") is marked "Partial by design" with
D4b. The harness's acceptance criteria includes:

> `agents resume` / `repl --resume` check the lock before starting a
> write path and fail closed or wait according to documented behavior.

This PR alone does not satisfy that criterion. The criterion is
satisfied only after sibling observer PRs land.

This is correctly scoped per the initiative artifact: §53 names this
PR as "establishes the lock primitive"; §115 defers observer wiring;
§77–§89 sequencing rationale puts pause-handshake fourth and
import-replace fifth, with the explicit understanding that observers
adopt the API after it lands. The proposal honestly marks the §13 row
"Partial by design" rather than claiming end-to-end blocking.

Suggestion: none. The decomposition is consistent with the initiative
contract. Phase 5 hookpoint research should record the exact sibling
hookpoints that future PRs will edit, so the gap is narrowable into a
concrete checklist; that is a Phase 5 concern, not a Phase 4 scope
concern.

### F2 — LOW — `observe()` API exposed but not consumed in this PR

§6 lists `observe(&self, target) -> Result<Option<ExistingLockInfo>,
SessionLockError>`. Neither §4's `pause-handshake` flow nor
`resume-handshake` flow invokes `observe()`. It exists for future
sibling features (§13 row 3, D4b).

This is the smallest example of "API surface ahead of consumer." It is
defensible because:

- The initiative §115 names this PR as the home of the lock primitive
  observed by named siblings.
- Forcing each sibling PR to either copy-paste the observe path or
  refactor §6 fragments back into a shared module is worse than
  defining the read shape once.
- `ExistingLockInfo` shape decisions (TTL semantics, release marker
  semantics, error mapping) belong with the writer that owns the
  metadata format.

Suggestion: none required. Optionally, §6 could add one sentence
naming the consumer set ("expected callers: future
`agents session import-replace`, `migrate_chain_segment`, `run_repl`,
`run_resume`, balanced one-shot") to make the deferred consumption
explicit. Cosmetic.

### F3 — LOW — TTL bounds (1s / 30m / 5m default) introduced without harness mandate

§4 D3 sets minimum 1000 ms, maximum 1800000 ms, default 300000 ms.
The harness ask shows `[--ttl-ms <ms>]` with no specified bounds and
no default. Out-of-range values exit `2` (clap usage).

Justification: 5 minutes is a sensible default for an interactive
import-replace handshake; 30 minutes prevents abuse; 1 second
prevents accidental near-zero leases that would race their own
acquirer. These are reasonable guardrails for a lease primitive, and
exit `2` on out-of-range is consistent with usage-class errors.

Suggestion: none. The bounds are documented in §10 README work, so the
constraint is discoverable.

### F4 — LOW — Token format diverges from harness's ULID-shaped example

Harness ask shows `pause_01HV...` (ULID-shaped). §3.1 D2 chooses
random 128-bit hex (`pause_<32 hex>`) and rejects ULID/UUIDv4/UUIDv7
on entropy grounds.

The harness's example is illustrative, not normative — the harness
ask never says "must be ULID." The `pause_` prefix is preserved, so
prefix-matching consumers continue to work. CSPRNG hex avoids the
time-structure entropy loss that ULID would impose on a secret.

Suggestion: none. The choice is defensible and the §10 README work
documents the regex.

### F5 — LOW — Idempotent-release marker shape deferred to Phase 5

§6 enumerates two options for release-marker storage (in-place
overwrite vs sibling marker file under `locks/releases/`) and defers
the choice to Phase 5. Behavior (`already_released: true` for
same-token replay) is committed.

This is appropriate behavior-vs-mechanism scope discipline. The
behavior contract is in the proposal; the file-layout choice belongs
with the implementer who can weigh atomicity tradeoffs against
filesystem semantics. Two enumerated options is bounded — Phase 5 will
not invent a third.

Suggestion: none.

## Cross-feature consistency

Cross-checked each row of §13 against `initiatives/06-session-override-contract.md:106-122`:

- Shared error namespace (`10`/`11`/`13`/`16`/`17` used; `12`/`14`/`15` reserved): aligned with initiative §107–§110.
- Ownership through `resolve_resume`: aligned with initiative §112.
- Lock observation by sibling features: marked "Partial by design"; aligned with initiative §115 ("once 06-pause-handshake lands").
- Read-only `StateDb` open belongs to schema-probe: aligned with initiative §118; §12 explicitly inherits mutating open here.
- No auto-resume / spawn / quota / config-edit / migrate-config: aligned with initiative §121–§122; honored throughout §7/§8.

No initiative constraint is violated. No initiative constraint is
silently extended.

## Spot-checks verified

- `~/.local/share/oulipoly-agent-runner/locks/` does not exist today
  (problem-map §1.12 confirms README documents only `state.db` under
  that directory). §3.1's `lock_path` is genuinely net-new state, not
  re-using a path that another command already owns.
- `StateDb::resolve_resume` is the existing single ownership path
  (problem-map §1.22). §4 step 2–3 calls it directly when unstacked
  and through locate's wrapper when stacked — neither introduces a
  parallel resolver.
- Initiative shared-error-code namespace
  (`initiatives/06-session-override-contract.md:107-110`) reserves
  `12 unsupported-storage`, `14 schema-incompatible`, `15
  invalid-input or preimage mismatch`. §5.1 / §5.2 do not use any of
  these codes — confirming sibling-feature codes are not silently
  reused here.
- Initiative §76–§89 sequencing places this PR fourth (after locate,
  schema-probe, export) and before import-replace. §1 statement matches
  this ordering.
- Problem-map §3.1 risk ("no current way to block a second writer for
  a resolved session") is addressed structurally by §6 + §8; the
  cross-process concurrency test row in §9 ("Atomic acquire") covers
  the actual blocking behavior.

## Recommended revisions (if any)

None that change the scope shape. F1–F5 are all LOW and are honestly
documented by the proposal itself (D4b in §7, D2 in §3.1, D3 in §4,
the two-option marker in §6, "Partial by design" in §13).

Optional cosmetic nits the author can take or leave:

1. **§6 — name the deferred consumer set for `observe()`.** One
   sentence ("expected callers: future `agents session import-replace`,
   `migrate_chain_segment`, `run_repl`, `run_resume`, balanced
   one-shot") would make F2's deferred-consumption justification
   self-evident in the proposal text. Cosmetic.

2. **§13 — surface F1 explicitly.** §13 row 3 says "Partial by design"
   and points to D4b. A second sentence noting "the harness AC bullet
   `agents resume`/`repl --resume` check is satisfied across this PR
   plus the four sibling observer PRs named in the initiative
   artifact" makes the cross-PR completion path explicit. Cosmetic.

Neither nit is a scope concern. The proposal as written is correctly
scoped.
