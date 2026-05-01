**Rev 2 changes** (in response to Phase 4 Round 1 audit):

- §6 / §3 / §4 / §12: idempotent release marker is sibling marker file `session-<uuid>.released` (R1-F01).
- §1 / §12 / §13: writer-path observer wiring explicitly deferred to sibling PRs; harness acceptance surface narrowed for v1; lock is advisory in v1 until 06-import-replace etc. retrofit (R1-F02).
- §8: explicit `StateDb::open` side-effect clause matching 06-locate / 06-export (R1-F03).
- §9.1: added assumption_link and residual_risk columns (R1-F04).

**Rev 3 changes** (Round 2 audit R2-F01 closure):

- §4 / §6 / §8: replace `flock` on removable path with race-free
  `O_CREAT | O_EXCL` atomic create-or-fail. Stale-acquire uses
  bounded `unlink + retry-create_new` (kernel atomic). Eliminates
  the inode/path mismatch race POSIX advisory locks introduce when
  the lockfile is unlinked under contention (R2-F01).

# 1. Scope statement

06-pause-handshake adds two CLI surfaces:

```bash
agents session pause-handshake <session-id> [--ttl-ms <ms>]
agents session resume-handshake <session-id> --token <token>
```

They provide a session-scoped exclusive lease for the Initiative 06
override flow. `agent-harness` obtains a bounded lease before transcript
replacement and releases it afterward. While valid, the lease is the
stable refusal signal for future lock-aware session writers.

06-pause-handshake v1 ships the lock primitive only. Sibling writer-path
observation in `import-replace`, migration's `migrate_chain_segment`,
`run_repl`, `run_resume`, and balanced one-shot is deferred to those
features' own PRs as a cross-cutting concern. Specifically,
06-import-replace's PR will observe the lock during write attempts and
exit `13 session-busy` on conflict; migration's
`migrate_chain_segment` retrofit is a follow-up; resume/repl observe in
their own follow-up. This narrows the harness acceptance surface for v1:
pause-handshake acquires a lock, but cannot prevent concurrent writes by
sibling commands until those commands are retrofitted in subsequent PRs.
The harness consumer should treat the lock as advisory in v1; full
mutual-exclusion arrives when 06-import-replace lands.

This is the fourth Initiative 06 feature in technical order, after
`locate`, `schema-probe`, and `export`, and before `import-replace`.
It uses the same ownership resolution rule as `agents session locate`;
no second resolver is introduced.

This proposal does not implement code. It defines the command shape,
JSON receipts, lock primitive, TTL policy, resolver policy, exit-code
mapping, side-effect contract, tests, README work, residuals, and
cross-feature compliance. It consumes
`research/06-pause-handshake-problem-map.md`; §1.1 replaces the draft
assumption register in that map.

What changes:

- Add `pause-handshake` and `resume-handshake` under `agents session`.
- Add `src-tauri/src/session_lock/` with a reusable lock manager.
- Add lock state under
  `~/.local/share/oulipoly-agent-runner/locks/`.
- Add stdout JSON receipts and stderr JSON semantic errors.

What does not change:

- No transcript export/import/replace or content mutation.
- No provider spawn, provider suspension, auto-resume, quota refresh, or
  config edit.
- No GUI/Tauri frontend surface.
- No sibling writer-path observation in this PR beyond adding the API
  those paths will use later; v1 lock enforcement is advisory until
  sibling PRs wire observers.

## 1.1 Assumption register

