# WU-13-01 Release Restore Problem Map

Phase: 2.5 existing-state risk profile

Scope source: `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md`.
The work unit targets the Windows release regression and the bare-binary release asset collision. The ticket defines the in-scope code boundary as `session_lock`, `session_replace`, `session_metadata`, `src-tauri/Cargo.toml`, `.github/workflows/release.yml`, `DECISIONS.md`, and new `src-tauri/tests/` coverage; it explicitly excludes `src-tauri/src/balancer/`, `src-tauri/src/quota/`, `src-tauri/src/state/db.rs`, frontend code, body-storage work, and routing-fanout reproduction harness deletion. (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:134-160`, `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:162-180`, `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:182-197`)

No implementation choices are proposed here. This map records the existing terrain and the constraints that Phase 3 must account for.

## 1. Touched-surface enumeration

### `/home/nes/projects/agent-runner/worktrees/impl-wu-13-01/src-tauri/src/session_lock/mod.rs`

- Symbol-level change points:
  - `SessionLock` struct owns a `sentinel: File` and a canonical `lock_dir: PathBuf`. (`src-tauri/src/session_lock/mod.rs:44-48`)
  - `SessionLock::new` creates the lock directory, sets Unix `0o700` permissions, canonicalizes the directory, opens `sentinel.lock`, and sets Unix `0o600` mode on the sentinel file. (`src-tauri/src/session_lock/mod.rs:86-103`)
  - `SessionLock::acquire` serializes all mutation through `with_flock`, reads existing lease JSON, rejects unexpired leases as `LockError::Busy`, generates a token, writes `session-{session_id}.lock`, deletes the release marker, and fsyncs the lock directory. (`src-tauri/src/session_lock/mod.rs:105-163`)
  - `SessionLock::release` validates token syntax, serializes through `with_flock`, validates the stored lease token hash, writes `session-{session_id}.released`, removes the live lock file, and supports idempotent replay against the release marker. (`src-tauri/src/session_lock/mod.rs:165-221`)
  - `SessionLock::with_flock` uses the `nix` `flock` wrapper on `sentinel.as_raw_fd()` for exclusive lock and unlock. (`src-tauri/src/session_lock/mod.rs:223-242`)
  - `any_active_for_session` is a public helper for recovery/orphan cleanup that reads lock metadata without taking the sentinel lock. (`src-tauri/src/session_lock/mod.rs:253-272`)
  - `atomic_write_json` creates a temp file under `lock_dir`, sets Unix `0o600` mode, writes JSON, syncs the file, renames the temp file to the final lock path, and returns without fsyncing the parent itself. (`src-tauri/src/session_lock/mod.rs:290-321`)
  - `fsync_dir` opens the directory path as `File` and calls `sync_all`. (`src-tauri/src/session_lock/mod.rs:331-335`)

- Current direct dependencies that constrain the cross-platform fix:
  - `nix::fcntl::{FlockArg, flock}` is imported unconditionally, which is the immediate Windows compile blocker called out by the ticket. (`src-tauri/src/session_lock/mod.rs:2-3`, `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:33-39`)
  - `std::os::fd::AsRawFd` is imported unconditionally and used by `with_flock`. (`src-tauri/src/session_lock/mod.rs:8`, `src-tauri/src/session_lock/mod.rs:223-231`)
  - `std::os::unix::fs::{OpenOptionsExt, PermissionsExt}` is gated with `#[cfg(unix)]`, but callers of `SessionLock::new` still require equivalent lock-dir and file privacy semantics on Windows. (`src-tauri/src/session_lock/mod.rs:9-10`, `src-tauri/src/session_lock/mod.rs:89-100`, `src-tauri/src/session_lock/mod.rs:301-306`)
  - `fs::set_permissions(lock_dir, fs::Permissions::from_mode(0o700))` pins the current Unix-only directory privacy contract. (`src-tauri/src/session_lock/mod.rs:89-92`)
  - `options.mode(0o600)` appears in sentinel creation and temp lock metadata creation. (`src-tauri/src/session_lock/mod.rs:95-101`, `src-tauri/src/session_lock/mod.rs:301-309`)
  - `fs::rename` is used in `atomic_write_json`; Windows behavior must be considered for lock metadata publication even though the ticket names `session_replace` as the rename surface. (`src-tauri/src/session_lock/mod.rs:318-319`)
  - `nix` is a normal dependency rather than target-scoped in `src-tauri/Cargo.toml`. (`src-tauri/Cargo.toml:10-30`)

- Callers / call-sites that pin the public API contract:
  - `src-tauri/src/main.rs` imports `LockError` and `SessionLock`, then uses `SessionLock::new`, `acquire`, and `release` for `agents session pause-handshake` and `resume-handshake`. (`src-tauri/src/main.rs:12`, `src-tauri/src/main.rs:397-419`, `src-tauri/src/main.rs:1303-1339`, `src-tauri/src/main.rs:1354-1378`)
  - `src-tauri/src/session_replace/mod.rs` imports `Lease`, `LockError`, and `SessionLock`, acquires a 300-second lease before transcript replacement, and releases through `ImportReplaceLease::commit` or `Drop`. (`src-tauri/src/session_replace/mod.rs:3`, `src-tauri/src/session_replace/mod.rs:188-210`, `src-tauri/src/session_replace/mod.rs:468-487`, `src-tauri/src/session_replace/mod.rs:579-579`)
  - `src-tauri/src/session_replace/mod.rs` calls `any_active_for_session` during orphan canonical-record cleanup. (`src-tauri/src/session_replace/mod.rs:682-720`)
  - `src-tauri/tests/fixtures/initiative_06_import_replace.rs` creates active locks directly through `SessionLock::new` and `acquire`. (`src-tauri/tests/fixtures/initiative_06_import_replace.rs:320-324`)
  - `src-tauri/tests/initiative_09_internal_unification.rs` imports the public `session_lock` module and asserts `any_active_for_session`, `SessionLock::new`, `acquire`, and busy behavior. (`src-tauri/tests/initiative_09_internal_unification.rs:1-7`, `src-tauri/tests/initiative_09_internal_unification.rs:201-237`)
  - The crate exports `session_lock` as a public module, so integration tests and external crate consumers can reference the module surface. (`src-tauri/src/lib.rs:1-15`)

### `/home/nes/projects/agent-runner/worktrees/impl-wu-13-01/src-tauri/src/session_replace/mod.rs`

- Symbol-level change points:
  - `map_lock_error` maps `LockError::Busy` into `ReplaceError::SessionBusy` while stripping an optional `sha256:` prefix for the public error token. (`src-tauri/src/session_replace/mod.rs:144-166`)
  - `ImportReplaceLease` wraps a borrowed `SessionLock`, session id, and optional `Lease`; `commit` releases on success and `Drop` releases on early return or failure. (`src-tauri/src/session_replace/mod.rs:188-210`)
  - `run_import_replace` reads either a file or stdin and delegates to `run_import_replace_bytes`. (`src-tauri/src/session_replace/mod.rs:306-319`)
  - `run_import_replace_bytes` validates UUID/input, writes canonical staging bytes, resolves metadata, constructs `SessionLock`, acquires the lock, publishes canonical records with `fs::rename`, writes the pending journal, writes the replacement provider transcript to a temp sibling file, renames that temp file to `metadata.jsonl_path`, verifies postimage/fresh export, updates SQLite state, deletes journal files, fsyncs, and releases the lease. (`src-tauri/src/session_replace/mod.rs:417-592`)
  - `recover_pending_replaces` reads pending journals, reconstructs metadata from the journal, compares current provider transcript hashes, updates DB state when the postimage already landed, removes/quarantines journals, and invokes orphan cleanup. (`src-tauri/src/session_replace/mod.rs:594-680`)
  - `cleanup_orphan_canonical_records` uses `session_lock::any_active_for_session` before deleting a canonical side file that has no pending or quarantined pending journal. (`src-tauri/src/session_replace/mod.rs:682-720`)
  - `atomic_write_bytes` writes a temp sibling file then uses `fs::rename(&tmp, path)`. (`src-tauri/src/session_replace/mod.rs:1045-1064`)
  - `move_to_quarantine` uses `fs::rename(path, dest)` and intentionally ignores the result. (`src-tauri/src/session_replace/mod.rs:1170-1176`)

- Current direct dependencies that constrain the cross-platform fix:
  - `session_replace` depends on `SessionLock`, `Lease`, and `LockError` for its critical section and public error mapping. (`src-tauri/src/session_replace/mod.rs:3`, `src-tauri/src/session_replace/mod.rs:144-166`, `src-tauri/src/session_replace/mod.rs:188-210`, `src-tauri/src/session_replace/mod.rs:468-487`)
  - It uses `StateDb`, `rusqlite::Connection`, and direct SQL for `session_turns`, so changes that move lock errors into state/db types risk crossing anti-scope. (`src-tauri/src/session_replace/mod.rs:7-9`, `src-tauri/src/session_replace/mod.rs:865-929`, `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:151-157`, `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:182-186`)
  - It relies on sibling temp-path publication for atomic writes: `staging_path` and `canonical_records_path` are both under `journal_root`; `tmp_path` is derived from `metadata.jsonl_path.with_extension(...)`; `atomic_write_bytes` derives `tmp` from `path.with_extension(...)`. (`src-tauri/src/session_replace/mod.rs:438-445`, `src-tauri/src/session_replace/mod.rs:498-506`, `src-tauri/src/session_replace/mod.rs:536-548`, `src-tauri/src/session_replace/mod.rs:1045-1064`)
  - Current tree verification found no `hard_link` call in `src-tauri/src/session_replace/mod.rs` even though the ticket and D-006 list one. The command `rg -n "hard_link" src-tauri/src/session_replace/mod.rs` returns no matches in this worktree; the current direct filesystem publication primitive in this file is `fs::rename`. (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:39-40`, `DECISIONS.md:124-129`, `src-tauri/src/session_replace/mod.rs:500-506`, `src-tauri/src/session_replace/mod.rs:540-548`, `src-tauri/src/session_replace/mod.rs:1051-1064`, `src-tauri/src/session_replace/mod.rs:1170-1176`)

- Callers / call-sites that pin the public API contract:
  - `src-tauri/src/main.rs::run_session_import_replace` validates CLI arguments, calls `session_replace::run_import_replace`, prints the success receipt, and maps `ReplaceError` to exit codes. (`src-tauri/src/main.rs:558-590`)
  - `src-tauri/tests/initiative_06_import_replace.rs` drives the CLI import-replace path through fixture helpers for success, busy-lock, crash recovery, concurrency, postimage failure, unsupported storage, mismatch, and validation behavior. (`src-tauri/tests/initiative_06_import_replace.rs:295-313`, `src-tauri/tests/initiative_06_import_replace.rs:316-379`, `src-tauri/tests/initiative_06_import_replace.rs:526-575`, `src-tauri/tests/initiative_06_import_replace.rs:630-675`)
  - `src-tauri/tests/initiative_09_internal_unification.rs` asserts public lock visibility and error-path release through `run_import_replace` and pause/resume helpers. (`src-tauri/tests/initiative_09_internal_unification.rs:13-47`, `src-tauri/tests/initiative_09_internal_unification.rs:86-125`)
  - `src-tauri/tests/fixtures/initiative_06_import_replace.rs` pins the subprocess-oriented fixture shape, including direct lock-writing, CLI spawning, and the import-replace command construction. (`src-tauri/tests/fixtures/initiative_06_import_replace.rs:320-390`)

### `/home/nes/projects/agent-runner/worktrees/impl-wu-13-01/src-tauri/src/session_metadata/mod.rs`

- Symbol-level change points:
  - `locate_session_metadata` validates the input UUID, resolves active provider/session through `StateDb`, loads effective provider config, derives `storage_type`, active segment, canonical JSONL path, workspace root, and `mutable`. (`src-tauri/src/session_metadata/mod.rs:83-145`)
  - `available_jsonl_path` calls `locate_transcript`, requires an absolute existing path, canonicalizes it, and rejects non-UTF-8 canonical paths. (`src-tauri/src/session_metadata/mod.rs:213-255`)
  - `derive_claude_workspace_root` canonicalizes `projects_dir`, requires the transcript parent to equal `projects_dir`, extracts the encoded project directory name, decodes possible workspace candidates, canonicalizes existing candidates, rejects zero or multiple candidates, and returns the only existing candidate. (`src-tauri/src/session_metadata/mod.rs:257-336`)
  - `decode_claude_project_dir_candidates` strips a leading `-`, returns `PathBuf::from("/")` for an empty encoded rest, and otherwise recursively decomposes hyphens into path separators. (`src-tauri/src/session_metadata/mod.rs:338-351`)
  - `decode_claude_rest` builds every candidate as an absolute Unix-rooted `PathBuf::from("/")` and pushes decoded components with `PathBuf::push`. (`src-tauri/src/session_metadata/mod.rs:353-388`)
  - `derive_codex_workspace_root` reads the first `session_meta` line, takes `payload.cwd`, requires it to be absolute and existing, canonicalizes it, and rejects non-UTF-8 paths. (`src-tauri/src/session_metadata/mod.rs:390-456`)

- Current direct dependencies that constrain the cross-platform fix:
  - `locate_session_metadata` depends on `StateDb`, `ModelStore`, provider config, sessions config, and `locate_transcript`; path normalization changes cannot be isolated to a pure string helper if they change caller-observable `workspace_root` or `mutable`. (`src-tauri/src/session_metadata/mod.rs:1-8`, `src-tauri/src/session_metadata/mod.rs:83-145`)
  - `decode_claude_project_dir_candidates` currently assumes encoded Claude project directory names represent Unix-rooted paths because candidates are rooted at `/`; no branch constructs a Windows drive prefix or UNC prefix. (`src-tauri/src/session_metadata/mod.rs:338-349`, `src-tauri/src/session_metadata/mod.rs:363-368`)
  - Existing fixtures encode Claude workspace paths by trimming leading `/` and replacing `/` with `-`; this pins the test fixture path-hash model to Unix-style separators. (`src-tauri/tests/fixtures/initiative_06.rs:886-888`, `src-tauri/tests/fixtures/initiative_06_import_replace.rs:995-997`, `src-tauri/tests/fixtures/initiative_06_export.rs:605-607`)
  - Codex `payload.cwd` takes a raw path string into `PathBuf::from` and therefore can accept Windows-style absolute paths on a Windows runtime, but Linux tests currently write `workspace_root.display()` from Unix temp paths. (`src-tauri/src/session_metadata/mod.rs:416-449`, `src-tauri/tests/fixtures/initiative_06.rs:687-699`, `src-tauri/tests/fixtures/initiative_06_import_replace.rs:551-565`)

- Callers / call-sites that pin the public API contract:
  - `src-tauri/src/main.rs::run_session_locate` calls `locate_session_metadata` and serializes `SessionMetadata` JSON. (`src-tauri/src/main.rs:680-723`)
  - `src-tauri/src/session_replace.rs::resolve_replace_metadata` loads state/models/providers/sessions and calls `locate_session_metadata`. (`src-tauri/src/session_replace/mod.rs:852-863`)
  - `src-tauri/tests/session_metadata_component.rs` asserts Claude path-hash inversion, one-decomposition success, zero-decomposition failure, multiple-decomposition failure, and Codex `payload.cwd` workspace derivation. (`src-tauri/tests/session_metadata_component.rs:272-338`)
  - Initiative 06 export/import fixtures stage Claude transcript directories using the same path-hash encoding helper and therefore exercise the Unix-style encoded path convention indirectly. (`src-tauri/tests/fixtures/initiative_06_import_replace.rs:252-265`, `src-tauri/tests/fixtures/initiative_06_export.rs:274-285`)

### `/home/nes/projects/agent-runner/worktrees/impl-wu-13-01/src-tauri/Cargo.toml`

- Symbol-level change points:
  - The package name is `oulipoly-agent-runner`; no `[package]` entry named `agent-runner-tauri` exists in this manifest. (`src-tauri/Cargo.toml:1-4`)
  - Dependencies are currently a flat `[dependencies]` table; there are no `[target.'cfg(unix)'.dependencies]` or `[target.'cfg(windows)'.dependencies]` sections. (`src-tauri/Cargo.toml:10-30`)
  - `nix = { version = "0.29", features = ["fs"] }` is a non-target-scoped dependency. (`src-tauri/Cargo.toml:27`)
  - `libc = "0.2"` and `signal-hook = "0.3"` are also non-target-scoped dependencies; current build may still compile them on Windows depending on crate cfgs, but they are part of the same flat dependency layout. (`src-tauri/Cargo.toml:23-24`)
  - `getrandom = "0.2"` supplies lock-token entropy in `session_lock`. (`src-tauri/Cargo.toml:28`, `src-tauri/src/session_lock/mod.rs:373-388`)

- Current direct dependencies that constrain the cross-platform fix:
  - Any external locking crate or `windows-sys` dependency must be introduced into this manifest without disturbing unrelated dependencies or unscoped `nix` consumers. (`src-tauri/Cargo.toml:10-30`)
  - The ticket expects target-cfg dependency layout work if platform-specific dependencies are needed. (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:143-145`, `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:213-214`)

- Callers / call-sites that pin the public API contract:
  - The release workflow invokes `bunx tauri build --target ${{ matrix.target }} --bundles ${{ matrix.bundles }}`, so manifest dependency layout must satisfy cross-target Tauri builds. (`.github/workflows/release.yml:126-139`)
  - The ticket's AC-2 names `cargo check --target x86_64-pc-windows-msvc -p agent-runner-tauri`; the manifest's actual package name is `oulipoly-agent-runner`, making that command string a verification-risk item for Phase 3/6a. (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:98-101`, `src-tauri/Cargo.toml:1-4`)

### `/home/nes/projects/agent-runner/worktrees/impl-wu-13-01/.github/workflows/release.yml`

- Symbol-level change points:
  - `jobs.build.strategy.matrix.include` currently contains Linux and macOS rows only. (`.github/workflows/release.yml:100-116`)
  - `dtolnay/rust-toolchain@stable` receives `targets: ${{ matrix.target }}`. (`.github/workflows/release.yml:126-128`)
  - The Tauri build step uses the matrix target and bundles values. (`.github/workflows/release.yml:138-139`)
  - Linux collection copies `*.deb` and bare `oulipoly-agent-runner` into `artifacts/`. (`.github/workflows/release.yml:140-145`)
  - macOS collection copies `*.dmg` and bare `oulipoly-agent-runner` into `artifacts/`. (`.github/workflows/release.yml:146-151`)
  - `actions/upload-artifact@v4` names each workflow artifact by target triple and uploads `artifacts/*`. (`.github/workflows/release.yml:152-155`)
  - The release job downloads all artifacts with `merge-multiple: true` into one `artifacts` directory, then passes `files: artifacts/*` to `softprops/action-gh-release@v2`. (`.github/workflows/release.yml:157-174`)

- Current direct dependencies that constrain the release fix:
  - Current build comments explicitly state Windows is absent because of POSIX-only primitives and cite D-006. (`.github/workflows/release.yml:100-105`)
  - `actions/download-artifact@v4` with `merge-multiple: true` puts multiple artifacts into one directory; the Actions migration docs state same-name files collide with "last writer wins". (`.github/workflows/release.yml:162-165`, `upload-artifact/docs/MIGRATION.md via web: turn4search0`)
  - `softprops/action-gh-release` uploads files matched by the `with.files` glob, and GitHub release assets carry the uploaded asset `name`; duplicate filenames are rejected by the GitHub release asset API. (`.github/workflows/release.yml:170-174`, `softprops/action-gh-release README via web: turn3view0 lines 360-381`, `GitHub REST release assets docs via web: turn3view1 lines 367-367`)

- Callers / call-sites that pin the public API contract:
  - Manual `workflow_dispatch` is the only release trigger. (`.github/workflows/release.yml:3-16`)
  - `jobs.version` computes the version/tag and both build and release jobs depend on it. (`.github/workflows/release.yml:68-99`, `.github/workflows/release.yml:100-101`, `.github/workflows/release.yml:157-158`)
  - The ticket AC-1 requires restoration of a Windows row with `windows-latest`, target `x86_64-pc-windows-msvc`, and bundles `msi,nsis`. (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:91-97`)
  - The ticket AC-5 requires distinguishable bare-binary artifacts and a structural release.yml contract test. (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:114-117`)

### `/home/nes/projects/agent-runner/worktrees/impl-wu-13-01/DECISIONS.md`

- Symbol-level change points:
  - D-001 records that `SessionLock` has no lease-renewal API and fixed-TTL one-shot leases are intentional. (`DECISIONS.md:9-32`)
  - D-001 identifies `agents session import-replace` as the single in-tree consumer at the time of the decision, while the current tree also uses `SessionLock` in pause/resume CLI paths. (`DECISIONS.md:18-28`, `src-tauri/src/main.rs:1303-1378`, `src-tauri/src/session_replace/mod.rs:468-487`)
  - D-006 currently says Windows is not supported, names POSIX primitives as the cause, removes Windows from the release matrix, and lists revisit paths for Windows support. (`DECISIONS.md:122-162`)

- Current direct dependencies that constrain the cross-platform fix:
  - D-006's decision text conflicts with this WU's acceptance criterion that Windows be supported with a cross-platform abstraction. (`DECISIONS.md:134-137`, `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:128-132`)
  - D-001 constrains lock API shape: a Windows port must not accidentally introduce renewal semantics or token rotation unless Phase 3 explicitly scopes that, because renewal is recorded out of scope. (`DECISIONS.md:14-32`)

- Callers / call-sites that pin the public API contract:
  - D-001's rationale references pause-handshake CLI behavior for external scripts and TTL sizing. (`DECISIONS.md:26-28`)
  - D-006 references the release workflow regression from `windows-latest` builds and directly explains the current release matrix shape. (`DECISIONS.md:124-137`, `.github/workflows/release.yml:100-116`)

### `/home/nes/projects/agent-runner/worktrees/impl-wu-13-01/src-tauri/tests/` new or existing tests

- Symbol-level change points:
  - New lock integration tests are in scope under `src-tauri/tests/`, with acquire/release and exclusivity on each platform. (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:149-150`, `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:162-175`)
  - New structural release workflow test is in scope and may live at `src-tauri/tests/release_yml_contract.rs`. (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:169-172`)
  - Existing `src-tauri/tests/initiative_06_*` tests are in scope for preservation, but most are currently `#![cfg(unix)]`. (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:173-175`, `src-tauri/tests/initiative_06_export.rs:1`, `src-tauri/tests/initiative_06_import_replace.rs:1`, `src-tauri/tests/initiative_06_locate.rs:1`, `src-tauri/tests/initiative_06_pause_handshake.rs:1`, `src-tauri/tests/initiative_06_schema_probe.rs:1`)

- Current direct dependencies that constrain the cross-platform fix:
  - Initiative 06 fixtures use Unix-only imports such as `std::os::unix::fs::PermissionsExt`, `std::os::unix::ffi::OsStringExt`, and `std::os::unix::fs::symlink`. (`src-tauri/tests/fixtures/initiative_06.rs:1-15`, `src-tauri/tests/fixtures/initiative_06_export.rs:1-12`, `src-tauri/tests/fixtures/initiative_06_import_replace.rs:1-11`, `src-tauri/tests/fixtures/initiative_06_pause_handshake.rs:1-9`, `src-tauri/tests/fixtures/initiative_06_schema_probe.rs:1-9`)
  - Existing pause-handshake concurrency tests assert process-shared advisory lock behavior through subprocesses. (`src-tauri/tests/initiative_06_pause_handshake.rs:94-128`, `src-tauri/tests/initiative_06_pause_handshake.rs:211-242`)
  - Existing import-replace concurrency and recovery tests assert one winner, busy exit code, lock-held orphan behavior, and crash after rename before DB commit. (`src-tauri/tests/initiative_06_import_replace.rs:295-313`, `src-tauri/tests/initiative_06_import_replace.rs:316-379`, `src-tauri/tests/initiative_06_import_replace.rs:477-508`, `src-tauri/tests/initiative_06_import_replace.rs:526-575`)

- Callers / call-sites that pin the public API contract:
  - Fixture helpers construct real CLI subprocess commands and therefore pin binary CLI behavior, env variables, and filesystem layout. (`src-tauri/tests/fixtures/initiative_06_import_replace.rs:330-390`, `src-tauri/tests/fixtures/initiative_06_pause_handshake.rs:140-181`)
  - The tests currently rely on the released binary path through `env!("CARGO_BIN_EXE_oulipoly-agent-runner")` in fixture helpers. (`src-tauri/tests/fixtures/initiative_06_import_replace.rs:425-425`, `src-tauri/tests/fixtures/initiative_06_pause_handshake.rs:168-181`)

### Current lock metadata contract details

- The public `Lease` receipt serializes `session_id`, `provider_name`, `token`, `expires_at`, and `lock_path`; the release receipt serializes `session_id`, `token`, `released_at`, and `already_released`. (`src-tauri/src/session_lock/mod.rs:14-29`)
- `LockError` has four variants: `Busy`, `TokenInvalid`, `LockExpired`, and `Operational`. This enum is public and is matched by `main.rs` and `session_replace`. (`src-tauri/src/session_lock/mod.rs:31-42`, `src-tauri/src/session_replace/mod.rs:144-166`, `src-tauri/src/main.rs:1368-1378`)
- Stored live leases are read through `StoredLease` with `version`, `session_id`, optional `token_hash`, and `expires_at`; `StoredLeaseOut` writes `provider_name`, `created_at`, and `owner_pid` in addition to those fields. (`src-tauri/src/session_lock/mod.rs:50-68`)
- Stored release markers are read through `StoredReleaseMarker` with `version`, `session_id`, optional `token_hash`, and `released_at`; release-marker writes use `StoredReleaseMarkerOut`. (`src-tauri/src/session_lock/mod.rs:70-84`)
- Tokens are generated as `pause_` plus 32 lowercase hex characters, and `valid_token` rejects missing prefix, wrong length, non-hex, or uppercase hex. (`src-tauri/src/session_lock/mod.rs:363-375`)
- Token hashes are stored as `sha256:<hex>` over the pause token bytes; busy errors preserve the stored optional token hash. (`src-tauri/src/session_lock/mod.rs:120-123`, `src-tauri/src/session_lock/mod.rs:390-394`)
- `session_replace::map_lock_error` strips `sha256:` before returning `ReplaceError::SessionBusy.token`, so changing hash formatting would affect CLI-visible import-replace error JSON. (`src-tauri/src/session_replace/mod.rs:144-157`)
- `main.rs` pause-handshake success JSON returns the raw pause token and lock path; this differs from import-replace busy JSON, which only returns a hash-derived token string. (`src-tauri/src/main.rs:1317-1336`, `src-tauri/src/session_replace/mod.rs:144-157`)
- `any_active_for_session` intentionally does not check token validity; it only verifies metadata version, session id match, and expiry time. (`src-tauri/src/session_lock/mod.rs:253-272`)
- Expired live lock files are overwritten by a later `acquire`; there is no cleanup pass before reading a lock. (`src-tauri/src/session_lock/mod.rs:115-151`)
- Release removes the live lock file but leaves a release marker for idempotent replay. (`src-tauri/src/session_lock/mod.rs:182-217`)
- `remove_if_exists` treats missing files as success but converts all other remove failures to operational errors. (`src-tauri/src/session_lock/mod.rs:323-328`)

### Current import-replace transaction contract details

- `ReplaceError::exit_code` maps invalid input to `2`, missing/ambiguous/unsupported/busy/schema/invalid transcript/preimage classes to `10-15`, and operational errors to `1`. (`src-tauri/src/session_replace/mod.rs:78-92`)
- `ReplaceError::code` is the stable JSON error-code surface for import-replace failures. (`src-tauri/src/session_replace/mod.rs:94-107`)
- `ReplaceError::to_json` is the direct stderr payload serialized by `main.rs::run_session_import_replace`. (`src-tauri/src/session_replace/mod.rs:109-141`, `src-tauri/src/main.rs:586-589`)
- `run_import_replace_bytes` writes canonical input to a staging file before metadata resolution and removes that staging file on early metadata/storage/validation failures. (`src-tauri/src/session_replace/mod.rs:434-463`)
- The lock is acquired after metadata validation and before preimage calculation and transcript publication. (`src-tauri/src/session_replace/mod.rs:461-490`)
- The preimage hash is calculated from the provider file while the lock is held. (`src-tauri/src/session_replace/mod.rs:490-496`)
- The canonical side file is published before the pending journal is written. (`src-tauri/src/session_replace/mod.rs:498-527`)
- Preimage mismatch is checked after the pending journal is written; an error from this point relies on `ImportReplaceLease::Drop` for release. (`src-tauri/src/session_replace/mod.rs:526-534`, `src-tauri/src/session_replace/mod.rs:205-210`)
- The provider transcript temp file is written and synced before `fs::rename` replaces the provider transcript. (`src-tauri/src/session_replace/mod.rs:536-548`, `src-tauri/src/session_replace/mod.rs:1066-1080`)
- The after-rename test hook blocks before postimage verification and before SQLite mutation. (`src-tauri/src/session_replace/mod.rs:550-555`)
- SQLite replacement deletes old turns for `(provider_name, session_id)`, inserts rendered canonical records as new `session_turns`, refreshes the active segment last turn, and refreshes chain `last_used_at`. (`src-tauri/src/session_replace/mod.rs:865-929`)
- Success cleanup deletes the pending journal and canonical side file, fsyncs the journal root, and releases the lease explicitly. (`src-tauri/src/session_replace/mod.rs:576-579`)
- Recovery can repair the DB when the provider transcript already matches the postimage hash and canonical records are available. (`src-tauri/src/session_replace/mod.rs:642-662`)
- Recovery removes the pending journal without DB mutation when the provider transcript still matches the preimage hash or when there was no preimage recorded. (`src-tauri/src/session_replace/mod.rs:663-672`)
- Recovery quarantines ambiguous cases by moving the pending journal into the quarantine directory. (`src-tauri/src/session_replace/mod.rs:673-675`, `src-tauri/src/session_replace/mod.rs:1170-1176`)

### Current path metadata contract details

- `SessionMetadata` JSON includes `session_id`, `chain_id`, `provider_name`, `storage_type`, `jsonl_path`, `workspace_root`, `transcript_state`, and `mutable`; `active_segment_id` is not serialized. (`src-tauri/src/session_metadata/mod.rs:11-23`)
- `SessionStorageType` serializes as `snake_case` variants `claude_code`, `codex_session`, and `other`. (`src-tauri/src/session_metadata/mod.rs:25-31`)
- `TranscriptState` variants exist, but `locate_session_metadata` returns `TranscriptState::Available` after `available_jsonl_path` succeeds. (`src-tauri/src/session_metadata/mod.rs:43-60`, `src-tauri/src/session_metadata/mod.rs:133-143`)
- `mutable` is derived from supported storage, `provider.resume.is_some()`, absolute JSONL path, and absolute workspace root. (`src-tauri/src/session_metadata/mod.rs:128-131`)
- `available_jsonl_path` rejects relative transcript-locator outputs before canonicalization. (`src-tauri/src/session_metadata/mod.rs:230-235`)
- `available_jsonl_path` rejects missing transcript paths before deriving workspace roots. (`src-tauri/src/session_metadata/mod.rs:236-247`)
- Claude workspace root derivation rejects transcripts whose parent directory is not directly under the configured `projects_dir`. (`src-tauri/src/session_metadata/mod.rs:272-292`)
- Claude workspace root derivation rejects non-UTF-8 project directory names and non-UTF-8 canonical candidate paths. (`src-tauri/src/session_metadata/mod.rs:294-300`, `src-tauri/src/session_metadata/mod.rs:317-323`)
- Codex workspace root derivation rejects missing `session_meta`, missing `payload.cwd`, relative cwd, missing cwd, and non-UTF-8 canonical cwd. (`src-tauri/src/session_metadata/mod.rs:399-456`)

### Current test inventory inside the blast radius

- `initiative_06_pause_handshake.rs` validates pause success shape, invalid UUID early exit, resolver error mapping, concurrent pause, per-session lock scoping, token format, TTL bounds/expiry, stale lock replacement, active second-pause rejection, release idempotency, and bad token handling. (`src-tauri/tests/initiative_06_pause_handshake.rs:8-355`)
- `initiative_06_import_replace.rs` validates success, preimage protection, busy lock, crash recovery, ambiguous recovery, orphan canonical behavior, concurrent import-replace, unsupported storage, mismatched session/provider records, invalid UUID/input, stdin/file input, and binary input rejection. (`src-tauri/tests/initiative_06_import_replace.rs:31-1235`)
- `initiative_06_locate.rs` is Unix-only and uses the same session metadata path surface that WU-13-01 may need for Claude path-hash hardening. (`src-tauri/tests/initiative_06_locate.rs:1`, `src-tauri/src/session_metadata/mod.rs:83-145`)
- `initiative_06_export.rs` is Unix-only and uses the same storage metadata/path fixtures, but export itself is anti-scope for body-storage changes. (`src-tauri/tests/initiative_06_export.rs:1`, `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:151-157`)
- `initiative_09_internal_unification.rs` is Unix-only and directly asserts public lock module unification with import-replace. (`src-tauri/tests/initiative_09_internal_unification.rs:1-8`, `src-tauri/tests/initiative_09_internal_unification.rs:13-47`, `src-tauri/tests/initiative_09_internal_unification.rs:201-237`)
- `session_metadata_component.rs` is not Unix-gated at the file top in the inspected range, but it uses fixtures from `initiative_06`, which are Unix-gated. (`src-tauri/tests/session_metadata_component.rs:1-14`, `src-tauri/tests/fixtures/initiative_06.rs:1-15`)

### Current release workflow contract details

- The workflow is manual-only through `workflow_dispatch`; release publication is not on every push. (`.github/workflows/release.yml:3-16`)
- `permissions.contents: write` is required for tag creation and release upload. (`.github/workflows/release.yml:17-18`)
- `lint` and `test` run on `ubuntu-latest` only, before version resolution. (`.github/workflows/release.yml:23-67`)
- The `version` job reads `src-tauri/Cargo.toml` version, auto-increments patch if the exact `v<version>` tag exists, and exposes `version` and `tag` outputs. (`.github/workflows/release.yml:68-99`)
- The `build` job currently depends only on `version`, not on `lint` or `test` directly; transitive ordering comes through `version` needing `[lint, test]`. (`.github/workflows/release.yml:68-70`, `.github/workflows/release.yml:100-101`)
- Linux system dependencies are installed only when `runner.os == 'Linux'`; Windows and macOS rely on no analogous system dependency step in the current workflow. (`.github/workflows/release.yml:133-137`)
- The release job checks out, downloads artifacts, creates and pushes the tag, then invokes `softprops/action-gh-release@v2`. (`.github/workflows/release.yml:157-174`)
- Current release job creates the git tag after build artifacts exist, so a build failure prevents tag creation. (`.github/workflows/release.yml:157-170`)

## 2. Cross-platform constraint analysis

### POSIX-only API calls in `session_lock`

- `nix::fcntl::flock` / `FlockArg`:
  - Current use: unconditional import plus exclusive lock and unlock around every critical section. (`src-tauri/src/session_lock/mod.rs:2-3`, `src-tauri/src/session_lock/mod.rs:223-242`)
  - Windows-equivalent primitive: `LockFileEx` over the sentinel file handle is the Windows primitive used by cross-platform locking crates; `fs4` documents Unix `flock(2)` and Windows `LockFileEx`, and its Windows implementation calls `LockFileEx` with whole-file byte range `!0, !0`. (`fs4-1.1.0/src/lib.rs:274-277`, `fs4-1.1.0/src/windows.rs:42-85`)
  - Current contract pressure: `with_flock` blocks until the sentinel is available; it does not use a nonblocking try-lock. (`src-tauri/src/session_lock/mod.rs:223-230`)

- `std::os::fd::AsRawFd`:
  - Current use: converts the sentinel `File` to a raw file descriptor for `flock`. (`src-tauri/src/session_lock/mod.rs:8`, `src-tauri/src/session_lock/mod.rs:223-231`)
  - Windows-equivalent primitive: Windows locking code needs a Windows handle, not a Unix fd; `fs4` crosses the raw-handle boundary at `as_raw_handle() as HANDLE`. (`fs4-1.1.0/src/windows.rs:6-11`, `fs4-1.1.0/src/windows.rs:45-46`)

- `std::os::unix::fs::PermissionsExt::from_mode(0o700)`:
  - Current use: the lock directory is chmodded to owner-only access on Unix. (`src-tauri/src/session_lock/mod.rs:89-92`)
  - Windows-equivalent primitive: there is no `0o700` mode API in `std::os::windows::fs`; equivalent owner-only semantics require either Windows ACL configuration after creation or relying on the default ACL inherited for the current profile/application data directory. The ticket names this as an explicit Phase 3 decision point, not a settled current behavior. (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:49-51`, `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:209-212`)

- `std::os::unix::fs::OpenOptionsExt::mode(0o600)`:
  - Current use: sentinel and temp metadata files are created owner-readable/writable on Unix. (`src-tauri/src/session_lock/mod.rs:95-101`, `src-tauri/src/session_lock/mod.rs:301-309`)
  - Windows-equivalent primitive: `OpenOptionsExt::mode` has no direct Windows equivalent; Windows privacy must be provided after open through ACL APIs or by relying on inherited defaults. (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:49-51`, `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:209-212`)

- `File::open(path).and_then(|dir| dir.sync_all())` on directories:
  - Current use: `fsync_dir` opens directories and syncs them after lock metadata mutation. (`src-tauri/src/session_lock/mod.rs:331-335`)
  - Windows-equivalent risk: Rust `File::open` on a directory and `sync_all` directory semantics are not established by this source file. No Windows-specific confirmation was found in the listed inputs. This should remain an assumption/verification point rather than an implicit guarantee.

- `fs::rename` for lock metadata:
  - Current use: temp metadata file is renamed to final lease/release marker path under the same lock directory. (`src-tauri/src/session_lock/mod.rs:290-321`)
  - Windows-equivalent primitive: Rust documents `std::fs::rename` as `MoveFileEx` with `MOVEFILE_REPLACE_EXISTING` on Windows and reports an error when source and destination are on separate filesystems. (`Rust std::fs::rename docs via web: turn3view2 lines 19-32`)

### `session_replace` rename and hard-link calls

- Current rename calls:
  - `fs::rename(&staging_path, &canonical_records_path)` publishes normalized canonical records from `replace_journal/staging` to `replace_journal/session-<id>.canonical.jsonl`. (`src-tauri/src/session_replace/mod.rs:438-445`, `src-tauri/src/session_replace/mod.rs:498-506`)
  - `fs::rename(&tmp_path, &metadata.jsonl_path)` replaces the provider transcript using a temp path derived from `metadata.jsonl_path.with_extension(...)`. (`src-tauri/src/session_replace/mod.rs:536-548`)
  - `atomic_write_bytes` uses `fs::rename(&tmp, path)` after writing/syncing a temp sibling path. (`src-tauri/src/session_replace/mod.rs:1045-1064`)
  - `move_to_quarantine` uses `fs::rename(path, dest)` to move bad pending journals into `quarantine_dir`. (`src-tauri/src/session_replace/mod.rs:1170-1176`)

- Windows rename behavior:
  - Rust documents `std::fs::rename` as Unix `rename` and Windows `MoveFileEx` with `MOVEFILE_REPLACE_EXISTING`. (`Rust std::fs::rename docs via web: turn3view2 lines 19-23`)
  - Rust documents separate filesystems as an error case. (`Rust std::fs::rename docs via web: turn3view2 lines 25-32`)
  - Same-volume atomicity is the current WU assumption, but the opened Rust docs confirm the Windows primitive and cross-filesystem error, not a formal atomicity guarantee. The existing tests model atomicity through recovery invariants rather than a direct OS-level atomicity probe. (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:52-54`, `src-tauri/tests/initiative_06_import_replace.rs:316-379`, `src-tauri/tests/initiative_06_import_replace.rs:526-575`)
  - Current call-site volume analysis:
    - `staging_path` and `canonical_records_path` are both under `journal_root`, so the current code constructs a same-root rename. (`src-tauri/src/session_replace/mod.rs:438-445`, `src-tauri/src/session_replace/mod.rs:498-506`)
    - `tmp_path` is derived directly from `metadata.jsonl_path.with_extension(...)`, so source and destination are siblings in the same transcript directory. (`src-tauri/src/session_replace/mod.rs:536-548`)
    - `atomic_write_bytes` derives `tmp` from `path.with_extension(...)`, so source and destination are siblings. (`src-tauri/src/session_replace/mod.rs:1045-1053`)
    - `move_to_quarantine` moves a pending journal from `journal_root` to `journal_root/quarantine`; those directories share the same `journal_root`. (`src-tauri/src/session_replace/mod.rs:594-604`, `src-tauri/src/session_replace/mod.rs:1170-1176`)
  - Based on those constructors, current `session_replace` rename call sites should not cross volumes unless the underlying filesystem presents a single path subtree across multiple mounted volumes or reparse points. No such mount/reparse-point handling exists in the file. (`src-tauri/src/session_replace/mod.rs:438-445`, `src-tauri/src/session_replace/mod.rs:536-548`, `src-tauri/src/session_replace/mod.rs:1045-1064`, `src-tauri/src/session_replace/mod.rs:1170-1176`)

- Current hard-link calls:
  - No `std::fs::hard_link` call exists in `src-tauri/src/session_replace/mod.rs` in this worktree. The ticket and D-006 list hard-link publication as a concern, but current code evidence shows rename-only publication. (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:39-40`, `DECISIONS.md:124-129`, `src-tauri/src/session_replace/mod.rs:500-506`, `src-tauri/src/session_replace/mod.rs:540-548`, `src-tauri/src/session_replace/mod.rs:1051-1064`, `src-tauri/src/session_replace/mod.rs:1170-1176`)
  - If a hard-link call is introduced or rediscovered outside the current file, Rust documents `std::fs::hard_link` as `CreateHardLink` on Windows and notes that systems often require both paths to be on the same filesystem. (`Rust std::fs::hard_link docs via web: turn3view4 lines 32-38`)
  - Microsoft documents `CreateHardLink` hard links as multiple directory entries for the same file and requires all hard links to a file to be on the same volume. (`Microsoft CreateHardLinkA docs via web: turn3view3 lines 78-88`)
  - Microsoft's support table says ReFS is not supported for `CreateHardLink`, while NTFS-specific metadata behavior is documented. (`Microsoft CreateHardLinkA docs via web: turn3view3 lines 88-99`)
  - The ticket's note that hard links may require developer mode or `SeCreateSymbolicLinkPrivilege` is not confirmed by the current code or Microsoft `CreateHardLink` documentation reviewed here. The privilege statement is commonly associated with symbolic links, not current `CreateHardLink` docs. This should remain unconfirmed unless Phase 3 explicitly relies on hard links. (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:52-54`, `Microsoft CreateHardLinkA docs via web: turn3view3 lines 78-99`)

- Existing tests and rename assumptions:
  - Import-replace tests expect crash recovery after transcript rename and before DB commit. (`src-tauri/tests/initiative_06_import_replace.rs:316-379`)
  - Import-replace tests expect exactly one concurrent process to win under the lock and no journal pollution. (`src-tauri/tests/initiative_06_import_replace.rs:526-575`)
  - Import-replace tests expect postimage verification failure to leave the pending journal and canonical records path for recovery. (`src-tauri/tests/initiative_06_import_replace.rs:630-669`)
  - These tests are `#![cfg(unix)]`, so they currently do not validate Windows rename behavior. (`src-tauri/tests/initiative_06_import_replace.rs:1`)

### `path_hash_decomposition` Windows-style path assessment

- `derive_claude_workspace_root` identifies a Claude transcript directory by taking the transcript path's parent directory name and feeding it to `decode_claude_project_dir_candidates`. (`src-tauri/src/session_metadata/mod.rs:272-301`)
- `decode_claude_project_dir_candidates` requires the encoded directory name to start with `-`, and an empty rest maps to `PathBuf::from("/")`. (`src-tauri/src/session_metadata/mod.rs:338-344`)
- `decode_claude_rest` always starts candidates at `PathBuf::from("/")`, pushes decoded components, and treats `-` as an ambiguous component separator or literal. (`src-tauri/src/session_metadata/mod.rs:353-388`)
- Existing fixtures encode Claude workspace roots by `raw.trim_start_matches('/').replace('/', "-")`, which assumes slash-separated Unix absolute paths. (`src-tauri/tests/fixtures/initiative_06.rs:886-888`, `src-tauri/tests/fixtures/initiative_06_import_replace.rs:995-997`, `src-tauri/tests/fixtures/initiative_06_export.rs:605-607`)
- Call sites that may pass a Windows-style path:
  - `available_jsonl_path` accepts a transcript locator output through `PathBuf::from(line)`, then canonicalizes it; on Windows, locator output may be `C:\...` or UNC. (`src-tauri/src/sessions/mod.rs:187-198`, `src-tauri/src/session_metadata/mod.rs:213-255`)
  - `derive_claude_workspace_root` receives the canonical JSONL path from `available_jsonl_path`; if the provider is `SessionStorage::ClaudeCode`, a Windows Claude project directory name could reach `decode_claude_project_dir_candidates`. (`src-tauri/src/session_metadata/mod.rs:111-119`, `src-tauri/src/session_metadata/mod.rs:257-336`)
  - `derive_codex_workspace_root` receives `payload.cwd` from provider JSONL; it is not path-hash decomposed but may carry a Windows-style cwd string on Windows. (`src-tauri/src/session_metadata/mod.rs:390-456`)
  - `session_replace::resolve_replace_metadata` uses `locate_session_metadata`, so import-replace inherits the same path-hash behavior. (`src-tauri/src/session_replace/mod.rs:852-863`)
  - `main::run_session_locate` exposes the same result through CLI JSON. (`src-tauri/src/main.rs:705-723`)

## 3. Adjacent / non-target risk surface

- `src-tauri/src/balancer/`, `src-tauri/src/quota/`, and `src-tauri/src/state/db.rs` are explicitly anti-scope for WU-13-01 because they belong to routing-fanout/#36 territory. (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:151-157`, `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:182-186`)
- The public crate exports `balancer`, `quota`, `session_lock`, `session_replace`, and `state` from the same library root. A shared error-type or trait refactor that moves lock behavior into a top-level common module could create compile pressure across anti-scope modules. (`src-tauri/src/lib.rs:1-15`)
- `session_replace` directly imports and mutates `StateDb` and `session_turns` state while holding a `SessionLock`; lock API changes that alter `ReplaceError` or DB update sequencing could accidentally pull `state/db.rs` into scope. (`src-tauri/src/session_replace/mod.rs:7-9`, `src-tauri/src/session_replace/mod.rs:570-579`, `src-tauri/src/session_replace/mod.rs:865-929`)
- `main.rs` imports `balancer`, `quota` through `StateDb`/`InvocationStart`, and `session_lock` in the same CLI binary. A broad CLI error-enum refactor around lock errors would have high adjacency risk. (`src-tauri/src/main.rs:1-16`, `src-tauri/src/main.rs:1288-1378`)
- Routing-fanout tests are isolated under `src-tauri/tests/routing_fanout_rca*` and use `agent_runner_lib::balancer` plus `quota::InFlight`; they should not be edited for a lock/release workflow change. (`src-tauri/tests/routing_fanout_rca.rs:1-2`, `src-tauri/tests/routing_fanout_rca/mod.rs:1-12`)
- Existing Initiative 06 tests that depend on POSIX-specific lock semantics:
  - `initiative_06_pause_handshake.rs` is Unix-only and has subprocess concurrency tests for one active pause winner, stale lock replacement, active second-pause rejection, and release/idempotency behavior. (`src-tauri/tests/initiative_06_pause_handshake.rs:1`, `src-tauri/tests/initiative_06_pause_handshake.rs:94-128`, `src-tauri/tests/initiative_06_pause_handshake.rs:211-300`)
  - `initiative_06_import_replace.rs` is Unix-only and has busy lock, crash recovery, lock-held orphan retention, and concurrent import-replace tests. (`src-tauri/tests/initiative_06_import_replace.rs:1`, `src-tauri/tests/initiative_06_import_replace.rs:295-313`, `src-tauri/tests/initiative_06_import_replace.rs:316-379`, `src-tauri/tests/initiative_06_import_replace.rs:477-575`)
  - Initiative 06 fixtures are Unix-only and use Unix file permissions and symlinks, so making Windows-portable subsets green is not a mechanical target flip. (`src-tauri/tests/fixtures/initiative_06.rs:1-15`, `src-tauri/tests/fixtures/initiative_06_pause_handshake.rs:1-9`, `src-tauri/tests/fixtures/initiative_06_import_replace.rs:1-11`)
- Body-storage / canonical-record adjacency:
  - The deferred empty-bodies RCA identifies direct body storage as a separate work unit covering `state/db.rs`, `sessions`, `session_export`, `session_replace`, `trace`, and scripts. (`/home/nes/projects/agent-runner/worktrees/rca-empty-bodies-ref/research/12-empty-bodies-ref-rca.md:209-228`)
  - WU-13-01 anti-scope explicitly forbids `session_export/`, `session_metadata/` body storage and canonical-record/session_turns schema changes. (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:151-157`, `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:182-186`)

## 4. CI / release-pipeline surface

### Current `release.yml` matrix shape

```yaml
build:
  needs: version
  # Windows is intentionally absent. session_lock and session_replace use
  # POSIX-only primitives (nix::fcntl::flock, AsRawFd, atomic rename and
  # hard-link publication, 0o600 file modes) by design. See DECISIONS.md
  # entry D-006. macOS and Linux are the supported targets.
  strategy:
    fail-fast: false
    matrix:
      include:
        - os: ubuntu-latest
          target: x86_64-unknown-linux-gnu
          bundles: deb
        - os: macos-latest
          target: aarch64-apple-darwin
          bundles: dmg
```

Source: `.github/workflows/release.yml:100-116`.

### Pre-#24 Windows row from `git show 9df5603^ -- .github/workflows/release.yml`

The pre-#24 matrix included this row verbatim:

```yaml
          - os: windows-latest
            target: x86_64-pc-windows-msvc
            bundles: msi,nsis
```

Source: `git show 9df5603^:.github/workflows/release.yml`, lines 105-114 in the numbered output; commit `9df5603` removes this row. (`9df5603 -- .github/workflows/release.yml`)

The pre-#24 Windows collect step was:

```yaml
      - name: Collect artifacts (Windows)
        if: runner.os == 'Windows'
        shell: pwsh
        run: |
          New-Item -ItemType Directory -Force -Path artifacts
          Copy-Item src-tauri/target/${{ matrix.target }}/release/bundle/msi/*.msi artifacts/ -ErrorAction SilentlyContinue
          Copy-Item src-tauri/target/${{ matrix.target }}/release/bundle/nsis/*.exe artifacts/ -ErrorAction SilentlyContinue
          Copy-Item src-tauri/target/${{ matrix.target }}/release/oulipoly-agent-runner.exe artifacts/ -ErrorAction SilentlyContinue
```

Source: `git show 9df5603^:.github/workflows/release.yml`, lines 151-158 in the numbered output; commit `9df5603` deletes this block. (`9df5603 -- .github/workflows/release.yml`)

### Current bare-binary collect and upload pattern

- Linux collect step:
  - `cp .../bundle/deb/*.deb artifacts/`
  - `cp .../release/oulipoly-agent-runner artifacts/`
  - Source: `.github/workflows/release.yml:140-145`
- macOS collect step:
  - `cp .../bundle/dmg/*.dmg artifacts/`
  - `cp .../release/oulipoly-agent-runner artifacts/`
  - Source: `.github/workflows/release.yml:146-151`
- Upload step:
  - `uses: actions/upload-artifact@v4`
  - `name: ${{ matrix.target }}`
  - `path: artifacts/*`
  - Source: `.github/workflows/release.yml:152-155`
- Download/release-publish step:
  - `actions/download-artifact@v4` uses `merge-multiple: true` and `path: artifacts`.
  - `softprops/action-gh-release@v2` uses `files: artifacts/*`.
  - Source: `.github/workflows/release.yml:162-174`

Exact current line ranges to change, depending on where naming is performed:

- Rename-at-collect-time option affects Linux and macOS collect copies and the restored Windows collect copy: current lines `.github/workflows/release.yml:140-151`, plus the pre-#24 Windows collect block at `git show 9df5603^:.github/workflows/release.yml` lines 151-158.
- Rename-at-publish-time option affects the flattened download/publish area: `.github/workflows/release.yml:162-174`.
- Any structural test for this contract must read the workflow shape around matrix rows, collect steps, upload path/name, download `merge-multiple`, and release `files`. (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:114-117`, `.github/workflows/release.yml:100-174`)

### Filename preservation and collision

- `actions/upload-artifact@v4` requires unique artifact names across jobs; the current workflow satisfies that by naming artifacts with `${{ matrix.target }}`. (`.github/workflows/release.yml:152-155`, `actions/upload-artifact README via web: turn3view5 lines 513-530`)
- `actions/download-artifact@v4` with `merge-multiple: true` downloads multiple artifacts into the same directory; the migration docs state same-name files in artifacts collide with last writer wins. (`.github/workflows/release.yml:162-165`, `upload-artifact docs via web: turn4search0`)
- `softprops/action-gh-release` receives the final filesystem paths from `files: artifacts/*`; the action docs describe `with.files` as glob expressions matching files to upload. (`.github/workflows/release.yml:170-174`, `softprops/action-gh-release README via web: turn3view0 lines 360-381`)
- GitHub release assets expose a `name` and reject duplicate asset filenames; therefore any intended release-asset filename change must happen before `softprops/action-gh-release` uploads. (`GitHub REST release assets docs via web: turn3view1 lines 367-367`, `GitHub REST release assets docs via web: turn2search0`)
- Current `.deb` and `.dmg` bundle assets are copied by extension-specific globs and do not share the literal bare filename `oulipoly-agent-runner`; the ticket says the `.deb` and `.dmg` are correct and only the bare binary collides. (`.github/workflows/release.yml:140-151`, `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:25-27`, `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:194-196`)

## 5. Assumption register

- A1: `fs2` and `fs4` are both available on crates.io and provide `lock_exclusive` / `unlock` on both Unix and Windows.
  - Status: unconfirmed as written; availability is confirmed, and equivalent exclusive-lock APIs are confirmed, but `fs4` names the exclusive method `lock`, not `lock_exclusive`.
  - Evidence: `cargo info fs2` reports `fs2 v0.4.3` with crates.io URL; `fs2::FileExt` defines `lock_exclusive` and `unlock`; Unix implementation calls `flock`; Windows implementation calls `LockFileEx` and `UnlockFile`. (`cargo info fs2`, `fs2-0.4.3/src/lib.rs:76-93`, `fs2-0.4.3/src/unix.rs:30-43`, `fs2-0.4.3/src/windows.rs:85-112`)
  - Evidence: `cargo info fs4` reports `fs4 v1.1.0` with crates.io URL; `fs4::FileExt` exposes `lock` for exclusive locking and `unlock`, not a method literally named `lock_exclusive`; docs state Unix `flock(2)` and Windows `LockFileEx`; implementations call those primitives. (`cargo info fs4`, `fs4-1.1.0/src/lib.rs:274-322`, `fs4-1.1.0/src/unix.rs:13-30`, `fs4-1.1.0/src/windows.rs:19-33`, `fs4-1.1.0/src/windows.rs:42-85`)
  - Verification command for any proposal text that requires the literal method name: `rg -n "fn lock_exclusive|fn lock\\(|fn unlock" ~/.cargo/registry/src/index.crates.io-*/fs{2,4}-*/src`.

- A2: Windows file locks via `LockFileEx` are advisory plus per-handle, so they match Unix `flock` semantics for the `SessionLock` use case.
  - Status: confirmed with nuance.
  - Evidence: `fs4` docs say file locks may only be relied upon as advisory, are released when the file handle is closed, and are implemented with Unix `flock(2)` / Windows `LockFileEx`. (`fs4-1.1.0/src/lib.rs:260-277`)
  - Evidence: `fs4` tests assert an exclusive lock blocks another handle's exclusive and shared try-lock until unlock. (`fs4-1.1.0/src/file_ext/sync_impl.rs:132-165`)
  - Nuance: the current `SessionLock` use case serializes cooperating code through a sentinel file and metadata, so advisory/cooperating semantics match the current code's reliance on `flock`; exact Windows handle inheritance and byte-range details are not currently tested in this repository. (`src-tauri/src/session_lock/mod.rs:223-242`, `DECISIONS.md:139-149`)

- A3: `session_replace` rename calls never cross volumes (single scratch dir to single sessions dir).
  - Status: confirmed for current constructors, with mount/reparse-point caveat.
  - Evidence: canonical staging and final canonical records are both under `journal_root`; transcript temp and final paths are siblings under `metadata.jsonl_path`; `atomic_write_bytes` uses a sibling temp path; quarantine is under `journal_root`. (`src-tauri/src/session_replace/mod.rs:438-445`, `src-tauri/src/session_replace/mod.rs:498-506`, `src-tauri/src/session_replace/mod.rs:536-548`, `src-tauri/src/session_replace/mod.rs:1045-1064`, `src-tauri/src/session_replace/mod.rs:1170-1176`)
  - Caveat: no code verifies device/volume identity, so unusual reparse/mount arrangements under the same path subtree remain unverified. Verification command for implementation phase: run the Windows test suite on a normal app-data path and, if needed, add a diagnostic test that compares volume identity for temp/final paths.

- A4: `session_replace` hard-link calls happen on platforms where the destination is an NTFS or POSIX filesystem (inside project data dir).
  - Status: unconfirmed / currently not applicable.
  - Evidence: current `src-tauri/src/session_replace/mod.rs` contains no `hard_link` call; current publication is rename-based. (`src-tauri/src/session_replace/mod.rs:500-506`, `src-tauri/src/session_replace/mod.rs:540-548`, `src-tauri/src/session_replace/mod.rs:1051-1064`, `src-tauri/src/session_replace/mod.rs:1170-1176`)
  - Verification command needed if a hard-link path is introduced or found: `rg -n "hard_link|CreateHardLink|linkat|std::fs::hard_link" src-tauri/src src-tauri/tests`.

- A5: The `oulipoly-agent-runner` binary is the only artifact whose name collides; `.deb`, `.dmg`, `.msi` already have platform-distinct extensions or names.
  - Status: confirmed for current Linux/macOS workflow and pre-#24 Windows shape, with release-run verification still required for generated bundle names.
  - Evidence: Linux and macOS collect steps both copy bare `oulipoly-agent-runner`; bundle copies use `*.deb` and `*.dmg`; pre-#24 Windows bundle copies used `*.msi` and NSIS `*.exe`, plus bare `oulipoly-agent-runner.exe`. (`.github/workflows/release.yml:140-151`, `git show 9df5603^:.github/workflows/release.yml` lines 151-158)
  - Evidence: the ticket reports `.deb` and `.dmg` are correct and only the bare binary collides. (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:25-27`, `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:194-196`)
  - Verification command for generated Windows bundle names: inspect `src-tauri/target/x86_64-pc-windows-msvc/release/bundle/{msi,nsis}/` after a Windows release build.

- A6: `dtolnay/rust-toolchain@stable` supports `targets: x86_64-pc-windows-msvc` on the `windows-latest` runner.
  - Status: confirmed for current workflow shape and pre-#24 precedent.
  - Evidence: the current workflow passes `${{ matrix.target }}` into `dtolnay/rust-toolchain@stable` `targets`; the pre-#24 workflow included `x86_64-pc-windows-msvc` in the matrix. (`.github/workflows/release.yml:126-128`, `git show 9df5603^:.github/workflows/release.yml` lines 105-114)
  - Local evidence: `rustc --print target-list | rg '^x86_64-pc-windows-msvc$'` prints the target triple in the local toolchain.

## 6. Risk hotspots

Ranked highest first:

1. Locking-abstraction shape: external crate (`fs2`/`fs4`) versus hand-rolled `cfg`-gated module.
   - Current risk driver: `SessionLock::with_flock` is the serialization point for both pause/resume and import-replace, and the public tests assert cross-module visibility and process-shared behavior. (`src-tauri/src/session_lock/mod.rs:223-242`, `src-tauri/src/main.rs:1303-1378`, `src-tauri/src/session_replace/mod.rs:468-487`, `src-tauri/tests/initiative_09_internal_unification.rs:13-47`)
   - Gate question for Phase 3: whether the abstraction preserves blocking exclusive sentinel semantics, release-on-handle-close behavior, public error mapping, and testable Windows exclusivity without broad public API churn. (`DECISIONS.md:9-32`, `src-tauri/src/session_lock/mod.rs:31-42`)

2. Windows ACL strategy: explicit restrictive DACL versus accepting Windows default ACLs with a single-user equivalence argument.
   - Current risk driver: Unix explicitly applies `0o700` to the lock dir and `0o600` to sentinel/temp metadata; the ticket requires semantically equivalent current-user read/write behavior or documented limitation. (`src-tauri/src/session_lock/mod.rs:89-100`, `src-tauri/src/session_lock/mod.rs:301-306`, `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:49-51`, `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:209-212`)
   - Gate question for Phase 3: whether default app-data ACL inheritance is sufficient evidence or whether explicit ACL code is required to preserve the current privacy contract.

3. Artifact rename location: collect-time versus publish-time.
   - Current risk driver: same internal bare filename is uploaded per target, then `download-artifact` flattens all artifacts into one directory before `softprops/action-gh-release` uploads. (`.github/workflows/release.yml:140-155`, `.github/workflows/release.yml:162-174`, `upload-artifact docs via web: turn4search0`)
   - Gate question for Phase 3: where the workflow can make the final `artifacts/*` filenames unambiguous while preserving `.deb`, `.dmg`, `.msi`, and NSIS conventional names. (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:114-122`, `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:194-196`)

4. Structural release workflow test location: `src-tauri/tests/release_yml_contract.rs` versus workflow-level YAML lint.
   - Current risk driver: the ticket explicitly allows a Rust integration test under `src-tauri/tests/` and requires parsing `release.yml` for artifact naming. (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:169-172`, `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:114-117`)
   - Gate question for Phase 3: which test home provides stable line-of-defense without needing frontend/e2e infrastructure, which is out of scope. (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:177-180`)

5. Path-hash Windows behavior.
   - Current risk driver: Claude path-hash decomposition currently constructs Unix-rooted candidates and tests encode paths by replacing `/`; Windows-style drive or UNC paths are not represented in current tests. (`src-tauri/src/session_metadata/mod.rs:338-388`, `src-tauri/tests/fixtures/initiative_06.rs:886-888`, `src-tauri/tests/session_metadata_component.rs:272-323`)
   - Gate question for Phase 3: whether Windows support requires hardening Claude path-hash before release, or whether no supported Windows provider path currently reaches Claude Code storage.

6. Existing Unix-only tests versus portable subset.
   - Current risk driver: Initiative 06 tests and fixtures are `#![cfg(unix)]`, while AC-4 requires existing tests stay green on Linux/macOS and at least the platform-portable subset run green on Windows. (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:108-112`, `src-tauri/tests/initiative_06_import_replace.rs:1`, `src-tauri/tests/fixtures/initiative_06_import_replace.rs:1-11`)
   - Gate question for Phase 3: which tests are portable lock contract tests versus Unix-only fixture tests, without broad fixture rewrites outside the WU boundary.

## 7. Cross-WU non-interference

- No overlap with #36 routing-fanout territory is confirmed by the WU anti-scope and current touched-surface needs.
  - WU-13-01 explicitly excludes `src-tauri/src/balancer/`, `src-tauri/src/quota/`, and `src-tauri/src/state/db.rs`, and forbids deleting routing reproduction harnesses. (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:151-157`, `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:182-193`)
  - Routing-fanout reproduction tests live under `src-tauri/tests/routing_fanout_rca*` and import balancer/quota directly, not `session_lock`. (`src-tauri/tests/routing_fanout_rca.rs:1-2`, `src-tauri/tests/routing_fanout_rca/mod.rs:1-12`)
  - Current WU scope files do not need balancer or quota code changes to compile on Windows unless a lock refactor is over-broadened. (`src-tauri/src/session_lock/mod.rs:1-12`, `src-tauri/src/session_replace/mod.rs:1-18`, `src-tauri/src/session_metadata/mod.rs:1-9`)

- No overlap with the deferred body-storage WU is confirmed, with one adjacency warning.
  - The deferred RCA covers direct DB body storage, body ingestion, export body source, trace inline transcript, and related files. (`/home/nes/projects/agent-runner/worktrees/rca-empty-bodies-ref/research/12-empty-bodies-ref-rca.md:1-40`, `/home/nes/projects/agent-runner/worktrees/rca-empty-bodies-ref/research/12-empty-bodies-ref-rca.md:209-237`)
  - WU-13-01 anti-scope explicitly excludes `session_export/`, body storage, canonical-record/session_turns schema changes, and frontend/e2e. (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:151-160`, `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:182-186`)
  - Adjacency warning: `session_replace` currently rewrites `session_turns` from canonical records after transcript replacement; lock or rename changes must not alter canonical record shape, body fields, or session_turns schema. (`src-tauri/src/session_replace/mod.rs:865-929`, `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:182-186`)

- No overlap with canonical-record / `session_turns` schema is confirmed.
  - WU-13-01 does not require changing `CanonicalRecord`, content chunks, renderers, or `replace_db_turns`; those are adjacent only because import-replace uses them inside the locked critical section. (`src-tauri/src/session_replace/mod.rs:20-20`, `src-tauri/src/session_replace/mod.rs:213-284`, `src-tauri/src/session_replace/mod.rs:732-849`, `src-tauri/src/session_replace/mod.rs:865-929`)
  - The state schema for `session_turns` is in `src-tauri/src/state/db.rs`, which the ticket forbids touching. (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:151-157`, `src-tauri/src/state/db.rs:628-673`, `src-tauri/src/state/db.rs:1017-1044`)
  - Structural release workflow testing and Windows lock testing can be added without schema migrations or canonical serialization changes if kept within the ticket's Code Boundary. (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:134-150`, `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:162-180`)
