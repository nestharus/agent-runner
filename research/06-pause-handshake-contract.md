# Phase 6 Step 6a — Contract for `agents session pause-handshake` / `resume-handshake`

This contract bridges `proposals/06-pause-handshake.md` (Rev 4) and Phase 6
implementation.

## 1. CLI surface

```text
agents session pause-handshake <session-id> [--ttl-ms <ms>]
agents session resume-handshake <session-id> --token <token>
```

Default TTL: 60_000 ms. Maximum: 600_000 ms (10 min). Tokens are 128-bit random hex (`pause_<32hex>`).

Clap shape — extend `SessionSubcommands`:

```rust
enum SessionSubcommands {
    Locate { ... },
    SchemaProbe,
    Export { ... },
    PauseHandshake { session_id: String, #[arg(long)] ttl_ms: Option<u64> },
    ResumeHandshake { session_id: String, #[arg(long)] token: String },
}
```

## 2. Public types (new module `src-tauri/src/session_lock/mod.rs`)

```rust
pub struct Lease {
    pub session_id: String,
    pub provider_name: String,
    pub token: String,
    pub expires_at: String,        // ISO-8601
    pub lock_path: PathBuf,
}

pub struct ReleaseReceipt {
    pub session_id: String,
    pub token: String,
    pub released_at: String,
    pub already_released: bool,
}

pub enum LockError {
    Busy { expires_at: String },
    TokenInvalid,
    LockExpired,
    Operational { message: String },
}

pub struct SessionLock { /* internal: sentinel fd */ }

impl SessionLock {
    pub fn new(lock_dir: &Path) -> std::io::Result<Self>;
    pub fn acquire(&self, session_id: &str, provider_name: &str, ttl: Duration) -> Result<Lease, LockError>;
    pub fn release(&self, session_id: &str, token: &str) -> Result<ReleaseReceipt, LockError>;
}
```

Sentinel: `<lock_dir>/sentinel.lock`, created with `O_CREAT | O_RDWR` (idempotent), never unlinked.

## 3. Algorithm (per Rev 4 §4)

### Acquire

1. Open and `flock(sentinel_fd, LOCK_EX)`.
2. Try `open(<lock_dir>/session-<session_id>.lock, O_RDONLY)`:
   - Exists → read lease JSON.
     - `expires_at > now` → `flock_unlock`; return `Err(Busy {..})`.
     - Else (stale) → step 3.
   - `ENOENT` → step 3.
3. Atomic write:
   a. Generate token; build lease JSON.
   b. Write to `<lock_dir>/session-<session_id>.lock.acquire-<pid>-<uuid>.tmp`.
   c. `fsync`.
   d. `rename(tmp, <lock_dir>/session-<session_id>.lock)` — atomic.
4. `flock_unlock`.
5. Return `Ok(Lease)`.

### Release

1. `flock(sentinel_fd, LOCK_EX)`.
2. Try open session_lock_path:
   - Exists → read lease.
     - Token match → write released marker JSON (tempfile + rename) → unlink session_lock_path → `flock_unlock`; return `Ok(ReleaseReceipt { already_released: false })`.
     - Token mismatch → `flock_unlock`; return `Err(TokenInvalid)`.
   - `ENOENT` → try read released marker.
     - Same token → `flock_unlock`; return `Ok(ReleaseReceipt { already_released: true })`.
     - Different token → `flock_unlock`; return `Err(TokenInvalid)`.
     - Missing → `flock_unlock`; return `Err(LockExpired)`.

## 4. JSON outputs

### Pause success (stdout)

```json
{"session_id":"...","chain_id":"...","provider_name":"...","token":"pause_...","expires_at":"...","lock_path":"/abs/.../session-uuid.lock"}
```

### Resume success (stdout)

```json
{"session_id":"...","token":"...","released_at":"...","already_released":false}
```

### Errors (stderr)

```json
{"error":{"code":"<code>","message":"..."}}
```

## 5. Exit codes

| Exit | Trigger |
|---|---|
| 0 | Acquire/release success |
| 1 | Operational error |
| 2 | Clap usage / invalid session-id |
| 10 | session-not-found |
| 11 | ambiguous-session |
| 12 | model-resolution-failed |
| 13 | session-busy (acquire while held) |
| 16 | lock-token-invalid |
| 17 | lock-expired |

## 6. CLI wrapper resolution flow

For pause-handshake:
1. Parse session-id; exit 2 on bad UUID.
2. `StateDb::open_default()`.
3. `StateDb::resolve_resume`. Errors → 10/11/12/1.
4. `SessionLock::new(<state-data-dir>/locks/)`.
5. `acquire(session_id, provider_name, ttl)`.
6. On Ok → emit JSON; exit 0. On Err → exit per §5.

For resume-handshake:
1. Parse session-id; exit 2.
2. `StateDb::open_default()`.
3. `SessionLock::new(...)`.
4. `release(session_id, token)`.
5. On Ok → emit JSON; exit 0. On Err → exit per §5.

## 7. Side-effect contract

Permitted:
- Read state DB (resolver-only access).
- Create `lock_dir` (idempotent mkdir).
- Create `sentinel.lock` (idempotent O_CREAT).
- Acquire/release: write/rename/unlink session-specific lock and released marker files under sentinel flock.

Forbidden:
- INSERT/UPDATE/DELETE on any session table.
- Provider commands, quota refresh, auth.
- Migration, scan.
- Telemetry, invocation rows.

`StateDb::open_default()` open-time effects (parent dir creation, WAL, schema-ensure, chain backfill) are accepted matching 06-locate / 06-export's §8.

## 8. Test-intent track

Per proposal §9.1. T1-T11. Each carries risk annotation per Rust doc comment.

Critical concurrency tests (load-bearing):
- T-concurrent-pause: spawn N subprocesses calling pause-handshake on same session_id simultaneously; assert exactly one returns 0 and N-1 return 13.
- T-concurrent-stale: pre-create stale lockfile; spawn N subprocesses; assert exactly one wins eviction.
- T-pause-release-cycle: pause → release → pause again succeeds.
- T-release-wrong-token: exit 16.
- T-release-after-expiry-no-marker: exit 17.
- T-release-after-expiry-with-marker: exit 0 (already_released: true).
- T-ttl-respected: lease expires after TTL.
- T-token-invalid-format: exit 16.
- T-resolver-error-mapping: 10/11/12.

These MUST use `std::process::Command` to spawn the binary, not in-process calls. The contract is multi-process mutual exclusion.

## 9. Process-tree audit

Step 6b and Step 6c are separate agent invocations. Step 6c writes `.tmp/phase6/step6c-reads.md` BEFORE product-code change.

Output index at `.tmp/phase6/step6b-output-index.md` with all required Phase 6 provenance fields (proposal, contract, problem-map, supported-surface, hookpoints, prompt path, log path; per-row T-id, named risk, level, source, file path, identifier, residual, fixture source).

## 10. References

- Proposal: `proposals/06-pause-handshake.md` (Rev 4).
- Hookpoints: `research/06-pause-handshake-hookpoints.md`.
- 06-locate / 06-schema-probe / 06-export contracts.