| ID | Assumption | Evidence | Invalidator | Used by |
| --- | --- | --- | --- | --- |
| A1 | Pause/resume uses the same owner semantics as locate, ultimately backed by `StateDb::resolve_resume`. | Harness says pause resolves through locate; Initiative 06 forbids a second ownership path. | Locate changes ownership semantics before this lands. | §4, §5, §6 |
| A2 | The lock key is the resolved active provider session id, with chain/provider metadata retained in receipts. | `ResolvedResume` exposes chain id and active provider/session; harness response names `session_id` and `provider_name`. | A prior feature introduces a distinct mutable-target key. | §3, §4, §6 |
| A3 | Existing `running` invocation rows are not a safe active-writer lock. | They are not session-scoped leases, can survive hard crash, and lack token/TTL semantics. | Invocation lifecycle gains durable session writer leases first. | §4, §7, §12 |
| A4 | The lease must outlive the `pause-handshake` process. | The command returns a token and exits; an fd-held process lock would release on exit. | Harness changes to a long-lived pause process. | D1, §6, §12 |
| A5 | TTL-based crash recovery is sufficient for v1. | No daemon exists; harness asks for crash-safe TTL cleanup. | A lock manager daemon or stricter cleanup requirement appears. | D3, D5, §9 |
| A6 | v1 should add the primitive first; scattered sibling write paths should observe in their own PRs. | Write paths span migration, repl/resume, balanced execution, ingestion, backfill, and future import-replace. | Phase 4 requires all observers in this PR. | D4, §7, §13 |
| A7 | File-backed lock receipts are acceptable if files are owner-private. | Harness response includes `lock_path`; runner state already lives in the per-user data dir. | Shared multi-user state dirs become supported. | D1, §8, §11 |

## 1.2 Net-value statement

Yes: this reduces a concrete current-state risk. Today a second process
can resume, migrate, ingest, backfill, or eventually import-replace a
session while another process is preparing a transcript override.
SQLite WAL protects individual DB writes but not provider JSONL files or
multi-step filesystem plus DB operations.

The value is a stable refusal surface: `session-busy`,
`lock-token-invalid`, and `lock-expired`. The blast radius is bounded
because v1 creates/removes lock state only. The main residual is that
sibling writer paths must adopt the observer API for full end-to-end
blocking.

# 2. Subcommand surface

Add two children to the Initiative 06 `SessionSubcommands` enum:

```text
session pause-handshake <session-id> [--ttl-ms <ms>]
session resume-handshake <session-id> --token <token>
```

Expected clap shape:

```rust
enum SessionSubcommands {
    PauseHandshake {
        session_id: String,
        #[arg(long, default_value_t = DEFAULT_LOCK_TTL_MS)]
        ttl_ms: u64,
    },
    ResumeHandshake {
        session_id: String,
        #[arg(long)]
        token: String,
    },
}
```

`<session-id>` must parse as a full UUID before state/config/lock access.
Invalid UUID is exit `2` with stderr JSON code `invalid-session-id`.

`--ttl-ms` is bounded by §4. Values outside bounds are clap usage errors
and exit `2`. `resume-handshake` requires `--token`; missing token is
also exit `2`.

Success output is always one compact JSON object on stdout. Semantic
failures emit one compact JSON object on stderr. Clap structural usage
may still use clap's default usage text.

# 3. JSON output schema

## 3.1 `pause-handshake` success stdout

```json
{
  "session_id": "9e69e8cc-616d-4640-bf1d-96f5391b1a2e",
  "chain_id": "5169694d-de0f-40d1-890c-6e28e55bab27",
  "provider_name": "claude2",
  "token": "pause_f4c1e2d7a9b84d0c92e608c7bbf0a113",
  "expires_at": "2026-04-30T12:35:56Z",
  "lock_path": "/home/me/.local/share/oulipoly-agent-runner/locks/session-9e69e8cc-616d-4640-bf1d-96f5391b1a2e.lock"
}
```

Fields:

| Field | Type | Meaning |
| --- | --- | --- |
| `session_id` | UUID string | Resolved active provider session id, not necessarily raw input. |
| `chain_id` | UUID string | Logical chain id. |
| `provider_name` | string | Active owner provider/account. |
| `token` | string | Secret release token. |
| `expires_at` | RFC3339 UTC | Absolute lease expiry. |
| `lock_path` | path string | Absolute UTF-8 lockfile path. |

D2 decision: choose random 128-bit hex with a `pause_` prefix:

