# Phase 8 Test-Audit Gate - WU-13-01 release-restore

## 1. Verdict

```text
verdict: LOW
```

Audit scope: actual branch diff `main..HEAD` on `impl/wu-13-01`, focused on
whether the added/retained tests satisfy the contract test boundary and the
proposal test-intent track. The binding contract names AC-1 through AC-8 at
`product-strategy/contracts/wu-13-01-release-restore.md:21-54`, requires the
portable `SessionLock` test file at
`product-strategy/contracts/wu-13-01-release-restore.md:213-252`, requires the
structural YAML test at
`product-strategy/contracts/wu-13-01-release-restore.md:254-294`, and defines
the observable local/CI/release signals at
`product-strategy/contracts/wu-13-01-release-restore.md:365-373`.

Local audit execution:

- `cd src-tauri && cargo test --test session_lock_cross_platform -- --nocapture`
  passed: 4 normal tests passed; 1 ignored helper test was spawned through the
  parent test and passed under exact filtering.
- `cd src-tauri && cargo test --test release_yml_contract -- --nocapture`
  passed: 1 test passed.
- `git diff --name-only main..HEAD -- src-tauri/tests/initiative_06_*`
  returned no paths.
- Full Rust, frontend, Windows target, and release-pipeline gates were not
  rerun by this audit; the contract assigns those to CI/release evidence.

Input note: the prompt-requested worktree path
`tmp/scratch/wu-13-01/phase6/step6b-output-index.md` is absent in this
worktree, but the corresponding external Step 6b index exists at
`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/phase6/step6b-output-index.md`.
That external index names the two produced test files at lines 3-7, lists
per-test invariants at lines 8-46, records no added dev-dependencies at
lines 48-59, documents the inline `current_exe()` helper shape at lines 61-67,
and records residuals at lines 83-88. The verdict below is based on the actual
branch diff plus that corroborating process artifact.

## 2. Acceptance-criteria coverage

