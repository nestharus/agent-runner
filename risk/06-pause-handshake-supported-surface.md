# 06-pause-handshake — Phase 4 Supported-Surface Risk Report (Rev 3)

**Termination signal:** `none`
**Verdict:** **LOW** — supported-surface track itself does not regress.

**Cross-track flag (audit lane):** Rev 3's stale-acquire algorithm has a
residual multi-contender race that, on supported-surface review of the
algorithm text, does not fully close R2-F01. See "Closure check on
R2-F01" below. The authoritative call belongs to the audit reviewer; this
report records the supported-surface reading for cross-track visibility.

## Closure check on R2-F01 (audit-only, cross-track)

R2-F01 (Round 2 audit, HIGH): "Removable flock target can split the lock
critical section." Rev 3 replaces flock-on-removable-path with
`O_CREAT | O_EXCL` atomic create-or-fail (§4 step 7), and adds a bounded
stale-acquire path (§4 step 10) consisting of `read T_old` →
`unlink(lock_path)` → single retry of `create_new(lock_path)`.

The first-acquire path (§4 step 7) is race-free: `O_CREAT | O_EXCL` is a
kernel-atomic create-if-absent, so for any single create attempt only one
contender wins. The §3.3 / §1.2 / R1-F02 advisory-scope framing is
unchanged and still honest.

The stale-acquire path (§4 step 10) is the surface where the closure
weakens. The proposal claims: "only one contender can create the
replacement lockfile" (`proposals/06-pause-handshake.md:264`). On a
supported-surface reading of the algorithm text, that claim holds for
any individual `create_new` call but does **not** hold for the protocol
as a whole when two stale-acquire contenders run concurrently. The
following interleaving appears to remain reachable under the Rev 3
algorithm as written:

1. Lockfile `P` exists with expired lease `E1`.
2. A: `create_new(P)` → `EEXIST`. Reads `E1`, sees expired. Reads
   `T_old`. (`§4 step 9` → `§4 step 10.1`.)
3. B: `create_new(P)` → `EEXIST`. Reads `E1`, sees expired. Reads
   `T_old`. (Same sequence as A.)
4. A: `unlink(P)` (§4 step 10.2) — `P` now has no entry.
5. A: retry `create_new(P)` (§4 step 10.3) → succeeds. A creates inode
   `I_A` at `P`, writes lease `E_A`, fsyncs (§4 step 10.5).
6. B: `unlink(P)` (§4 step 10.2). This removes the directory entry for
   `I_A`. The unlink is unconditional — no inode/T_old verification.
7. B: retry `create_new(P)` (§4 step 10.3) → succeeds. B creates inode
   `I_B` at `P`, writes lease `E_B`, fsyncs.
8. Both A and B exit `0` with distinct tokens. Path `P` resolves to
   `I_B`; A's lease is reachable only via A's stdout token, never via
   `P`.