```text
pause_<32 lowercase hex chars>
```

Tokens are generated from OS CSPRNG bytes. ULID and UUIDv7 are rejected
because they carry time structure; UUIDv4 is rejected because version and
variant bits reduce random entropy below the stated 128-bit token format.
If implementation needs a new crate, use a small OS-random source such
as `getrandom`.

## 3.2 `resume-handshake` success stdout

```json
{
  "session_id": "9e69e8cc-616d-4640-bf1d-96f5391b1a2e",
  "chain_id": "5169694d-de0f-40d1-890c-6e28e55bab27",
  "provider_name": "claude2",
  "released": true,
  "already_released": false,
  "lock_path": "/home/me/.local/share/oulipoly-agent-runner/locks/session-9e69e8cc-616d-4640-bf1d-96f5391b1a2e.lock",
  "release_marker_path": "/home/me/.local/share/oulipoly-agent-runner/locks/session-9e69e8cc-616d-4640-bf1d-96f5391b1a2e.released"
}
```

`already_released` is `true` only for idempotent same-token replay.
`released` is always `true` on exit `0`. `lock_path` is the active
lease file path. `release_marker_path` is the sibling marker written on
successful release and consulted for same-token idempotent replay.
`note` may be included with value `"released expired token"` when the
matching lockfile had already expired before release.

## 3.3 Error stderr

Semantic failures emit:

```json
{"code":"session-busy","message":"session is locked until 2026-04-30T12:35:56Z"}
```

Required fields are `code` and `message`. Optional fields may include
`session_id`, `chain_id`, `provider_name`, `expires_at`, and `lock_path`
when known.

# 4. Resolution flow

1. Parse `<session-id>` as a full UUID before state or lock access.
2. Open default CLI state/config in the same manner as prior Initiative
   06 session commands. If stacked on locate, call its reusable metadata
   resolver; otherwise call `StateDb::resolve_resume` directly.
3. Resolve ownership through the shared path. `NoChainFound` maps to
   exit `10`; `Ambiguous` maps to exit `11`; there is no fallback to
   direct `session_turns` queries.
4. Compute lock and release-marker paths from the resolved active
   provider session id:

   ```text
   <data_dir>/oulipoly-agent-runner/locks/session-<session_id>.lock
   <data_dir>/oulipoly-agent-runner/locks/session-<session_id>.released
   ```

5. Create the lock directory with owner-private permissions where Unix
   mode bits are available: directory `0700`, files `0600`.
6. D1 decision: choose D1a, lockfile-backed lease, with clarification.
   The durable lease is lockfile metadata created by atomic
   create-if-absent. The implementation must not rely on POSIX advisory
   locks for mutual exclusion because the lockfile path may be unlinked
   during stale cleanup. This preserves the harness `lock_path` contract
   and avoids a DB schema migration.
7. `pause-handshake` attempts:

   ```rust
   OpenOptions::new()
       .create_new(true)
       .write(true)
       .open(&lock_path)
   ```

   This is `O_CREAT | O_EXCL | O_WRONLY`: the filesystem atomically
   creates the path only if it does not already exist.
8. If creation succeeds, generate the token, write lease JSON containing
   `version`, `session_id`, `token_hash`, `owner_pid`, `created_at`, and
   `expires_at`, fsync the file, close it, remove any previous sibling
   release marker for the same session, fsync the directory when
   practical, emit the success JSON, and exit `0`.
9. If creation fails with `EEXIST`, read the existing lease JSON. If the
   lease is malformed, unreadable, or missing required fields, return
   exit `1 operational-error`; do not guess whether the lease is stale.
   If `expires_at >= now`, return exit `13 session-busy`.
