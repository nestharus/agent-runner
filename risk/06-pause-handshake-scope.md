# 06-pause-handshake — Phase 4 Scope Risk Assessment (Rev 3)

**Assessor:** scope reviewer
**Verdict:** **LOW** — Rev 3 closes the single Round 2 audit finding
(R2-F01: removable `flock` target splits the critical section) by
swapping the synchronization mechanism from POSIX advisory locks to
atomic `O_CREAT | O_EXCL` create-if-absent. This is a mechanism
correction, not a scope change: the contract envelope (commands, JSON
receipts, exit codes, lock/marker file paths, side-effect contract,
anti-scope) is identical to Rev 2. R1-F01..R1-F04 closures from Rev 2
remain intact. No regression.

## Round 2 closure check (audit only)

| ID | Round 2 ask | Rev 3 close | Closed |
| --- | --- | --- | --- |
| R2-F01 | Replace removable-`flock` critical section with a stable synchronization contract that does not split under stale cleanup or release/acquire timing. | Rev 3 picks the "atomic-create/rename protocol that does not depend on flocking a removable pathname" branch of the audit's required closure. §4 step 6 explicitly forbids relying on POSIX advisory locks because the lockfile path may be unlinked. §4 step 7 specifies `OpenOptions::new().create_new(true).write(true).open(&lock_path)` (= `O_CREAT \| O_EXCL \| O_WRONLY`). §4 step 10 specifies bounded stale-acquire (read T_old → `unlink` → single retry of `create_new`) and pins the safety case to kernel atomic create-if-absent: "only one contender can create the replacement lockfile." §6 mirrors the same algorithm in the `SessionLock` API description; the prior "flock-around-the-critical-section" language is gone. §8 side-effect list now reads "Atomically create … with `O_CREAT \| O_EXCL`" and "Unlink an expired … followed by one atomic retry-create." | yes |

R2-F01 closure verification (audit only, scope reviewer cross-check):

- **Mutual exclusion source.** The kernel guarantees that for a given
  pathname only one concurrent `O_CREAT | O_EXCL` open can succeed; all
  others receive `EEXIST`. This atomicity holds across the original
  acquire path (§4 step 7) and the stale-acquire retry (§4 step 10.3).
  No process state, advisory-lock fd, or inode binding is required for
  exclusion; the pathname-vs-inode mismatch class that motivated R2-F01
  is removed at its root.
- **Stale-acquire interleavings.** With multiple contenders all reading
  an expired lease: at most one will successfully `create_new` the
  replacement, regardless of `unlink` ordering. A losing contender's
  `unlink` may return `ENOENT` (benign — the file is already gone), but
  its subsequent `create_new` will fail with `EEXIST` and map to exit
  `13 session-busy` per §4 step 10.4 / §6's "If that retry sees EEXIST,
  another contender won and the result is `LockError::Busy`." The
  audit's "split-brain double-`0`-acquire" scenario cannot occur under
  this primitive.
