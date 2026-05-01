# 06-pause-handshake — Phase 4 Scope Risk Assessment (Rev 4)

**Assessor:** scope reviewer
**Verdict:** **LOW** — Rev 4 closes the Round 3 audit finding (R3-F01:
stale-acquire pathname-`unlink` TOCTOU) by replacing Rev 3's
`unlink + retry-create_new` stale-eviction with a sentinel-flock plus
same-directory atomic-`rename` protocol. The sentinel file's inode is
never unlinked, so all contenders serialize on a stable advisory-lock
target, and the session lockfile is installed by atomic rename inside
that critical section rather than by an unlink-and-recreate sequence
that the prior round could split. Same contract envelope as Rev 3
(commands, JSON receipts, exit codes, lock/marker file paths,
side-effect contract, anti-scope) plus one additive implementation
artifact (`sentinel.lock`) and one additive assumption row (A8). R1-F01
through R1-F04 closures from Rev 2 and the R2-F01 closure intent from
Rev 3 stand. No regression.

## Round 3 closure check (audit only)

| ID | Round 3 ask | Rev 4 close | Closed |
| --- | --- | --- | --- |
| R3-F01 | Replace stale-cleanup `unlink + retry-create_new` with a stable synchronization or true compare-and-replace contract that cannot delete a newer lease after reading an older expired one. Acceptable shapes: never-removed guard file with all lock/marker mutation serialized under it; lock-directory generation/CAS protocol; or implementation that proves unlink applies to the same object that was read. | Rev 4 picks branch one — never-removed guard. §1.1 A8 adds the assumption "atomic rename plus advisory flock on a non-removable sentinel is sufficient for cross-process mutual exclusion on POSIX filesystems supporting `flock(2)` and `rename(2)` atomicity." §4 step 6 names the sentinel as the real mutex. §4 step 7 opens `sentinel.lock` with `O_CREAT \| O_RDWR` and takes `flock(LOCK_EX)`; the sentinel is never unlinked (§6, §8, §12). §4 step 10 specifies acquire-by-atomic-replace-or-create: write lease JSON to a unique sibling temp file `<session_lock_path>.acquire-<pid>-<random>.tmp` with `O_CREAT \| O_TRUNC \| O_WRONLY`, fsync, atomic rename onto the session lockfile while still holding the sentinel flock — no unlink of the session lockfile in the acquire path. §4 step 14 puts release inside the same sentinel flock; step 16 writes the marker via temp + atomic rename, then unlinks the lockfile, all under the sentinel flock. §6 mirrors the same algorithm in the `SessionLock` API description and adds a private `Sentinel { with_locked }` helper. §8 enumerates the sentinel-flock-bracketed mutations and the atomic-rename installation pattern. §9.1 stale-acquire row reads "Expired lockfile is lazily replaced under the sentinel flock by atomic rename." §12 D1a residual updated to "depends on POSIX filesystems with working `flock(2)` on a never-unlinked sentinel, same-directory atomic `rename(2)`, and private filesystem permissions." | yes |

R3-F01 closure verification (scope reviewer cross-check):

- **Mutual exclusion source.** The sentinel inode is created once with
  `O_CREAT | O_RDWR` (no `O_EXCL` per §4 step 7) and is never unlinked
  by acquire or release (§6, §8, §12). All contenders therefore
  serialize on a stable file descriptor, not on a pathname whose inode
  binding could change. The R2-F01 class (flock target unlinkable) and
  the R3-F01 class (stale `unlink` deletes a newer lease created
  between an old read and a later unlink) both depend on the
  synchronization target being deletable; both classes are removed at
  the root.
- **Stale-acquire interleavings.** Two contenders A and B both
  attempt to pause an expired session under sentinel flock. One — say
  A — wins the `flock(LOCK_EX)` first. A's full read-decision-rename
  (§4 step 10) runs to completion: A reads expired metadata, writes
  fresh lease JSON to a unique temp file, fsyncs, atomic-renames it
  onto `session-<uuid>.lock`, removes the prior marker, fsyncs the
  directory, releases the sentinel flock, and returns `0`. B then
  acquires the sentinel flock and reads the lockfile installed by A.
  B sees `expires_at > now` and returns `13 session-busy`. The R3-F01
  failing interleaving (B's stale `unlink` deletes A's replacement)
  cannot occur: B never unlinks the session lockfile during acquire
  in Rev 4 — it overwrites by atomic rename or aborts on a still-valid
  lease. The "two `0`s for the same session" outcome is unreachable.