10. If `expires_at < now`, the existing lease is stale. Stale-acquire is
    bounded and atomic at the create step:

    1. Read the existing token evidence `T_old` from the lease.
    2. `unlink(lock_path)`.
    3. Re-attempt `create_new(true).write(true).open(&lock_path)` once.
    4. If the retry fails with `EEXIST`, another process won the race;
       return exit `13 session-busy` and do not retry further.
    5. If the retry succeeds, write the new lease JSON, fsync, close,
       remove any previous sibling release marker, emit success JSON,
       and exit `0`.

    The race between `unlink` and retry-create is closed by the same
    kernel atomic create-if-absent guarantee: only one contender can
    create the replacement lockfile.
11. D5 decision: stale means `expires_at < now`. Stale removal is lazy
    on the next acquire attempt. There is no background reaper.
12. D3 decision: default TTL is `300000` ms (5 minutes), minimum is
    `1000` ms, maximum is `1800000` ms (30 minutes). Out-of-range
    `--ttl-ms` is clap usage exit `2`.
13. `resume-handshake` resolves the session and computes the same lock
    and marker paths.
14. If the lockfile exists, read the lease JSON and compare the supplied
    token to the persisted token evidence. If the token mismatches,
    return `16 lock-token-invalid` without altering the lock.
15. If the lockfile exists and the token matches, write the sibling
    release marker `session-<session_id>.released`, fsync the marker,
    unlink the lockfile, fsync the directory when practical, and exit
    `0`. If the lease had already expired, use the same release path and
    include an implementation note/message equivalent to
    `"released expired token"`; this is still exit `0` because the token
    proves ownership of the stale lease.
16. If the lockfile does not exist, read the sibling release marker, if
    present, as idempotency evidence. Same-token marker hash match
    returns `0` with `already_released: true`. Marker hash mismatch
    returns `16 lock-token-invalid`.
17. If neither lockfile nor release marker exists, return
    `17 lock-expired`: the lease is absent and no marker proves a prior
    release for this token.

# 5. Exit codes

## 5.1 `pause-handshake`

| Exit | Error code | Condition |
| --- | --- | --- |
| `0` | none | Lease acquired. |
| `1` | `operational-error` | DB/config/load/serialization/I/O/randomness failure. |
| `2` | `invalid-session-id` or clap usage | Bad UUID, missing args, invalid TTL. |
| `10` | `session-not-found` | Shared resolver cannot find a chain/session. |
| `11` | `ambiguous-session` | Shared resolver returns ambiguous. |
| `13` | `session-busy` | Valid lock exists, or an active writer cannot be proven safe to pause. |

## 5.2 `resume-handshake`

| Exit | Error code | Condition |
| --- | --- | --- |
| `0` | none | Matching token released, or same-token replay accepted. |
| `1` | `operational-error` | DB/config/load/serialization/I/O failure. |
| `2` | `invalid-session-id` or clap usage | Bad UUID or missing `--token`. |
| `10` | `session-not-found` | Shared resolver cannot find a chain/session. |
| `11` | `ambiguous-session` | Shared resolver returns ambiguous. |
| `16` | `lock-token-invalid` | Wrong token for an existing lock or release marker. |
| `17` | `lock-expired` | Lockfile is absent and no release marker proves this token. |

Exit `12`, `14`, and `15` remain reserved for sibling Initiative 06
features and are not used here.

# 6. Reusable lock primitive API

Create:

```text
src-tauri/src/session_lock/
```

Public types:

```rust
pub struct Lease { session_id, token, expires_at, lock_path }
pub struct ReleaseReceipt { session_id, released, already_released, lock_path, release_marker_path, note }
pub enum LockError { Busy, TokenInvalid, LockExpired, Operational }
pub struct SessionLock { root: PathBuf }
```

Core methods:

- `SessionLock::from_default_data_dir()`.
- `lock_path(&self, session_id: &str) -> PathBuf`.
- `release_marker_path(&self, session_id: &str) -> PathBuf`.
- `acquire(&self, session_id, ttl) -> Result<Lease, LockError>`.
- `release(&self, session_id, token) -> Result<ReleaseReceipt, LockError>`.
- `observe(&self, session_id) -> Result<Option<ExistingLockInfo>, LockError>`.