- **Release/acquire interleavings.** Release unlinks the lockfile
  (§4 step 15). A concurrent fresh acquire that arrives mid-release will
  either see the lockfile still present (→ EEXIST, then either `13` if
  the lease is still valid or stale-acquire if the file is somehow
  expired-but-still-present, which is a no-op transient) or see the
  lockfile already removed (→ `create_new` succeeds with the new lease).
  Either branch is consistent: at most one process holds an active lease
  at any moment, and the marker is written before `unlink` (§4 step 15:
  "write the sibling release marker `session-<session_id>.released`,
  fsync the marker, unlink the lockfile") so idempotent-replay evidence
  is durable before the lockfile disappears.
- **Acquire/marker interaction.** §4 step 8 still removes the prior
  sibling release marker after writing fresh lock metadata, preserving
  R1-F01's "fresh acquire reaps the prior marker" guarantee. The order
  is now: `create_new` succeeds → write lease JSON → fsync → close →
  remove old marker → fsync dir → success. The old marker can only be
  removed by the process that just won the atomic create, so marker
  removal is naturally serialized through the create-if-absent winner.

Audit-only conclusion: R2-F01 is closed on the proposal text.

## Round 1 closure check (carry-forward, no regression)

| ID | Rev 2 close | Rev 3 carry-forward | Held |
| --- | --- | --- | --- |
| R1-F01 (release marker shape) | §6 sibling marker `session-<uuid>.released` with versioned JSON; §3.2 receipt field `release_marker_path`; §4 acquire reaps prior marker; §8 marker mutations enumerated; §12 deletes "marker-shape deferral" residual. | All marker text identical in Rev 3. §4 step 8 (acquire path, fresh lease success) still says "remove any previous sibling release marker for the same session"; step 10.5 (stale-acquire retry success) repeats it. §6 acquire/release prose unchanged on marker handling. §3.2/§8 unchanged. | yes |
| R1-F02 (advisory-v1 framing) | §1, §12 D4b, §13 row 3 "Partial by design"; sibling PRs named (`import-replace`, `migrate_chain_segment`, `run_repl`, `run_resume`, balanced one-shot); README §10 mandate. | All advisory-framing language unchanged in Rev 3. The Rev 3 mechanism change does not alter the harness acceptance surface — the lock primitive is still a per-session lease whose end-to-end mutual exclusion depends on sibling observers. | yes |
| R1-F03 (`StateDb::open` clause) | §8 explicit `StateDb::open_default()` clause matching 06-locate / 06-export side-effect contracts (parent dir, WAL enable, schema-ensure, chain backfill); §12 read-only follow-up tied to 06-schema-probe. | §8 prose is unchanged in Rev 3; the lock-mechanism rewrite touches only the lock-state mutation enumerations (filesystem create/unlink shape), not the DB-open clause. | yes |
| R1-F04 (test matrix `assumption_link` + `residual_risk` columns) | §9.1 columns populated for every row; A1–A7 references; residual_risk notes per row. | §9.1 matrix structure unchanged in Rev 3. The "Atomic acquire" and "Stale acquire" rows continue to assert "Two concurrent pause calls grant one token" / "Expired lockfile is lazily replaced" — both behaviors are now backed by the race-free Rev 3 algorithm rather than the flawed Rev 2 flock algorithm, but the test-intent text and assumption/residual columns themselves are unchanged. | yes |

All four R1 closures stand under Rev 3.

## Fresh assessment of Rev 3 deltas (scope dimension only)

| Rev 3 change | Direction | Magnitude | Scope verdict |
| --- | --- | --- | --- |
| §4 step 6 D1a clarification: "implementation must not rely on POSIX advisory locks for mutual exclusion because the lockfile path may be unlinked during stale cleanup" | mechanism-pin (negative constraint) | none (clarification) | In-scope. Constrains the implementer away from a known-broken pattern; does not change what the v1 build delivers. |
| §4 step 7 algorithm pin: `OpenOptions::new().create_new(true).write(true).open(&lock_path)` (= `O_CREAT \| O_EXCL \| O_WRONLY`) | mechanism replacement | small (rewrites one critical section) | In-scope. Same lockfile path, same lockfile schema, same lease/release/replay behavior; only the kernel primitive that enforces mutual exclusion changes. The contract surface (`lock_path`, `expires_at`, `token`, exit codes `13`/`16`/`17`) is unchanged. |
| §4 step 10 stale-acquire bound: read T_old → `unlink` → single `create_new` retry; EEXIST on retry → `13`; success on retry → write fresh lease | mechanism replacement | small | In-scope. Stale-acquire was already part of D5 ("stale removal is lazy on the next acquire attempt"). Rev 3 specifies *how* the lazy replacement is done atomically; it does not add a new behavior class. The bound — single retry, no further retries — is tighter than Rev 2's underspecified flock-protected replace. |
| §4 step 9 malformed-lease branch: explicit "if malformed, unreadable, or missing required fields, return exit 1 operational-error; do not guess whether the lease is stale" | mechanism-pin (defensive) | tiny | In-scope. This is a content-visibility safeguard for the brief window between `create_new` and JSON-content fsync, where a contending reader could see an empty/partial file. The exit-1 mapping was already part of §5.1's `operational-error` row; Rev 3 just names the trigger explicitly. Conservative, not symptom-masking. |
| §6 `acquire()` / API prose rewrite: "uses only atomic create-if-absent for mutual exclusion" + EEXIST-as-Busy semantics | API-doc realignment | small | In-scope. The `SessionLock` public types and method signatures (`Lease`, `ReleaseReceipt`, `LockError`, `acquire`, `release`, `observe`, `lock_path`, `release_marker_path`) are unchanged. Only the prose describing how `acquire()` enforces exclusion changes. |
| §8 side-effect list rewrites: "Atomically create … with `O_CREAT \| O_EXCL`"; "Unlink an expired … followed by one atomic retry-create" | side-effect contract realignment | small | In-scope. Side effects on disk (create/unlink lockfile, create/remove marker, lock dir creation) are the same set as Rev 2; the list now describes the create operation as atomic-exclusive rather than flock-protected. No new state mutation surface. |

**Net direction.** Rev 3 replaces a flawed synchronization mechanism
with a race-free one inside the same contract envelope. No new
commands, no new files, no new exit codes, no new receipt fields, no
new DB access, no new anti-scope leaks. The build envelope shrinks
slightly (one fewer dependency on advisory-lock semantics) and the
correctness floor rises. Pure scope-positive mechanism refinement.

The Rev 1 expansions (chain_id receipt, TTL bounds, lock module,
`observe()` API, hex-32 token) and Rev 2 expansions (sibling marker,
explicit advisory-v1 framing, pinned `StateDb::open` clause, test
matrix columns) are unchanged in Rev 3 and still inside scope.

## Anti-scope §7 audit vs Rev 3 §2–§6

Re-checked each §7 clause against the Rev 3 algorithm rewrite:

| §7 clause | Rev 3 leakage check | Result |
| --- | --- | --- |
| No transcript content mutation or import-replace implementation | `O_CREAT \| O_EXCL` operates on the lockfile path under `locks/`; not a transcript, not a `session_turns` row, not import-replace | honored |
| No provider spawn / signal / suspend / resume / kill | The atomic-create primitive is filesystem-level; no executor, no signal, no provider invocation | honored |
| No proof of safety for provider CLIs launched outside agent-runner | Advisory-v1 framing in §1 / §12 / §13 is unchanged by Rev 3 | honored |
| No global runner lock | Lock path is still `session-<uuid>.lock`, per-session-id-scoped | honored |
| No DB lock table in v1 | Lock state remains filesystem-only; Rev 3 explicitly uses kernel filesystem atomicity, not SQLite | honored |
| No strict ambiguity query outside the shared resolver | §4 steps 1–3 unchanged | honored |
| No fallback to raw `session_turns` | §4 unchanged | honored |
| No GUI / frontend lock indicator | No frontend file added; no GUI surface added | honored |
| No quota/auth refresh, provider selection, config edit, `migrate-config` coupling | §4/§8 do not invoke quota, balancer, or any provider-config writer | honored |

D4b's wording in §7 is unchanged. The Rev 3 mechanism switch does not
weaken any anti-scope clause: the v1 lock primitive still does not
silently spawn, drain, write transcripts, or touch DB rows beyond the
accepted `StateDb::open` open-time effects already pinned in §8.

## Decomposition assessment (unchanged from Rev 2)

Pause + resume must ship as one PR (Rev 1 analysis stands). D4b's
sibling-observer deferral is the only meaningful split, and Rev 3 makes
no new merge surface that would invite further decomposition. The
mechanism rewrite is local to the `acquire()` / `release()` critical
sections inside the new `src-tauri/src/session_lock/` module; it cannot
be split off without duplicating the lockfile schema and the marker
schema, which are already minimal.

## Coverage matrix delta vs Rev 2

| Source ask / constraint | Rev 3 status |
| --- | --- |
| Harness same-token idempotent replay | Unchanged from Rev 2 — concrete sibling marker still backs replay. |
| Initiative §112 reuse `StateDb::resolve_resume` | Unchanged; §4 step 2–3. |
| Initiative §115 deferred observers | Unchanged from Rev 2 advisory-v1 framing. |
| Initiative §118 read-only `StateDb` open belongs to schema-probe | Unchanged; §8 + §12 read-only follow-up. |
| Problem-map §3.1 risk: no token identity | Unchanged; §6 token + sha256 hash. |
| Problem-map §6.2 gap: no JSON pause/release receipts | Unchanged; §3.2 release receipt with marker path. |
| Problem-map §3.x mutual-exclusion correctness (implicit) | **Tightened by Rev 3** — atomic create-if-absent eliminates the inode/path mismatch class that the flock-on-removable-path approach exposed in Rev 2. |

No previously covered row regresses; one row tightens.

## Cross-feature consistency (no regression)

- Shared error namespace (`10`/`11`/`13`/`16`/`17`; `12`/`14`/`15`
  reserved): aligned with initiative §107–§110. Rev 3 unchanged.
- Ownership through `resolve_resume`: aligned with initiative §112.
  Rev 3 unchanged.
- Lock observation by sibling features: §13 row 3 still reads
  "Partial by design" with each deferred observer PR named;
  aligned with initiative §115 ("once 06-pause-handshake lands").
- Read-only `StateDb` open belongs to schema-probe: §8 still pins this
  inheritance; aligned with initiative §118.
- No auto-resume / spawn / quota / config-edit / migrate-config:
  aligned with initiative §121–§122; honored throughout §7/§8.

No cross-feature row regresses under Rev 3.

## Findings (severity ≥ MEDIUM)

None.

## Findings (LOW)

- **F1 (carried) — Harness AC bullet #6 satisfied across multiple PRs.**
  Unchanged from Rev 2. v1 ships the primitive only; sibling observers
  land in their own PRs. Phase 5 hookpoint research will pin exact
  hookpoints. No Phase 4 remediation.
- **F2 (carried) — `observe()` API exposed but not consumed in this
  PR.** Unchanged from Rev 2. §6 defines the read shape; consumer set
  is the four sibling PRs named in §12. Cosmetic nit unchanged: §6
  itself does not name the consumers (§1 and §12 do).
- **F3 (carried) — TTL bounds (1s / 30m / 5m default) without harness
  mandate.** Unchanged from Rev 2.
- **F4 (carried) — Token format diverges from harness's ULID-shaped
  example.** Unchanged from Rev 2. `pause_<32 hex>` keeps prefix;
  ULID/UUID rejection defensible on entropy grounds.
- **F5 (closed by Rev 2) — Idempotent-release marker shape was
  deferred to Phase 5.** Closed; carried-closed under Rev 3.
- **F6 (new, advisory) — Lease-content visibility window between
  `create_new` and JSON fsync.** §4 step 9's "malformed → exit 1
  operational-error" handles a contending reader who races the
  creator before the lease body is written. This is correctness-safe
  (no false grant, no false release) but produces a transient
  `operational-error` rather than `session-busy` for callers who
  observe the empty-but-existing window. Phase 5 may close the
  window via write-to-temp-then-`renameat2(RENAME_NOREPLACE)`, but
  this is implementer ergonomics, not scope. Non-blocking.

## Recommended revisions

None that change scope. R2-F01 is closed; R1-F01..R1-F04 closures
hold; the proposal as written is correctly scoped. Optional cosmetic
nit (carried from Rev 1 / Rev 2): §6 could name the deferred consumer
set inline next to the API definition, mirroring §1 / §12. Not a
scope concern. Optional Phase 5 ergonomics (F6 above): consider
write-to-temp-then-atomic-rename to eliminate the empty-file
transient-read window. Not a scope concern.