- **Stale-acquire vs release interleaving.** Audit's secondary R3-F01
  scenario was: resume reads an expired matching lockfile, then a
  stale acquire replaces it, then resume's later `unlink` removes the
  new lease. Rev 4 §4 steps 14–16 hold the sentinel flock for the
  entire resume read-write-unlink cycle. A concurrent stale acquire
  cannot interleave: it must wait for resume to release the sentinel
  flock before it sees an updated lock state. Either (a) the resume
  cycle completes first and the acquire then reads a missing/expired
  lock, or (b) the acquire cycle completes first and the resume then
  reads a fresh non-matching lease and returns `16 lock-token-invalid`.
  No interleaving leaves a freshly acquired lease unlinked by an
  old-token resume.
- **Acquire/marker interaction.** §4 step 10.5 still removes the prior
  sibling release marker after writing fresh lock metadata, preserving
  R1-F01's "fresh acquire reaps the prior marker" guarantee. The
  removal now happens under sentinel flock and after the new
  lockfile's atomic rename, so the marker can never be removed by a
  contender that has not committed a new lease.
- **Empty-file transient.** Rev 3 created the lockfile with
  `create_new` and only later wrote JSON, opening the R3-F02 partial-
  metadata window (advisory). Rev 4 writes the full JSON to a temp
  file, fsyncs it, then atomically renames it onto the session
  lockfile. A reader that opens `session-<uuid>.lock` either sees the
  prior fully-written lease (or its absence) or sees the new fully-
  written lease — never an empty/half-written file. Combined with
  sentinel-flock serialization, the loser-reads-partial-metadata
  failure mode in R3-F02 is also closed.

Audit-only conclusion: R3-F01 is closed on the proposal text, and the
related R3-F02 advisory is closed as a side effect of the
write-to-temp-then-rename pattern.

## Round 2 closure check (carry-forward)

| ID | Rev 3 close | Rev 4 carry-forward | Held |
| --- | --- | --- | --- |
| R2-F01 (removable-`flock` target splits the critical section) | Rev 3 swapped POSIX advisory locks on the lockfile path for `O_CREAT \| O_EXCL` create-if-absent on the same path. The "split-brain double-`0`-acquire" via flock-on-removable-inode was eliminated. | Rev 4 keeps the no-`flock`-on-removable-pathname property: the only file ever flocked is the sentinel, and the sentinel is never removed (§4 step 7, §6 sentinel helper, §8, §12). The session lockfile is installed by atomic rename, never created with `O_EXCL` and never explicitly unlinked during acquire, so the R2-F01 inode/path mismatch class also cannot return through the new mechanism. The intent of R2-F01 (do not synchronize on a removable target) is preserved and tightened. | yes |

## Round 1 closure check (carry-forward, no regression)

| ID | Rev 2 close | Rev 4 carry-forward | Held |
| --- | --- | --- | --- |
| R1-F01 (release marker shape) | §6 sibling marker `session-<uuid>.released` with versioned JSON; §3.2 receipt field `release_marker_path`; §4 acquire reaps prior marker; §8 marker mutations enumerated; §12 deletes "marker-shape deferral" residual. | Rev 4 marker text identical. §4 step 10.5 (acquire path on fresh/stale success) still says "Remove any previous sibling release marker for the same session." §4 step 16 (release path) writes the marker via temp + atomic rename under sentinel flock — same marker schema, same path, just installed atomically rather than by direct write. §3.2 / §8 unchanged on marker contract. | yes |
| R1-F02 (advisory-v1 framing) | §1, §12 D4b, §13 row 3 "Partial by design"; sibling PRs named (`import-replace`, `migrate_chain_segment`, `run_repl`, `run_resume`, balanced one-shot); README §10 mandate. | Rev 4 advisory-framing language unchanged. The mechanism rewrite does not alter the harness acceptance surface — the lock primitive is still a per-session lease whose end-to-end mutual exclusion depends on sibling observers in their own PRs. | yes |
| R1-F03 (`StateDb::open` clause) | §8 explicit `StateDb::open_default()` clause matching 06-locate / 06-export side-effect contracts (parent dir, WAL enable, schema-ensure, chain backfill); §12 read-only follow-up tied to 06-schema-probe. | Rev 4 §8 prose around `StateDb::open_default()` is unchanged; the lock-mechanism rewrite touches only the lock-state mutation enumerations (sentinel + atomic-rename shape), not the DB-open clause. | yes |
| R1-F04 (test matrix `assumption_link` + `residual_risk` columns) | §9.1 columns populated for every row; A1–A7 references; residual_risk notes per row. | Rev 4 §9.1 matrix structure unchanged. Several rows now also reference A8 (atomic acquire, per-session scope, stale acquire, busy lock, correct release, expired matching release, permissions, side effects), and a few `Intended behavior` cells are tightened to name the sentinel-flock + atomic-rename pattern. The columns themselves and the residual_risk wording remain in place. | yes |

