# 06-pause-handshake - Phase 4 Audit Risk Report (Round 3 / Rev 3)

**Verdict: HIGH**

Rev 3 closes the specific Round 2 `flock`-on-removable-inode failure mode: the
proposal no longer relies on POSIX advisory locks for mutual exclusion and now
uses `OpenOptions::create_new(true)` / `O_CREAT | O_EXCL` for the create step
(`proposals/06-pause-handshake.md:226`, `proposals/06-pause-handshake.md:232`,
`proposals/06-pause-handshake.md:350`).

The audit gate still does not clear. The replacement algorithm remains
path-racy during stale acquisition because stale contenders unlink by pathname
after reading old metadata, without proving the path still names that same old
lease. A process that already read the expired lease can delete a newer lockfile
created by another contender, then successfully create its own lock. This still
permits two `pause-handshake` calls to return `0` for the same session under
realistic stale-acquire interleavings.

## Round 2 Closure Check

### R2-F01 - Not Fully Closed

The Rev 2 bug was specifically that `flock` was held on a pathname that the
algorithm could unlink and recreate. Rev 3 removes that primitive and correctly
states that `flock` must not be used for this lockfile because the path may be
unlinked during stale cleanup (`proposals/06-pause-handshake.md:226`).

That part of R2-F01 is closed. However, the replacement protocol does not supply
an equivalent stable compare-and-replace guard. Stale acquire now does:

1. read expired lease evidence,
2. `unlink(lock_path)`,
3. retry `create_new` once,
4. treat retry `EEXIST` as another winner (`proposals/06-pause-handshake.md:252`).

The race is not between `unlink` and retry-create alone. It is between an old
stale read and a later pathname unlink. The proposal never proves that the file
being unlinked is still the expired file that was read.

Failing interleaving:

1. Expired lockfile `E` exists.
2. Process A and process B both fail `create_new` with `EEXIST`, read `E`, and
   classify it as expired.
3. A unlinks `E`.
4. A retries `create_new`, creates replacement lockfile `A`, and proceeds toward
   success.
5. B executes its already-authorized `unlink(lock_path)`. The pathname now names
   `A`, not `E`, so B removes A's replacement.
6. B retries `create_new`, creates replacement lockfile `B`, and proceeds toward
   success.
7. A may still write/fsync/return success through its open fd even though its
   lease is no longer reachable at `lock_path`; B also returns success with a
   different token.

Impact: this violates the core single-lease guarantee and the test-intent row
that two concurrent pause calls grant one token and one `13 session-busy`
(`proposals/06-pause-handshake.md:504`). It is the same safety-class failure as
R2-F01, even though the concrete mechanism changed from split `flock` inodes to
stale-read pathname deletion.

## Rev 3 Algorithm Check

### R3-F01 - HIGH - Stale-acquire unlink is not compare-and-replace

Rev 3 says the race is closed because only one contender can create the
replacement lockfile with `O_CREAT | O_EXCL` (`proposals/06-pause-handshake.md:264`).
That statement is incomplete. `O_EXCL` makes each individual create atomic, but
it does not protect the preceding pathname `unlink` from deleting a replacement
created after the stale metadata read.

The side-effect contract explicitly permits stale acquire to unlink an expired
lockfile and then retry create (`proposals/06-pause-handshake.md:456`). The API
section repeats the same behavior (`proposals/06-pause-handshake.md:350`). There
is no stable guard file, no lock directory ownership token, no generation CAS,
no inode validation tied atomically to unlink, and no "rename only if old token
still matches" primitive. The old token evidence `T_old` is read
(`proposals/06-pause-handshake.md:255`) but is not used to constrain the unlink.

Release has a related race with stale acquisition. `resume-handshake` may read
an expired matching lockfile, then write a marker and unlink `lock_path`
(`proposals/06-pause-handshake.md:277`). If a stale acquire replaces the expired
lockfile between the resume read and resume unlink, the old-token resume can
remove the new lease. That can leave a successful new pause without a reachable
lockfile or allow a later pause to acquire again.

Required closure: replace the stale cleanup protocol with a stable
synchronization or true compare-and-replace contract. Acceptable shapes include:

- a separate per-session guard file that is created once and never removed, with
  lock and marker mutation serialized while holding the guard;
- a lock directory or generation protocol where stale cleanup cannot delete a
  newer owner after an old read;
- a platform-specific implementation that proves the unlink applies to the same
  object that was read, not merely the same pathname.

The proposal must update §4, §6, §8, and §9.1 to cover concurrent stale-acquire
and stale-acquire/resume interleavings. The acceptance signal should explicitly
require one success and one `13`, with the winning token's lockfile still
present and containing the winning token hash after all contenders exit.

### R3-F02 - MEDIUM - Loser can observe partial metadata during initial acquire

Rev 3 creates the lockfile before writing lease JSON (`proposals/06-pause-handshake.md:243`).
A concurrent pause that loses `create_new` can immediately read the just-created
file before the winner has written complete metadata. The current contract maps
malformed, unreadable, or missing required fields to exit `1 operational-error`
(`proposals/06-pause-handshake.md:248`).

This does not create a double lease, but it weakens the stated concurrent-acquire
surface. The matrix expects two concurrent pause calls to produce one `0` and
one `13` (`proposals/06-pause-handshake.md:504`), not intermittent operational
errors caused by a normal in-progress writer.

This can be closed together with R3-F01 by serializing metadata writes under a
stable guard, or by defining an in-progress state that losers treat as
`session-busy` rather than corruption.

## Round 1 Closure Recheck

### R1-F01 - Still Closed

The sibling release marker remains concrete: `session-<uuid>.released` is in the
resume receipt, path computation, marker schema, idempotent replay behavior, and
residuals (`proposals/06-pause-handshake.md:174`,
`proposals/06-pause-handshake.md:216`, `proposals/06-pause-handshake.md:384`,
`proposals/06-pause-handshake.md:591`).

### R1-F02 - Still Closed As Accepted Residual

Writer-path observation remains explicitly deferred with narrowed v1 acceptance:
scope statement, anti-scope, tests, README work, residuals, and cross-feature
table all say the primitive is advisory until sibling PRs wire observers
(`proposals/06-pause-handshake.md:30`, `proposals/06-pause-handshake.md:443`,
`proposals/06-pause-handshake.md:518`, `proposals/06-pause-handshake.md:535`,
`proposals/06-pause-handshake.md:581`, `proposals/06-pause-handshake.md:609`).

### R1-F03 - Still Closed With Explicit Side-Effect Residual

The `StateDb::open` exception remains pinned to inherited open-time behavior:
parent directory creation, WAL enable, schema ensure, and chain backfill are
accepted; command-added DDL/row mutation remains out of scope
(`proposals/06-pause-handshake.md:478`, `proposals/06-pause-handshake.md:594`).

### R1-F04 - Still Closed

The test matrix still includes `assumption_link` and `residual_risk` columns on
each row (`proposals/06-pause-handshake.md:498`).

## Regression Check

No regression found on the Round 1 closure surfaces.

Rev 3 does regress the Round 2 closure intent by replacing the `flock` race with
an insufficient pathname-unlink protocol. The proposed `O_CREAT | O_EXCL` create
step is atomic, but the full stale-acquire/release algorithm is not race-free.

## Required Rev 4 Closure

Rev 4 must keep the Round 1 closures intact and define a stale-replacement
protocol that cannot unlink a newer lease after reading an older expired lease.
Because R3-F01 is HIGH and affects the core single-token guarantee, Phase 4 audit
must rerun after the proposal revision.
