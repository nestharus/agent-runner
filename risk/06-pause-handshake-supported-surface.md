# 06-pause-handshake — Phase 4 Supported-Surface Risk Report (Rev 4)

**Termination signal:** `none`
**Verdict:** **LOW** — supported-surface track itself does not regress;
Rev 4's sentinel-flock + atomic-rename algorithm closes R3-F01 without
altering the harness consumer contract.

## Closure check on R3-F01 (audit lane, cross-track confirmation)

R3-F01 (Round 3 audit, HIGH): "Stale-acquire `unlink` is not
compare-and-replace." Rev 4 replaces the Rev 3 `unlink + retry-create`
sequence with a sentinel-flock + atomic-rename protocol
(`proposals/06-pause-handshake.md:17-29`,
`proposals/06-pause-handshake.md:240-289`,
`proposals/06-pause-handshake.md:366-406`,
`proposals/06-pause-handshake.md:511-534`).

### Multi-stale-contender interleaving check

The R3-F01 failing interleaving was: two contenders both read an
expired lease, A unlinks the path, A creates a replacement, B unlinks
A's replacement (because B's `unlink(lock_path)` was authorized off the
stale read), B creates its own replacement; both exit `0` with distinct
tokens.

Under Rev 4 (`proposals/06-pause-handshake.md:259-289`):

1. The sentinel `<lock_dir>/sentinel.lock` is opened
   `O_CREAT | O_RDWR` (no `O_EXCL`) and its open fd is taken under
   `flock(LOCK_EX)`. The sentinel inode is **never unlinked** by acquire
   or release (`proposals/06-pause-handshake.md:380-381`,
   `proposals/06-pause-handshake.md:460-461`,
   `proposals/06-pause-handshake.md:519-520`,
   `proposals/06-pause-handshake.md:559-560`).
2. All read/write/rename/unlink decisions on
   `session-<uuid>.{lock,released}` happen while the sentinel flock is
   held (`proposals/06-pause-handshake.md:260-289`,
   `proposals/06-pause-handshake.md:296-319`).
3. Inside the critical section, lease installation is `tmpfile + fsync + rename`
   onto the same directory
   (`proposals/06-pause-handshake.md:271-282`).

Replay of the R3-F01 interleaving:

1. Lockfile `P` exists with expired lease `E1`.
2. A takes sentinel flock. B blocks on the same flock.
3. A reads `P`, sees `E1` expired. A writes
   `P.acquire-<pid_A>-<rand>.tmp`, fsyncs, atomically renames onto `P`.
   `P` now atomically refers to A's lease `E_A`.
4. A removes any prior `.released` marker, fsyncs the directory,
   releases the sentinel flock, exits `0`.
5. B acquires the sentinel flock. B opens `P` read-only — sees `E_A`,
   not expired (B's start clock is at most A's release time). B
   releases the sentinel flock and returns `13 session-busy`
   (`proposals/06-pause-handshake.md:266-268`).

Result: one `0`, one `13`. The §9.1 "Atomic acquire" oracle holds
(`proposals/06-pause-handshake.md:583`).

The R3-F01 closure depends on two POSIX guarantees, both pinned in A8
(`proposals/06-pause-handshake.md:98`): `flock(2)` works on the sentinel
fd, and same-mount `rename(2)` is atomic. Both are stated invariants;
their violation is the explicit A8 invalidator and is documented as the
NFSv2/3 / cross-mount edge.

### Stale-acquire / release interleaving check

R3-F01's secondary failure was a resume reading an expired matching
lockfile, then unlinking after a stale-acquire had already replaced it.
Under Rev 4, both `pause-handshake` and `resume-handshake` open and hold
`<lock_dir>/sentinel.lock` under `LOCK_EX` for the full inspect/mutate
window (`proposals/06-pause-handshake.md:262`,
`proposals/06-pause-handshake.md:299`). Pause cannot replace `P`
between resume's read and resume's unlink because pause cannot enter
the critical section while resume holds the flock, and vice versa.

