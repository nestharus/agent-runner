# 06-pause-handshake - Phase 4 Audit Risk Report (Round 4 / Rev 4)

**Verdict: LOW**

Rev 4 closes the Round 3 safety finding. The proposal now makes
`locks/sentinel.lock` the only advisory-lock target, never removes it, and
requires both `pause-handshake` and `resume-handshake` to hold an exclusive
sentinel `flock` while reading, replacing, renaming, or unlinking per-session
lock and marker files (`proposals/06-pause-handshake.md:242`,
`proposals/06-pause-handshake.md:248`, `proposals/06-pause-handshake.md:261`,
`proposals/06-pause-handshake.md:295`, `proposals/06-pause-handshake.md:396`,
`proposals/06-pause-handshake.md:408`, `proposals/06-pause-handshake.md:454`,
`proposals/06-pause-handshake.md:511`).

No new Phase 4 blocking finding is open.

## Closure Check

### R3-F01 - Closed

The Rev 3 failure was stale-read pathname deletion: a contender could read an
expired lock, another contender could replace it, and the first contender could
then unlink the newer replacement by path. Rev 4 removes that interleaving.

`pause-handshake` now enters the sentinel critical section before inspecting the
session lock. If the lock is absent or stale, it writes new metadata to a unique
temporary file, fsyncs it, atomically renames it onto
`session-<session_id>.lock`, removes the old release marker, fsyncs the
directory when practical, and only then releases the sentinel flock
(`proposals/06-pause-handshake.md:261`, `proposals/06-pause-handshake.md:268`,
`proposals/06-pause-handshake.md:271`, `proposals/06-pause-handshake.md:278`,
`proposals/06-pause-handshake.md:280`, `proposals/06-pause-handshake.md:282`).

Because all compliant contenders serialize on a never-unlinked sentinel inode,
there is no longer a window where a process can act on old stale evidence after
another process has installed a newer lease. The stale path also no longer
unlinks the session lock before replacement (`proposals/06-pause-handshake.md:404`).

The related release/acquire race is also closed. `resume-handshake` holds the
same sentinel flock from lockfile inspection through marker rename and lockfile
unlink (`proposals/06-pause-handshake.md:297`, `proposals/06-pause-handshake.md:303`,
`proposals/06-pause-handshake.md:306`). A release using an old token cannot read
one lease and unlink another replacement produced by a concurrent stale acquire,
because the replacement cannot occur until the release critical section exits.

### R3-F02 - Closed

The Rev 3 partial-metadata loser case is closed as a consequence of the same
serialization. A losing pause process cannot observe a just-created empty or
partially written lockfile because new lease metadata is written and fsynced to a
temporary file, then atomically renamed onto the lock path while the sentinel is
held (`proposals/06-pause-handshake.md:271`, `proposals/06-pause-handshake.md:278`,
`proposals/06-pause-handshake.md:396`). The next contender enters after the
rename and sees either a complete non-expired lease, returning `13
session-busy`, or a complete stale/malformed lease under the documented rules.

## Rev 4 Race Review

For the documented local POSIX threat model, the sentinel-flock plus
same-directory atomic-rename algorithm is race-free for the v1 lock primitive.
The required assumptions are now explicit in A8: working `flock(2)` on a
non-removable sentinel, same-mount atomic `rename(2)`, and local filesystems
rather than known-bad NFS variants (`proposals/06-pause-handshake.md:98`,
`proposals/06-pause-handshake.md:657`).

Concurrent initial acquire:

- Process A holds the sentinel, sees no active lock, writes temp metadata, and
  renames it onto the session lock path.
- Process B cannot inspect the session lock until A releases the sentinel.
- B then sees A's complete non-expired lease and returns `13 session-busy`.

Concurrent stale acquire:

- Only one process can classify the old lease as stale and replace it at a time.
- Later contenders read the replacement after entering the sentinel critical
  section and return `13` while it is valid.
- No stale contender can unlink or overwrite a newer lease based on stale
  pre-replacement evidence.

Concurrent release versus acquire:

- Release and acquire share the same sentinel critical section.
- If release runs first, the lockfile is removed and marker is installed before
  acquire evaluates absence/staleness.
- If acquire runs first, release evaluates the new token evidence and cannot
  remove the new lease with an old token.

Crash behavior remains bounded by the proposal's TTL design and residuals:
orphan temp files are outside the active lock path, stale lockfiles are lazily
replaced, and there is no background reaper (`proposals/06-pause-handshake.md:559`,
`proposals/06-pause-handshake.md:587`, `proposals/06-pause-handshake.md:655`).
This is acceptable for Phase 4 because the requested guarantee is mutual
exclusion among compliant local pause/resume processes, not durable recovery
from every host crash boundary.

## Prior Closure Recheck

### R1-F01 - Still Closed

The idempotent release marker remains concrete as
`session-<uuid>.released`. It appears in path computation, resume receipt,
release behavior, test intent, README requirements, and residuals
(`proposals/06-pause-handshake.md:196`, `proposals/06-pause-handshake.md:231`,
`proposals/06-pause-handshake.md:435`, `proposals/06-pause-handshake.md:592`,
`proposals/06-pause-handshake.md:612`, `proposals/06-pause-handshake.md:674`).

### R1-F02 - Still Closed As Accepted Residual

Writer-path observation is still explicitly deferred and the v1 harness surface
is narrowed to an advisory primitive until sibling PRs wire observers
(`proposals/06-pause-handshake.md:44`, `proposals/06-pause-handshake.md:83`,
`proposals/06-pause-handshake.md:503`, `proposals/06-pause-handshake.md:597`,
`proposals/06-pause-handshake.md:664`, `proposals/06-pause-handshake.md:692`).

### R1-F03 - Still Closed

The `StateDb::open` side-effect exception remains explicit and bounded to
inherited open-time behavior: parent directory creation, WAL enable, schema
ensure, and chain backfill. The commands may not add DDL or row mutation beyond
that inherited behavior (`proposals/06-pause-handshake.md:546`,
`proposals/06-pause-handshake.md:549`, `proposals/06-pause-handshake.md:596`,
`proposals/06-pause-handshake.md:677`).

### R1-F04 - Still Closed

The test matrix retains both required columns, `assumption_link` and
`residual_risk`, across the matrix (`proposals/06-pause-handshake.md:577`).

### R2-F01 - Still Closed

Rev 4 preserves the Round 2 closure and strengthens it. The per-session lockfile
is no longer the advisory-lock target; the advisory `flock` is held only on
`sentinel.lock`, which acquire/release never remove
(`proposals/06-pause-handshake.md:378`, `proposals/06-pause-handshake.md:460`,
`proposals/06-pause-handshake.md:559`). This avoids split-inode locking on a
path that stale cleanup can unlink.

## Regression Check

No regression found on R1-F01 through R1-F04 or R2-F01.

No regression found in the command surface, exit-code mapping, side-effect
contract, or supported-surface boundaries. The remaining risks are documented
residuals: v1 is advisory until sibling writer paths observe the lock, and the
file-backed primitive depends on local POSIX filesystem semantics with working
`flock` and same-directory atomic `rename`.