| AC | Status | Asserting test / evidence | Fixture-external | Promised level | Concrete assertions | Finding |
|---|---|---|---|---|---|---|
| AC-1 | covered | `src-tauri/tests/release_yml_contract.rs:release_yml_restores_windows_and_target_suffixed_bare_binaries` | Yes. The test reads the repository workflow file through `../.github/workflows/release.yml`, not a production data dir or shared runtime location (`src-tauri/tests/release_yml_contract.rs:8-12`). | Structural workflow integration test, matching the contract's release workflow test boundary (`product-strategy/contracts/wu-13-01-release-restore.md:254-294`). | The test asserts matrix length 3 and the exact Linux/macOS/Windows `(os, target, bundles)` rows, including `windows-latest`, `x86_64-pc-windows-msvc`, and `msi,nsis` (`src-tauri/tests/release_yml_contract.rs:14-46`). It also asserts the restored Windows collect step guard and bare-binary `.exe` suffix (`src-tauri/tests/release_yml_contract.rs:152-170`). | none |
| AC-2 | covered | n/a - CI evidence | n/a | Component build verification, as proposed (`proposals/13-release-restore.md:501-510`). | The diff removes the POSIX-only `nix::fcntl` / `AsRawFd` imports, keeps Unix mode traits under `#[cfg(unix)]`, and uses `fs4::FileExt::lock` / `unlock` (`src-tauri/src/session_lock/mod.rs:1-8`, `src-tauri/src/session_lock/mod.rs:83-99`, `src-tauri/src/session_lock/mod.rs:220-235`). Windows target compilation remains CI evidence under the contract (`product-strategy/contracts/wu-13-01-release-restore.md:24-27`, `product-strategy/contracts/wu-13-01-release-restore.md:372-373`). | none |
| AC-3 | covered | `src-tauri/tests/session_lock_cross_platform.rs:test_single_process_busy`; `test_release_idempotency`; `test_cross_process_exclusivity`; `test_acquire_after_release_succeeds` | Yes. Each parent test creates isolated scratch state with `tempfile::tempdir()` (`src-tauri/tests/session_lock_cross_platform.rs:16-17`, `src-tauri/tests/session_lock_cross_platform.rs:52-53`, `src-tauri/tests/session_lock_cross_platform.rs:81-85`, `src-tauri/tests/session_lock_cross_platform.rs:122-123`). | Particular-integration `SessionLock` coverage, as proposed (`proposals/13-release-restore.md:514-548`). | The tests assert single-process `LockError::Busy` with nonempty `expires_at` and `Some(token_hash)`, idempotent release, bad-token `TokenInvalid`, cross-process sibling-held `Busy`, child success, and acquire-after-release named values (`src-tauri/tests/session_lock_cross_platform.rs:23-47`, `src-tauri/tests/session_lock_cross_platform.rs:58-76`, `src-tauri/tests/session_lock_cross_platform.rs:88-117`, `src-tauri/tests/session_lock_cross_platform.rs:125-139`). | none |
| AC-4 | covered | Existing `src-tauri/tests/initiative_06_import_replace.rs` suite plus AC-3 lock tests; Windows side is n/a - CI evidence | Yes for new AC-3 lock evidence via `tempfile`; the proposal explicitly keeps Initiative 06 Unix import-replace tests as the runtime atomicity evidence (`proposals/13-release-restore.md:559-572`). | Existing particular-integration tests plus Windows build/lock evidence (`proposals/13-release-restore.md:559-578`). | The contract requires existing Initiative 06 tests to stay green on Linux/macOS and Windows AC-4 to be build-only (`product-strategy/contracts/wu-13-01-release-restore.md:32-36`). The branch diff does not modify `src-tauri/tests/initiative_06_*`, and the new AC-3 tests exercise the shared lock behavior around acquire/release and sibling-process exclusion (`src-tauri/tests/session_lock_cross_platform.rs:80-117`). | none |
| AC-5 | covered | `src-tauri/tests/release_yml_contract.rs:release_yml_restores_windows_and_target_suffixed_bare_binaries` | Yes. The fixture is the repo workflow file parsed from a relative path (`src-tauri/tests/release_yml_contract.rs:8-12`). | Particular-integration structural YAML test (`proposals/13-release-restore.md:587-623`). | The test parses YAML with `serde_yml::from_str` (`src-tauri/tests/release_yml_contract.rs:11-12`), asserts exact matrix rows (`src-tauri/tests/release_yml_contract.rs:14-46`), per-OS collect-step guards and target-suffixed bare binaries (`src-tauri/tests/release_yml_contract.rs:48-76`, `src-tauri/tests/release_yml_contract.rs:105-131`, `src-tauri/tests/release_yml_contract.rs:152-170`), bundle globs and forbidden cross-platform bundle substrings (`src-tauri/tests/release_yml_contract.rs:77-103`, `src-tauri/tests/release_yml_contract.rs:124-150`, `src-tauri/tests/release_yml_contract.rs:171-206`), upload/download/release wiring (`src-tauri/tests/release_yml_contract.rs:208-262`), and bare-binary token locality (`src-tauri/tests/release_yml_contract.rs:264-273`). | none |
| AC-6 | covered | n/a - release-pipeline evidence | n/a | Release-pipeline integration evidence, not a local structural test (`proposals/13-release-restore.md:633-670`). | The residual file correctly states that structural YAML cannot prove real `workflow_dispatch` publication and preserves the required evidence fields (`risk/13-release-restore-test-residuals.md:25-29`). The contract likewise records AC-6 as an external release-run evidence record (`product-strategy/contracts/wu-13-01-release-restore.md:41-48`, `product-strategy/contracts/wu-13-01-release-restore.md:386-388`). | none |
| AC-7 | covered | n/a - CI evidence plus structural diff check | Yes. No existing Initiative 06 fixture/test file was edited. | Component and integration verification (`proposals/13-release-restore.md:690-698`). | Targeted local commands for the two added test files passed. The structural diff check returned no `src-tauri/tests/initiative_06_*` modifications, satisfying the AC-7 regression-shape check requested by this audit. Full Rust/frontend gates remain the broader CI/local evidence named by the contract (`product-strategy/contracts/wu-13-01-release-restore.md:49-51`, `product-strategy/contracts/wu-13-01-release-restore.md:370-371`). | none |
| AC-8 | covered | n/a - documentation structural review | n/a | Documentation contract plus grep/structural review (`proposals/13-release-restore.md:703-728`). | D-006 now states Windows is supported, names `fs4`, `flock(2)`, `LockFileEx`, Unix `0o700`/`0o600`, Windows default ACL inheritance, same-root/sibling `rename`, and platform-suffixed bare binary names (`DECISIONS.md:122-146`). Search found no retained "Unix-only by design", "No Windows shim", "Windows removed", or "Windows is not a supported target" framing in the modified D-006 surface (`DECISIONS.md:122-146`; proposal forbids those phrases at `proposals/13-release-restore.md:725-726`). | none |

## 3. Risk-reduction check

### AC-3: cross-process exclusivity

The test actually spawns a sibling process and asserts `LockError::Busy`; it
does not rely on the forbidden single-process-only shortcut. In
`test_cross_process_exclusivity`, the parent creates a temp lock directory and
ready/release sentinel paths, calls `spawn_helper`, waits for the helper's
ready sentinel, then attempts `parent_lock.acquire(...)` and requires
`Err(LockError::Busy { expires_at, token_hash })`
(`src-tauri/tests/session_lock_cross_platform.rs:80-104`).

