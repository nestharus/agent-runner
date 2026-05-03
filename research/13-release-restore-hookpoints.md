# WU-13-01 Release Restore Hookpoint Report

Phase: 5 hookpoint research
Work unit: `release-restore`
Worktree: `/home/nes/projects/agent-runner/worktrees/impl-wu-13-01`
Date: 2026-05-03

## Input Status

1. Read revised proposal `proposals/13-release-restore.md`; it selects `fs4`,
   default Windows ACL inheritance, collect-time release binary renaming, and
   no rename wrapper. `proposals/13-release-restore.md:81-131`,
   `proposals/13-release-restore.md:148-197`,
   `proposals/13-release-restore.md:199-252`,
   `proposals/13-release-restore.md:254-292`.
2. Read problem map `research/13-release-restore-problem-map.md`; it identifies
   `session_lock`, `session_replace`, `session_metadata`, Cargo, release YAML,
   D-006, and new `src-tauri/tests/` as the target surface.
   `research/13-release-restore-problem-map.md:10-31`,
   `research/13-release-restore-problem-map.md:41-63`,
   `research/13-release-restore-problem-map.md:65-103`,
   `research/13-release-restore-problem-map.md:104-145`.
3. Read all Phase 4 reports; all verdicts are LOW. `risk/13-release-restore-audit.md:1-6`,
   `risk/13-release-restore-scope.md:12-16`,
   `risk/13-release-restore-shortcut.md:14-18`,
   `risk/13-release-restore-supported-surface.md:9-14`.
4. The worktree-local `tmp/scratch/wu-13-01/ticket.md` is absent; the trunk ticket
   exists at `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md`.
   The ticket defines the in-scope files and anti-scope. `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:134-160`,
   `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:162-196`.
5. Read current `session_lock`, `session_replace`, `session_metadata`, Cargo, and
   release workflow sources. `src-tauri/src/session_lock/mod.rs:1-400`,
   `src-tauri/src/session_replace/mod.rs:1-1176`,
   `src-tauri/src/session_metadata/mod.rs:1-456`,
   `src-tauri/Cargo.toml:1-33`,
   `.github/workflows/release.yml:1-174`.
6. Read historical release workflow evidence. `git show 9df5603 -- .github/workflows/release.yml`
   shows the Windows row and collect step deletion, and `git show 9df5603^:.github/workflows/release.yml`
   shows the pre-removal Windows row and collect block. `git show 9df5603 -- .github/workflows/release.yml:23-56`,
   `git show 9df5603^:.github/workflows/release.yml:100-162`.

## 1. `session_lock` Cross-Platform Abstraction

### Public API to Preserve

1. Preserve `Lease` fields `session_id`, `provider_name`, `token`, `expires_at`,
   and `lock_path`. `src-tauri/src/session_lock/mod.rs:14-21`.
2. Preserve `ReleaseReceipt` fields `session_id`, `token`, `released_at`, and
   `already_released`. `src-tauri/src/session_lock/mod.rs:23-29`.
3. Preserve `LockError` variants exactly as `Busy`, `TokenInvalid`,
   `LockExpired`, and `Operational`. `src-tauri/src/session_lock/mod.rs:31-42`.
4. Preserve `SessionLock` as the owning lock type with `sentinel: File` and
   `lock_dir: PathBuf`; this storage shape is already correct for `fs4` because
   locks live on the underlying `File` handle. `src-tauri/src/session_lock/mod.rs:44-48`,
   `/home/nes/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/fs4-1.1.0/src/lib.rs:270-322`.
5. Preserve public methods `SessionLock::new`, `SessionLock::acquire`,
   `SessionLock::release`, and free helper `any_active_for_session`.
   `src-tauri/src/session_lock/mod.rs:86-110`,
   `src-tauri/src/session_lock/mod.rs:165-221`,
   `src-tauri/src/session_lock/mod.rs:253-272`.
6. Preserve owner PID metadata; it is orthogonal to file locking. `StoredLeaseOut`
   has `owner_pid`, and `acquire` writes `std::process::id()` into it.
   `src-tauri/src/session_lock/mod.rs:59-68`,
   `src-tauri/src/session_lock/mod.rs:137-145`.

### Call Sites Depending on the API

1. CLI pause-handshake uses `SessionLock::new`, `acquire`, and serialized
   `Lease` fields. `src-tauri/src/main.rs:1303-1338`.
2. CLI resume-handshake uses `SessionLock::new`, `release`, `ReleaseReceipt`,
   and `LockError` mapping. `src-tauri/src/main.rs:1354-1378`,
   `src-tauri/src/main.rs:1438-1456`.
3. `session_replace` imports `Lease`, `LockError`, and `SessionLock`, maps
   `LockError::Busy`, acquires a 300-second lease, and releases through
   `ImportReplaceLease::commit` and `Drop`. `src-tauri/src/session_replace/mod.rs:3-18`,
   `src-tauri/src/session_replace/mod.rs:144-166`,
   `src-tauri/src/session_replace/mod.rs:188-210`,
   `src-tauri/src/session_replace/mod.rs:468-487`.