A residual edge remains: a resume holding token `T_old` for an
already-expired lease that races a fresh `pause-handshake` and loses
the flock race will see the fresh lease at `P` (different `token_hash`)
and return `16 lock-token-invalid` rather than `0` with `note: released
expired token`. This is graceful — the old lease is gone and a marker
would be a lie — but it is a small UX shift versus Rev 3's stated "if
the lease had already expired … this is still exit `0`"
(`proposals/06-pause-handshake.md:308-311`). The proposal text presumes
the lease at `P` still hashes to the supplied token, which only holds
when no stale-acquire intervened. This is not a safety regression and
not a contract-surface change (exit code `16` is already part of §5.2),
but supported-surface wording in §10 / §12 may want to acknowledge the
"expired-token-after-stale-replacement" outcome explicitly. Non-blocking
advisory; logged below.

### R3-F02 (medium, partial-metadata loser) implicit closure

R3-F02 (Round 3 audit, MEDIUM): "Loser can observe partial metadata
during initial acquire" because Rev 3 created the lockfile then wrote
metadata. Under Rev 4, lease metadata is written to a uniquely named
sibling tmpfile and atomically renamed onto `P`
(`proposals/06-pause-handshake.md:273-279`). Losers either observe the
prior (expired or absent) state or the fully-written new lease; partial
metadata is unreachable at `P`. The "intermittent operational error
caused by a normal in-progress writer" failure mode disappears. Implicit
closure of R3-F02 follows from the same mechanism that closes R3-F01.

**R3-F01 closure status from supported-surface vantage: closed.**
Authoritative call belongs to the audit reviewer; this report records
cross-track confirmation.

## R1-F01..R1-F04 and R2-F01 closure status under Rev 4

| Finding | Rev N closure surface | Rev 4 impact | Status |
| --- | --- | --- | --- |
| **R1-F01** idempotent release marker shape | §3.2 `release_marker_path`, §6 marker JSON, §8 marker side-effects, §12 "no future marker-shape deferral" | §6/§8 retain the sibling-marker shape verbatim; §4 step 10.5 still removes the prior marker on stale-replacing acquire; §4 step 16 still writes the marker via tmpfile + fsync + rename. No regression. | **CLOSED (still)** |
| **R1-F02** writer-path observer narrowing | §1 advisory-lock framing, §12 narrowed acceptance surface, §13 "Partial by design", §10 README mandate | Unchanged in Rev 4 (Rev 4 changelog only touches §4/§6/§8/§9 and §1.1 A8). §1 advisory framing and §13 "Partial by design" preserved verbatim. | **CLOSED (still)** |
| **R1-F03** `StateDb::open` mutation exception pinned | §8 explicit accepted open-time effects matching 06-locate / 06-export | §8 retains the explicit clause (`proposals/06-pause-handshake.md:549-558`). Rev 4 only adds sentinel/tmp/rename bullets — all on the lock-state domain, no DB surface added. | **CLOSED (still)** |
| **R1-F04** §9.1 `assumption_link` + `residual_risk` columns | §9.1 matrix carries both columns, A1–A7 references | Rev 4 keeps both columns and threads A8 through eight rows ("Atomic acquire", "Per-session scope", "Stale acquire", "Busy lock", "Correct release", "Expired matching release", "Permissions", "Side effects"). Existing A1–A7 links preserved. | **CLOSED (still)** |
| **R2-F01** removable flock target | Rev 3 retired `flock` on a removable inode; Rev 4 reintroduces `flock` only on the **never-removed** sentinel inode | The Rev 2 failure mode (flock held on a path the algorithm could unlink and recreate) is structurally absent: §8 explicitly states the sentinel "is never deleted by acquire or release" (`proposals/06-pause-handshake.md:560`); only `session-<uuid>.{lock,released,*.tmp}` are mutated, and they are not flock targets. | **CLOSED** |

All four R1 closures and the R2 closure stand under Rev 4.

## Fresh assessment of Rev 4 changes (supported-surface lane)

### Wire-format / public API (§3, §6)

Receipt fields, exit codes, `token` format, `lock_path`, marker path,
`expires_at`, `released`, `already_released`, TTL bounds, and
`SessionLock` public method signatures (`acquire`, `release`, `observe`,
`from_default_data_dir`, `lock_path`, `release_marker_path`) are
byte-identical to Rev 2/3 (`proposals/06-pause-handshake.md:155-220`,
`proposals/06-pause-handshake.md:359-364`,
`proposals/06-pause-handshake.md:386-390`).

The only public-surface addition is the `Sentinel` private helper inside
`session_lock/` (`proposals/06-pause-handshake.md:367-381`). It is
described as a **private** type used internally by `acquire`, `release`,
and (when needed) stale-eviction. It is not exposed to harness or CLI
consumers. Harness consumer contract: unchanged.

