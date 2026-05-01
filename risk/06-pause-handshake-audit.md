# 06-pause-handshake - Phase 4 Audit Risk Report (Round 2 / Rev 2)

**Verdict: HIGH**

Rev 2 closes the four Round 1 audit findings on their stated surfaces: the
idempotent release marker shape is selected, writer-path observation is now an
explicit accepted cross-PR residual, `StateDb::open` side effects are pinned as
accepted open-time behavior, and the test matrix carries assumption/residual
columns.

The audit gate still does not clear. Fresh review of the now-concrete lockfile
algorithm found a high-risk synchronization flaw: Rev 2 uses `flock` on the same
path that acquire/release may remove or recreate. On POSIX systems, `flock`
coordinates open file descriptions/inodes, not a pathname. Removing and
recreating the path can split the critical section and permit concurrent lease
granting under realistic stale-acquire interleavings.

## Round 1 Closure Check

### R1-F01 - Closed

Rev 1 left idempotent release marker storage as a Phase 5 design fork. Rev 2
selects a sibling marker file, includes it in success JSON, computes its path
beside the lockfile, defines its JSON body, and records that there is no future
marker-shape deferral (`proposals/06-pause-handshake.md:173`,
`proposals/06-pause-handshake.md:206`, `proposals/06-pause-handshake.md:325`,
`proposals/06-pause-handshake.md:528`).

This is sufficient for Phase 6 contract/test authors to know where same-token
release evidence lives and how missing-lock replay maps to `0` vs `16`.

### R1-F02 - Closed As Accepted Residual

Rev 1 deferred writer-path observation without an explicit acceptance decision.
Rev 2 now states that v1 ships the primitive only, sibling writers are deferred
to their own PRs, and the harness acceptance surface is narrowed until those
observers land (`proposals/06-pause-handshake.md:22`,
`proposals/06-pause-handshake.md:61`, `proposals/06-pause-handshake.md:382`,
`proposals/06-pause-handshake.md:518`, `proposals/06-pause-handshake.md:546`).

This conflicts with the initiative's original "observe once pause lands" wording
(`/home/nes/projects/agent-runner/worktrees/06-locate/initiatives/06-session-override-contract.md:114`),
but the proposal now makes the narrowing explicit enough for this audit surface.
The risk remains a named residual rather than an unresolved contract fork.

### R1-F03 - Closed With Explicit Side-Effect Residual

Rev 1 allowed an unbounded `StateDb::open` exception while claiming lock-state
only behavior. Rev 2 names the exact inherited open-time effects accepted for
v1: parent directory creation, WAL enable, schema ensure, and chain backfill
(`proposals/06-pause-handshake.md:416`). It also records read-only open as a
follow-up after schema-probe is mergeable (`proposals/06-pause-handshake.md:425`,
`proposals/06-pause-handshake.md:531`).

The §8 wording is still easy to misread because it says "No DDL, no row
mutation" immediately after accepting schema ensure and chain backfill
(`proposals/06-pause-handshake.md:418`). The §12 wording clarifies the intended
meaning as "no command-added DDL or row mutation beyond open-time effects"
(`proposals/06-pause-handshake.md:533`). Treat as closed, not reopened.

### R1-F04 - Closed

Rev 2 adds `assumption_link` and `residual_risk` columns to the test-intent
matrix and fills them for each test group (`proposals/06-pause-handshake.md:436`).
Rows now explicitly connect resolver, TTL, token, permissions, advisory scope,
and README verification to assumptions A1-A7 and named unverified residuals.

## Fresh Rev 2 Findings

### R2-F01 - HIGH - Removable flock target can split the lock critical section

Rev 2 makes the durable lease a file-backed lock at
`locks/session-<session_id>.lock` and says `flock` is held around
acquire/release/read critical sections (`proposals/06-pause-handshake.md:216`).
It then specifies that `pause-handshake` opens or creates that lockfile and
takes an exclusive `flock` (`proposals/06-pause-handshake.md:222`).

The same algorithm also permits stale metadata to be "remove/truncate[d] under
the same flock" before acquiring (`proposals/06-pause-handshake.md:225`) and
requires matching release to write the sibling marker and remove the lockfile
while still in the critical section (`proposals/06-pause-handshake.md:248`).
The side-effect contract repeats that acquire/release may create, replace, or
remove `locks/session-<session_id>.lock` (`proposals/06-pause-handshake.md:392`,
`proposals/06-pause-handshake.md:400`).

This is not a stable mutual-exclusion primitive. POSIX advisory locks protect
the opened file/inode. If process A holds a flock on the old lockfile and
unlinks it, process B can create the same pathname as a new file and take a
separate flock on the new inode. Both processes can believe they are inside the
exclusive section.

The highest-risk interleaving is stale acquire. A locks an expired file and
unlinks it as allowed by §4. B then creates and locks a new file at the same
path, sees no metadata, writes a fresh lease, and returns exit `0`. A can still
complete its own "acquire" path against the old fd or a replacement write path
and return a different token. That violates the core test-intent claim that two
concurrent pause calls grant one lease and one `13 session-busy`
(`proposals/06-pause-handshake.md:442`).

Release/acquire races are also underspecified. A matching release removes the
lockfile while holding a flock on the old inode. A fresh pause can create and
lock a new inode before the release critical section has completed marker fsync
and directory fsync work. The proposal relies on marker deletion by fresh
acquire for idempotency isolation (`proposals/06-pause-handshake.md:366`), but
does not define a stable guard that serializes marker and metadata updates.

Impact: Phase 6 could implement exactly the proposed file-backed design and pass
ordinary concurrent-process tests while retaining a split-brain race under
stale cleanup or release/acquire timing. The external harness depends on the
lease token as the exclusive write guard; a double `0` acquire collapses the
feature's safety case even under the narrowed advisory v1 surface.

Required Rev 3 closure: define a synchronization object that is never unlinked
or replaced while used for `flock`, or choose an atomic-create/rename protocol
that does not depend on flocking a removable pathname. Examples that would close
the contract gap: a separate per-session guard file such as
`session-<uuid>.guard` that is created once and never removed, with `.lock` and
`.released` metadata mutated only while holding the guard; or a lock-directory
protocol using atomic `mkdir` plus explicit stale-owner replacement rules. The
proposal must then update §4, §6, §8, and the atomic-acquire/release tests to
cover stale-acquire and release/acquire interleavings against that stable guard.

## Checklist Notes

- Present: proposal artifact, assumption register, net-value statement,
  command schemas, JSON receipts, exit namespace, lock/marker paths,
  side-effect section, README work, supported-surface track, residuals, and
  cross-feature compliance table.
- Closed from Round 1: R1-F01, R1-F02, R1-F03, R1-F04.
- No regression found on the Round 1 closure surfaces.
- Not audit-clear: Rev 2's lockfile/flock contract does not guarantee single
  lease ownership when the flock target can be removed or recreated.

## Required Rev 3 Closure

Rev 3 should keep the Round 1 closures intact and replace the removable-lockfile
critical section with a stable synchronization contract. Because R2-F01 is HIGH,
Phase 4 audit must rerun after the proposal revision.