4. Orphan cleanup uses `any_active_for_session`. `src-tauri/src/session_replace/mod.rs:682-720`.
5. Initiative 06 import-replace fixture writes active locks through
   `SessionLock::new` and `acquire`, but that fixture is Unix-gated.
   `src-tauri/tests/fixtures/initiative_06_import_replace.rs:1-14`,
   `src-tauri/tests/fixtures/initiative_06_import_replace.rs:320-324`.
6. Initiative 09 imports `LockError` and `SessionLock`, asserts `Busy`, and
   calls `any_active_for_session`; this test is Unix-gated.
   `src-tauri/tests/initiative_09_internal_unification.rs:1-8`,
   `src-tauri/tests/initiative_09_internal_unification.rs:201-237`.

### Insertion Point A: Imports

1. File:line range to replace: `src-tauri/src/session_lock/mod.rs:2-10`.
2. Old code shape: imports `nix::fcntl::{FlockArg, flock}` and
   `std::os::fd::AsRawFd`, while Unix file-mode imports are already
   `#[cfg(unix)]`. `src-tauri/src/session_lock/mod.rs:2-10`.
3. New code shape: import `fs4::FileExt`; remove `nix::fcntl::{FlockArg, flock}`
   and `std::os::fd::AsRawFd`; keep the existing `#[cfg(unix)] use
   std::os::unix::fs::{OpenOptionsExt, PermissionsExt}`. `src-tauri/src/session_lock/mod.rs:2-10`,
   `/home/nes/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/fs4-1.1.0/src/lib.rs:278-322`.
4. API surface preserved: public structs/enums and methods stay unchanged;
   only private imports change. `risk/13-release-restore-scope.md:280-307`.
5. Wider call sites: no caller imports `nix::fcntl`; grep finds `nix::fcntl`,
   `FlockArg`, and `flock(` only in `session_lock`. `src-tauri/src/session_lock/mod.rs:3`,
   `src-tauri/src/session_lock/mod.rs:225-231`.
6. Cfg-gating: do not gate the `fs4::FileExt` import by platform; `fs4` gates
   its Unix and Windows internals. `/home/nes/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/fs4-1.1.0/Cargo.toml:127-137`.

### Insertion Point B: Directory Privacy

1. File:line range to keep with no Windows equivalent: `src-tauri/src/session_lock/mod.rs:87-93`.
2. Old code shape: `create_dir_all`, then Unix `set_permissions` with
   `PermissionsExt::from_mode(0o700)`, then canonicalize. `src-tauri/src/session_lock/mod.rs:87-93`.
3. New code shape: keep `create_dir_all` on all platforms; keep the
   `PermissionsExt::from_mode(0o700)` block under `#[cfg(unix)]`; do not add
   Windows DACL code in this WU. `src-tauri/src/session_lock/mod.rs:87-93`,
   `proposals/13-release-restore.md:148-161`.
4. API surface preserved: `SessionLock::new(lock_dir: &Path) -> io::Result<Self>`
   stays unchanged. `src-tauri/src/session_lock/mod.rs:86-103`.
5. Cfg-gating: only the Unix mode block is gated; the lock abstraction itself is
   not gated. `proposals/13-release-restore.md:153-157`,
   `risk/13-release-restore-supported-surface.md:291-309`.

### Insertion Point C: Sentinel Open Mode and Lifetime

1. File:line range to keep with minor import change: `src-tauri/src/session_lock/mod.rs:93-102`.
2. Old code shape: canonicalizes `lock_dir`, opens `sentinel.lock` with
   `OpenOptions::create(true).read(true).write(true)`, and sets Unix
   `options.mode(0o600)`. `src-tauri/src/session_lock/mod.rs:93-102`.
3. New code shape: keep the same `File` open and store it in
   `SessionLock { sentinel, lock_dir }`; keep `options.mode(0o600)` under
   `#[cfg(unix)]`; rely on Windows inherited ACLs. `src-tauri/src/session_lock/mod.rs:95-102`,
   `proposals/13-release-restore.md:150-157`.
4. Sentinel-file lifetime: the `File` must remain a `SessionLock` field, not a
   local temporary in the lock helper, because `fs4` releases locks when the file
   handle is closed. `src-tauri/src/session_lock/mod.rs:44-48`,
   `/home/nes/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/fs4-1.1.0/src/lib.rs:270-322`.
5. Cfg-gating: only the Unix file mode call is gated; the same sentinel open
   path compiles on Windows. `src-tauri/src/session_lock/mod.rs:95-101`.

### Insertion Point D: Private Lock Helper

1. File:line range to replace: `src-tauri/src/session_lock/mod.rs:223-242`.
2. Old code shape: `with_flock` calls `flock(self.sentinel.as_raw_fd(),
   FlockArg::LockExclusive)`, runs the closure, then calls
   `flock(..., FlockArg::Unlock)`. `src-tauri/src/session_lock/mod.rs:223-242`.
3. New code shape: equivalent private helper calls `FileExt::lock(&self.sentinel)`,
   runs the closure, then calls `FileExt::unlock(&self.sentinel)`, preserving
   current result-vs-unlock error precedence. `/home/nes/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/fs4-1.1.0/src/lib.rs:298-322`,
   `src-tauri/src/session_lock/mod.rs:236-241`.
