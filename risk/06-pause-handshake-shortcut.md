# 06-pause-handshake — Phase 4 Shortcut Risk Assessment (Rev 4)

## Verdict: LOW — R3-F01 closed; algorithm verified race-free

Rev 4 replaces Rev 3's `unlink + retry-create_new` stale-eviction with a
**sentinel-flock + atomic-rename** protocol. The sentinel
(`<lock_dir>/sentinel.lock`) is created idempotently with `O_CREAT |
O_RDWR`, never unlinked, and its open-fd `flock(LOCK_EX)` is held across
every read/decision/write in both `acquire` and `release`. Inside the
critical section, lease state is installed via `O_CREAT | O_TRUNC |
O_WRONLY` to a unique sibling temp file plus same-directory
`rename(2)`. The proposal also adds A8 to §1.1 to record the new
filesystem dependency. Shortcut-indicator grep on the Rev 4 deltas is
clean; R1-F01..R1-F04 and R2-F01 closures are untouched and still
standing; the obligation-2 trace from Round 3 cannot reproduce. Verdict
returns to LOW with no cross-track escalation.

## Obligation 1 — R3-F01 closure

R3-F01 named the stale-acquire TOCTOU between `read(expired) → unlink →
retry create_new` as the HIGH defect: two contenders could both classify
the lockfile as expired, then B's unconditional `unlink` could remove A's
freshly created replacement inode, allowing both to emit `0`.

Rev 4 eliminates that surface:

- `proposals/06-pause-handshake.md:17-29` (changelog) explicitly tags the
  sentinel-flock pattern as "Eliminates the TOCTOU window in Rev 3
  between stale-read and unlink (R3-F01)".
- `:248-259` (§4 step 7) introduces the sentinel: opened
  `O_CREAT | O_RDWR` (no `O_EXCL`, idempotent), then `flock(sentinel_fd,
  LOCK_EX)` is held "for the full session-lock read/write decision".
- `:261-282` (§4 step 10) replaces the prior unlink-then-retry with:
  write temp → fsync → atomic `rename` onto session lock path → remove
  any stale `.released` marker → fsync directory → release sentinel
  flock.
- `:284-289` states the structural argument explicitly: "all contenders
  serialize on the never-unlinked sentinel inode, no process can unlink
  or replace a session lock created by another contender between
  stale-read and stale-eviction. Inside the critical section,
  same-directory `rename` provides atomic installation of the new
  lease."
- `:366-385` (§6) mirrors the runtime: a private `Sentinel { path,
  file }` helper with `with_locked<F, R>(F) -> R` wraps every
  `acquire`/`release` body; "The sentinel file is never deleted by
  acquire or release."
- `:510-538` (§8) updates the side-effect contract: "all contenders
  serialize on …", "Read, write, rename, and unlink session lock files
  only while holding the sentinel flock", and the temp-file naming rule
  (`.acquire-<pid>-<random>.tmp`, `.release-<pid>-<random>.tmp`) so a
  crashed mid-write does not collide with the live lock path.
- §9.1 row "Stale acquire" updated (`:587`) to "Expired lockfile is
  lazily replaced under the sentinel flock by atomic rename" with
  fixture `Prewritten expired metadata` and assumption_link `A5, A7,
  A8`.
- §1.1 A8 (`:98`) records the new dependency: "Atomic rename plus
  advisory flock on a non-removable sentinel is sufficient … on POSIX
  filesystems supporting `flock(2)` and `rename(2)` atomicity",
  evidence/invalidator filled in.

R3-F01 is **closed**.

## Obligation 2 — Algorithm verification (race-free for documented threat model)

### Two concurrent stale-acquire contenders (the R3-F01 trace)

Initial state: lock dir contains expired lease `L0` and the sentinel
inode `S` (created on first run). Processes A and B start near
simultaneously.

| Step | Process | Effect |
| --- | --- | --- |
| 1 | A | Opens `sentinel.lock` `O_CREAT\|O_RDWR` → fd_A (inode `S`); `flock(fd_A, LOCK_EX)` succeeds. |
| 2 | B | Opens `sentinel.lock` `O_CREAT\|O_RDWR` → fd_B (inode `S`); `flock(fd_B, LOCK_EX)` **blocks** because A holds the lock on the same inode (POSIX/Linux flock is per-inode across fds). |
| 3 | A | Opens session lock read-only, reads `L0`, sees `expires_at <= now`. Stale path. |
| 4 | A | Writes lease `L_A` (token `T_A`) to `<session>.lock.acquire-<pidA>-<r>.tmp`, fsyncs, `rename(...tmp, <session>.lock)`. Path entry → `I_A`. |
| 5 | A | Removes any old `.released` marker, fsyncs dir, closes fd_A (releasing flock). Returns `0` with `T_A`. |
| 6 | B | `flock(fd_B, LOCK_EX)` unblocks. |
| 7 | B | Opens session lock read-only; sees `L_A` with `expires_at > now`. |
| 8 | B | `expires_at > now` → release sentinel flock, exit `13 session-busy`. |

