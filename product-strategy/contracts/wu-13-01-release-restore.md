# Contract — WU-13-01 release-restore

Owner: implementation-pipeline-orchestrator (Phase 6a; orchestrator-authored)
Source:
- `proposals/13-release-restore.md` (revised, Phase 4 LOW)
- `research/13-release-restore-problem-map.md`
- `research/13-release-restore-hookpoints.md`
- `tmp/scratch/wu-13-01/ticket.md`
Inputs to Step 6b (test writer) and Step 6c (code writer).

This contract is the orchestrator's interface between the test agent
(Step 6b) and the code agent (Step 6c). The test agent does NOT see
the code agent's output. The code agent reads this contract, the
proposal, the hookpoints, the problem map, and the Step 6b output
index — and only then writes product code.

---

## 1. Acceptance criteria (from ticket)

- **AC-1**: `.github/workflows/release.yml` build matrix includes a
  `windows-latest` row with target `x86_64-pc-windows-msvc` and bundle
  list `msi,nsis`. Restore the "Collect artifacts (Windows)" step.
- **AC-2**: `cargo check --target x86_64-pc-windows-msvc` succeeds on
  CI for the workspace `src-tauri` crate set; `session_lock` and
  `session_replace` compile without `nix::fcntl::flock` / `AsRawFd` /
  `OpenOptionsExt` errors.
- **AC-3**: A new portable `SessionLock` integration test compiles and
  runs on both Unix and Windows targets, exercising single-process
  Busy detection and a cross-process exclusivity check via a sibling
  helper process. Existing Unix tests still pass.
- **AC-4**: `session_replace::import_replace` retains its atomicity
  contract on all platforms. Existing
  `src-tauri/tests/initiative_06_*` tests stay green on Linux/macOS.
  Windows AC-4 evidence is **build-only**: `cd src-tauri && cargo
  check --target x86_64-pc-windows-msvc --tests` succeeds.
- **AC-5**: `release.yml` upload step produces target-distinguishable
  bare-binary artifacts (`-${{ matrix.target }}` suffix; `.exe` on
  Windows). A new structural Rust integration test parses the YAML
  and asserts the contract.
- **AC-6**: A trial release run via `workflow_dispatch` against a
  pre-release tag publishes Linux, macOS, and Windows binaries to the
  GitHub release. AC-6 evidence record fields: `workflow_run_url`,
  `workflow_run_id`, `release_url`, release tag,
  `asset_filename_inventory`, `linux_bare_binary_sha256`,
  `macos_bare_binary_sha256`, `windows_bare_binary_sha256`,
  `matrix_artifacts_listing`, `windows_bundle_filenames`. Documented
  as a residual until release infrastructure is exercised.
- **AC-7**: Existing CI gates remain green:
  - Rust: `cd src-tauri && cargo fmt --check && cargo clippy -- -D warnings && cargo test --no-fail-fast`
  - Frontend (regression): `bun run check && bunx tsc --noEmit && bun run test`
- **AC-8**: `DECISIONS.md` D-006 is rewritten to describe Windows as
  **supported**, calling out the Windows-default-ACL choice
  explicitly. The "Unix-only by design" framing is removed.

## 2. Code surfaces (in-scope)

- `src-tauri/src/session_lock/mod.rs` — replace POSIX-only `flock`
  primitive with `fs4::FileExt::lock`/`unlock`. Drop `nix::fcntl::*`
  and `std::os::fd::AsRawFd` imports. Move `OpenOptionsExt` /
  `PermissionsExt` calls into `cfg(unix)` blocks.
- `src-tauri/Cargo.toml` — add the `fs4` dependency; remove `nix`
  if no longer used outside the workspace. (Keep `nix` if other
  modules still reference it; check.)
- `src-tauri/tests/session_lock_cross_platform.rs` (new) — portable
  AC-3 integration test, including the cross-process sibling helper.
- `src-tauri/tests/release_yml_contract.rs` (new) — structural AC-5
  test; uses `serde_yml` (or `serde_yaml` if already present).
- `.github/workflows/release.yml` — add Windows matrix row, restore
  Windows collect step, suffix bare-binary names at collect-time on
  Linux + macOS + Windows. Keep bundle (`.deb`, `.dmg`, `.msi`,
  NSIS `.exe`) names conventional.
- `DECISIONS.md` — rewrite D-006 per AC-8.

## 3. Code surfaces (anti-scope; do NOT touch)

- `src-tauri/src/balancer/`, `src-tauri/src/quota/`,
  `src-tauri/src/state/db.rs` — #36 routing-fanout territory.
- `src-tauri/src/session_export/`, body-storage in
  `session_metadata/` — deferred WU.
