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
  those paths will use later.

## 1.1 Assumption register

| ID | Assumption | Evidence | Invalidator | Used by |
| --- | --- | --- | --- | --- |
| A1 | Pause/resume uses the same owner semantics as locate, ultimately backed by `StateDb::resolve_resume`. | Harness says pause resolves through locate; Initiative 06 forbids a second ownership path. | Locate changes ownership semantics before this lands. | §4, §5, §6 |
| A2 | The lock key is the resolved active provider session id, with chain/provider metadata retained in receipts. | `ResolvedResume` exposes chain id and active provider/session; harness response names `session_id` and `provider_name`. | A prior feature introduces a distinct mutable-target key. | §3, §4, §6 |
| A3 | Existing `running` invocation rows are not a safe active-writer lock. | They are not session-scoped leases, can survive hard crash, and lack token/TTL semantics. | Invocation lifecycle gains durable session writer leases first. | §4, §7, §12 |
| A4 | The lease must outlive the `pause-handshake` process. | The command returns a token and exits; an fd-held `flock` would release on exit. | Harness changes to a long-lived pause process. | D1, §6, §12 |
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
  "lock_path": "/home/me/.local/share/oulipoly-agent-runner/locks/session-9e69e8cc-616d-4640-bf1d-96f5391b1a2e.lock"
}
```

`already_released` is `true` only for idempotent same-token replay.
`released` is always `true` on exit `0`.

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
4. Compute lock path from the resolved active provider session id:

   ```text
   <data_dir>/oulipoly-agent-runner/locks/session-<session_id>.lock
   ```

5. Create the lock directory with owner-private permissions where Unix
   mode bits are available: directory `0700`, files `0600`.
6. D1 decision: choose D1a, lockfile-backed lease, with clarification.
   The durable lease is lockfile metadata. POSIX `flock` is held only
   around acquire/release/read critical sections; it is not the entire
   returned lease because the pause process exits after printing JSON.
   This preserves the harness `lock_path` contract and avoids a DB
   schema migration.
7. `pause-handshake` opens/creates the lockfile and takes exclusive
   `flock`.
8. If metadata is valid and `now <= expires_at`, return exit
   `13 session-busy`. If no metadata exists, acquire. If metadata is
   stale, remove/truncate under the same flock and acquire.
9. D5 decision: stale means `now > expires_at`. Stale removal is lazy on
   the next acquire attempt. There is no background reaper.
10. D3 decision: default TTL is `300000` ms (5 minutes), minimum is
    `1000` ms, maximum is `1800000` ms (30 minutes). Out-of-range
    `--ttl-ms` is clap usage exit `2`.
11. Generate the token, write metadata atomically, fsync the file, and
    release the flock.
12. `resume-handshake` resolves the session, computes the same path, and
    takes exclusive `flock`.
13. If the lockfile is missing, accept only an idempotent same-token
    release proven by the release marker in §6; otherwise return
    `16 lock-token-invalid`.
14. If metadata exists and `now > expires_at`, return
    `17 lock-expired`. The command may remove stale state after
    classification.
15. If metadata exists and token mismatches, return
    `16 lock-token-invalid` without altering the lock.
16. If metadata exists and token matches, remove the lockfile or replace
    it with a release marker, fsync the directory when practical, and
    exit `0`.

# 5. Exit codes

## 5.1 `pause-handshake`

| Exit | Error code | Condition |
| --- | --- | --- |
| `0` | none | Lease acquired. |
| `1` | `operational-error` | DB/config/load/serialization/I/O/flock/randomness failure. |
| `2` | `invalid-session-id` or clap usage | Bad UUID, missing args, invalid TTL. |
| `10` | `session-not-found` | Shared resolver cannot find a chain/session. |
| `11` | `ambiguous-session` | Shared resolver returns ambiguous. |
| `13` | `session-busy` | Valid lock exists, or an active writer cannot be proven safe to pause. |

## 5.2 `resume-handshake`

| Exit | Error code | Condition |
| --- | --- | --- |
| `0` | none | Matching token released, or same-token replay accepted. |
| `1` | `operational-error` | DB/config/load/serialization/I/O/flock failure. |
| `2` | `invalid-session-id` or clap usage | Bad UUID or missing `--token`. |
| `10` | `session-not-found` | Shared resolver cannot find a chain/session. |
| `11` | `ambiguous-session` | Shared resolver returns ambiguous. |
| `16` | `lock-token-invalid` | Wrong token, or no marker proves this token. |
| `17` | `lock-expired` | Matching lock exists but `now > expires_at`. |

Exit `12`, `14`, and `15` remain reserved for sibling Initiative 06
features and are not used here.

# 6. Reusable lock primitive API

Create:

```text
src-tauri/src/session_lock/
```

Public types:

```rust
pub struct SessionLockTarget { session_id, chain_id, provider_name }
pub struct SessionLockReceipt { session_id, chain_id, provider_name, token, expires_at, lock_path }
pub struct SessionLockRelease { session_id, chain_id, provider_name, released, already_released, lock_path }
pub enum SessionLockError { Busy, TokenInvalid, Expired, Operational }
pub struct SessionLockManager { root: PathBuf }
```

Core methods:

- `SessionLockManager::from_default_data_dir()`.
- `lock_path(&self, session_id: &str) -> PathBuf`.
- `acquire(&self, target, ttl) -> Result<SessionLockReceipt, SessionLockError>`.
- `release(&self, target, token) -> Result<SessionLockRelease, SessionLockError>`.
- `observe(&self, target) -> Result<Option<ExistingLockInfo>, SessionLockError>`.

Lockfile metadata is JSON and versioned:

```json
{
  "version": 1,
  "session_id": "...",
  "chain_id": "...",
  "provider_name": "...",
  "token_hash": "sha256:<hex>",
  "created_at": "2026-04-30T12:30:56Z",
  "expires_at": "2026-04-30T12:35:56Z",
  "owner_pid": 12345
}
```

The lockfile stores `token_hash`, not the raw token. The raw token is
printed once to stdout. Release hashes the supplied token for comparison.

Idempotent release requires short-lived release evidence. Phase 5 chooses
one shape:

- Replace the lockfile with a release marker containing `released_at`
  and `token_hash`, overwritten by the next acquire.
- Write a sibling marker under `locks/releases/session-<uuid>.json`.

The marker is not an active lock. It only distinguishes same-token retry
from arbitrary missing-lock release.

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
- Create, replace, or remove `locks/session-<session_id>.lock`.
- Create or update same-token release marker state.
- Open default state/config for shared session resolution.

`resume-handshake` may:

- Remove an active matching lockfile.
- Replace an active lockfile with a release marker.
- Remove stale lockfiles after classifying `lock-expired`, if Phase 5
  chooses cleanup-on-expired-release.

Neither command may:

- Modify transcripts, `session_turns`, chain/segment ownership, quota
  rows, provider/model/session config, or invocation lifecycle rows.
- Run provider commands, quota scripts, auth refresh commands, migration
  code, or scanner/import logic.
- Backfill or repair chains beyond unavoidable existing `StateDb::open`
  behavior in the local codebase.

Permissions are contract surface. On Unix, new lock directories are
`0700` and lock/marker files are `0600`. Failure to set or verify those
permissions is exit `1`, not a silent downgrade.

# 9. Test-intent track

| Risk | Intended behavior | Level | Fixture/application point | Signal |
| --- | --- | --- | --- | --- |
| Resolver pass-through | Pause receipt uses resolved active session, chain, and provider. | component + CLI | Temp DB/config from locate fixtures. | Exit `0`; stdout fields match owner. |
| Invalid UUID | Bad id fails before state/lock access. | e2e | CLI with invalid id. | Exit `2`; no lock dir. |
| Not found | Unknown UUID maps to `10`. | integration | Empty/unrelated temp DB. | stderr code `session-not-found`. |
| Ambiguous | Resolver ambiguity maps to `11`. | component | Two recent candidate chains. | stderr code `ambiguous-session`. |
| Atomic acquire | Two concurrent pause calls grant one token. | process integration | Two CLI processes, same temp data dir. | One `0`, one `13`. |
| Per-session scope | Different sessions can be locked concurrently. | component | Two active sessions. | Two lockfiles; both `0`. |
| Token format | Token matches `pause_[0-9a-f]{32}` and differs across acquisitions. | unit | Token generator. | Regex pass; repeated values differ. |
| TTL bounds | Default/min/max policy enforced. | unit + CLI | Parser and injected clock. | Default 5m; out-of-range exits `2`. |
| Stale acquire | Expired lockfile is lazily replaced. | component | Prewritten expired metadata. | New pause exits `0`. |
| Busy lock | Non-expired lock blocks second pause. | integration | Pause twice before expiry. | Second exits `13`. |
| Correct release | Matching token releases. | integration | Pause then resume. | Resume `0`; future pause succeeds. |
| Wrong token | Mismatch cannot release. | integration | Pause then wrong token. | Exit `16`; lock remains. |
| Expired release | Release after expiry returns `17`. | component | Injected clock. | stderr code `lock-expired`. |
| Idempotent replay | Same-token release retry succeeds. | integration | Pause, release, release again. | Second `0`, `already_released: true`. |
| Missing lock wrong token | Unknown token with no marker returns `16`. | component | No lock/marker. | stderr code `lock-token-invalid`. |
| Permissions | Lock state is owner-private on Unix. | Unix integration | Inspect mode bits. | Dir `0700`, files `0600`. |
| Side effects | Only lock state mutates. | integration | Snapshot DB counts/transcript mtimes. | Unchanged except existing open effects. |
| README truth | Docs match synopsis, JSON, TTL, exits. | doc check | README snippets/manual checklist. | Fields and codes match proposal. |

Test fixtures should reuse locate/session metadata fixtures once merged.
If this worktree is temporarily unstacked, Phase 6b may provide a
minimal resolver fixture without changing the public design.

# 10. README updates

Update `README.md` near the CLI synopsis and session/resume sections:

- Add both command synopses.
- Document pause and resume receipt fields exactly as §3.
- Document token format, token secrecy, and TTL default/min/max: `300000`, `1000`, `1800000`.
- Document exit codes `0`, `1`, `2`, `10`, `11`, `13`, `16`, `17`.
- Document that this is a session lease, not provider process suspension,
  and that v1 creates/removes lock state only.
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
  matches `lock_path`, but depends on POSIX advisory locking and private
  filesystem permissions. A pure fd-held `flock` cannot be the lease
  because the CLI exits; lockfile metadata is the lease.
- No active provider process drain is implemented in v1. Existing running
  invocation rows are not sufficient session writer leases.
- D4b leaves sibling write-path observation to later PRs; release
  idempotency needs a marker policy selected in Phase 5.
- Physical read-only DB open is inherited; Windows semantics are not
  designed; token hashing must use a cryptographic hash, not a checksum.
- Balanced one-shot observation needs a future fail-closed point for
  sessions discovered only after provider execution.

# 13. Cross-feature constraint compliance

| Constraint | Compliance | Note |
| --- | --- | --- |
| Shared error namespace uses `10`, `11`, `13`, `16`, `17`. | Yes | §5. |
| Ownership resolution reuses shared locate/resume path. | Yes | §4 steps 1-3. |
| Lock observation by import-replace, migration, repl/resume, balanced one-shot once pause lands. | Partial by design | D4b: primitive/API here, sibling PRs wire observers. |
| Read-only `StateDb` open belongs to schema-probe. | Yes / inherited | §8, §12. |
| No auto-resume. | Yes | §7, §8. |
| No provider spawn/suspension. | Yes | §7, §8. |
| No quota refresh. | Yes | §7, §8. |
| No config edits or migrate-config coupling. | Yes | §7, §8. |
| Creates/removes lock state only. | Yes | §8. |
| Crash recovery via TTL. | Yes | D3/D5 in §4. |
| Harness `lock_path` is stable. | Yes | §3 and §4. |