4. API surface preserved: `acquire` and `release` keep calling a private helper
   around metadata mutation; callers still see `LockError` only through
   `acquire`, `release`, and `any_active_for_session`. `src-tauri/src/session_lock/mod.rs:105-221`,
   `src-tauri/src/session_lock/mod.rs:253-272`.
5. Cfg-gating: do not add platform `cfg` around this helper. `fs4` implements
   Unix with `flock` and Windows with `LockFileEx`. `/home/nes/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/fs4-1.1.0/src/unix.rs:13-30`,
   `/home/nes/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/fs4-1.1.0/src/windows.rs:19-33`.
6. Blocking behavior: use `FileExt::lock`, not `FileExt::try_lock`, because the
   current helper blocks on the sentinel before inspecting lease metadata.
   `src-tauri/src/session_lock/mod.rs:223-230`,
   `proposals/13-release-restore.md:119-122`.

### `try_lock` Failure Mapping

1. The current public enum has no `LockError::Held` variant. It has
   `LockError::Busy` for live lease metadata and `LockError::Operational` for
   I/O failures. `src-tauri/src/session_lock/mod.rs:31-42`.
2. `fs4` names the nonblocking exclusive method `try_lock`, not
   `try_lock_exclusive`. `/home/nes/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/fs4-1.1.0/src/lib.rs:313-318`.
3. `fs4::TryLockError::WouldBlock` is the contended-lock signal for `try_lock`.
   `/home/nes/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/fs4-1.1.0/src/try_lock_error.rs:10-16`,
   `/home/nes/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/fs4-1.1.0/src/windows.rs:61-80`.
4. Production `SessionLock` should not translate `TryLockError::WouldBlock`
   into a new public `LockError::Held`; adding that variant would break the
   preserved API contract and force caller matches in `main.rs` and
   `session_replace`. `src-tauri/src/main.rs:1438-1456`,
   `src-tauri/src/session_replace/mod.rs:144-166`,
   `risk/13-release-restore-scope.md:280-307`.
5. The visible "held" condition for same-session second acquire remains
   `LockError::Busy`, generated after the sentinel lock is acquired and live
   lease metadata is read. `src-tauri/src/session_lock/mod.rs:111-123`,
   `proposals/13-release-restore.md:527-542`.

### Unlock Semantics

1. `fs4` documents automatic release when the file handle closes and says
   explicit `unlock` is optional. `/home/nes/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/fs4-1.1.0/src/lib.rs:270-272`.
2. Keep an explicit `unlock` call in the private helper so `acquire` and
   `release` keep the current bounded critical section around metadata writes
   rather than holding until `SessionLock` drop. `src-tauri/src/session_lock/mod.rs:223-242`.
3. `fs4` tests cover drop-release behavior, but that should remain fallback
   behavior, not the normal `SessionLock` release path. `/home/nes/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/fs4-1.1.0/src/file_ext/sync_impl.rs:168-197`.

### Insertion Point E: Temp Metadata File Mode

1. File:line range to keep with existing cfg: `src-tauri/src/session_lock/mod.rs:290-321`.
2. Old code shape: creates temp metadata file in `lock_dir`, applies Unix
   `options.mode(0o600)`, writes JSON, syncs the file, drops it, then renames.
   `src-tauri/src/session_lock/mod.rs:290-321`.
3. New code shape: keep the same temp-file publication logic and the
   `#[cfg(unix)] options.mode(0o600)` block; no Windows ACL code. `src-tauri/src/session_lock/mod.rs:301-306`,
   `proposals/13-release-restore.md:153-157`.
4. API surface preserved: lock metadata JSON shape and token hashing do not
   change. `src-tauri/src/session_lock/mod.rs:50-84`,
   `src-tauri/src/session_lock/mod.rs:390-394`.

## 2. `session_replace` Windows Behavior

### Rename Call 1: Staging Canonical Records to Canonical Side File

1. Call site: `fs::rename(&staging_path, &canonical_records_path)`.
   `src-tauri/src/session_replace/mod.rs:498-506`.
2. Source and destination: `staging_path` is under `journal_root/staging`, and
   `canonical_records_path` is under `journal_root`, so the constructor keeps
   them in one `journal_root` subtree. `src-tauri/src/session_replace/mod.rs:436-445`,
   `src-tauri/src/session_replace/mod.rs:498-499`.
3. Same-volume guarantee: same subtree by construction, but no device or
   volume identity probe exists; unusual mount or reparse layouts remain
   residual risk. `research/13-release-restore-problem-map.md:263-269`,
   `proposals/13-release-restore.md:351-375`.
4. Windows cross-volume failure mode: Rust's Windows rename maps to
   `MoveFileEx` replacement behavior and errors across filesystems.
   `research/13-release-restore-problem-map.md:259-262`.
5. DB transaction relation: this rename is before pending journal write, before
   transcript rename, and before DB replacement commit. `src-tauri/src/session_replace/mod.rs:498-527`,
   `src-tauri/src/session_replace/mod.rs:570-579`,
   `src-tauri/src/session_replace/mod.rs:865-929`.
6. Reordering: do not reorder; the recovery contract depends on canonical
   side-file and pending-journal sequencing. `research/13-release-restore-problem-map.md:172-188`,
   `risk/13-release-restore-shortcut.md:174-202`.