This collapses the test-intent claim "two concurrent pause calls grant
one token. One `0`, one `13`" (`proposals/06-pause-handshake.md:504`)
under the recovery-after-crash scenario that TTL-based recovery is
specifically designed for (D3/D5, A5). It also undermines the §1.2
"stable refusal surface" framing of the harness consumer contract:
resume-handshake against A's token will return `16 lock-token-invalid`
(because `P` carries `E_B`'s `token_hash`), so A's harness has been
handed a token that never belonged to a winning lease.

Two design hooks would close this on the supported-surface side without
adding new contract surface:

- A separate, never-unlinked per-session guard file (the path the Round
  2 audit suggested as `session-<uuid>.guard`), used to serialize the
  unlink/retry-create sequence. The lock and marker paths in §3 / §6
  remain unchanged on the wire; the guard is purely internal.
- Conditional unlink that verifies the on-disk lease still hashes to
  `T_old` before unlinking (`renameat2(RENAME_EXCHANGE)` with a
  comparison tmpfile, or `open` + `fstat` + `flock(LOCK_EX|LOCK_NB)` on
  the read fd before issuing the unlink, gated on a re-read of the
  lease body matching `T_old`).

Either fix preserves the §3 wire-format and §6 public API. The proposal
text at `proposals/06-pause-handshake.md:256` (the bare `unlink`) and
the closure claim at `proposals/06-pause-handshake.md:264-266` are the
only sites that need to move.

R2-F01 status from supported-surface vantage: **partially closed**
(first-acquire path is race-free; stale-acquire path remains racy under
multi-contender stale interleaving). Authoritative closure call belongs
to the audit reviewer.

## R1-F01..R1-F04 closure status under Rev 3

| R1 finding | Rev 2 closure surface | Rev 3 impact | Status |
| --- | --- | --- | --- |
| **R1-F01** idempotent release marker shape | §3.2 `release_marker_path`, §6 marker JSON, §8 marker side-effects, §12 "no future marker-shape deferral" | §6/§8 retain the sibling-marker shape verbatim; §4 step 8 still removes prior marker on fresh acquire; §4 step 10.5 still removes prior marker on stale-acquire success. No regression. | **CLOSED (still)** |
| **R1-F02** writer-path observer narrowing | §1 advisory-lock framing, §12 narrowed acceptance surface, §13 "Partial by design", §10 README mandate | Unchanged in Rev 3 (the changelog's Rev 3 entry only touches §4/§6/§8 lock primitive surfaces). §1's advisory framing is preserved verbatim. | **CLOSED (still)** |
| **R1-F03** `StateDb::open` mutation exception pinned | §8 explicit accepted open-time effects matching 06-locate / 06-export | §8 retains the explicit clause; Rev 3 only adds the side-effect bullet for "Unlink an expired … `.lock` during stale-acquire, followed by one atomic retry-create" — same lock-state-only domain, no DB surface added. | **CLOSED (still)** |
| **R1-F04** §9.1 `assumption_link` + `residual_risk` columns | §9.1 matrix carries both columns, A1–A7 references | Rev 3 does not re-shape the matrix. Existing rows still link to A1–A7 and carry residuals. | **CLOSED (still)** |

All four R1 closures stand under Rev 3.

## Fresh assessment of Rev 3 changes (supported-surface lane)

### Wire-format / public API (§3, §6)

Unchanged from Rev 2. Receipt fields, exit-code namespace, lock/marker
paths, token format, TTL bounds, and `SessionLock` public methods all
preserved verbatim. Harness consumer contract surface is byte-identical
to Rev 2.

### Side-effect contract (§8)

Rev 3 adds one bullet: "Unlink an expired
`locks/session-<session_id>.lock` during stale-acquire, followed by one
atomic retry-create." This is on-domain (lock state only), inside the
existing `~/.local/share/oulipoly-agent-runner/locks/` blast radius, and
accepted by the same §8 clause that already permits create/replace on
the lockfile path. No new state surface, no new permission surface, no
new DB surface. Permissions clause (`0700`/`0600`) unchanged; failure
remains exit `1`.

### Migration / rollback story

Unchanged. Lockfile path shape, marker path shape, and lock-dir layout
are all identical to Rev 2. Older binaries remain unaware of either
lockfile or marker. Operators may still delete stale lock-dir entries
after confirming no Rev-3 binary is observing.

### Observability

Unchanged. Receipts, stderr JSON, and lockfile/marker contents remain
the entire v1 surface. No new trace event, audit row, or telemetry
surface.

### Test matrix (§9.1)

Unchanged in Rev 3. The "Atomic acquire" and "Stale acquire" rows still
read in terms of expired-lockfile replacement; their existing residual
columns ("Does not prove behavior on non-local/network filesystems";
"Does not add a background reaper; cleanup remains lazy by design") are
silent on the multi-stale-contender interleaving documented above. If
the audit reviewer accepts R2-F01 as still-open, the §9.1 matrix should
gain a "stale-acquire multi-contender" row before Phase 6 implementation
takes the algorithm at face value.

## No-regression check vs Rev 2 supported-surface findings

| Rev 2 advisory | Rev 3 status |
| --- | --- |
| R1-F01-supported (orphaned-lockfile UX during sibling writes) | **unchanged** — root-cause fix still belongs to sibling PRs (D4b); §10 advisory-scope mandate still in force. |
| R1-F02-supported (Phase 5 marker shape) | **closed (still)** — Rev 3 preserves §6/§12 marker shape commitment. |
| R1-F03-supported (A2 multi-active-segment edge) | **unchanged** — same contract, same mitigation (sibling adoption). |
| R1-F04-supported (Windows residual in README) | **unchanged** — §12 still says "Windows semantics are not designed". The Rev 3 algorithm also implicitly assumes POSIX `O_CREAT|O_EXCL` and `unlink` semantics; Windows behavior under the new algorithm remains undesigned, in line with §12. Non-blocking, but the Rev 3 changelog could note that the algorithm change does not retire the Windows residual. |
| R1-F05-supported (CLI `observe` ergonomics) | **unchanged** — `observe` still library-only in §6. |
| R1-F06-supported (README v1-vs-eventual sentence) | **closed (still)** — §10 mandate preserved. |

No Rev 3 change degrades Rev 2's adjacent-paths verdicts. Migration /
rollback / observability story unchanged.

## Verdict rationale

- **Termination signal #1 (`invalidated-assumption`)** — does not fire.
  A1–A7 hold against problem-map evidence; Rev 3 introduces no new
  assumption that contradicts current state. A4 ("lease must outlive
  the pause-handshake process") and A5 ("TTL-based crash recovery is
  sufficient for v1") are about lease lifecycle and recovery
  *intent*, not the synchronization mechanism that implements them; the
  R2-F01 residual concerns the implementation, which is audit's lane.
- **Termination signal #2 (`non-positive-value`)** — does not fire. The
  Rev 1 retired-risk table is preserved across both Rev 2 and Rev 3;
  the lease primitive still produces positive value over status quo
  (no lock at all) even with the residual race surface, because the
  first-acquire path is race-free and covers the common harness flow.
  The residual narrows the safety case under the recovery-after-crash
  edge but does not collapse it.

**Standard verdict: LOW** for the supported-surface track itself. Phase
4 supported-surface gate does not independently block over R2-F01,
because the R2-F01 residual is a synchronization/correctness concern
inside the audit lane rather than a deployment, harness-contract,
migration, rollback, or observability concern. The harness consumer
contract surface (receipts, exit codes, lock_path, token format, TTL
policy) is byte-identical to Rev 2 and remains LOW.

The audit reviewer's Rev 3 reassessment authoritatively decides whether
R2-F01 is now closed; this report flags the supported-surface reading
of the algorithm for cross-track visibility but does not pre-empt that
call.

## Advisory items carried forward (non-blocking)

1. README §10 should still name Linux/macOS as v1-supported with
   Windows behavior undefined (R1-F04-supported), now reinforced because
   Rev 3 also assumes POSIX `O_CREAT|O_EXCL` and `unlink` semantics.
2. Sibling adoption PR (resume / repl / migrate / balanced one-shot)
   should consider exposing `SessionLock::observe` as
   `agents session observe <id>` for first-class read inspection
   (R1-F05-supported).
3. Sibling-PR observers should refuse-and-emit structured stderr JSON
   `session-busy` rather than waiting silently, preserving the §1.2
   "stable refusal surface" framing.
4. **(New, cross-track)** If the audit reviewer reassesses R2-F01 as
   still-open under Rev 3, the §9.1 matrix should gain an explicit
   "stale-acquire multi-contender" row before Phase 6 implementation,
   so that Phase 6 contract/test authors do not encode the racy
   protocol as the test oracle.
