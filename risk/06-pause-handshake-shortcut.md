# 06-pause-handshake — Phase 4 Shortcut Risk Assessment (Rev 1)

## Verdict: LOW

The proposal makes five named design decisions (D1a file-backed
lease, D2 128-bit hex token, D3 TTL bounds 1s/5m/30m, D4b primitive-
plus-observer-API only, D5 lazy stale reap). Each is stated with
a rationale, paired with a falsifiable invalidator in §1.1, and
defended in §7 anti-scope and §12 residuals. None defeats the
underlying purpose: the v1 product is a **stable refusal surface**
(`session-busy`, `lock-token-invalid`, `lock-expired`) that the
harness can use as a lease lock around transcript override. The
proposal is honest about what the v1 lease *is not* — provider
process suspension, a global runner lock, a DB lock table, or an
end-to-end block of sibling write paths in the same PR — and the
"is not" list is repeated in §1.2, §7, §10's `lock_path` framing,
§11 deployment mode, §12, and §13. No shortcut-indicator grep flag
fires in a non-negation sense (see scan below). No deferred stub
(workflow `no-deferred-stubs.md`) and no backwards-compat shim
(workflow `no-backwards-compatibility.md`).

## Decision-by-decision shortcut audit

### D1a — File-backed lease, lockfile metadata is the lease (§4 step 6, §12)

Purpose-fit, not a shortcut. The harness response shape names
`lock_path` (`/home/nes/projects/agent-harness/tmp/scratch/agent-runner-feature-requests/04-session-pause-handshake.md:31`),
which the proposal exposes verbatim in §3.1. A DB lock table
(D1b alternative) would force a schema migration (problem-map
§6.1) and break that public path contract; D1a lands without
schema work and matches the harness contract directly. The
proposal correctly isolates the subtlety that an fd-held `flock`
cannot itself be the lease because the CLI exits after printing
JSON — the lease is the **lockfile metadata**, with `flock` held
only around acquire/release/read critical sections (§4 step 6,
§12). That is a real design decision, not a workaround. The
Windows/POSIX scope is acknowledged as a residual (§12), not
masked.

### D2 — Token format `pause_<32 hex>` (§3.1)

Purpose-fit. Rejecting ULID and UUIDv7 because they "carry time
structure," and rejecting UUIDv4 because version+variant bits
"reduce random entropy below the stated 128-bit token format,"
is the principled call here. A token that leaks `created_at` is
a real attack-surface concern for a release credential, even on
a per-user data dir. CSPRNG via OS `getrandom` is the right
source. The lockfile stores `token_hash` (sha256:hex) rather
than the raw token (§6) — this is the correct hardening. §12
explicitly notes "token hashing must use a cryptographic hash,
not a checksum," closing the cheap-hash trap.

### D3 — TTL default 5m, min 1s, max 30m (§4 step 10)

Purpose-fit. The 5m default matches the harness implicit
expectation of bounded transcript override windows. Out-of-range
values exit `2` (clap usage) rather than silently clamping —
that is the no-symptom-masking choice. `1000` ms minimum
prevents a caller from accidentally getting an already-expired
lease.

### D4b — Primitive + observer API only; sibling observers in their own PRs (§7, §13)

This is the design decision most likely to read as a shortcut
on first pass. It is not. Two things keep it honest:

1. **The harness is itself the v1 consumer**, and the harness
   orchestrates the transcript override flow externally
   (`pause-handshake → import-replace → resume-handshake`,
   harness spec lines 81–85). The v1 lease is therefore
   immediately useful as a refusal surface for *other*
   agent-runner processes that would race the harness — but
   only once those processes adopt the observer API.
   Until 06-import-replace lands, the lease is a no-op against
   import-replace because import-replace doesn't exist yet.
   Pause-handshake landing without observers is a clean
   primitive PR; landing with observers would force this PR to
   touch `run_repl`, `run_resume`, `run_with_balancing`,
   `migrate_chain_segment`, and the open-path backfill all at
   once — exactly the "scattered hookpoints" the proposal cites
   in D4b.