### Rename Call 2: Transcript Temp File to Provider Transcript

1. Call site: `fs::rename(&tmp_path, &metadata.jsonl_path)`.
   `src-tauri/src/session_replace/mod.rs:536-548`.
2. Source and destination: `tmp_path` is derived from
   `metadata.jsonl_path.with_extension(...)`, so source and destination are
   siblings. `src-tauri/src/session_replace/mod.rs:536-540`.
3. Same-volume guarantee: sibling path by construction, with the same
   mount/reparse residual as the proposal's A3. `research/13-release-restore-problem-map.md:263-269`,
   `proposals/13-release-restore.md:351-375`.
4. Windows cross-volume failure mode: if an unusual filesystem layout puts the
   temp sibling and destination on different volumes, Rust reports the rename
   error and `session_replace` surfaces `ReplaceError::OperationalError`.
   `research/13-release-restore-problem-map.md:259-262`,
   `src-tauri/src/session_replace/mod.rs:540-545`.
5. DB transaction relation: this rename happens before postimage verification
   and before `replace_db_turns` opens the DB transaction and commits it.
   `src-tauri/src/session_replace/mod.rs:550-574`,
   `src-tauri/src/session_replace/mod.rs:865-929`.
6. Reordering: not permissible; crash-after-rename-before-DB-commit is an
   explicit test hook and recovery scenario. `src-tauri/src/session_replace/mod.rs:22-25`,
   `src-tauri/src/session_replace/mod.rs:550-555`,
   `research/13-release-restore-problem-map.md:277-281`.

### Rename Call 3: `atomic_write_bytes`

1. Call site: `fs::rename(&tmp, path)`. `src-tauri/src/session_replace/mod.rs:1045-1064`.
2. Source and destination: `tmp` is `path.with_extension(...)`, so source and
   destination are siblings. `src-tauri/src/session_replace/mod.rs:1051-1053`.
3. Current callers include canonical staging write before lock acquisition and
   pending journal write before transcript replacement. `src-tauri/src/session_replace/mod.rs:443-445`,
   `src-tauri/src/session_replace/mod.rs:526-527`,
   `src-tauri/src/session_replace/mod.rs:1038-1043`.
4. Same-volume guarantee: sibling path by construction, no explicit OS volume
   probe. `research/13-release-restore-problem-map.md:263-269`.
5. Windows cross-volume failure mode: same Rust rename error model; caller
   receives `ReplaceError::OperationalError`. `research/13-release-restore-problem-map.md:259-262`,
   `src-tauri/src/session_replace/mod.rs:1053-1059`.
6. DB transaction relation: the helper itself is not followed directly by a DB
   commit; each caller's sequence controls recovery semantics. `src-tauri/src/session_replace/mod.rs:443-445`,
   `src-tauri/src/session_replace/mod.rs:526-548`,
   `src-tauri/src/session_replace/mod.rs:570-579`.
7. Reordering: do not replace with copy/delete or non-atomic publication; the
   proposal explicitly preserves rename behavior. `proposals/13-release-restore.md:199-252`,
   `risk/13-release-restore-shortcut.md:174-202`.

### Rename Call 4: Pending Journal to Quarantine

1. Call site: `let _ = fs::rename(path, dest)`. `src-tauri/src/session_replace/mod.rs:1170-1176`.
2. Source and destination: recovery scans `journal_root`, and
   `quarantine_dir` is `journal_root/quarantine`. `src-tauri/src/session_replace/mod.rs:594-604`,
   `src-tauri/src/session_replace/mod.rs:1170-1176`.
3. Same-volume guarantee: same `journal_root` subtree by construction, without
   an OS volume identity check. `research/13-release-restore-problem-map.md:263-269`.
4. Windows cross-volume failure mode: rename failure is intentionally ignored
   at this call site, so a failed quarantine move is best-effort cleanup only.
   `src-tauri/src/session_replace/mod.rs:1170-1176`.
5. DB transaction relation: this is a recovery quarantine path, not the success
   replace path; no DB transaction commit follows this helper. `src-tauri/src/session_replace/mod.rs:594-680`,
   `src-tauri/src/session_replace/mod.rs:1170-1176`.
6. Reordering: no success-path reorder is involved; leave as cleanup behavior
   unless Phase 6 finds a compile blocker. `proposals/13-release-restore.md:770-775`.

### Hard-Link Call Sites

1. Current tree result: no `std::fs::hard_link`, `hard_link`,
   `CreateHardLink`, or `linkat` call exists in `src-tauri/src/session_replace/mod.rs`
   or the searched `src-tauri/src` and `src-tauri/tests` surface. The only
   `nix::fcntl` matches are in `session_lock`. `research/13-release-restore-problem-map.md:270-275`,
   `proposals/13-release-restore.md:377-390`,
   `src-tauri/src/session_lock/mod.rs:3`,
   `src-tauri/src/session_lock/mod.rs:225-231`.
2. Destination reachability on Windows: not applicable because there is no
   current hard-link destination. `research/13-release-restore-problem-map.md:270-275`.