All four R1 closures stand, and the R2-F01 closure intent stands, under
Rev 4.

## Fresh assessment of Rev 4 deltas (scope dimension only)

| Rev 4 change | Direction | Magnitude | Scope verdict |
| --- | --- | --- | --- |
| §1.1 new assumption A8: atomic rename + advisory flock on a non-removable sentinel is sufficient cross-process mutual exclusion on POSIX filesystems with working `flock(2)` and `rename(2)`. | additive (assumption registry) | tiny | In-scope. Names the new dependency (POSIX `flock` semantics on a stable inode plus same-mount `rename(2)`) without adding any new build artifact beyond the sentinel file itself. Invalidator captures the platform caveats (NFSv2/3 quirks, cross-mount renames). |
| §4 step 4: lock paths now include `<lock_dir>/sentinel.lock`. | additive (path enumeration) | tiny | In-scope. Same `<lock_dir>` as Rev 3; one additional well-known sibling filename. Per-user data dir, owner-private permissions per §8. |
| §4 step 6: D1a clarification names the sentinel as the real mutex. | mechanism-pin | none (clarification) | In-scope. Restates which artifact provides exclusion; lockfile metadata remains the durable lease. |
| §4 step 7: open `sentinel.lock` with `O_CREAT \| O_RDWR` (no `O_EXCL`) and take `flock(LOCK_EX)` for the full session-lock decision. | mechanism replacement | small | In-scope. Same critical-section boundary as Rev 3 but anchored on a stable inode. No new exit code, receipt field, or DB access. |
| §4 step 8–9: under sentinel flock, open the session lock read-only, classify, and return `13 session-busy` if `expires_at > now`. Malformed/unreadable lease → `1 operational-error`. | mechanism replacement | small | In-scope. Same exit-code mapping as Rev 3 §5.1. The R3-F02 "loser reads partial metadata" path becomes unreachable because writes are now atomic rename, not in-place create-then-write, so the empty-file transient does not exist. |
| §4 step 10: acquire-by-atomic-replace-or-create — write lease JSON to a unique `<session_lock_path>.acquire-<pid>-<random>.tmp` with `O_CREAT \| O_TRUNC \| O_WRONLY`, fsync, atomic rename onto the session lockfile under sentinel flock; no unlink of the session lockfile during acquire. | mechanism replacement | small | In-scope. Replaces Rev 3's `unlink + retry-create_new` with a single atomic-rename install. The "newer lease deleted by older read" failure mode of R3-F01 cannot occur because no acquire path unlinks the session lockfile. Same lockfile schema, same lease contents. |
| §4 step 14–16: release runs under sentinel flock; marker is written via temp + atomic rename; lockfile is unlinked under the same flock. | mechanism replacement | small | In-scope. Same exit codes (`0`, `16`, `17`) and same receipt fields. The release/stale-acquire interleaving in R3-F01 is closed because resume holds the sentinel flock for the full read-write-unlink cycle. |
| §6 `Sentinel { with_locked }` private helper added; `acquire()` / `release()` prose rewritten to "runs under `Sentinel::with_locked`" with atomic-rename install and same-direction marker via temp + atomic rename. | API-doc realignment | small | In-scope. The `SessionLock` public types (`Lease`, `ReleaseReceipt`, `LockError`) and method signatures (`acquire`, `release`, `observe`, `lock_path`, `release_marker_path`) are unchanged. The sentinel helper is private to the module. |
| §6 algorithm note: "stale path never unlinks the lock before replacement; the atomic rename happens while all contenders are serialized by the sentinel flock." | invariant restatement | tiny | In-scope. Names the property the audit asked for. |
| §8 side-effect contract: enumerate `sentinel.lock` create-idempotent + flock; enumerate the temp-file + fsync + atomic rename install pattern for both lock and marker; enumerate "Read, write, rename, and unlink session lock files only while holding the sentinel flock." | side-effect contract realignment | small | In-scope. Disk side-effect set is the same as Rev 3 plus the never-removed sentinel and the unique temp files. Sentinel is `0600`; temp files are `0600`; lock dir is `0700`. Permission contract unchanged. |
| §10 README mandate: name the never-deleted `sentinel.lock` next to per-session `.lock` / `.released`. | doc additive | tiny | In-scope. The persistent-state paragraph already mentions the lock dir; Rev 4 just adds one filename. |
| §11 supported-surface: "The sentinel file is harmless to leave in place." | residual additive | tiny | In-scope. Operator rollback path unchanged for per-session files; sentinel is acknowledged as inert. |
| §12 D1a residual rewritten to depend on `flock(2)` on a never-unlinked sentinel + same-directory atomic `rename(2)`. | residual realignment | tiny | In-scope. Replaces the prior `O_CREAT \| O_EXCL` dependency phrase. |
| §9.1 atomic acquire / per-session scope / stale acquire / busy lock / correct release / expired matching release / permissions / side effects rows: assumption_link cells gain A8; some `Intended behavior` cells named the sentinel-flock + atomic-rename pattern. | test-matrix additive | tiny | In-scope. R1-F04 columns retained; new mechanism is reflected in the assumption links and behavior descriptions without changing the test taxonomy. |