- `src-tauri/src/session_metadata/mod.rs` `path_hash`
  decomposition — Phase 5 hookpoint research § 3 confirmed that the
  current implementation is `Path::components()`-based and already
  cross-platform safe; do NOT touch unless Phase 6c discovers a
  concrete failure.
- `src/` (frontend), `e2e/`, `playwright.config.ts`.
- Routing-fanout reproduction harnesses
  (`src-tauri/tests/routing_fanout_rca/`).
- No backwards-compatibility shim for the old POSIX-only API.

## 4. Schemas, signatures, and constants

### Public API to preserve (no changes)

```rust
pub struct Lease {
    pub session_id: String,
    pub provider_name: String,
    pub token: String,
    pub expires_at: String,
    pub lock_path: PathBuf,
}

pub struct ReleaseReceipt {
    pub session_id: String,
    pub token: String,
    pub released_at: String,
    pub already_released: bool,
}

pub enum LockError {
    Busy { expires_at: String, token_hash: Option<String> },
    TokenInvalid,
    LockExpired,
    Operational { message: String },
}

pub struct SessionLock { /* private fields */ }

impl SessionLock {
    pub fn new(lock_dir: impl AsRef<Path>) -> Result<Self, LockError>;
    pub fn acquire(&self, /* ... existing args ... */) -> Result<Lease, LockError>;
    pub fn release(&self, /* ... existing args ... */) -> Result<ReleaseReceipt, LockError>;
}

pub fn any_active_for_session(lock_dir: &Path, session_id: &str) -> Result<bool, LockError>;
```

The exact `acquire` / `release` argument lists are preserved as-is
(see `src-tauri/src/session_lock/mod.rs:86-221`). Step 6c reads the
existing signatures verbatim and does not change them.

### Private helper — locking primitive

The current private helper `with_flock` calls
`flock(self.sentinel.as_raw_fd(), FlockArg::LockExclusive)`,
runs a closure, then `flock(..., FlockArg::Unlock)`. The replacement:

```rust
fn with_lock<T>(&self, f: impl FnOnce() -> Result<T, LockError>) -> Result<T, LockError> {
    use fs4::fs_std::FileExt;
    FileExt::lock(&self.sentinel)
        .map_err(|e| LockError::Operational { message: format!("acquire sentinel lock: {e}") })?;
    let result = f();
    let unlock_err = FileExt::unlock(&self.sentinel)
        .err()
        .map(|e| LockError::Operational { message: format!("release sentinel lock: {e}") });
    match (result, unlock_err) {
        (Ok(t), None) => Ok(t),
        (Ok(_), Some(e)) => Err(e),
        (Err(e), _) => Err(e),
    }
}
```

Notes for Step 6c:

- `fs4` v1+ exposes `FileExt` via the `fs_std::FileExt` module path
  for `std::fs::File`. If the cargo registry's `fs4` exports
  `FileExt` directly under the crate root, prefer that path.
- Use blocking `lock`, NOT `try_lock`. The current helper is
  blocking; preserving blocking behavior keeps the lease-metadata
  contention model intact.
- Keep the same result-vs-unlock error precedence as the original:
  the closure's error wins; the unlock error replaces a successful
  result.

### File-mode permissions

- Unix: keep `OpenOptionsExt::mode(0o600)` for the sentinel file
  open (current behavior at `session_lock/mod.rs:99,305`) and
  `PermissionsExt::from_mode(0o700)` for the lock directory create.
- Windows: do NOT add explicit DACL construction. Rely on Windows
  default per-user ACL inheritance for the user's app-data location.
- Use `cfg(unix)` blocks for the existing mode calls; do NOT
  introduce `cfg(windows)` branches that set ACLs.

### Cargo.toml

Add (under `[dependencies]`, unconditional):

```toml
fs4 = { version = "0.13", default-features = false, features = ["sync"] }
```

The exact version pinned by Step 6c must satisfy the project's MSRV
(currently `rustc 1.92`; `fs4` MSRV is `1.75`). If `0.13` is not
the latest at implementation time, Step 6c may pin to the latest
stable. `default-features = false, features = ["sync"]` keeps the
dep tree minimal; do NOT enable `tokio` / `async-std` features.

If `nix` is no longer referenced outside removed code paths, remove
it from `[target.'cfg(unix)'.dependencies]` (or wherever it
currently lives). If other modules still need `nix`, keep the dep
but drop the `fcntl` feature if previously enabled.

### `session_replace` Windows behavior

Per Phase 5 hookpoint research § 2 and proposal A3: the four
`std::fs::rename` call sites all operate within the project data
directory (same volume), and `std::fs::hard_link` calls are
restricted to NTFS-supported scenarios. Step 6c does NOT need to
change `session_replace` source for AC-4. AC-4 closure is build-only
on Windows.

