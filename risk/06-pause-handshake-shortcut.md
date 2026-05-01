# 06-pause-handshake — Phase 4 Shortcut Risk Assessment (Rev 3)

## Verdict: LOW (shortcut posture); race-free verification FAILS — see §"Algorithm verification (obligation 2)"

Shortcut-indicator grep on Rev 3 is clean: the §4 / §6 / §8 edits that
swap `flock` for atomic `O_CREAT | O_EXCL` introduce no new
compat/shim/transitional/feature-flag/temporary/workaround/fallback/
defer/partial language, and the prior negation-by-naming "advisory in
v1" frame from Rev 2 carries over unchanged in substance. R1-F01..R1-F04
closures from Rev 2 are not touched by Rev 3 and remain intact. R2-F01
on its **stated surface** (the flock-on-removable-path race) is closed:
no `flock` calls remain in §4, §6, or §8.

However, obligation 2 (verify Rev 3 algorithm is race-free) **does not
clear**. The replacement protocol — read-T_old → unlink → retry-
`create_new` once — has a residual TOCTOU under concurrent stale-acquire
that lets two contenders both emit a `0` lease for the same session.
The shortcut posture is preserved (Rev 3 does not "advisory-away" the
gap; it asserts the gap is closed), but the assertion is incorrect, so
this report flags the algorithm finding for the next audit pass rather
than relaxing the shortcut verdict.

## R2-F01 closure check (audit-only, on stated surface)

R2-F01 named the flock/inode mismatch under contention as the HIGH
defect. Rev 3 retires the flock primitive entirely:

- §4 step 6 explicitly rejects POSIX advisory locks: "The
  implementation must not rely on POSIX advisory locks for mutual
  exclusion because the lockfile path may be unlinked during stale
  cleanup"
  (`proposals/06-pause-handshake.md:230-231`).
- §4 step 7 replaces the lock-then-truncate-or-create pattern with
  pure `OpenOptions::new().create_new(true).write(true).open(...)`
  (= `O_CREAT | O_EXCL | O_WRONLY`)
  (`proposals/06-pause-handshake.md:233-241`).
- §6 mirror: "`acquire()` uses only atomic create-if-absent for mutual
  exclusion" (`proposals/06-pause-handshake.md:350-352`).
- §8 side-effect bullets describe atomic create / unlink-then-retry-
  create only; no `flock` mention
  (`proposals/06-pause-handshake.md:451-468`).

On the literal surface R2-F01 named ("flock target can be removed and
replaced under contention"), the defect is gone — there is no flock
target anymore. **Closed on stated surface.** A different race emerges
from the replacement protocol; see obligation 2 below.

## R1 closure check (still standing)

| Finding | Rev 3 evidence | Status |
| --- | --- | --- |
| **R1-F01** marker shape | Rev 3 does not edit §3.2, §6 marker schema, or §4 marker steps. Sibling marker `session-<uuid>.released` and same-token replay rule unchanged. | **STILL CLOSED** |
| **R1-F02** advisory-scope framing | Rev 3 does not edit §1, §1.2, §10, §11, §12, or §13's advisory-scope language. Five-place narrowing intact. | **STILL CLOSED** |
| **R1-F03** `StateDb::open` clause | §8's "matching 06-locate and 06-export's §8 contracts" sentence and the §12 read-only follow-up commitment are unchanged. | **STILL CLOSED** |
| **R1-F04** §9.1 columns | `assumption_link` and `residual_risk` columns intact; no row removed. | **STILL CLOSED** |

No regression on Rev 2's closure surfaces.

## Algorithm verification (obligation 2)

**Result: NOT race-free.** Two concurrent stale-acquire contenders can
both emit a `0` lease for the same session.

### Trace

Initial state: expired lease `L0` exists at `lock_path`, inode `I0`.
Two pause-handshake processes A and B start near-simultaneously after a
crashed predecessor.

| Step | Process | Effect |
| --- | --- | --- |
| 1 | A | step 7 `create_new` → EEXIST. step 9 reads `L0`, sees expired. |
| 2 | B | step 7 `create_new` → EEXIST. step 9 reads `L0`, sees expired. |
| 3 | A | step 10.2 `unlink(lock_path)` → success. Path empty. |
| 4 | A | step 10.3 `create_new` → success. Path → `I1`. |
| 5 | A | step 10.5 writes lease `L1` (token T_A), fsync, close. **A returns 0.** |
| 6 | B | step 10.2 `unlink(lock_path)` → success. **This unlinks A's `I1`.** Path empty. |
| 7 | B | step 10.3 `create_new` → success. Path → `I2`. |
| 8 | B | step 10.5 writes lease `L2` (token T_B), fsync, close. **B returns 0.** |

Both processes have valid pause receipts with distinct tokens. Step 10's
core test-intent claim — "two concurrent pause calls grant one token,
one `13 session-busy`" (§9.1 Atomic acquire row,
`proposals/06-pause-handshake.md:504`) — is violated.

### Why §4 step 10's atomicity claim does not cover this

The proposal (`proposals/06-pause-handshake.md:264-266`) states: "The
race between `unlink` and retry-create is closed by the same kernel
atomic create-if-absent guarantee: only one contender can create the
replacement lockfile."