The command layer resolves `session_id`, `chain_id`, and `provider_name`
before calling `SessionLock`; it then combines resolver metadata with
`Lease` or `ReleaseReceipt` for the CLI stdout JSON in §3.

`acquire()` uses only atomic create-if-absent for mutual exclusion:
`OpenOptions::new().create_new(true).write(true).open(&lock_path)`.
On success it writes the lease JSON, fsyncs, closes, removes any stale
sibling release marker for that session, and returns `Lease`. On
`EEXIST` it reads the existing lease. A non-expired lease returns
`LockError::Busy`; an expired lease is unlinked and replaced by one
single retry of the same `create_new` open. If that retry sees `EEXIST`,
another contender won and the result is `LockError::Busy`.

`release()` reads the lockfile when it exists. Matching token evidence
writes the sibling release marker, unlinks the lockfile, and returns
`ReleaseReceipt`, even if the matching lease has already expired; the
receipt note records that an expired token was released. Token mismatch
returns `LockError::TokenInvalid`. If the lockfile is absent, a matching
marker returns idempotent success, a mismatching marker returns
`LockError::TokenInvalid`, and no marker returns
`LockError::LockExpired`.

Lockfile metadata is JSON and versioned:

```json
{
  "version": 1,
  "session_id": "...",
  "token_hash": "sha256:<hex>",
  "created_at": "2026-04-30T12:30:56Z",
  "expires_at": "2026-04-30T12:35:56Z",
  "owner_pid": 12345
}
```

The lockfile stores `token_hash`, not the raw token. The raw token is
printed once to stdout. Release hashes the supplied token for comparison.

Idempotent release evidence is a sibling marker file in the same lock
directory:

```text
<lock_dir>/session-<session_id>.released
```

`acquire()` writes:

```text
<lock_dir>/session-<session_id>.lock
```

`release()` writes:

```text
<lock_dir>/session-<session_id>.released
```

The marker is JSON and versioned:

```json
{
  "version": 1,
  "session_id": "...",
  "chain_id": "...",
  "provider_name": "...",
  "token_hash": "sha256:<hex>",
  "released_at": "2026-04-30T12:33:10Z"
}
```

It contains token evidence (`token_hash`, derived from the release
token) and `released_at`; the raw token is not persisted.

The marker is not an active lock. It only distinguishes same-token retry
from arbitrary missing-lock release. Same-token release replay succeeds
with `already_released: true` when this marker exists and the token hash
matches. Wrong-token release returns `16 lock-token-invalid` when this
marker exists and the token hash differs. Expired lockfile release
succeeds when the supplied token matches the expired lease and returns
`16 lock-token-invalid` when it does not. A missing lockfile with no
marker returns `17 lock-expired`. A fresh acquire removes the old
sibling marker after writing new lock metadata so a previous token cannot
shadow a later lease.

# 7. Anti-scope

- No transcript content mutation or import-replace implementation.
- No provider spawn, signal, suspend, resume, or kill.
- No proof of safety for provider CLIs launched outside agent-runner.
- No global runner lock.
- No DB lock table in v1.
- No strict ambiguity query outside the shared resolver.
- No fallback to raw `session_turns` for segmentless sessions.
- No GUI or frontend lock indicator.
- No quota/auth refresh, provider selection, config edit, or
  `migrate-config` coupling.

D4 decision: choose option (b). This PR adds the primitive and observer
API only. `import-replace`, migration, `run_repl`, `run_resume`, and
balanced one-shot wire observation in their own PRs. The hookpoints are
too scattered to make this primitive PR a clean place for all sibling
behavior.

# 8. Side-effect contract

`pause-handshake` may:

- Create `~/.local/share/oulipoly-agent-runner/locks/`.
- Atomically create `locks/session-<session_id>.lock` with
  `O_CREAT | O_EXCL`.
- Unlink an expired `locks/session-<session_id>.lock` during
  stale-acquire, followed by one atomic retry-create.