3. Failure surface: not applicable for production code; if Phase 6 rediscovers
   a hard-link call outside the mapped surface, the proposal requires stopping
   and returning to research. `proposals/13-release-restore.md:206-215`,
   `risk/13-release-restore-supported-surface.md:408-424`.
4. Necessity for AC-4 or AC-2: no hard-link removal is necessary for AC-2
   compile-time support or AC-4 atomicity because no hard-link code is currently
   present. `research/13-release-restore-problem-map.md:270-281`.

### A3 Mount-Identity Assertion Hookpoint

1. Approved proposal choice: keep rename calls as-is and treat same-root path
   construction as the invariant; no platform-specific rename wrapper is chosen.
   `proposals/13-release-restore.md:201-215`,
   `proposals/13-release-restore.md:244-252`.
2. If Phase 6a encodes A3 evidence in production, the insertion point is
   `run_import_replace_bytes` after `data_root`, `journal_root`,
   `staging_dir`, and `quarantine_dir` are constructed and before the first
   staging write. `src-tauri/src/session_replace/mod.rs:436-445`.
3. If Phase 6a chooses a helper, the helper name should be private and local to
   `session_replace`, for example `verify_same_volume`, and the contract should
   call out that it verifies the constructor invariant only, not every Windows
   reparse-point layout. `proposals/13-release-restore.md:361-370`,
   `risk/13-release-restore-shortcut.md:337-359`.
4. If no helper is encoded, Phase 6 evidence must record the residual as
   "same-subtree constructor invariant, volume identity not probed."
   `proposals/13-release-restore.md:361-370`,
   `risk/13-release-restore-shortcut.md:349-359`.

## 3. `path_hash_decomposition` Windows Path Handling

1. Decomposition entry point: `derive_claude_workspace_root` extracts the
   transcript directory name and calls `decode_claude_project_dir_candidates`.
   `src-tauri/src/session_metadata/mod.rs:257-336`.
2. Decomposition logic: `decode_claude_project_dir_candidates` requires a
   leading `-`, maps empty rest to `PathBuf::from("/")`, and calls
   `decode_claude_rest`. `src-tauri/src/session_metadata/mod.rs:338-351`.
3. Actual decomposition shape: it does not call `split('/')` and does not call
   `Path::components()`; it recursively treats `-` as either component
   separator or literal and builds candidates rooted at `/`. `src-tauri/src/session_metadata/mod.rs:353-388`.
4. Production call site that may pass Windows-style transcript paths:
   `locate_transcript` converts locator stdout to `PathBuf::from(line)`, and
   `available_jsonl_path` canonicalizes that path before Claude derivation.
   `src-tauri/src/sessions/mod.rs:171-198`,
   `src-tauri/src/session_metadata/mod.rs:213-255`.
5. `locate_session_metadata` reaches Claude path-hash decomposition when the
   provider storage is `SessionStorage::ClaudeCode`. `src-tauri/src/session_metadata/mod.rs:111-119`.
6. `session_replace::resolve_replace_metadata` inherits the same behavior
   through `locate_session_metadata`. `src-tauri/src/session_replace/mod.rs:852-863`.
7. `main::run_session_locate` exposes the same metadata result in CLI JSON.
   `src-tauri/src/main.rs:678-723`.
8. Codex path handling is separate: `derive_codex_workspace_root` reads
   `payload.cwd` into `PathBuf::from(cwd)` and does not use path-hash
   decomposition. `src-tauri/src/session_metadata/mod.rs:390-456`.
9. Existing fixtures encode Claude project dirs with Unix-only
   `trim_start_matches('/').replace('/', "-")`, and those fixtures are
   Unix-gated. `src-tauri/tests/fixtures/initiative_06.rs:1-16`,
   `src-tauri/tests/fixtures/initiative_06.rs:886-888`,
   `src-tauri/tests/fixtures/initiative_06_import_replace.rs:1-14`,
   `src-tauri/tests/fixtures/initiative_06_import_replace.rs:995-997`.
10. Minimal recommendation: there is no `split('/')` replacement hookpoint in
    production code. If Phase 6 verifies that supported Windows Claude Code
    paths reach this function, this is not a one-line `Path::components()`
    hardening; it needs a path-hash strategy decision or a return to Phase 3.
    `risk/13-release-restore-supported-surface.md:384-406`,
    `proposals/13-release-restore.md:776-783`.
11. If Phase 6 does not verify a supported Windows Claude path reaching this
    logic, leave `session_metadata` unchanged and document the residual in the
    AC-8/D-006 evidence. `proposals/13-release-restore.md:776-783`,
    `risk/13-release-restore-supported-surface.md:384-406`.

## 4. `Cargo.toml` Dependency Injection

1. Current dependency table is flat `[dependencies]` with no target-specific
   sections. `src-tauri/Cargo.toml:10-30`,
   `research/13-release-restore-problem-map.md:87-99`.
2. Exact insertion point: add `fs4` in the unconditional `[dependencies]`
   table near existing filesystem/platform dependencies, for example after
   `tempfile = "3"` or before `nix`. `src-tauri/Cargo.toml:18-28`.
3. Exact dependency shape for Phase 6:

```toml
fs4 = { version = "1.1", default-features = false, features = ["sync"] }
```