If Step 6c discovers a `cfg(unix)` import or trait method in
`session_replace` that breaks the Windows build, the minimal fix is
to gate that import with `#[cfg(unix)]`. Do NOT introduce a
`cfg(windows)` branch with new behavior.

## 5. Test boundary (Step 6b)

### Test file 1 — `src-tauri/tests/session_lock_cross_platform.rs`

Portable integration test for `SessionLock`. Compiles on both Unix
and Windows targets. Uses `tempfile` for isolated lock-dir scratch.

Required test cases:

- `test_single_process_busy` — one `SessionLock`, one `acquire`,
  then a second `acquire` for the same `session_id` BEFORE
  releasing. Asserts the second call returns `LockError::Busy {
  expires_at, token_hash: Some(_) }`. Then `release` with the
  returned token; asserts a third `acquire` succeeds. Compiles and
  runs on both Unix and Windows.
- `test_release_idempotency` — releasing twice with the same token
  returns `ReleaseReceipt { already_released: true, .. }` on the
  second call. Releasing with an invalid token returns
  `LockError::TokenInvalid`. Compiles and runs on both platforms.
- `test_cross_process_exclusivity` — spawns a sibling helper (a
  bin under `src-tauri/tests/bin/` or via `std::process::Command`
  pointing at an `examples/` binary, OR via a child Cargo test
  binary using `escargot` if simpler). The sibling acquires the
  lease, writes a "ready" sentinel file, and waits on a "release"
  sentinel file. The parent waits for "ready", attempts
  `SessionLock::acquire` for the same session, and asserts
  `LockError::Busy`. Then writes the "release" sentinel; sibling
  releases and exits; parent acquires successfully. Compiles and
  runs on both platforms.
  - Implementation guidance: use `std::env::current_exe()` +
    `std::process::Command` to spawn a helper. The simplest shape
    is a separate `[[bin]]` entry in `Cargo.toml` under
    `src-tauri/` (or use `examples/`). Step 6b decides the helper
    location and documents it in the output index.
  - Use timeouts (e.g. `Duration::from_secs(30)`) on the wait
    loops to avoid CI hangs.
- `test_acquire_after_release_succeeds` — basic happy path that
  must not regress.

The test file must NOT use `#![cfg(unix)]`. Per-test `#[cfg]` gating
is allowed only if a specific assertion truly cannot run on a
platform; in that case the test must still compile on both platforms.

### Test file 2 — `src-tauri/tests/release_yml_contract.rs`

Structural test for `.github/workflows/release.yml`. Parses the YAML
and asserts the invariants below.

YAML loader: prefer `serde_yml` (active fork of `serde_yaml`). If
already a workspace dep, reuse; otherwise add `serde_yml = "0.0"` to
`[dev-dependencies]` in `src-tauri/Cargo.toml`. Step 6b decides the
exact crate (`serde_yml` or `serde_yaml`) and pins it.

YAML invariants the test MUST assert (one assertion per bullet):

- `jobs.build.strategy.matrix.include` is an array of length **3**.
- The 3 entries (in any order) match exactly:
  - `{ os: "ubuntu-latest", target: "x86_64-unknown-linux-gnu", bundles: "deb" }`
  - `{ os: "macos-latest", target: "aarch64-apple-darwin", bundles: "dmg" }`
  - `{ os: "windows-latest", target: "x86_64-pc-windows-msvc", bundles: "msi,nsis" }`
- `jobs.build.steps` contains a step with `name: "Collect artifacts (Linux)"`
  whose `if` is `"runner.os == 'Linux'"` and whose `run` block
  contains the substring `oulipoly-agent-runner-${{ matrix.target }}`
  (no `.exe` suffix).
- `jobs.build.steps` contains a step with `name: "Collect artifacts (macOS)"`
  whose `if` is `"runner.os == 'macOS'"` and whose `run` block
  contains the substring `oulipoly-agent-runner-${{ matrix.target }}`.
- `jobs.build.steps` contains a step with `name: "Collect artifacts (Windows)"`
  whose `if` is `"runner.os == 'Windows'"` and whose `run` block
  contains the substring `oulipoly-agent-runner-${{ matrix.target }}.exe`.
- `jobs.build.steps` contains a step `uses: actions/upload-artifact@v4`
  with `with.name: "${{ matrix.target }}"` and `with.path` matching
  `artifacts/*` (or `artifacts`).
- `jobs.release.steps` contains a step `uses: actions/download-artifact@v4`
  with `with.merge-multiple: true` and `with.path: "artifacts"`.
- `jobs.release.steps` contains a step `uses: softprops/action-gh-release@v2`
  with `with.files: "artifacts/*"`.
