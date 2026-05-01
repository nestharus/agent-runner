# Phase 5 Hookpoints — 06-pause-handshake (`agents session pause-handshake` / `resume-handshake`)

> **Note (pre-change evidence):** This hookpoint map describes the current
> `06-pause-handshake` worktree before Phase 6 implementation. The approved
> proposal is `proposals/06-pause-handshake.md` Rev 4. Rev 4 replaces stale
> `unlink + retry` locking with a never-unlinked sentinel file whose exclusive
> `flock` is the real mutex, plus same-directory atomic rename for per-session
> lease files. The local worktree is not yet stacked on 06-locate/schema/export:
> it has no `session` subcommand parent, no `session_metadata` module, and no
> read-only state open. Hookpoints below therefore name both the local current
> surface and the expected stacked Initiative 06 surfaces.

## A. `session pause-handshake` and `resume-handshake` subcommand surface hookpoints (proposal §2)

- **Extend:** `Subcommands` currently lives in `src-tauri/src/main.rs:77-166`
  and contains `Trace`, `Repl`, `Resume`, hidden `ResumeList`, `MigrateDb`, and
  `MigrateConfig`. In the stacked 06-locate branch, `Subcommands::Session` is
  already inserted before `ResumeList`, with a nested required
  `SessionSubcommands` child. Pause-handshake should extend that nested enum,
  not create a second top-level command group.
- **Stacked placement:** in 06-locate, `SessionSubcommands` lives near
  `Subcommands` and currently contains `Locate { session_id, json }`. Add
  `PauseHandshake { session_id: String, ttl_ms: u64 }` and
  `ResumeHandshake { session_id: String, token: String }` alongside `Locate`.
- **Clap shape:** `pause-handshake` needs
  `#[arg(long, default_value_t = DEFAULT_LOCK_TTL_MS)] ttl_ms: u64`.
  `resume-handshake` needs `#[arg(long)] token: String`. Missing token and
  out-of-range TTL remain clap/usage failures with exit `2`; no custom prompt
  or stdin read should be introduced.
- **Constants:** define `DEFAULT_LOCK_TTL_MS = 300_000`,
  `MIN_LOCK_TTL_MS = 1_000`, and `MAX_LOCK_TTL_MS = 1_800_000` close to the
  session command wrapper or in `session_lock` with the command wrapper
  enforcing the clap-visible bounds.
- **TTL validator:** use a clap `value_parser` or a typed wrapper so
  out-of-range `--ttl-ms` fails before state/config/lock access. A plain
  `u64` field alone is insufficient because proposal §4 requires bounds.
- **Dispatch:** top-level dispatch is the `match command` in
  `run(cli)` at `src-tauri/src/main.rs:287-338`. In the stacked branch,
  `Subcommands::Session { command }` dispatches `Locate`; extend that nested
  match with `PauseHandshake` and `ResumeHandshake` arms.
- **Ordering:** dispatch the `Session` arm before hidden `ResumeList` and before
  the top-level `--resume` routing at `src-tauri/src/main.rs:341-389`. This
  preserves existing `repl` / `resume` behavior and keeps `agents session ...`
  structurally separate from top-level resume.
- **Bare parent/root conflicts:** keep `command: SessionSubcommands`
  non-optional and preserve root `args_conflicts_with_subcommands = true` at
  `src-tauri/src/main.rs:18-23`; bare `agents session` remains clap usage error.
- **UUID validation:** the command wrapper must parse `<session-id>` as a full
  UUID before `StateDb::open_default`, config loading, lock-dir creation, or
  sentinel open. This is stricter than relying only on `StateDb::resolve_resume`
  because proposal §4 requires no state/lock access on invalid UUID.
- **Output:** success stdout is one compact JSON object via
  `serde_json::to_string`, not pretty JSON. Semantic failures are one compact
  JSON object on stderr with `code` and `message`.
- **Exit plumbing:** current `main` returns `Result<i32, String>` and generic
  errors generally become exit `1` through the outer main path. The session
  handshake wrappers need explicit `Ok(code)` returns after writing JSON stderr
  for `10`, `11`, `13`, `16`, and `17`; do not surface these as `Err(String)`.
- **README/no reuse hook:** update the synopsis near `README.md:127` and do not
  reuse hidden `resume-list`; it is text-only and unrelated to lease receipts.