**Net direction.** Rev 4 replaces a TOCTOU-prone unlink-and-retry stale
eviction with a never-removed sentinel flock plus same-directory atomic
rename, and writes the lease and the release marker via
write-to-temp-then-rename rather than create-then-fill. The contract
envelope (commands, JSON shapes, exit codes, receipt fields, anti-scope,
DB access) is unchanged; the only additive artifact is `sentinel.lock`,
which is owner-private, per-data-dir, and never deleted. The build
envelope grows by one well-known filename and one new assumption row;
the correctness floor rises by removing both the R3-F01 stale-unlink
TOCTOU and the R3-F02 empty-file transient. Pure scope-positive
mechanism refinement.

The Rev 1 expansions (chain_id receipt, TTL bounds, lock module,
`observe()` API, hex-32 token), Rev 2 expansions (sibling marker,
explicit advisory-v1 framing, pinned `StateDb::open` clause, test
matrix columns), and Rev 3 expansions (atomic-create algorithm,
malformed-lease defensive branch) are unchanged in Rev 4 and still
inside scope.

## Anti-scope §7 audit vs Rev 4 §2–§6

Re-checked each §7 clause against the Rev 4 sentinel + atomic-rename
rewrite:

| §7 clause | Rev 4 leakage check | Result |
| --- | --- | --- |
| No transcript content mutation or import-replace implementation | `flock`, `O_CREAT \| O_TRUNC \| O_WRONLY`, and `rename(2)` operate on `<lock_dir>/sentinel.lock`, `<lock_dir>/session-<uuid>.lock`, `<lock_dir>/session-<uuid>.released`, and unique sibling temp files only; not transcripts, not `session_turns` rows, not import-replace | honored |
| No provider spawn / signal / suspend / resume / kill | The atomic-rename + flock pattern is filesystem-level; no executor, no signal, no provider invocation | honored |
| No proof of safety for provider CLIs launched outside agent-runner | Advisory-v1 framing in §1 / §12 / §13 is unchanged by Rev 4 | honored |
| No global runner lock | Per-session lockfile path is unchanged; the sentinel is shared across sessions in one data dir but does not extend the lease scope — it only serializes the brief read-decision-rename critical section | honored |
| No DB lock table in v1 | Lock state remains filesystem-only; sentinel + atomic rename use kernel filesystem semantics, not SQLite | honored |
| No strict ambiguity query outside the shared resolver | §4 steps 1–3 unchanged | honored |
| No fallback to raw `session_turns` | §4 unchanged | honored |
| No GUI / frontend lock indicator | No frontend file added; sentinel is not surfaced to GUI | honored |
| No quota/auth refresh, provider selection, config edit, `migrate-config` coupling | §4/§8 do not invoke quota, balancer, or any provider-config writer | honored |

D4b's wording in §7 is unchanged. The Rev 4 mechanism switch does not
weaken any anti-scope clause: the v1 lock primitive still does not
silently spawn, drain, write transcripts, or touch DB rows beyond the
accepted `StateDb::open` open-time effects already pinned in §8.

## Decomposition assessment (unchanged from Rev 2)

Pause + resume must ship as one PR (Rev 1 analysis stands). D4b's
sibling-observer deferral remains the only meaningful split, and Rev 4
makes no new merge surface that would invite further decomposition.
The sentinel + atomic-rename rewrite is local to the
`acquire()` / `release()` critical sections inside the new
`src-tauri/src/session_lock/` module and the private `Sentinel` helper;
splitting the sentinel out as a separate crate would duplicate the
lockfile schema and marker schema, both of which are minimal.