4. Rationale for explicit features: `fs4` v1.1.0 declares `default = ["sync"]`,
   optional async/wrapper features, and platform dependencies internally.
   `/home/nes/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/fs4-1.1.0/Cargo.toml:52-70`,
   `/home/nes/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/fs4-1.1.0/Cargo.toml:127-137`.
5. MSRV check: `fs4` declares Rust 1.75.0; local `rustc --version` is 1.92.0,
   and the proposal leaves Actions stable verification to Phase 6 evidence.
   `/home/nes/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/fs4-1.1.0/Cargo.toml:12-16`,
   `proposals/13-release-restore.md:434-448`.
6. `nix` removal candidate: `rg` finds `nix` usage only in `session_lock` and
   `src-tauri/Cargo.toml`; after replacing `flock`, remove
   `nix = { version = "0.29", features = ["fs"] }` if no new Phase 6 code uses
   `nix`. `src-tauri/Cargo.toml:27`,
   `src-tauri/src/session_lock/mod.rs:3`,
   `src-tauri/src/session_lock/mod.rs:225-231`.
7. No `[target.'cfg(unix)'.dependencies]` split is required for `fs4`; the new
   dependency belongs in unconditional `[dependencies]` because `fs4` abstracts
   platforms internally. `/home/nes/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/fs4-1.1.0/Cargo.toml:127-137`,
   `proposals/13-release-restore.md:124-131`.

## 5. `.github/workflows/release.yml` Insertion Points

### Matrix Row and Unix-Only Comment

1. Modify line range: `.github/workflows/release.yml:100-116`.
2. Delete or rewrite the Unix-only comment at lines 102-105; it conflicts with
   ticket AC-8 and D-006 rewrite requirements. `.github/workflows/release.yml:100-116`,
   `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:128-132`,
   `DECISIONS.md:122-162`.
3. Insert Windows row in `jobs.build.strategy.matrix.include`:

```yaml
          - os: windows-latest
            target: x86_64-pc-windows-msvc
            bundles: msi,nsis
```

4. Historical reference: this row existed before commit `9df5603`.
   `git show 9df5603^:.github/workflows/release.yml:105-114`,
   `git show 9df5603 -- .github/workflows/release.yml:34-43`.
5. Gating: matrix row itself has no `if`; per-platform steps below carry
   `runner.os` guards. `.github/workflows/release.yml:140-151`.

### Rust Toolchain Target List

1. Existing line range: `.github/workflows/release.yml:126-128`.
2. Keep this snippet unchanged:

```yaml
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}
```

3. Verification: `dtolnay/rust-toolchain` documents a `targets` input as a
   comma-separated string of additional targets to install. `https://raw.githubusercontent.com/dtolnay/rust-toolchain/master/README.md:2-5`.
4. Verification: `windows-latest` is a documented GitHub-hosted Windows x64
   runner label. `https://docs.github.com/en/actions/reference/runners/github-hosted-runners:300-318`.
5. Residual: actual `windows-latest` MSVC link-tool availability is not closed
   by YAML shape; Phase 6 must close it with the Windows target check or AC-6
   release run. `proposals/13-release-restore.md:416-448`,
   `risk/13-release-restore-shortcut.md:277-305`.
6. Gating: no `if: runner.os == 'Windows'` is needed here because all matrix
   rows use the same target input shape. `.github/workflows/release.yml:126-128`.

### Linux Collect Step

1. Modify line range: `.github/workflows/release.yml:140-145`.
2. Replace only the bare-binary copy destination; keep `.deb` conventional.
   `.github/workflows/release.yml:140-145`,
   `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:194-196`.
3. Exact snippet:

```yaml
      - name: Collect artifacts (Linux)
        if: runner.os == 'Linux'
        run: |
          mkdir -p artifacts
          cp src-tauri/target/${{ matrix.target }}/release/bundle/deb/*.deb artifacts/ 2>/dev/null || true
          cp src-tauri/target/${{ matrix.target }}/release/oulipoly-agent-runner artifacts/oulipoly-agent-runner-${{ matrix.target }} 2>/dev/null || true
```

4. Gating: keep `if: runner.os == 'Linux'`. `.github/workflows/release.yml:140-145`.

### macOS Collect Step

1. Modify line range: `.github/workflows/release.yml:146-151`.
2. Replace only the bare-binary copy destination; keep `.dmg` conventional.
   `.github/workflows/release.yml:146-151`,
   `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:194-196`.
3. Exact snippet:

```yaml
      - name: Collect artifacts (macOS)
        if: runner.os == 'macOS'
        run: |
          mkdir -p artifacts
          cp src-tauri/target/${{ matrix.target }}/release/bundle/dmg/*.dmg artifacts/ 2>/dev/null || true
          cp src-tauri/target/${{ matrix.target }}/release/oulipoly-agent-runner artifacts/oulipoly-agent-runner-${{ matrix.target }} 2>/dev/null || true
```

4. Gating: keep `if: runner.os == 'macOS'`. `.github/workflows/release.yml:146-151`.

### Windows Collect Step

1. Add after macOS collect and before `actions/upload-artifact@v4`.
   `.github/workflows/release.yml:146-155`.