## B. Reusable `SessionLock` API hookpoints: sentinel + acquire/release (proposal §6)

- **New module:** create `src-tauri/src/session_lock/` with `mod.rs`. Export it
  from `src-tauri/src/lib.rs:1-11` with `pub mod session_lock;` because future
  06-import-replace, migration, resume/repl, and balanced one-shot observers
  must share the same primitive.
- **Public API:** define `SessionLock { root: PathBuf }`, `Lease`,
  `ReleaseReceipt`, `ExistingLockInfo`, and `LockError`; keep filesystem
  helpers private.
- **Command boundary:** `SessionLock` should know only lock roots, paths,
  tokens, hashes, expiry, and filesystem operations. The command layer should
  resolve `session_id`, `chain_id`, and `provider_name`, then combine resolver
  metadata with `Lease` / `ReleaseReceipt` for CLI JSON.
- **Constructor/path helpers:** `SessionLock::from_default_data_dir()` should
  use `dirs::data_dir()/oulipoly-agent-runner/locks`; expose
  `lock_path()` / `release_marker_path()` and keep `sentinel_path()` private.
- **Acquire method:** `acquire(&self, session_id, ttl) -> Result<Lease,
  LockError>` owns the whole Rev 4 decision under the sentinel flock: read
  existing lock, reject active lock, replace stale/absent lock by atomic rename,
  remove old marker, return a lease with raw token.
- **Release method:** `release(&self, session_id, token) ->
  Result<ReleaseReceipt, LockError>` owns the whole release/idempotency
  decision under the sentinel flock: compare token evidence, write marker,
  unlink lock, or check existing marker.
- **Observe method:** `observe(&self, session_id) -> Result<Option<ExistingLockInfo>,
  LockError>` is needed for sibling commands later. In this PR it should be
  implemented and tested enough to be reusable, but no current sibling command
  should be retrofitted in v1 per proposal A6/D4.
- **Sentinel/dependencies:** a private sentinel helper opens `sentinel.lock`
  with `O_CREAT | O_RDWR`, takes exclusive `flock`, runs a closure, and drops
  the fd. `src-tauri/Cargo.toml` currently lacks `libc`/`fs2`,
  `getrandom`/`rand`, and `sha2`; add any needed dependency intentionally.
- **Error mapping boundary:** `LockError::Busy` maps to exit `13`;
  `TokenInvalid` maps to `16`; `LockExpired` maps to `17`; malformed/unreadable
  JSON, fsync, rename, permission, hash, and randomness failures map to
  operational exit `1`.
- **Persistence boundary:** `Lease` may carry raw `token` for stdout, but files
  persist only `token_hash`; do not add a DB lock table.

## C. Resolution flow hookpoints (proposal §4)

- **Step 1 UUID parse:** parse with `Uuid::parse_str` in the session command
  wrapper before state/config/lock operations. Current `run_resume` does an
  upfront parse at `src-tauri/src/main.rs:1065-1068`; mirror the timing but map
  to structured stderr JSON `invalid-session-id`.
- **Step 2 state open:** use `StateDb::open_default()` at
  `src-tauri/src/state/db.rs:611-615` until the stacked read-only open is
  available. This may create the state parent, set WAL, ensure schemas, and
  run chain backfill through `StateDb::open` at `src-tauri/src/state/db.rs:431-608`.
- **Step 3 config load:** mirror resume/repl config load parity. Resume loads
  models from default or override model dir, then provider/session config from
  the default config root with `unwrap_or_default` behavior around
  `src-tauri/src/main.rs:1071-1084`. Pause/resume-handshake should not invent a
  stricter config parser.
- **Step 4 shared ownership:** use the same ownership path as locate. In the
  local lower-level code this is `StateDb::resolve_resume` at
  `src-tauri/src/state/db.rs:2577-2670`; in the stacked branch this may be
  reached through `locate_session_metadata` in `session_metadata`.
- **Resolver choice:** if stacked on 06-locate, prefer a reusable ownership
  helper that returns active `session_id`, `chain_id`, and `provider_name`
  without forcing transcript availability; `locate_session_metadata` may be too
  strong if it requires transcript/workspace derivation.
- **No raw `session_turns` fallback:** `StateDb::resolve_resume` reads chain and
  segment ownership and does not use raw `session_turns` as direct ownership
  candidates. Pause-handshake must inherit that limitation rather than adding a
  second resolver.