- Remove a stale `locks/session-<session_id>.released` marker while
  acquiring a new lock for the same session.
- Open default state/config for shared session resolution.

`resume-handshake` may:

- Remove a matching lockfile, including a matching expired lockfile.
- Create or update
  `locks/session-<session_id>.released` with token evidence
  (`token_hash`) and `released_at` for idempotent release replay.

Neither command may:

- Modify transcripts, `session_turns`, chain/segment ownership, quota
  rows, provider/model/session config, or invocation lifecycle rows.
- Run provider commands, quota scripts, auth refresh commands, migration
  code, or scanner/import logic.
- Backfill or repair chains beyond accepted `StateDb::open` behavior in
  the local codebase.

`agents session pause-handshake` and `resume-handshake` open the state
DB via `StateDb::open_default()` for resolver-only access. Inherent
`StateDb::open` side effects (parent dir creation, WAL enable,
schema-ensure, chain backfill) are accepted, matching 06-locate and
06-export's §8 contracts. No DDL, no row mutation, no
`session_turns`/`session_chains`/`session_chain_segments` writes. Lock
state lives outside the DB at
`<lock_dir>/session-<uuid>.{lock,released}`.

When 06-schema-probe's `StateDb::open_read_only` lands and is mergeable,
switch pause/resume resolver access to read-only open as a follow-up.

Permissions are contract surface. On Unix, new lock directories are
`0700` and lock/marker files are `0600`. Failure to set or verify those
permissions is exit `1`, not a silent downgrade.

# 9. Test-intent track

## 9.1 Test matrix