The sibling is an ignored test in the same integration-test binary. The parent
starts it with `Command::new(env::current_exe().unwrap())`, exact ignored-test
filtering, and environment variables for lock dir, ready path, release path,
session id, and provider name (`src-tauri/tests/session_lock_cross_platform.rs:142-166`,
`src-tauri/tests/session_lock_cross_platform.rs:168-181`). The helper acquires
the lease, writes `ready`, waits for `release`, and releases its own token
(`src-tauri/tests/session_lock_cross_platform.rs:155-165`). That matches the
contract's sibling-helper requirement and timeout guidance
(`product-strategy/contracts/wu-13-01-release-restore.md:230-246`) and closes
SHORT-02's cross-process shortcut risk (`risk/13-release-restore-shortcut.md:55-82`).

The source shape is portable: the file contains ordinary `#[test]` functions
and no `#![cfg(unix)]` file gate (`src-tauri/tests/session_lock_cross_platform.rs:14-15`,
`src-tauri/tests/session_lock_cross_platform.rs:50-51`,
`src-tauri/tests/session_lock_cross_platform.rs:79-80`,
`src-tauri/tests/session_lock_cross_platform.rs:120-121`). Windows execution of
the helper remains external CI evidence, which is already documented as a
residual (`risk/13-release-restore-test-residuals.md:17-23`).

### AC-5: structural YAML test

The release-workflow test parses YAML and asserts property invariants; it does
not use a flat string-grep shortcut. The test loads
`.github/workflows/release.yml`, parses it into `serde_yml::Value`, and then
navigates the parsed structure through `sequence_at`, `string_at`, `bool_at`,
`step_by_name`, and `step_by_uses` helpers
(`src-tauri/tests/release_yml_contract.rs:8-18`,
`src-tauri/tests/release_yml_contract.rs:320-377`).

The assertions cover the property set called out by SHORT-03: exact matrix
length and target triples, collect-step guards, target-suffixed bare binary
paths including Windows `.exe`, upload artifact target naming, release-job
download merge/path, `softprops/action-gh-release@v2` files path, and bundle
globs tied to matrix targets (`risk/13-release-restore-shortcut.md:84-115`;
`src-tauri/tests/release_yml_contract.rs:14-46`,
`src-tauri/tests/release_yml_contract.rs:48-206`,
`src-tauri/tests/release_yml_contract.rs:208-273`). The remaining
`.contains(...)` checks are scoped to parsed step `run` strings, which the
contract expressly required for command substrings
(`product-strategy/contracts/wu-13-01-release-restore.md:271-290`).

### AC-7: Initiative 06 structural diff

The structural diff check did not find modifications under
`src-tauri/tests/initiative_06_*`. This preserves the proposal's selected
AC-4/AC-7 evidence model: do not port or rewrite the Initiative 06 Unix
fixtures in this WU; keep them green on Unix and use Windows build/lock
evidence for the Windows side (`proposals/13-release-restore.md:559-572`,
`proposals/13-release-restore.md:679-696`). The unchanged file set also means
the new tests did not weaken, delete, or baseline-update those existing
atomicity/recovery checks.

## 4. Findings

No findings.

Notes that do not affect the LOW verdict:

- The worktree-local `tmp/scratch/wu-13-01/phase6/step6b-output-index.md`
  path is absent, while the external trunk copy exists and matches the test
  files under review (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/phase6/step6b-output-index.md:3-7`).
- `git diff --check main..HEAD` reports pre-existing trailing whitespace in
  proposal/review markdown files, not in the two added test files. This is
  outside the requested test-intent coverage gate.

## 5. Verdict justification

The tests satisfy the intended coverage with fixture-external, concrete
assertions at the promised levels. AC-3 uses isolated temp lock dirs and a real
sibling-process helper to prove `LockError::Busy`, with idempotency,
bad-token, and acquire-after-release assertions also covered
(`src-tauri/tests/session_lock_cross_platform.rs:16-47`,
`src-tauri/tests/session_lock_cross_platform.rs:50-117`,
`src-tauri/tests/session_lock_cross_platform.rs:120-181`). AC-5 parses the
workflow YAML and asserts the target matrix, Windows collect restoration,
target-suffixed bare binaries, bundle globs, and upload/download/release shape
without resorting to a fragile raw grep
(`src-tauri/tests/release_yml_contract.rs:8-46`,
`src-tauri/tests/release_yml_contract.rs:48-273`). The remaining ACs are either
covered by those structural tests, deliberately assigned to existing
Initiative 06 tests, or correctly recorded as CI/release-pipeline evidence per
the contract and residuals (`product-strategy/contracts/wu-13-01-release-restore.md:32-54`,
`risk/13-release-restore-test-residuals.md:1-29`). No AC has a partial or
uncovered test-intent shortfall, and neither named shortcut risk is present.