## Coverage matrix delta vs Rev 3

| Source ask / constraint | Rev 4 status |
| --- | --- |
| Harness same-token idempotent replay | Unchanged from Rev 3 — concrete sibling marker still backs replay; marker is now installed by temp + atomic rename, which strengthens crash-during-release durability slightly. |
| Initiative §112 reuse `StateDb::resolve_resume` | Unchanged; §4 step 2–3. |
| Initiative §115 deferred observers | Unchanged from Rev 2 advisory-v1 framing. |
| Initiative §118 read-only `StateDb` open belongs to schema-probe | Unchanged; §8 + §12 read-only follow-up. |
| Problem-map §3.1 risk: no token identity | Unchanged; §6 token + sha256 hash. |
| Problem-map §6.2 gap: no JSON pause/release receipts | Unchanged; §3.2 release receipt with marker path. |
| Problem-map §3.x mutual-exclusion correctness (implicit) | **Tightened by Rev 4** — sentinel-flock + atomic rename closes both the R3-F01 stale-unlink TOCTOU and the R3-F02 empty-file transient that the Rev 3 `unlink + create_new` pattern allowed. |

No previously covered row regresses; one row tightens.

## Cross-feature consistency (no regression)

- Shared error namespace (`10`/`11`/`13`/`16`/`17`; `12`/`14`/`15`
  reserved): aligned with initiative §107–§110. Rev 4 unchanged.
- Ownership through `resolve_resume`: aligned with initiative §112.
  Rev 4 unchanged.
- Lock observation by sibling features: §13 row 3 still reads
  "Partial by design" with each deferred observer PR named; aligned
  with initiative §115 ("once 06-pause-handshake lands").
- Read-only `StateDb` open belongs to schema-probe: §8 still pins this
  inheritance; aligned with initiative §118.
- No auto-resume / spawn / quota / config-edit / migrate-config:
  aligned with initiative §121–§122; honored throughout §7/§8.

No cross-feature row regresses under Rev 4.

## Findings (severity ≥ MEDIUM)

None.

## Findings (LOW)

- **F1 (carried) — Harness AC bullet #6 satisfied across multiple PRs.**
  Unchanged from Rev 2/Rev 3. v1 ships the primitive only; sibling
  observers land in their own PRs. Phase 5 hookpoint research will pin
  exact hookpoints. No Phase 4 remediation.
- **F2 (carried) — `observe()` API exposed but not consumed in this
  PR.** Unchanged from Rev 2/Rev 3. §6 defines the read shape; consumer
  set is the four sibling PRs named in §12. Cosmetic nit unchanged: §6
  itself does not name the consumers (§1 and §12 do).
- **F3 (carried) — TTL bounds (1s / 30m / 5m default) without harness
  mandate.** Unchanged from Rev 2/Rev 3.
- **F4 (carried) — Token format diverges from harness's ULID-shaped
  example.** Unchanged from Rev 2/Rev 3. `pause_<32 hex>` keeps prefix;
  ULID/UUID rejection defensible on entropy grounds.
- **F5 (closed by Rev 2) — Idempotent-release marker shape was
  deferred to Phase 5.** Closed; carried-closed under Rev 3 / Rev 4.
- **F6 (closed by Rev 4) — Lease-content visibility window between
  `create_new` and JSON fsync.** Rev 3 advisory. Closed by Rev 4's
  write-to-temp-then-atomic-rename install: a reader on
  `session-<uuid>.lock` either sees the prior fully-written lease (or
  its absence) or the new fully-written lease, never an empty/partial
  file.
- **F7 (new, advisory) — A8 invalidators name NFSv2/3 quirks and
  cross-mount renames as out-of-scope platforms.** §11 already pins
  deployment to local CLI under the per-user data dir, which is
  expected to be a same-mount POSIX filesystem with working `flock(2)`.
  No remediation needed; this is a discoverability nit that could be
  cross-linked in §10 README work, but it is not a scope concern.

## Recommended revisions

None that change scope. R3-F01 is closed by the sentinel-flock +
atomic-rename pattern; R2-F01 closure intent and R1-F01..R1-F04
closures hold; the proposal as written is correctly scoped. Optional
cosmetic nits (carried from Rev 1 / Rev 2 / Rev 3): §6 could name the
deferred consumer set inline next to the API definition, mirroring §1
/ §12. §10 README could add a sentence on the A8 platform caveats
(NFSv2/3 / cross-mount). Neither is a scope concern.