2. Base historical block was deleted by `9df5603`; restore the shape but change
   only the bare-binary destination to the target-suffixed name.
   `git show 9df5603 -- .github/workflows/release.yml:44-56`,
   `git show 9df5603^:.github/workflows/release.yml:151-162`.
3. Exact snippet:

```yaml
      - name: Collect artifacts (Windows)
        if: runner.os == 'Windows'
        shell: pwsh
        run: |
          New-Item -ItemType Directory -Force -Path artifacts
          Copy-Item src-tauri/target/${{ matrix.target }}/release/bundle/msi/*.msi artifacts/ -ErrorAction SilentlyContinue
          Copy-Item src-tauri/target/${{ matrix.target }}/release/bundle/nsis/*.exe artifacts/ -ErrorAction SilentlyContinue
          Copy-Item src-tauri/target/${{ matrix.target }}/release/oulipoly-agent-runner.exe artifacts/oulipoly-agent-runner-${{ matrix.target }}.exe -ErrorAction SilentlyContinue
```

4. Gating: required `if: runner.os == 'Windows'` and `shell: pwsh`.
   `git show 9df5603^:.github/workflows/release.yml:151-158`,
   `proposals/13-release-restore.md:602-606`.

### Upload, Download, Release Publish

1. Existing upload range: `.github/workflows/release.yml:152-155`.
2. Keep upload artifact naming and path:

```yaml
      - uses: actions/upload-artifact@v4
        with:
          name: ${{ matrix.target }}
          path: artifacts/*
```

3. Existing download range: `.github/workflows/release.yml:162-165`.
4. Keep flattened download:

```yaml
      - uses: actions/download-artifact@v4
        with:
          merge-multiple: true
          path: artifacts
```

5. Existing release range: `.github/workflows/release.yml:170-174`.
6. Keep release file glob:

```yaml
      - uses: softprops/action-gh-release@v2
        with:
          tag_name: ${{ needs.version.outputs.tag }}
          generate_release_notes: true
          files: artifacts/*
```

7. Verification: `softprops/action-gh-release` documents `files` as newline
   delimited globs of release assets to upload; the current glob will preserve
   the collect-time filenames because those names are the actual matched paths.
   `https://github.com/softprops/action-gh-release:360-408`,
   `https://github.com/softprops/action-gh-release:457-465`,
   `.github/workflows/release.yml:170-174`.
8. Gating: no Windows-specific `if` is needed for upload, download, or release
   publish. `.github/workflows/release.yml:152-174`.

## 6. Reuse Points

1. Lock-directory creation already uses `std::fs::create_dir_all`; no shared
   helper is needed for production lock directory creation. `src-tauri/src/session_lock/mod.rs:87-88`.
2. Sentinel and metadata temp files already use `OpenOptions`; the only reusable
   pattern is the local `atomic_write_json` helper. `src-tauri/src/session_lock/mod.rs:95-101`,
   `src-tauri/src/session_lock/mod.rs:290-321`.
3. `tempfile = "3"` already exists in `src-tauri/Cargo.toml`, and current tests
   use `tempfile::tempdir()` for sandboxes. `src-tauri/Cargo.toml:18-20`,
   `src-tauri/tests/initiative_09_internal_unification.rs:207-214`.
4. `src-tauri/tests/fixtures/mod.rs` only re-exports Initiative 06 fixtures,
   and those fixtures are Unix-gated, so do not reuse them for the new portable
   `SessionLock` integration test. `src-tauri/tests/fixtures/mod.rs:1-5`,
   `src-tauri/tests/fixtures/initiative_06_pause_handshake.rs:1-12`,
   `src-tauri/tests/fixtures/initiative_06_import_replace.rs:1-14`.
5. The new lock integration test can reuse the package binary path pattern
   `env!("CARGO_BIN_EXE_oulipoly-agent-runner")` from existing CLI fixtures if
   it needs a sibling process. `src-tauri/tests/fixtures/initiative_06_pause_handshake.rs:349-355`,
   `src-tauri/tests/fixtures/initiative_06_import_replace.rs:956-966`.
6. For simple in-process tests, reuse `tempfile::tempdir`, `SessionLock::new`,
   `acquire`, `release`, and `any_active_for_session` directly instead of
   bootstrapping config/state fixtures. `src-tauri/tests/initiative_09_internal_unification.rs:207-237`,
   `src-tauri/src/session_lock/mod.rs:86-221`.
7. Structural release YAML test should reuse existing `serde_yml = "0.0.12"`,
   not add `serde_yaml`. `src-tauri/Cargo.toml:10-17`,
   `proposals/13-release-restore.md:294-323`.

## 7. Conflicting Systems

1. `nix::fcntl` imports outside `session_lock`: none found in `src-tauri/src`
   or `src-tauri/tests`; only `session_lock` imports and calls it.
   `src-tauri/src/session_lock/mod.rs:3`,
   `src-tauri/src/session_lock/mod.rs:225-231`.
2. `nix` dependency outside `session_lock`: no Rust source uses `nix`; the only
   manifest entry is `src-tauri/Cargo.toml:27`. `src-tauri/Cargo.toml:27`.