- **Resolved key:** compute the lock key from `resolved.active_session_id`, not
  the raw input. If the user passes a chain id, the receipt `session_id` must be
  the active provider session id and `chain_id` must be the chain id.
- **Receipt metadata:** carry `chain_id` and `provider_name` from the resolver
  into stdout JSON. `SessionLock` does not own those fields; the CLI wrapper
  joins them with the lock receipt.
- **Error mapping:** `ResumeError::NoChainFound` maps to exit `10`
  `session-not-found`; `ResumeError::Ambiguous` maps to exit `11`
  `ambiguous-session`; invalid UUID maps to exit `2`. Provider/model/config DB
  failures are operational exit `1` unless proposal text later narrows them.
- **No side work:** do not call migration, provider target expansion,
  quota/auth refresh, or process-liveness proof. Existing `running` invocation
  rows are not lock evidence and should not be treated as busy.

## D. Sentinel-flock + atomic-rename filesystem hookpoints (proposal Rev 4 / §4 / §8)

- **Lock directory:** create `<data_dir>/oulipoly-agent-runner/locks/` on first
  use. On Unix, set or verify mode `0700`; failure is operational exit `1`.
- **Sentinel creation:** open `locks/sentinel.lock` with
  `OpenOptions::new().create(true).read(true).write(true).open(...)`. Do not
  use `O_EXCL` and do not delete the sentinel after acquire or release.
- **Permissions/critical section:** new sentinel/lock/marker/temp files should
  be owner-private mode `0600` on Unix. Hold exclusive `flock` while reading,
  validating, writing temp files, renaming, removing markers, unlinking lock
  files, and fsyncing the directory. Release it only after the full decision.
- **Acquire read:** under the flock, open
  `session-<session_id>.lock` read-only if present. `ENOENT` is absent; other
  open/read/parse failures are operational. Malformed JSON is not stale.
- **Active lock:** if parsed `expires_at > now`, return `Busy` without touching
  the lockfile or marker.
- **Stale/absent lock:** generate token, render versioned lease JSON, write to
  a unique sibling temp file such as
  `session-<uuid>.lock.acquire-<pid>-<random>.tmp`, fsync file, then rename it
  onto `session-<uuid>.lock` while still holding the sentinel flock.
- **No stale unlink/marker cleanup:** replace expired lockfiles by rename under
  the sentinel mutex, then remove any old same-session `.released` marker.
- **Release write:** matching release writes
  `session-<uuid>.released.release-<pid>-<random>.tmp`, fsyncs it, renames it
  onto `session-<uuid>.released`, then unlinks `session-<uuid>.lock`.
- **Release ordering:** write durable marker before unlinking the active lock so
  same-token retry has evidence after a successful release. Keep both
  operations under the sentinel flock.
- **Fsync/temp/atomicity:** fsync files and directory when practical; crash
  orphan temps are inert; keep temp and final files in the same lock directory
  so rename is same-mount atomic per A8.

## E. Lease JSON shape and token generation hookpoints (proposal §3 / §6)

- **Pause stdout:** emit compact JSON with `session_id`, `chain_id`,
  `provider_name`, `token`, `expires_at`, and `lock_path`.
- **Resume stdout:** emit compact JSON with `session_id`, `chain_id`,
  `provider_name`, `released`, `already_released`, `lock_path`, and
  `release_marker_path`; include `note` only for the expired matching release
  case if implemented as proposal suggests.
- **Error stderr:** semantic errors emit compact JSON with `code` and
  `message`, plus known optional metadata.
- **Lease file shape:** persist versioned JSON:
  `version`, `session_id`, `token_hash`, `created_at`, `expires_at`, and
  `owner_pid`. Do not persist `chain_id` or `provider_name` in the lockfile
  unless the implementation has a specific reason; the proposal's lockfile
  shape omits them.
- **Marker file shape:** persist versioned JSON:
  `version`, `session_id`, `chain_id`, `provider_name`, `token_hash`, and
  `released_at`. The marker contains resolver metadata because resume receipts
  and idempotent replay need to report it.
- **Token format/entropy:** generate `pause_<32 lowercase hex chars>` from OS
  CSPRNG bytes. `uuid` is already in
  `src-tauri/Cargo.toml`, but UUIDv4 is rejected by the proposal because the
  version/variant bits reduce entropy below the exact 128-bit token format.