| Track | Risk | Intended behavior | Level | Fixture/application point | assumption_link | Signal | residual_risk |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Resolver pass-through | Pause may lock the wrong mutable target. | Pause receipt uses resolved active session, chain, and provider. | component + CLI | Temp DB/config from locate fixtures. | A1, A2 | Exit `0`; stdout fields match owner. | Does not re-prove the shared resolver's own correctness beyond fixture coverage. |
| Invalid UUID | Bad input may create lock state before validation. | Bad id fails before state/lock access. | e2e | CLI with invalid id. | A1 | Exit `2`; no lock dir. | Does not constrain clap's exact human-readable usage text. |
| Not found | Missing sessions may collapse into operational failure. | Unknown UUID maps to `10`. | integration | Empty/unrelated temp DB. | A1 | stderr code `session-not-found`. | Covers resolver absence, not every corrupt-state variant. |
| Ambiguous | Ambiguous ownership may silently choose one writer target. | Resolver ambiguity maps to `11`. | component | Two recent candidate chains. | A1 | stderr code `ambiguous-session`. | Synthetic ambiguity may not cover all future resolver ambiguity causes. |
| Atomic acquire | Concurrent harnesses may both receive valid leases. | Two concurrent pause calls grant one token. | process integration | Two CLI processes, same temp data dir. | A4, A5, A7 | One `0`, one `13`. | Does not prove behavior on non-local/network filesystems. |
| Per-session scope | One session lock may over-block unrelated sessions. | Different sessions can be locked concurrently. | component | Two active sessions. | A2, A7 | Two lockfiles; both `0`. | Does not test cross-user shared data dirs, which are out of scope. |
| Token format | Tokens may be predictable or malformed. | Token matches `pause_[0-9a-f]{32}` and differs across acquisitions. | unit | Token generator. | A7 | Regex pass; repeated values differ. | Does not statistically certify OS CSPRNG quality. |
| TTL bounds | Leases may be unbounded or too short to be useful. | Default/min/max policy enforced. | unit + CLI | Parser and injected clock. | A5 | Default 5m; out-of-range exits `2`. | Does not model wall-clock skew between processes. |
| Stale acquire | Crashed harness may block forever. | Expired lockfile is lazily replaced. | component | Prewritten expired metadata. | A5, A7 | New pause exits `0`. | Does not add a background reaper; cleanup remains lazy by design. |
| Busy lock | Active lease may not refuse a second pause. | Non-expired lock blocks second pause. | integration | Pause twice before expiry. | A4, A5, A7 | Second exits `13`. | Does not prove sibling writer paths observe the lock in v1. |
| Correct release | Valid token may fail to release or leave stale lock state. | Matching token releases and writes sibling release marker. | integration | Pause then resume. | A4, A7 | Resume `0`; future pause succeeds; marker path exists before next acquire. | Does not fully simulate crash during the release critical section. |
| Wrong token | Caller may release a lock it does not own. | Mismatch cannot release. | integration | Pause then wrong token. | A7 | Exit `16`; lock remains. | Does not prove token secrecy outside filesystem and stdout handling. |
| Expired matching release | Expired lock ownership may become unreleasable. | Release after expiry succeeds when the token matches the expired lease and writes the marker. | component | Injected clock with expired metadata and matching token. | A5, A7 | Exit `0`; receipt note says released expired token. | Boundary precision at exact `expires_at` is covered only by unit clock tests. |
| Idempotent replay | Harness retry after successful release may fail. | Same-token release retry succeeds through `session-<uuid>.released`. | integration | Pause, release, release again. | A5, A7 | Second `0`, `already_released: true`. | Does not preserve idempotency after a later acquire removes the marker. |
| Missing lock no marker | Absent lease may be mistaken for a valid idempotent release. | Unknown token with no marker returns `17`. | component | No lock/marker. | A7 | stderr code `lock-expired`. | Cannot distinguish manual lock deletion from never-acquired state. |
| Marker token mismatch | Stale or foreign marker may authorize the wrong caller. | Existing marker with different token returns `16`. | component | Prewritten `session-<uuid>.released` with different token hash. | A5, A7 | stderr code `lock-token-invalid`. | Does not prove manual marker tampering is impossible, only that mismatch fails. |
| Permissions | Lock files may leak token evidence or permit tampering. | Lock state is owner-private on Unix. | Unix integration | Inspect mode bits. | A7 | Dir `0700`, files `0600`. | Windows ACL behavior remains implementation discovery. |
| Side effects | Pause/resume may mutate transcripts or DB rows. | Only lock state mutates beyond accepted `StateDb::open` side effects. | integration | Snapshot DB counts/transcript mtimes. | A1, A3, A7 | Unchanged except parent dir/WAL/schema/backfill effects inherent to open. | Cannot enforce future `StateDb::open` internals; read-only open is a follow-up. |
| Writer-path advisory scope | Harness may assume full mutual exclusion before sibling PRs land. | v1 docs/tests state lock primitive is advisory until import-replace/migration/resume/repl observers ship. | doc + integration | README/proposal check plus locked session followed by existing sibling command fixture if available. | A6 | Docs name deferred paths; no v1 test expects sibling command refusal. | Full mutual exclusion remains cross-PR work, not validated in this PR. |
| README truth | Public docs may drift from contract. | Docs match synopsis, JSON, TTL, exits, marker path, and advisory scope. | doc check | README snippets/manual checklist. | A5, A6, A7 | Fields and codes match proposal. | Manual doc checks can miss wording ambiguity. |

Test fixtures should reuse locate/session metadata fixtures once merged.
If this worktree is temporarily unstacked, Phase 6b may provide a
minimal resolver fixture without changing the public design.

# 10. README updates

Update `README.md` near the CLI synopsis and session/resume sections:

- Add both command synopses.
- Document pause and resume receipt fields exactly as §3.
- Document token format, token secrecy, and TTL default/min/max: `300000`, `1000`, `1800000`.
- Document exit codes `0`, `1`, `2`, `10`, `11`, `13`, `16`, `17`.
- Document the sibling release marker
  `session-<uuid>.released` and same-token idempotent replay.
- Document that this is a session lease, not provider process suspension,
  and that v1 creates/removes lock state only. State that sibling
  writer-path enforcement is advisory until 06-import-replace,
  migration, resume/repl, and balanced one-shot wire observers in their
  own PRs.