### Side-effect contract (§8)

Rev 4 §8 adds three substantive bullets relative to Rev 3:

1. "Create `locks/sentinel.lock` idempotently with `O_CREAT`; …
   acquire, release, and stale-eviction operations hold an exclusive
   `flock` on this never-deleted sentinel file's open file descriptor"
   (`proposals/06-pause-handshake.md:515-517`).
2. "Write a unique temp file such as
   `locks/session-<session_id>.lock.acquire-<pid>-<random>.tmp` … fsync
   it, and atomically rename it onto `locks/session-<session_id>.lock`"
   (`proposals/06-pause-handshake.md:519-522`).
3. Release path uses the same tmpfile + fsync + rename pattern for
   `.released` (`proposals/06-pause-handshake.md:535-538`).

All three are on-domain (lock state only), inside the existing
`~/.local/share/oulipoly-agent-runner/locks/` blast radius, owner-
private (`0700` dir, `0600` files —
`proposals/06-pause-handshake.md:570-572`), and inert to the DB. No new
DB surface, no new shared state, no new permission downgrade. Tmp files
have unique pid+random suffixes so a crashed mid-acquire/mid-release
cannot collide with the live lock or marker path
(`proposals/06-pause-handshake.md:560-565`); they will accumulate only
on crash and are not lock targets. This is acceptable supported-surface
behavior; cleanup-on-crash of orphaned `.tmp` files remains lazy and
manual, consistent with §11 rollback wording.

### Migration / rollback story

Lockfile path shape (`session-<uuid>.lock`) and marker path shape
(`session-<uuid>.released`) are unchanged. The new artifact is
`<lock_dir>/sentinel.lock`, a never-deleted shared file inside the
existing per-user lock dir.

§11 rollback wording handles this: "Operators may delete stale session
lock, marker, or orphaned temp files after confirming no newer binary
is observing them. The sentinel file is harmless to leave in place."
(`proposals/06-pause-handshake.md:646-649`). Older binaries are
unaware of `sentinel.lock`; it does not collide with any existing
runner artifact name. Rollback works cleanly.

§10 README mandate already lists "the never-deleted `sentinel.lock` and
per-session `.lock` / `.released` files"
(`proposals/06-pause-handshake.md:619-622`), satisfying the
supported-surface naming contract for operator-visible state.

### Observability

Unchanged. Receipts (§3.1, §3.2), stderr semantic JSON (§3.3), and lock
state files are the entire v1 surface. No trace event, audit row, or
telemetry surface added by Rev 4. The sentinel and tmpfiles are
implementation detail, not observability surface.

### Test matrix (§9.1)

Rev 4 threads A8 through the rows that exercise concurrency or atomic
file mutation:

- "Atomic acquire" — A4, A5, A7, **A8**
  (`proposals/06-pause-handshake.md:583`).
- "Per-session scope" — explicitly notes the "shared sentinel"
  (`proposals/06-pause-handshake.md:584`).
- "Stale acquire" — "Expired lockfile is lazily replaced under the
  sentinel flock by atomic rename"
  (`proposals/06-pause-handshake.md:587`).
- "Busy lock" — A4, A5, A7, A8 (`proposals/06-pause-handshake.md:588`).
- "Correct release" — "writes sibling release marker under the sentinel
  flock" (`proposals/06-pause-handshake.md:589`).
- "Expired matching release" — A5, A7, A8
  (`proposals/06-pause-handshake.md:591`).
- "Permissions" — sentinel/tmp added to file-set
  (`proposals/06-pause-handshake.md:595`).
- "Side effects" — A1, A3, A7, A8
  (`proposals/06-pause-handshake.md:596`).

The Rev 3 supported-surface advisory item #4 ("If R2-F01 stays open,
add an explicit stale-acquire multi-contender row") is partially
satisfied implicitly: the "Atomic acquire" row's residual already
covers concurrent stale-acquire because all contenders serialize
through the never-removed sentinel before any lockfile mutation. An
explicit row is no longer required, though Phase 6 may still find it
useful to write a dedicated "two stale contenders against an expired
lock" test row for documentation crispness. Non-blocking.

## No-regression check vs prior supported-surface findings