2. **The proposal commits to the observer API surface in the
   same PR** (§6 `observe()` method, `ExistingLockInfo` return
   type). Sibling PRs adopt a stable API rather than discovering
   it. This is the inverse of a shortcut: it is the *enabling*
   work for downstream PRs to do the right thing without
   reaching into pause-handshake's internals.

§13's "Partial by design" cell is the right framing. The cross-
feature constraint (`/home/nes/projects/agent-runner/worktrees/06-locate/initiatives/06-session-override-contract.md:114-117`)
says observation lands "once 06-pause-handshake lands" — the
initiative explicitly sequences observers after the primitive.
The proposal does not pretend v1 covers them.

### D5 — `now > expires_at` is stale; lazy reap on next acquire; no daemon (§4 step 9, §12)

Purpose-fit. The product is CLI-no-daemon (problem map §6.9,
README:224). A background reaper would be a new product surface,
not a leaner shortcut. Lazy reap is sufficient because:
expiry is honored at the *classification* layer (§4 step 14
returns `lock-expired`) regardless of whether the on-disk file
has been physically removed yet. Stale files persist as inert
artifacts only until the next acquire on the same `<session_id>`,
and they cannot grant ownership because the metadata is checked
against `now` before any acquire decision. No symptom-masking.

## Findings (severity >= MEDIUM)

None.

## Shortcut-indicator grep

Searched `proposals/06-pause-handshake.md` for the canonical
shortcut-indicator flags: `compat`, `shim`, `backward`, `legacy`,
`transitional`, `dual-write`, `feature flag`, `for now`, `in the
future`, `TODO`, `FIXME`, `workaround`, `temporary`, `graceful`,
`self-heal`, `placeholder`, `hardcode`, `magic`, `symptom`,
`hack`, `fallback`, `defer`, `partial`, `followup`, `follow-up`.

Hits and disposition:

- **`partial`** (line 442, §13 D4b row "Partial by design").
  Negation-by-naming: the cell explicitly names the partial
  surface and its compensating mechanism (the observer API),
  not a hidden gap. See D4b above.
- **`fallback`** (lines 180, 312). Both negations: §4 step 3
  "no fallback to direct `session_turns` queries" and §7 "No
  fallback to raw `session_turns` for segmentless sessions."
  Consistent with the cross-feature constraint requiring
  resolver-only ownership.
- **`TODO` / `FIXME`** — zero hits.
- **`compat` / `shim` / `backward` / `transitional`** — zero
  hits.
- **`dual-write` / `feature flag`** — zero hits.
- **`for now` / `in the future` / `temporary` / `workaround` /
  `hack` / `magic` / `placeholder` / `hardcode`** — zero hits.
- **`self-heal` / `graceful` / `symptom`** — zero hits.
- **`legacy`** — zero hits.

## Root-cause vs symptom check

**Lock as DB-table vs lockfile (D1).** Root-cause framing. The
problem-map established that no session-scoped lock primitive
exists today; D1a builds the primitive at the storage layer
that the harness contract names (`lock_path`). D1b would have
required schema work that hides the primitive behind a DB
abstraction the harness doesn't see.

**TTL/crash recovery (D3 + D5).** Root-cause framing for the
"running invocation rows are not session locks" gap (problem
map §1 item 26, §3 item 5). The lease has its own lifecycle
(`expires_at`) decoupled from invocation rows. Crash recovery
is property of the lease, not piggybacked on invocation cleanup.

**Idempotent release (§4 step 13, §6 release marker).** Root
fix for the "missing-lock release should not silently succeed"
class. The proposal forces same-token replay to be *proven*
by the release marker; an arbitrary missing-lock release with
an unknown token returns `16 lock-token-invalid`, not `0`. This
is the correct closure of an obvious symptom-masking trap.