- **Hashing:** use a cryptographic hash with explicit prefix such as
  `sha256:<hex>`; `src-tauri/Cargo.toml` currently lacks `sha2`.
- **Time fields:** use RFC3339 UTC serialization. Existing code already depends
  on `chrono` with serde support, so `DateTime<Utc>` is the natural local type.
- **PID/path/version boundaries:** `owner_pid` is observational only; non-UTF-8
  receipt paths are operational errors; unsupported future `version` values are
  operational, not stale.

## F. TTL handling hookpoints (proposal §4 / §5 / §9.1)

- **Bounds:** default TTL is 5 minutes (`300000` ms), minimum is 1 second
  (`1000` ms), maximum is 30 minutes (`1800000` ms). Enforce bounds at CLI parse
  time for user input and in `SessionLock::acquire` for API callers.
- **Clock:** calculate `created_at = now` and `expires_at = now + ttl` using a
  UTC wall clock. Component tests should use an injectable clock or helper to
  avoid sleeps.
- **Stale predicate:** stale means `expires_at <= now`. Exact equality is
  expired. Non-expired means `expires_at > now`.
- **Acquire stale behavior:** expired lockfiles are lazily replaced by atomic
  rename under sentinel flock. There is no background reaper and no cleanup
  command in v1.
- **Malformed lock:** malformed, unreadable, unsupported-version, or
  missing-required-field lock metadata is not stale and must not be evicted.
  Return operational exit `1`.
- **Busy error:** non-expired lock maps to `13 session-busy`, with stderr JSON
  message naming the expiry when known.
- **Release after expiry:** matching expired lockfile releases with exit `0`;
  expired wrong-token is `16`; missing lock checks marker, where match is
  idempotent success, mismatch is `16`, and no marker is `17`.
- **Marker/clock scope:** markers persist until next same-session acquire; do
  not add marker TTL, monotonic-clock persistence, or distributed-clock logic.

## G. Lock observation by sibling commands: deferred hookpoints per A6 (proposal §7 / §12)

- **Deferral status:** v1 adds the primitive and observer API only. It does not
  retrofit existing writer paths. This is intentional per proposal A6 and D4b,
  and README must state the lock is advisory until sibling PRs observe it.
- **Future import-replace:** `agents session import-replace` is not present in
  this worktree. Its future write path must call `SessionLock::observe` after
  resolving the active session and before transcript preimage/write attempts,
  then fail with exit `13 session-busy` on active foreign lock.
- **Future migration observer:** `migrate_chain_segment` lives at
  `src-tauri/src/migration/mod.rs:79-254`. It reads source transcript bytes,
  writes target JSONL, renames, closes the old segment, and opens the target
  segment. The observer hook belongs before source-read/target-write mutation,
  after the active source session is known.
- **Future `run_repl` observer:** `run_repl` lives at
  `src-tauri/src/main.rs:809-1054`. Its `--resume` branch resolves ownership
  around `src-tauri/src/main.rs:830-846` and may migrate around
  `src-tauri/src/main.rs:903`. Future observation belongs after resolution and
  before migration, invocation capture, or provider spawn.
- **Future `run_resume` observer:** `run_resume` lives at
  `src-tauri/src/main.rs:1056-1263`. It resolves ownership around
  `src-tauri/src/main.rs:1087-1107` and may migrate around
  `src-tauri/src/main.rs:1127`. Future observation belongs after resolution and
  before migration, invocation capture, or provider spawn.
- **Future top-level/balanced:** top-level `--resume` routes into `run_resume`
  or `run_repl`, so those retrofits cover it. `run_with_balancing` lives at
  `src-tauri/src/main.rs:1265-1411`. It may discover/write session state after
  provider execution, which means a simple preflight may not know a session key.
  Proposal §12 leaves this as residual fail-closed design work for that later
  observer PR.
- **Future scan/ingest observer:** `ingest_and_emit_session_id` at
  `src-tauri/src/main.rs:541-615` and `scan_provider` at
  `src-tauri/src/sessions/mod.rs:60-141` write session rows after provider
  success. Later observer work needs to decide whether checks occur before
  scan, before mint/promote, or both.