| Prior advisory | Rev 4 status |
| --- | --- |
| R1-F01-supported (orphaned-lockfile UX during sibling writes) | **unchanged** — root-cause fix still belongs to sibling PRs (D4b); §10 advisory-scope mandate still in force. |
| R1-F02-supported (Phase 5 marker shape) | **closed (still)** — Rev 4 preserves §6/§12 marker shape commitment. |
| R1-F03-supported (A2 multi-active-segment edge) | **unchanged** — same contract, same mitigation (sibling adoption). |
| R1-F04-supported (Windows residual in README) | **unchanged but reinforced** — Rev 4 explicitly assumes POSIX `flock(2)` and same-mount `rename(2)` atomicity (A8). §12 still says "Windows semantics are not designed". The README §10 should now name Linux/macOS as v1-supported and Windows as undefined; A8's invalidator wording (NFSv2/3, cross-mount rename) belongs in a §11 deployment caveat. Non-blocking. |
| R1-F05-supported (CLI `observe` ergonomics) | **unchanged** — `observe` still library-only in §6. |
| R1-F06-supported (README v1-vs-eventual sentence) | **closed (still)** — §10 mandate preserved. |
| Rev 3 advisory #4 (explicit stale-acquire multi-contender §9.1 row) | **implicitly satisfied** — sentinel-flock makes the multi-contender path serialize on a single never-removed inode; "Atomic acquire" oracle covers it. Explicit row optional, not required. |

No Rev 4 change degrades any prior supported-surface verdict.
Migration/rollback/observability surfaces remain LOW.

## Verdict rationale

- **Termination signal #1 (`invalidated-assumption`)** — does not fire.
  A1–A8 hold against problem-map evidence. A8 is the new assumption
  introduced by Rev 4; its evidence (sentinel inode never unlinked, all
  contenders serialize, same-directory atomic rename) and invalidator
  (NFSv2/3 quirks, cross-mount rename) are explicit and supported by
  the algorithm text in §4 / §6 / §8.
- **Termination signal #2 (`non-positive-value`)** — does not fire. The
  Rev 1 retired-risk table (locate-collapse, second-resolver,
  schema-migration, dual-store, no-recovery, ambiguity-fallback) is
  preserved across Rev 2/3/4. Rev 4 strengthens the lease's safety case
  versus Rev 3 (multi-contender stale-acquire is now race-free), so
  net-value strictly improves.

**Standard verdict: LOW** for the supported-surface track itself. The
harness consumer contract surface (receipts, exit codes, `lock_path`,
token format, TTL policy, marker path) is byte-identical to Rev 2/3
and remains LOW. Rev 4's mechanism change is a private synchronization
upgrade with one new operator-visible artifact (`sentinel.lock`)
already documented in §10 / §11.

The audit reviewer's Rev 4 reassessment authoritatively decides whether
R3-F01 is now closed; this report's cross-track reading is that the
sentinel-flock + atomic-rename protocol is sufficient for the documented
A8 threat model and that the multi-contender interleaving from R3-F01
is no longer reachable.

## Advisory items carried forward (non-blocking)

1. **README §10 / §11 should name Linux/macOS as v1-supported and
   Windows as undefined**, and should mention the A8 invalidator
   (NFSv2/3 quirks, cross-mount rename) as deployment caveats — Rev 4
   reinforces this from the algorithm side, not just the residual side.
2. **Sibling adoption PR** (resume / repl / migrate / balanced one-shot)
   should consider exposing `SessionLock::observe` as
   `agents session observe <id>` for first-class read inspection
   (R1-F05-supported).
3. **Sibling-PR observers should refuse-and-emit** structured stderr
   JSON `session-busy` rather than waiting silently, preserving the §1.2
   "stable refusal surface" framing.
4. **Expired-token-after-stale-replacement UX**: §10 / §12 may want to
   note that a `resume-handshake` whose token matched an expired lease
   can return `16 lock-token-invalid` (rather than `0` with the
   "released expired token" note) if a fresh `pause-handshake` won the
   sentinel-flock race ahead of resume. The exit code is already in
   §5.2 and the harness contract is unchanged, but the §4 step 16
   wording presumes the lease at `P` still hashes to the supplied
   token. Non-blocking; documentation polish only.
5. **Phase 6 may add an explicit "two stale contenders against an
   expired lock" §9.1 row** for test-author crispness, even though the
   "Atomic acquire" row's oracle already covers the case via sentinel
   serialization.