3. POSIX-only imports outside `session_lock` exist in tests and a few source
   test modules, but they are not `nix::fcntl` and are not part of this lock
   helper replacement. `src-tauri/tests/fixtures/initiative_06.rs:1-16`,
   `src-tauri/tests/fixtures/initiative_06_pause_handshake.rs:1-12`,
   `src-tauri/src/sessions/mod.rs:291`,
   `src-tauri/src/trace/mod.rs:432`.
4. Tests asserting POSIX-only `LockError` variants: none found. The direct
   `LockError` assertion is `LockError::Busy` in Initiative 09, which is not
   POSIX-specific. `src-tauri/tests/initiative_09_internal_unification.rs:220-230`.
5. CLI error mapping matches all current `LockError` variants; adding a new
   variant such as `Held` would require editing this match and is outside the
   preserved API contract. `src-tauri/src/main.rs:1438-1456`,
   `risk/13-release-restore-scope.md:280-307`.
6. `session_replace::map_lock_error` matches all current `LockError` variants;
   adding or renaming variants would change import-replace error mapping.
   `src-tauri/src/session_replace/mod.rs:144-166`.

## 8. Deletion Candidates

1. Delete direct `nix::fcntl` imports and `AsRawFd` import from
   `session_lock` when replacing the helper. `src-tauri/src/session_lock/mod.rs:2-10`.
2. Delete `#[allow(deprecated)]` on the old `with_flock` helper when the helper
   no longer calls deprecated `nix::fcntl::flock`. `src-tauri/src/session_lock/mod.rs:223-224`.
3. Delete the `nix` dependency from Cargo if Phase 6 confirms no other source
   uses it after the lock change. `src-tauri/Cargo.toml:27`.
4. Delete the Unix-only release-matrix comment in release workflow lines
   102-105. `.github/workflows/release.yml:100-116`.
5. The pre-#24 Windows row is informative but does not go back as-is because the
   bare binary must be copied to
   `artifacts/oulipoly-agent-runner-${{ matrix.target }}.exe`, not plain
   `artifacts/oulipoly-agent-runner.exe`. `git show 9df5603^:.github/workflows/release.yml:151-158`,
   `proposals/13-release-restore.md:258-267`.
6. No leftover Windows-only Cargo features were found in the current
   `src-tauri/Cargo.toml`; the manifest has no target-specific dependency
   sections. `src-tauri/Cargo.toml:10-33`.
7. Rewrite or remove D-006's "Windows is not a supported target" framing; the
   whole old decision conflicts with AC-8. `DECISIONS.md:122-162`,
   `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:128-132`,
   `proposals/13-release-restore.md:703-729`.

## 9. Cross-WU Non-Interference

1. No required reuse point pulls in `src-tauri/src/balancer/`, `src-tauri/src/quota/`,
   or `src-tauri/src/state/db.rs`. The new lock tests can use `tempfile` and
   `SessionLock` directly. `src-tauri/src/session_lock/mod.rs:86-221`,
   `src-tauri/tests/initiative_09_internal_unification.rs:207-237`.
2. `session_replace` currently imports `StateDb` and mutates `session_turns`, but
   this WU must not alter that DB sequence. `src-tauri/src/session_replace/mod.rs:7-9`,
   `src-tauri/src/session_replace/mod.rs:865-929`,
   `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:151-157`.
3. No reuse point pulls in `session_export` or body-storage paths. `session_replace`
   already uses `session_export` for canonical rendering, but the lock and
   release workflow changes do not need to edit that surface. `src-tauri/src/session_replace/mod.rs:1-6`,
   `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:151-157`.
4. Routing fanout harnesses are explicitly out of scope and should remain
   untouched. `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:190-193`,
   `research/13-release-restore-problem-map.md:296-309`.
5. Phase 6 final evidence should include `git diff --name-only` and targeted
   `rg` checks for the anti-scope list, as the proposal requires.
   `proposals/13-release-restore.md:801-813`,
   `risk/13-release-restore-audit.md:41-55`.

## Phase 6 Contract Notes

1. Production edit count should stay narrow: `src-tauri/Cargo.toml`,
   `src-tauri/src/session_lock/mod.rs`, `.github/workflows/release.yml`,
   `DECISIONS.md`, and new tests are enough unless Phase 6 verifies a blocker
   in `session_replace` or `session_metadata`. `risk/13-release-restore-scope.md:20-38`,
   `risk/13-release-restore-scope.md:76-107`.
2. Do not introduce a `cfg(unix)` compatibility shim for old POSIX locking.
   `proposals/13-release-restore.md:46-51`,
   `risk/13-release-restore-scope.md:108-137`.
3. Do not weaken import-replace atomicity by swapping rename for copy/delete,
   skipping fsyncs, or relaxing preimage/postimage verification.
   `risk/13-release-restore-shortcut.md:174-202`.
4. AC-3 must include a portable cross-process lock assertion, not only a
   same-process second acquire. `proposals/13-release-restore.md:521-542`,
   `risk/13-release-restore-shortcut.md:55-82`.
5. AC-5 structural test must parse YAML with `serde_yml` and assert the matrix,
   collect-step, upload, download, and release glob invariants.
   `proposals/13-release-restore.md:587-617`,
   `risk/13-release-restore-shortcut.md:84-115`.