The kernel guarantee for `O_CREAT|O_EXCL` is per-call: at most one of
N simultaneous `open(..., O_CREAT|O_EXCL)` calls on the same path
succeeds when the path starts empty. It does **not** sequence
`unlink` against another process's already-completed `create_new`. In
the trace above, B's `unlink` at step 6 occurs after A's `create_new`
already completed at step 4 — B's `unlink` removes the path entry to
A's freshly linked inode `I1`, leaving the path empty for B's own
`create_new` at step 7. Both create_new calls are individually atomic
and individually successful; the race is between A's create and B's
unlink, which the spec does not synchronize.

Step 10.2 is unconditional. The spec reads `T_old` at step 10.1 but
does not use it to predicate the unlink (POSIX has no compare-and-
swap unlink). Without that gate, B cannot tell whether the file at
`lock_path` at the moment of its `unlink` call is `I0` (the expired
file it observed at step 2) or `I1` (A's fresh replacement).

### Failure mode under release/acquire interleaving

The same class of race appears between release and a parallel stale-
acquire. If A holds an expired lease and is at §4 step 15 (write
marker → unlink lockfile), a parallel B in stale-acquire can `unlink`
A's I1 before A's own `unlink`, and `create_new` a fresh I2 that A's
trailing `unlink` then removes. Outcome: B holds an open fd to I2,
the path is empty, and B's later `resume-handshake` finds neither
lockfile nor matching marker (only A's release marker for the
original session) and exits `16 lock-token-invalid`. B is wedged.

### What would close the algorithm gap

R2-F01's required-closure list (`risk/06-pause-handshake-audit.md:113-
121`) named the two structural fixes that work against this whole
class:

1. A separate per-session **stable guard** (e.g.
   `session-<uuid>.guard`) that is `O_CREAT|O_EXCL`-created once,
   never unlinked, and `flock`ed before any acquire/release. The
   guard's pathname is never reused, so flock semantics are stable;
   `.lock` and `.released` mutate only under the guard.
2. A **lock-directory** protocol using atomic `mkdir` (which is
   compare-and-create at the directory entry level, like `O_EXCL`,
   but with stale-owner replacement rules that do not require
   unconditional `unlink` of a directory entry another contender may
   have just refreshed).

A non-structural patch that would also close the trace above:
serialize stale-acquire on the guard, or replace step 10.2's
unconditional `unlink` with a `renameat2(RENAME_EXCHANGE)`-style
atomic swap against a temp file (Linux-only; the spec already accepts
non-portability per §12 Windows residual). Rev 3 takes none of these.

## Shortcut-indicator grep (Rev 3 deltas only)

Re-ran the canonical flag list against the Rev 3 changelog header
(`proposals/06-pause-handshake.md:8-14`) and the changed §4/§6/§8
spans (`:208-291`, `:343-358`, `:451-468`).

- **`atomic`**, **`bounded`**, **`race-free`**, **`Eliminates`**
  (changelog and §4 step 10 lead-in) — assertions of correctness, not
  shortcut indicators. Whether they are technically accurate is the
  obligation-2 finding above; whether they paper over a deferral is
  what the shortcut grep checks, and they do not.
- **`advisory`** (line 13, line 230, plus the carried-over Rev 2
  occurrences) — line 13 ("POSIX advisory locks") and line 230
  ("rely on POSIX advisory locks") use the term in its precise POSIX
  sense (the `flock`/`fcntl` family is *advisory* as opposed to
  *mandatory* locking). Not a shortcut indicator. The Rev 2-era
  "advisory in v1" framing for sibling-writer scope is unchanged and
  remains negation-by-naming with named retrofit owners.
- **`bounded`** (changelog line 11, §4 step 10 lead-in) — describes
  the retry budget (one retry, then exit 13). Not a shortcut.
- **`defer` / `follow-up` / `followup` / `partial`** — only the
  carried-over Rev 2 occurrences (sibling-PR retrofits, schema-probe
  read-only open, §13 D4b row). No new occurrences in Rev 3 spans.
- **`compat`, `shim`, `backward`, `legacy`, `transitional`,
  `dual-write`, `feature flag`, `for now`, `in the future`, `TODO`,
  `FIXME`, `workaround`, `temporary`, `graceful`, `self-heal`,
  `placeholder`, `hardcode`, `magic`, `symptom`, `hack`, `fallback`**
  — zero hits in Rev 3 deltas. (`fallback` still appears at lines
  204 and 377 from Rev 1, both negations.)

Rev 3 introduces no new shortcut posture.

## Regression check vs Rev 2

No shortcut regression. Rev 3's edits are confined to §4 steps 6–10,
§6 acquire description, §8 side-effect bullets, and the changelog
header. The R1 closure surfaces (§1, §1.2, §3.2, §6 marker schema,
§9.1 columns, §10, §11, §12, §13) are not touched. Rev 1 LOW
observations L1, L3, L4, L5 carry forward unchanged as Phase 5
implementer notes.

## Findings (severity >= MEDIUM)

None on shortcut surface. The obligation-2 finding above is escalated
to the next audit pass because it concerns algorithmic correctness,
not shortcut posture: the Rev 3 algorithm asserts a fix that does not
hold, but it asserts it directly rather than via a shortcut hedge, so
the shortcut-track gate is preserved at LOW. The audit reviewer
should treat this as a fresh HIGH (R3-F01 candidate) and decide
whether to require Rev 4 with a stable-guard or atomic-rename
protocol per the R2-F01-required-closure menu.