The Rev 3 trace step 6 ("B: `unlink(P)` removes the directory entry for
`I_A`") is unreachable in Rev 4: B does not perform any
`unlink(session-<uuid>.lock)` during acquire — Rev 4 only mutates the
session lock via atomic rename, never unconditional unlink. And B's
read-and-decide cycle runs after A's rename, not before, because the
sentinel flock serializes them.

### Stale-acquire vs concurrent release (the R3-F01 §"Failure mode" trace)

Round 3's secondary trace had A holding an expired lease and racing a
parallel B's stale-acquire. Under Rev 4 both `acquire` and `release` run
inside `Sentinel::with_locked` (§6, `:366-381`; §4 steps 7, 14;
`:510-526` and `:531-538`). Whichever process first wins
`flock(LOCK_EX)` runs read-decide-write to completion (rename or
unlink + marker rename) before the other observes any state. There is no
window where one process's `unlink` of `<session>.lock` can race another
process's `rename` onto it: the loser sees the post-mutation state.
Concretely:

- If release wins first: lockfile is unlinked, marker is renamed in.
  Stale-acquire then sees `ENOENT` on the lockfile (and the new marker
  is irrelevant to acquire — `:280-281` removes any old marker on the
  way out), writes via temp+rename, exits `0` with a fresh token.
- If stale-acquire wins first: lease is replaced by atomic rename to a
  new token-hash. Release then reads the new lease, sees a different
  `token_hash`, and exits `16 lock-token-invalid`. This is the correct
  outcome — the original token no longer owns the lease — and matches
  §5.2's `lock-token-invalid` semantics.

### First-acquire (no prior lockfile)

Same pattern: acquire takes the sentinel flock, opens session lock
read-only → `ENOENT`, writes temp + atomic rename, releases flock,
returns `0`. A second contender blocks on the sentinel flock, then on
unblock reads the freshly renamed lease and exits `13`. Race-free.

### Crash-during-write

If A crashes between writing the temp file and the rename, the temp file
has a unique path (`.acquire-<pid>-<r>.tmp`) and never appears at
`<session>.lock`; the lock path remains in its pre-attempt state (or
`ENOENT`). §8 (`:558-564`) explicitly notes the orphaned temp does not
collide with the lock path and is not a future stale-eviction concern.
If A crashes between rename and flock-release, the kernel releases the
flock on fd close. The lease lives at the lock path with the recorded
TTL; crash recovery is the documented TTL-driven lazy replacement (D5,
A5) — now safely serialized through the sentinel.

### A8 invalidators

A8 (`:98`) names the two filesystem regimes that would invalidate the
race-free claim: "Filesystems without working `flock` such as NFSv2/3
quirks, or non-atomic rename across mount points." The lock dir is fixed
at `~/.local/share/oulipoly-agent-runner/locks/`, so all temp files
share the lock dir's mount and `rename(2)` atomicity is preserved. The
"local POSIX filesystem with working flock" assumption is consistent
with the supported-surface deployment mode (§11, local CLI binary,
owner-private state).

**Algorithm verified race-free for the documented threat model.**

## Obligation 3 — R1 / R2 closures still standing

| Finding | Rev 4 evidence | Status |
| --- | --- | --- |
| **R1-F01** sibling marker shape | §3.2 `release_marker_path` field unchanged (`:199`); §4 step 16 marker write/rename path (`:303-311`) keeps `session-<uuid>.released`; §6 marker schema (`:466-474`) and idempotency rule (`:480-488`) untouched. | **STILL CLOSED** |
| **R1-F02** advisory-scope framing | §1 narrowing (`:47-55`), §1.2 (`:108-112`), §10 README bullets (`:614-618`), §11 deferred observers list (`:638-640`), §12 (`:670-673`), §13 row (`:692`) — all unchanged. Five-place narrowing intact; named retrofit owners preserved. | **STILL CLOSED** |
| **R1-F03** `StateDb::open` clause | §8 (`:549-557`) "matching 06-locate and 06-export's §8 contracts" sentence preserved; §12 read-only follow-up commitment (`:677-680`) intact. | **STILL CLOSED** |
| **R1-F04** §9.1 columns | `assumption_link` and `residual_risk` columns present on every row (`:577-598`); A8 added to relevant rows ("Atomic acquire", "Per-session scope", "Stale acquire", "Busy lock", "Correct release", "Expired matching release", "Permissions", "Side effects"). | **STILL CLOSED** |
| **R2-F01** flock-on-removable-inode | Rev 4 reintroduces flock, but on a *non-removable* sentinel — exactly the structural fix R2-F01's required-closure list named ("a separate per-session **stable guard** … created once, never unlinked, and `flock`ed before any acquire/release"). §4 step 7, §6 `Sentinel`, §8 "never deleted by acquire or release" all enforce this. | **STILL CLOSED** (now closed structurally rather than by removal) |

No regression on Round 1 or Round 2 closure surfaces.

## Shortcut-indicator grep (Rev 4 deltas only)

Re-ran the canonical flag list against the Rev 4 changelog (`:17-29`),
A8 row (`:98`), §4 steps 6–10 (`:241-291`), §4 steps 14–17 (`:296-319`),
§6 sentinel/acquire/release prose (`:366-417, :454-461`), §8 bullets
(`:510-538, :558-564`), §9.1 row updates (`:583-598`), §12 (`:660-661`),
A8 column citations (`:691-700`).

- **`atomic`**, **`race-free`**, **`Eliminates`**, **`serialize`** —
  correctness assertions tied to the sentinel-flock + atomic-rename
  protocol. Verified above in obligation 2; not shortcut hedges.
- **`advisory`** — used in two distinct senses, both legitimate:
  (a) "POSIX advisory lock" in §1 A8 line `:98` and §6/§8 sentinel
  description (the technical opposite of mandatory locking — the
  protocol is precisely an advisory-lock-based mutex on a non-removable
  inode); (b) the carried-over Rev 2 "advisory in v1" framing for
  sibling-writer scope (R1-F02 closure surface, retrofit owners named).
  Neither is a shortcut hedge.
- **`when practical`** (§4 step 10.6, §8 bullet for fsync directory) —
  carries forward from Rev 3; portability hedge for `fsync(dir_fd)` on
  filesystems where it is a no-op. Not a Rev 4 introduction.
- **`defer` / `follow-up` / `partial`** — only the carried-over Rev 2
  occurrences (sibling-PR retrofits in §1, §12, §13; schema-probe
  read-only open in §8, §12). No new occurrences in Rev 4 spans.
- **`compat`, `shim`, `backward`, `legacy`, `transitional`,
  `dual-write`, `feature flag`, `for now`, `in the future`, `TODO`,
  `FIXME`, `workaround`, `temporary`, `graceful`, `self-heal`,
  `placeholder`, `hardcode`, `magic`, `symptom`, `hack`, `fallback`** —
  zero hits in Rev 4 deltas. (`fallback` still appears at `:230` and
  `:498` from Rev 1, both negations.)

Rev 4 introduces no new shortcut posture.

## Regression check vs Rev 3

Rev 4 edits are confined to:

- Changelog header (`:17-29`).
- A8 row added to §1.1 register (`:98`).
- §4 steps 6–10 (acquire), step 14 (release sentinel-open), step 16
  (release write/rename) (`:241-291, :296-311`).
- §6 sentinel struct + acquire/release prose (`:366-417`); shared
  sentinel paragraph (`:454-461`).
- §8 acquire/release/sentinel side-effect bullets and the temp-file
  naming clause (`:510-538, :558-564`).
- §9.1 assumption_link updates on the rows that depend on the new
  serialization (`:583-598`).
- §12 file-backed lease residual updated (`:660-661`).
- §13 cross-feature row notes preserved (`:691-700`).

R1 closure surfaces (§1, §1.2, §3.2, §6 marker schema, §9.1 column
shape, §10, §11, §12 advisory-scope residual, §13 D4b row) are not
substantively touched. Rev 1 LOW observations L1, L3, L4, L5 carry
forward unchanged as Phase 5 implementer notes. No shortcut regression.

## Findings (severity >= MEDIUM)

None on shortcut surface. The Round 3 cross-track escalation (R3-F01)
is closed by Rev 4. The algorithm is race-free under the documented
threat model (POSIX local fs with working `flock(2)` + atomic
same-directory `rename(2)`); the invalidator regimes are recorded as A8.
Shortcut-track gate clears at LOW; no further audit-track escalation
required from this report.