- **Future backfill/error contract:** `migrate-db` policy is later work; all
  future observers should reuse `LockError::Busy` and map to JSON
  `session-busy` / exit `13`. V1 tests must not expect current sibling blocking.

## H. Read-only behavior in resolver path hookpoints (proposal §8)

- **Current reality:** `StateDb::open` creates parent dirs, opens SQLite
  read/write, enables WAL, ensures schemas, and runs chain backfill before
  returning (`src-tauri/src/state/db.rs:431-608`). This is not physically
  read-only in the local pause worktree.
- **Accepted inheritance:** pause/resume-handshake may use
  `StateDb::open_default()` for resolver-only access and inherit those open-time
  side effects, matching the proposal's 06-locate/06-export side-effect
  contract.
- **No additional DB writes:** after open, the handshake path must not call
  `start_invocation`, `update_session_capture`, `finalize_invocation`,
  `scan_provider`, `mint_chain_for_invocation_session`, `open_chain_segment`,
  `close_active_segment_returning`, `migrate_chain_segment`, quota refresh, or
  config rewrite.
- **No transcript mutation:** neither command may read-modify-write provider
  JSONL, canonical export files, transcript temp files, or locator outputs.
- **No provider commands:** do not run provider CLIs, quota scripts, auth
  refresh commands, diagnostics models, scanner scripts, or locator scripts.
- **Read-only open follow-up:** when 06-schema-probe's `StateDb::open_read_only`
  is available in the merge base, switch resolver access to it as a follow-up.
  Do not block v1 on that local absence.
- **Snapshot/mutation boundary:** snapshot after `StateDb::open` to exclude
  accepted open effects. Only lock dir/sentinel/temp/`.lock`/`.released`
  mutations are intended; no config or GUI surface changes.

## I. Test-intent track hookpoints (proposal §9.1)

- **General test home:** put lock API unit/component tests in
  `src-tauri/src/session_lock/mod.rs` under `#[cfg(test)]`. Put CLI process
  integration tests in a new `src-tauri/tests/initiative_06_pause_handshake.rs`,
  following existing integration patterns that run
  `env!("CARGO_BIN_EXE_oulipoly-agent-runner")`.
- **Fixture reuse:** existing integration fixtures create temp XDG config/data,
  model dirs, scripts, and DB paths; reuse that style or any stacked Initiative
  06 fixture.
- **Resolver pass-through:** seed `session_chains` and
  `session_chain_segments`, then assert pause stdout uses resolved active
  provider session id, chain id, and provider name rather than raw input.
- **Invalid UUID:** run CLI with malformed id and impossible/temp data dir;
  assert exit `2`, stderr JSON `invalid-session-id`, and no lock directory.
- **Not found / ambiguous:** use temp DB resolver fixtures to assert exit `10`
  `session-not-found` and exit `11` `ambiguous-session`.
- **Atomic/per-session acquire:** concurrent same-session pauses grant exactly
  one lease; different sessions can both hold leases despite sharing sentinel.
- **Token/TTL/acquire states:** unit-test token format, TTL default/min/max,
  active busy lock (`13`), stale replacement, and malformed lock operational
  failure.
- **Release matrix:** cover correct release, wrong token (`16`), expired
  matching release (`0`), idempotent replay (`already_released: true`), missing
  lock/no marker (`17`), and marker mismatch (`16`).
- **Permissions:** Unix integration test inspects mode bits for lock directory,
  sentinel, lock, marker, and temp-created final files. Windows behavior is not
  designed by the proposal.
- **Side effects/docs/parser:** assert no DB/transcript/config mutation after
  open effects; README documents advisory scope; stdout/stderr are compact JSON;
  parser tests cover root conflicts, missing `--token`, and TTL bounds.

## J. Implementation surface summary