- The bare-binary token `oulipoly-agent-runner-${{ matrix.target }}`
  appears in the Linux, macOS, AND Windows collect steps and nowhere
  else (i.e., bundles `.deb`/`.dmg`/`.msi` are not target-suffixed).

The test must read `release.yml` from the repo root via a relative
path (`../.github/workflows/release.yml` from `src-tauri/`). It
must fail loudly with the offending field if any assertion fails.

### Step 6b output-index requirements

Step 6b MUST produce
`tmp/scratch/wu-13-01/phase6/step6b-output-index.md` containing:

- absolute paths to every test file written
- a per-test list of asserted invariants (one bullet per `assert!`)
- the exact `Cargo.toml` `[dev-dependencies]` additions, if any
- the exact path to the cross-process helper binary (and its
  declaration shape: `[[bin]]` in `Cargo.toml`, or `examples/`,
  or `tests/bin/` discovery)
- a "Step 6c MUST consume" checklist that names every file path
  Step 6c needs to read or modify
- a list of risks the tests could not encode (e.g., real
  cross-platform CI smoke run; that's an AC-6 concern, not
  Step 6b's)

Step 6b MUST NOT touch product code (`src-tauri/src/**/*.rs`),
`Cargo.toml` `[dependencies]` block, or `release.yml`. Step 6b MAY
add `[dev-dependencies]` (test-only) and the new test files.
Step 6b MAY add a small `[[bin]]` test helper under `src-tauri/`
ONLY IF the helper is purely for the cross-process exclusivity test;
document this clearly in the output index.

If a named risk cannot be verified by tests (e.g., the AC-6
release-pipeline trial run), Step 6b writes
`risk/13-release-restore-test-residuals.md` listing each unverified
risk with the residual mitigation.

## 6. Code boundary (Step 6c)

Step 6c reads, in order:

1. `tmp/scratch/wu-13-01/phase6/step6b-output-index.md`
2. The Step 6b test files referenced in the index
3. This contract
4. `proposals/13-release-restore.md`
5. `research/13-release-restore-hookpoints.md`
6. `research/13-release-restore-problem-map.md`

Step 6c then:

- Replaces `with_flock` with the `fs4`-based helper per § 4 above.
- Removes `nix::fcntl::*` and `AsRawFd` imports from `session_lock`.
- Gates `OpenOptionsExt` / `PermissionsExt` imports + calls under
  `#[cfg(unix)]` blocks.
- Adds `fs4` to `src-tauri/Cargo.toml` `[dependencies]`.
- Edits `.github/workflows/release.yml`:
  - Adds the Windows matrix row.
  - Restores the "Collect artifacts (Windows)" step using PowerShell
    `Copy-Item` to write `artifacts\oulipoly-agent-runner-${{ matrix.target }}.exe`.
  - Modifies the existing Linux + macOS collect steps to suffix the
    bare binary as `oulipoly-agent-runner-${{ matrix.target }}`.
  - Removes the "Windows is intentionally absent" comment block in
    the build-job header.
  - Confirms `dtolnay/rust-toolchain@stable` keeps `targets:
    ${{ matrix.target }}` (it should already; no change).
- Rewrites `DECISIONS.md` D-006 per AC-8.

After implementation, Step 6c runs:

- `cd src-tauri && cargo fmt --check`
- `cd src-tauri && cargo clippy -- -D warnings`
- `cd src-tauri && cargo test --no-fail-fast`
- (regression-only frontend) `bun run check && bunx tsc --noEmit && bun run test`

If any gate fails, Step 6c is re-dispatched with the failure output.
The test agent is NOT re-dispatched on a code-side failure.

## 7. Observable signals (success criteria for joins)

- `tests/session_lock_cross_platform::test_single_process_busy` passes on Unix.
- `tests/session_lock_cross_platform::test_cross_process_exclusivity` passes on Unix.
- `tests/release_yml_contract::*` passes on Unix.
- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test --no-fail-fast` green on Unix.
- Frontend regression gates green.
- Windows AC-2 / AC-4: deferred to CI; AC-6 trial release attaches
  the per-platform `_sha256` evidence record.

## 8. Risk annotations

- **R1** (carries from supported-surface gate): Windows ACL choice is
  default-inherited. Documented in D-006 rewrite. No code-side
  defense.
- **R2**: `fs4` MSRV / dep-tree growth. Closed by the `default-features
  = false, features = ["sync"]` shape; verify on `cargo test`.
- **R3**: cross-process helper bin discovery on CI. Mitigation: use
  `std::env::current_exe()` introspection or a dedicated
  `[[bin]]`/`examples/` declaration documented in the Step 6b
  output index.
- **R4**: trial-release evidence record (AC-6) is captured outside
  this WU's automated gates. Documented residual until release
  infrastructure is exercised.