- Mention `~/.local/share/oulipoly-agent-runner/locks/` next to the
  existing `state.db` persistent-state paragraph.

# 11. Supported-surface track

Deployment mode: local CLI binary only. No GUI command, daemon, server,
or background reaper.

Primary consumer: `agent-harness`, which needs a stable lease before
mid-session transcript override. Secondary consumers are local scripts
that need a refusal-before-write guard around future transcript
operations.

Adjacent paths:

- `agents session locate` remains the canonical metadata surface.
- `agents session export` is read-only and lock-blind in v1.
- Future `agents session import-replace`, `agents resume`,
  `agents repl --resume`, top-level `--resume`, migration, and balanced
  one-shot are future observers.

Migration path: no user state migration beyond on-demand lock directory
creation. Existing sessions are unlocked because no lockfile exists.

Rollback path: remove or stop invoking the subcommands. Existing lockfiles
are inert to older binaries. Operators may delete stale files after
confirming no newer binary is observing them.

Observability: stdout receipts, stderr JSON errors, and lockfiles are the
entire v1 surface. No invocation row, trace event, audit table, or
telemetry is added.

# 12. Implementation residuals

- D1a is file-backed, not DB-backed. It avoids schema migration and
  matches `lock_path`, but depends on local filesystem atomic
  `O_CREAT | O_EXCL` behavior and private filesystem permissions.
  Lockfile metadata is the lease because the CLI exits after printing
  the receipt.
- No active provider process drain is implemented in v1. Existing running
  invocation rows are not sufficient session writer leases.
- D4b leaves sibling writer-path observation to later PRs by design.
  06-import-replace's PR must observe the lock during write attempts and
  fail with `13 session-busy` on conflict. Migration's
  `migrate_chain_segment` retrofit is a follow-up. `run_repl` and
  `run_resume` observe in their own follow-up. Until those sibling PRs
  land, the harness acceptance surface is narrowed: pause-handshake
  acquires and releases the lock primitive, but sibling commands can
  still write unless they have been retrofitted. The harness consumer
  should treat the lock as advisory in v1; full mutual-exclusion arrives
  when 06-import-replace lands.
- Release idempotency uses the concrete sibling marker
  `<lock_dir>/session-<uuid>.released`; there is no future marker-shape
  deferral.
- Physical read-only DB open is inherited until
  06-schema-probe's `StateDb::open_read_only` lands and is mergeable.
  Inherent `StateDb::open` side effects are accepted per §8; no command
  should add DDL or row mutation beyond those open-time effects.
- Windows semantics are not designed; token hashing must use a
  cryptographic hash, not a checksum.
- Balanced one-shot observation needs a future fail-closed point for
  sessions discovered only after provider execution.

# 13. Cross-feature constraint compliance

| Constraint | Compliance | Note |
| --- | --- | --- |
| Shared error namespace uses `10`, `11`, `13`, `16`, `17`. | Yes | §5. |
| Ownership resolution reuses shared locate/resume path. | Yes | §4 steps 1-3. |
| Lock observation by import-replace, migration, repl/resume, balanced one-shot once pause lands. | Partial by design (deferred to sibling PRs) | Cross-PR dependency: 06-import-replace observes during write attempts and exits `13 session-busy`; migration's `migrate_chain_segment`, resume/repl, and balanced one-shot wire observers in follow-ups. v1 harness lock is advisory until those land. |
| Read-only `StateDb` open belongs to schema-probe. | Yes / inherited | §8, §12. |
| No auto-resume. | Yes | §7, §8. |
| No provider spawn/suspension. | Yes | §7, §8. |
| No quota refresh. | Yes | §7, §8. |
| No config edits or migrate-config coupling. | Yes | §7, §8. |
| Creates/removes lock state only. | Yes | §8. |
| Crash recovery via TTL. | Yes | D3/D5 in §4. |
| Harness `lock_path` is stable. | Yes | §3 and §4. |