| Proposal action | Hookpoint | Reuse / extend / new |
| --- | --- | --- |
| `session` parent | stacked 06-locate `Subcommands::Session`; local `src-tauri/src/main.rs:77-166` lacks it | extend stacked / add if unstacked |
| `pause-handshake` child | `SessionSubcommands` near `Subcommands` | extend |
| `resume-handshake` child | `SessionSubcommands` near `Subcommands` | extend |
| TTL constants and validator | session command wrapper or `session_lock` | new |
| Dispatch | `run(cli)` nested `Subcommands::Session` match | extend |
| Invalid UUID preflight | command wrapper before DB/config/lock access | new + reuse `Uuid::parse_str` |
| State open | `StateDb::open_default()` at `src-tauri/src/state/db.rs:611-615` | reuse |
| Resolver | `StateDb::resolve_resume` at `src-tauri/src/state/db.rs:2577-2670`; stacked locate resolver if factored | reuse |
| Resolver metadata receipt | command wrapper joins `chain_id` / `provider_name` with lock receipt | new |
| Compact stdout JSON | session command wrapper using `serde_json::to_string` | new |
| JSON stderr + exit mapping | session command wrapper | new |
| `session_lock` module | `src-tauri/src/session_lock/mod.rs`; export from `src-tauri/src/lib.rs` | new |
| `SessionLock` / `Lease` / `ReleaseReceipt` / `ExistingLockInfo` / `LockError` | `session_lock` module | new |
| Default lock root | `dirs::data_dir()/oulipoly-agent-runner/locks` | new + parallel DB root |
| Sentinel file | `<lock_dir>/sentinel.lock` | new |
| Sentinel flock | private helper using POSIX `flock` | new dependency / platform code |
| Per-session lock path | `<lock_dir>/session-<uuid>.lock` | new |
| Release marker path | `<lock_dir>/session-<uuid>.released` | new |
| Token generation | OS CSPRNG, `pause_` + 32 lowercase hex | new dependency likely |
| Token hashing | cryptographic `sha256:<hex>` | new dependency likely |
| Acquire algorithm | read active/stale lock under sentinel, temp write, fsync, atomic rename | new |
| Release algorithm | token compare, marker temp write, fsync, atomic rename, unlink lock | new |
| Observe API | `SessionLock::observe` | new, sibling consumers deferred |
| Permissions | Unix mode `0700` dir, `0600` files | new |
| Read-only follow-up | switch to 06-schema-probe `open_read_only` when available | follow-up |
| No transcript/DB/config mutation | avoid writers in `main`, `sessions`, `migration`, `state`, `quota` | preserve |
| Future import-replace observer | future `agents session import-replace` write path | deferred |
| Future migration observer | `src-tauri/src/migration/mod.rs:79-254` | deferred |
| Future `run_repl` observer | `src-tauri/src/main.rs:809-1054` | deferred |
| Future `run_resume` observer | `src-tauri/src/main.rs:1056-1263` | deferred |
| Future top-level `--resume` observer | routes through `run_repl` / `run_resume` at `src-tauri/src/main.rs:341-389` | deferred via callees |
| Future balanced observer | `src-tauri/src/main.rs:1265-1411` | deferred / needs policy |
| CLI integration tests | `src-tauri/tests/initiative_06_pause_handshake.rs` | new |
| API/component tests | `src-tauri/src/session_lock/mod.rs` tests | new |
| Parser tests | `src-tauri/src/main.rs` clap tests | extend |
| README synopsis/docs | `README.md:127`, persistent state paragraph, resume/session sections | extend |
| Keep hidden `resume-list` unchanged | `src-tauri/src/main.rs:155-157`, `:1887-1900` | retain |
| Keep existing resume/repl behavior unchanged | `run_repl`, `run_resume`, top-level `--resume` | retain in v1 |
| Keep no DB lock table | `src-tauri/src/state/db.rs` schema bootstrap | retain |

## What this hookpoint research deliberately does NOT cover

1. It does not implement `session_lock`, CLI commands, tests, or README edits.
2. It does not design 06-import-replace or transcript replacement atomicity.
3. It does not retrofit migration, resume/repl, balanced one-shot, scan/ingest,
   or future import-replace observers; it only documents their deferred
   hookpoints.
4. It does not redesign `StateDb::open`, schema-probe, or physical read-only DB
   access beyond naming the follow-up switch point.
5. It does not change current resume ambiguity, active-segment, provider/model,
   or chain ownership semantics.
6. It does not add provider process suspension, signals, drains, pid liveness,
   auto-resume, quota refresh, diagnostics, or config migration.
7. It does not specify Windows locking/ACL semantics beyond noting the Unix
   permission and POSIX `flock` assumptions in Rev 4.
8. It does not add GUI/Tauri frontend visibility, HomeView/StatusView work, or
   design-system changes.

deliberately does NOT cover