**Token hashing on disk (§6 `token_hash`).** Root fix for
"raw token leak via state directory." Even though the data dir
is owner-private, hashing the stored token means a backup,
sync, or accidental world-readable mode change does not yield
a usable token. §12 forbids non-cryptographic checksums.

## LOW-severity observations / nits

**L1. §5.1 exit-13 row second clause has no v1 trigger.**
`§5.1` lists `13 session-busy` as "Valid lock exists, **or an
active writer cannot be proven safe to pause**." The second
clause mirrors the harness spec
(`/home/nes/projects/agent-harness/tmp/scratch/agent-runner-feature-requests/04-session-pause-handshake.md:21-22`),
but §12 explicitly says "No active provider process drain is
implemented in v1. Existing running invocation rows are not
sufficient session writer leases." So in v1 the second clause
never fires — there is no detection mechanism. This is not a
shortcut that defeats purpose (the harness orchestrates its own
provider lifecycle and uses pause as a refusal surface for
other runner processes), but the §5.1 row could mislead a
reader into thinking pause-handshake will detect a running
provider. A one-line clarification in §5.1 or §10 noting that
the second clause is forward-compat for a sibling-PR signal
would tighten the contract. Not a Phase 4 blocker.

**L2. §6 release marker shape is two-options-pick-one.**
"Phase 5 chooses one shape: replace the lockfile with a release
marker… or write a sibling marker under
`locks/releases/session-<uuid>.json`." Both options are
algorithm-compatible with §4 step 13's same-token replay check,
and §6 commits to marker contents (`released_at` + `token_hash`)
and the "not an active lock" semantic. So the deferral is
bounded and Phase 5-shaped, not open-ended. Worth noting that
the sibling-marker variant (option 2) does not have a specified
cleanup policy in §8 — Phase 5 should pick a bound (e.g.,
overwritten on next acquire of the same session, or aged out
by a documented horizon) so `locks/releases/` does not grow
unbounded over a long-lived install. Phase 5 implementer note,
not a Phase 4 blocker.

**L3. §4 step 8 has no branch for malformed metadata.**
The three branches are "valid and not expired → busy," "no
metadata exists → acquire," and "stale → reap and acquire."
Malformed JSON, wrong `version`, or missing required fields
are not enumerated. The principled choices are: (a) treat as
`Operational` exit `1` (loud failure, owner can manually
delete), or (b) treat as stale (auto-recover). The proposal's
"v1 creates/removes lock state only" framing nudges toward (a)
because (b) is a silent overwrite. Phase 5 should pick and
document; not a Phase 4 blocker.

**L4. §4 step 16 "fsync the directory when practical" is
softer than the crash-recovery argument needs.** D1a's crash-
safety story relies on the lockfile's metadata surviving a
crash with the right contents. File `fsync` in step 11 covers
the metadata bytes; directory `fsync` is what guarantees the
directory entry (existence/rename) is durable. "When
practical" reads as best-effort, but on Linux/macOS directory
fsync after `rename(2)` is the documented crash-safety
recipe. Phase 5 should specify directory `fsync` as
unconditional on supported Unixes (with the platform residual
from §12 noting Windows is undesigned). Phase 5 implementer
note.

**L5. §4 step 2 stacked-vs-unstacked resolver branch.**
"If stacked on locate, call its reusable metadata resolver;
otherwise call `StateDb::resolve_resume` directly." This adapts
to whether 06-locate has merged. The cross-feature constraint
(`06-session-override-contract.md:112-113`) says "Reuse
`StateDb::resolve_resume`. No second ownership path." Locate's
resolver wraps the same path, so the two branches should be
behavior-equivalent on `NoChainFound` / `Ambiguous`. Worth a
Phase 6 contract test that verifies exit-code parity between
the two branches if the unstacked path is exercised; not a
Phase 4 blocker.
