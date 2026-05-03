# WU-13-01 Release Restore Proposal

Phase: 3 proposal  
Work unit: `release-restore`  
Intent: restore Windows release support and fix bare-binary release asset
collision without changing routing, body storage, frontend, or e2e surfaces.

## 1. Anti-scope

Ticket anti-scope, restated verbatim:

- Do NOT change the routing/balancer code paths landed in #36.
- Do NOT touch session_turns / canonical-record / body-storage paths
  (deferred WU).
- Do NOT ship Windows-specific behavior that diverges from Unix in user-
  observable ways unless it's necessary for the platform (e.g. ACL
  semantics differ; that's OK).
- Do NOT introduce a backwards-compatibility shim for the old
  POSIX-only API (`~/ai/conventions/no-backwards-compatibility.md`).
- Do NOT delete the existing reproduction harnesses for routing
  (`src-tauri/tests/routing_fanout_rca/`) — they belong to #36.
- Do NOT remove the GitHub release asset naming for `.deb` / `.dmg`;
  only the BARE binary needs platform-suffixed naming. The bundle
  artifacts should stay conventionally named.

Source: ticket anti-scope at
`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:182-196`.

Additional out-of-scope items carried from the problem map:

- No changes to `src-tauri/src/balancer/`, `src-tauri/src/quota/`, or
  `src-tauri/src/state/db.rs`; those are routing-fanout/#36 territory and
  the problem map confirms they are non-target risk surface.
  Source: ticket code boundary at
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:151-154`;
  problem map § 3 — Adjacent / non-target risk surface.
- No changes to canonical-record shape, `session_turns` schema, body fields,
  or DB replacement semantics. `session_replace` uses these surfaces inside
  the lock, but this WU only changes the platform support around the lock and
  release workflow.
  Source: problem map § 7 — Cross-WU non-interference; deferred body-storage
  RCA named `research/12-empty-bodies-ref-rca.md` at
  `/home/nes/projects/agent-runner/worktrees/rca-empty-bodies-ref/research/12-empty-bodies-ref-rca.md:1-40`
  and
  `/home/nes/projects/agent-runner/worktrees/rca-empty-bodies-ref/research/12-empty-bodies-ref-rca.md:209-237`.
- No backwards-compatibility shim for the POSIX-only `SessionLock` internals.
  The old direct `nix::fcntl::flock` implementation is replaced, not kept as
  a second public API path.
  Source: no-compatibility convention at
  `/home/nes/ai/conventions/no-backwards-compatibility.md:1-35`; ticket at
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:190-191`.
- No changes to `.github/workflows/ci.yml`. The ticket test boundary requires
  existing CI to stay green, but only `.github/workflows/release.yml` is in
  the code boundary unless Phase 6 finds a hard blocker.
  Source: ticket code boundary at
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:146-149`;
  AC-7 at
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:124-126`.
- No frontend edits under `src/`.
  Source: ticket out-of-scope at
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:158-160`.
- No e2e or Playwright test edits.
  Source: ticket test boundary at
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:177-180`.
- No broad rewrite of Initiative 06 Unix-only fixtures. Existing Unix tests
  remain green on Linux/macOS; Windows gets new portable lock/release tests
  rather than a wholesale fixture port.
  Source: problem map § 6 — Risk hotspots, item 6.

## 2. Supported-surface track

The current supported surface is a local desktop/control-plane CLI that must
publish working release binaries for Linux, macOS, and Windows. The concrete
current risk is that Windows was removed from `release.yml` and Linux users can
receive a macOS bare binary under the unsuffixed `oulipoly-agent-runner` asset
name.
Source: ticket symptom at
`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:20-27`;
release workflow current matrix at `.github/workflows/release.yml:100-155`.

### a. Locking primitive provider

Chosen option: external crate `fs4`.

The proposal replaces the private `SessionLock::with_flock` implementation
with `fs4::FileExt::lock` and `fs4::FileExt::unlock` over the existing
sentinel `File`. The public `SessionLock`, `Lease`, `ReleaseReceipt`, and
`LockError` shapes stay owned by `src-tauri/src/session_lock/mod.rs`; there is
no compatibility alias or parallel POSIX-only API.
Source: current public lock surface at `src-tauri/src/session_lock/mod.rs:14-48`;
current lock serialization point at `src-tauri/src/session_lock/mod.rs:223-242`;
no-compat convention at `/home/nes/ai/conventions/no-backwards-compatibility.md:16-35`.

Justification:

- Maintenance status: the ticket identifies `fs4` as the actively maintained
  successor to `fs2`, and `cargo info fs4` reports `fs4 v1.1.0` with repository
  `https://github.com/al8n/fs4`.
  Source: ticket note at
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:200-203`;
  local `cargo info fs4` output; `fs4-1.1.0/Cargo.toml:15-43`.
- Transitive dependency count: this adds one direct dependency to
  `src-tauri/Cargo.toml`. With default `sync`, `fs4` uses `rustix` on
  non-Windows and `windows-sys` on Windows; the optional async and wrapper-file
  dependencies are not enabled.
  Source: `src-tauri/Cargo.toml:10-30`; `fs4-1.1.0/Cargo.toml:52-70`;
  `fs4-1.1.0/Cargo.toml:127-137`.
- MSRV alignment: `fs4` declares `rust-version = "1.75.0"`, while the local
  toolchain is `rustc 1.92.0`; Phase 6 will verify against the actual runner
  stable toolchain.
  Source: `fs4-1.1.0/Cargo.toml:12-16`; local `rustc --version`.
- Semantic equivalence: `fs4` documents advisory whole-file locks, automatic
  release when the file handle closes, Unix `flock(2)`, and Windows
  `LockFileEx`. That matches the current `SessionLock` use of a cooperating
  sentinel file for exclusive critical-section serialization.
  Source: `fs4-1.1.0/src/lib.rs:252-277`;
  `fs4-1.1.0/src/lib.rs:298-322`;
  problem map § 2 — POSIX-only API calls in `session_lock`.
- Blocking behavior: `fs4::FileExt::lock` is the blocking exclusive operation,
  matching current `FlockArg::LockExclusive`, while `try_lock` remains unused.
  Source: `fs4-1.1.0/src/lib.rs:294-303`;
  current code at `src-tauri/src/session_lock/mod.rs:223-230`.

Immediate consequence:

- File count: modifies `src-tauri/src/session_lock/mod.rs` and
  `src-tauri/Cargo.toml`; no new production module is required by this choice.
- Dependency count: adds one direct dependency, `fs4`; Cargo resolves platform
  dependencies through `fs4`.
- Review surface: focused on one private locking helper plus Cargo dependency
  review and cross-platform tests.

Rejected options:

- `fs2` is rejected because the ticket and problem map classify it as the
  legacy option while `fs4` is the active successor. It also uses the older
  `lock_exclusive` API name and depends on legacy platform crates; using it
  would reduce API churn slightly but choose the less maintained path.
  Source: ticket note at
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:200-203`;
  problem map assumption A1.
- A hand-rolled `cfg` module around `nix::fcntl::flock` and
  `windows-sys::LockFileEx` is rejected because it increases file count,
  unsafe/FFI review surface, and platform byte-range behavior ownership
  without adding product-specific semantics beyond what `fs4` already exposes.
  Source: problem map § 6 — Risk hotspots, item 1.

### b. File-mode permissions strategy

Chosen option: keep `0o700`/`0o600` on Unix; rely on Windows default per-user
ACL inheritance and document the single-user equivalence in `DECISIONS.md`.

The implementation should preserve Unix `PermissionsExt::from_mode(0o700)` for
the lock directory and `OpenOptionsExt::mode(0o600)` for sentinel and temp lock
metadata files. On Windows, it should not add explicit DACL construction in
this WU; it should use the default ACLs of the current user's profile/app-data
location and record that decision in the replacement D-006 entry.
Source: current Unix mode code at `src-tauri/src/session_lock/mod.rs:87-101`
and `src-tauri/src/session_lock/mod.rs:290-309`; ticket choice point at
`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:49-51`
and `:209-212`.

Justification:

- Blast radius: explicit DACL code would require direct Windows security API
  calls and new failure paths in lock setup; default ACL inheritance avoids
  introducing `windows-sys::Win32::Security` code in the critical lock path.
  Source: problem map § 6 — Risk hotspots, item 2.
- Code volume: the current production change can stay localized to Unix-gated
  mode calls and `fs4` locking, without new ACL builders, SID lookup, or
  security descriptor serialization code.
  Source: current lock setup at `src-tauri/src/session_lock/mod.rs:87-103`.
- User value: this app coordinates local developer CLIs on a single machine.
  For that deployment, the meaningful Windows equivalent is current-user
  profile/app-data access, not multi-user hardening through bespoke DACL code.
  This is a documented platform-semantic difference allowed by the ticket when
  necessary.
  Source: ticket anti-scope at
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:187-189`;
  DECISIONS D-006 current local-CLI framing at `DECISIONS.md:139-155`.

Immediate consequence:

- File count: no new Windows security helper file.
- Dependency count: no direct `windows-sys::Win32::Security` dependency is
  added by application code.
- Review surface: DECISIONS documentation and tests for functional locking,
  not Windows ACL correctness.

Rejected option:

- Explicit restrictive DACLs are rejected for this WU because they would add
  significant platform-specific code and review risk for marginal value to a
  single-user local developer binary. If future requirements need multi-user
  hardening, that should be a separate security WU with dedicated Windows ACL
  tests.
  Source: problem map § 6 — Risk hotspots, item 2.

### c. Atomic-replace verification

Chosen option: keep `std::fs::rename` calls as-is and document that current
constructors keep source and destination in the same directory tree, with the
Windows caveats surfaced by the problem map.

This proposal does not introduce a `cfg`-gated rename wrapper and does not add
new hard-link behavior. Phase 6 must run `rg` to confirm no current
`std::fs::hard_link` use exists before implementation; if a hard-link call is
rediscovered outside the mapped surface, implementation must stop and return to
research because the current problem map says `session_replace` publication is
rename-only.
Source: problem map § 2 — `session_replace` rename and hard-link calls;
current rename calls at `src-tauri/src/session_replace/mod.rs:500-506`,
`src-tauri/src/session_replace/mod.rs:536-548`,
`src-tauri/src/session_replace/mod.rs:1045-1064`, and
`src-tauri/src/session_replace/mod.rs:1170-1176`.

Justification:

- The current code constructs same-root/sibling renames: staging and canonical
  side file under `journal_root`, transcript temp and final path as siblings,
  `atomic_write_bytes` temp and final path as siblings, and quarantine under
  `journal_root`.
  Source: problem map § 2 — Windows rename behavior; code at
  `src-tauri/src/session_replace/mod.rs:436-445`,
  `src-tauri/src/session_replace/mod.rs:498-506`,
  `src-tauri/src/session_replace/mod.rs:536-548`,
  `src-tauri/src/session_replace/mod.rs:1045-1064`, and
  `src-tauri/src/session_replace/mod.rs:1170-1176`.
- Rust documents Windows `std::fs::rename` as `MoveFileEx` with
  replacement behavior and cross-filesystem errors; the existing recovery
  tests verify crash/atomicity at the behavioral level on Unix.
  Source: problem map § 2 — Windows rename behavior; test inventory at problem
  map § 2 — Existing tests and rename assumptions.

Immediate consequence:

- File count: no production wrapper file.
- Dependency count: no dependency change for rename.
- Review surface: `session_replace` is inspected and documented, with only
  minor code edits if Phase 6 finds a compile or same-volume invariant blocker.

Rejected options:

- A `cfg`-gated rename wrapper is rejected because it would abstract a standard
  library primitive without changing the observed invariant and would add
  review surface in a crash-recovery path.
- An explicit import-replace same-volume rejection is rejected for now because
  current paths are already constructed as siblings/same-root paths; reliable
  Windows volume-identity checks would add platform code outside the evidence
  needed for this WU. The residual mount/reparse-point caveat remains in the
  assumption register.
  Source: problem map assumption A3 and § 2 — Windows rename behavior.

### d. `release.yml` artifact-naming strategy

Chosen option: rename the bare binary at collect-time.

Linux and macOS collect steps should copy the bare binary to
`artifacts/oulipoly-agent-runner-${{ matrix.target }}`. The restored Windows
collect step should copy the bare binary to
`artifacts/oulipoly-agent-runner-${{ matrix.target }}.exe`. Bundle globs for
`.deb`, `.dmg`, `.msi`, and NSIS `.exe` remain conventionally named.
Source: ticket artifact examples at
`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:79-89`;
ticket bundle naming constraint at
`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:194-196`;
current collect steps at `.github/workflows/release.yml:140-155`.

Justification:

- Collect-time renaming makes each uploaded artifact directory already contain
  final release-asset names. The flattened `download-artifact` step can remain
  simple because `artifacts/*` no longer contains colliding bare filenames.
  Source: current download/release steps at `.github/workflows/release.yml:162-174`;
  problem map § 4 — Filename preservation and collision.
- It is a single point of truth per platform collect step and directly matches
  the workflow-contract test boundary.
  Source: problem map § 4 — Current bare-binary collect and upload pattern.

Immediate consequence:

- File count: modifies only `.github/workflows/release.yml`.
- Dependency count: no dependency change.
- Review surface: matrix row restoration plus three collect steps.

Rejected option:

- Rename-at-publish-time is rejected because it leaves colliding internal
  artifact filenames until after flattening and requires extra release-job
  mutation logic close to tag/release publication. That is a larger review
  surface than making each build job emit final asset names.
  Source: problem map § 6 — Risk hotspots, item 3.

### e. Structural test for the `release.yml` contract

Chosen option: Rust integration test under `src-tauri/tests/`, using the
existing YAML parsing dependency already present in `src-tauri/Cargo.toml`.

The test should parse `.github/workflows/release.yml` and assert the matrix,
collect-step, upload, download, and release-file invariants. The manifest
already depends on `serde_yml = "0.0.12"`, so Phase 6 should prefer that crate
unless hookpoint research finds a strong reason to add `serde_yaml`.
Source: ticket test boundary at
`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:169-172`;
manifest dependency at `src-tauri/Cargo.toml:10-17`.

Immediate consequence:

- File count: adds one Rust integration test, likely
  `src-tauri/tests/release_yml_contract.rs`.
- Dependency count: no new dependency if `serde_yml` is sufficient.
- Review surface: test-only YAML structural assertions.

Rejected option:

- Workflow-level YAML lint through `actionlint` in `ci.yml` is rejected because
  it would edit `ci.yml`, which is outside this WU unless a CI-side blocker is
  discovered, and it would lint syntax rather than assert this release-specific
  artifact contract.
  Source: ticket code/test boundaries at
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:146-180`;
  problem map § 6 — Risk hotspots, item 4.

## 3. Assumption register

### A1

Statement: `fs2` and `fs4` are both available on crates.io and provide
equivalent exclusive locking and unlocking on Unix and Windows, though `fs4`
uses `lock` rather than a literal `lock_exclusive` method.

Status: confirmed with method-name nuance. Evidence: problem map assumption A1;
`fs4-1.1.0/src/lib.rs:298-322`; `fs4-1.1.0/src/unix.rs:13-30`;
`fs4-1.1.0/src/windows.rs:19-33`.

Impact if invalidated: AC-2 and AC-3 fail; supported-surface choice 2.a
retracts and Phase 3 must choose `fs2` or a hand-rolled module.

### A2

Statement: Windows file locks via `LockFileEx` are advisory and per-handle
enough to match Unix `flock` for the `SessionLock` sentinel-file use case.

Status: confirmed with nuance. Evidence: problem map assumption A2;
`fs4-1.1.0/src/lib.rs:252-277`; `src-tauri/src/session_lock/mod.rs:223-242`.

Impact if invalidated: AC-3 fails and 2.a retracts. The WU must return to
research because Windows lock semantics would not support the current product
contract.

### A3

Statement: `session_replace` rename calls do not cross volumes under normal
app-data paths because current source/final paths are siblings or share
`journal_root`.

Status: confirmed for current constructors, unconfirmed for unusual
mount/reparse-point layouts. Evidence: problem map assumption A3 and § 2 —
Windows rename behavior.

Verification step for Phase 6b: treat A3 as a runtime invariant maintained by
constructors, not by a new platform-specific rename wrapper. Add a debug-only
constructor assertion or diagnostic assertion at the import-replace path
construction point that canonicalized scratch/journal paths remain under the
expected sessions/app-data root parent, for example
`debug_assert!(scratch_root.starts_with(sessions_root.parent().unwrap_or(Path::new("/"))))`
with the actual local variable names used by `session_replace`. This does not
prove Windows device identity across every reparse-point layout; the residual
must be recorded as "same-subtree constructor invariant, volume identity not
probed" in Phase 6 evidence.
Source: risk report § AUDIT-04; problem map assumption A3 and § 2 — Windows
rename behavior.

Impact if invalidated: AC-4 fails; section 2.c retracts and Phase 6 must either
add a same-volume guard or return to research for a Windows publication design.

### A4

Statement: `session_replace` does not currently rely on `std::fs::hard_link`;
any hard-link concern is historical or outside the mapped current code.

Status: unconfirmed / currently not applicable. Evidence: problem map
assumption A4 and § 2 — Current hard-link calls.

Verification step for Phase 6: run
`rg -n "hard_link|CreateHardLink|linkat|std::fs::hard_link" src-tauri/src src-tauri/tests`
before product edits.

Impact if invalidated: AC-4 and AC-8 hard-link limitation text may change;
section 2.c retracts because hard-link behavior would need explicit design.

### A5

Statement: `oulipoly-agent-runner` is the only artifact name collision; bundle
artifacts keep conventional names and do not require target suffixes.

Status: confirmed for current Linux/macOS workflow and pre-#24 Windows shape,
with generated Windows bundle names to be verified on a release build.
Evidence: problem map assumption A5; `.github/workflows/release.yml:140-155`;
ticket at `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:25-27`
and `:194-196`.

Verification step for Phase 6: generated Windows bundle names are AC-6
release-run evidence, not AC-5 structural-test evidence. The AC-6 evidence
record must include `windows_bundle_filenames`, copied from the Windows matrix
target's `artifacts/` listing or the GitHub release asset list, for example
`[oulipoly-agent-runner_<v>_x64.msi, oulipoly-agent-runner_<v>_x64-setup.exe]`
if those are what Tauri/NSIS emits. Unexpected conventional Windows bundle
names are acceptable as-is; only the bare binary requires the
`x86_64-pc-windows-msvc` suffix.
Source: risk report § AUDIT-05; problem map assumption A5.

Impact if invalidated: AC-5/AC-6 fail; section 2.d must broaden artifact
renaming beyond the bare binary, which may conflict with ticket anti-scope.

### A6

Statement: `dtolnay/rust-toolchain@stable` can install
`x86_64-pc-windows-msvc` on the `windows-latest` runner when the matrix row is
restored.

Status: unconfirmed for the future release runner, confirmed as a current
workflow shape and local target-list fact. Evidence: problem map assumption A6;
`.github/workflows/release.yml:126-128`; local
`rustc --print target-list | rg '^x86_64-pc-windows-msvc$'`.

Verification step for Phase 6: run the release build job or at minimum a
Windows-target `cargo check` under the same target triple.

Impact if invalidated: AC-1 may still be structurally satisfied, but AC-2 and
AC-6 fail; section 2.d stays valid while release-run execution requires runner
toolchain remediation.

### A7

Statement: `fs4`'s declared MSRV is compatible with the Rust stable toolchain
used by this repository and by GitHub Actions release jobs.

Status: confirmed locally, unconfirmed on Actions until the release job runs.
Evidence: `fs4-1.1.0/Cargo.toml:12-16` declares Rust 1.75; local
`rustc --version` is 1.92.0; the workflow uses `dtolnay/rust-toolchain@stable`
at `.github/workflows/release.yml:126-128`.

Verification step for Phase 6: `cargo check --target x86_64-pc-windows-msvc`
and normal Unix test commands after adding `fs4`.

Impact if invalidated: AC-2 and AC-7 fail; section 2.a retracts and dependency
choice must be revisited.

### A8

Statement: Windows default ACL inheritance for the app's per-user data/profile
paths is sufficient for this WU's single-user lock metadata privacy story.

Status: unconfirmed by automated tests; accepted as a documented product
assumption for this proposal. Evidence: ticket allows platform-necessary ACL
semantic differences at
`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:187-189`;
problem map § 6 — Risk hotspots, item 2.

Verification step for Phase 6: ensure D-006 explicitly documents the Windows
default-ACL strategy and that tests verify functional locking, not ACL layout.

Impact if invalidated: AC-8 documentation is inadequate and section 2.b
retracts. A future security WU or this WU after research would need explicit
DACL implementation.

### A9

Statement: The package name mismatch in the ticket's AC-2 command is procedural,
not a product intent change: the manifest package is `oulipoly-agent-runner`,
not `agent-runner-tauri`.

Status: confirmed. Evidence: `src-tauri/Cargo.toml:1-4`; problem map
`src-tauri/Cargo.toml` enumeration.

Verification step for Phase 6: run the equivalent check against the actual
package name or from `src-tauri/` without `-p`, and record the command mapping
in the contract/evidence.

Impact if invalidated: AC-2 evidence would be mis-specified; implementation
does not change, but Phase 6a contract must name the runnable command.

## 4. Test-intent track

### AC-1

Intent: `.github/workflows/release.yml` includes a `windows-latest` build row
with target `x86_64-pc-windows-msvc` and `bundles: msi,nsis`, and restores a
Windows collect step compatible with current matrix variables.
Source: ticket AC-1 at
`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:93-97`;
pre-#24 evidence in problem map § 4 — Pre-#24 Windows row.

Test/evidence: structural Rust integration test parses the workflow and asserts
the matrix row plus collect-step presence. Release-run evidence later confirms
execution. Level: particular-integration for YAML contract.

### AC-2

Intent: Windows target compilation succeeds without `nix::fcntl::flock`,
`AsRawFd`, or `OpenOptionsExt` compile errors in the Windows target.
Source: ticket AC-2 at
`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:98-101`;
current blockers at `src-tauri/src/session_lock/mod.rs:1-10`.

Test/evidence: run `cargo check --target x86_64-pc-windows-msvc` against the
actual package/manifest shape from `src-tauri/Cargo.toml:1-4`. Structural grep
evidence should show no unconditional `nix::fcntl`/`AsRawFd`/`OpenOptionsExt`
imports in Windows-compiled code. Level: component build verification.

### AC-3

Intent: cross-platform `SessionLock` integration tests exercise
`SessionLock::new`, `acquire`, `release`, and exclusive locking through the
`fs4` sentinel-file primitive.
Source: ticket AC-3 at
`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:103-106`;
current lock API at `src-tauri/src/session_lock/mod.rs:86-221`.

Test plan:

- Add a new portable integration test file under `src-tauri/tests/`, likely
  `session_lock_cross_platform.rs`.
- Use `#[cfg(any(unix, windows))]` for the file or for test modules, with
  `#[test]` functions compiled on both Unix and Windows. Avoid `#![cfg(unix)]`.
- Exercise single-process behavior by creating one `SessionLock`, acquiring a
  lease, attempting a second acquire for the same session before release, and
  asserting `LockError::Busy` with an expiry/token-hash shape.
- Exercise release by releasing with the returned token, then acquiring again
  and asserting success.
- Exercise cross-process exclusivity without importing Unix-only fixtures: the
  test must spawn a sibling helper process from the portable test harness (or a
  dedicated helper compiled for the test) that acquires a lease for the same
  session and waits on a sentinel file; the parent then attempts
  `SessionLock::acquire` for that session and asserts `LockError::Busy` before
  signalling the helper to release. A single-process double-acquire is retained
  only as an additional metadata-state assertion, not as the AC-3 exclusivity
  proof.
- Error-shape assertion: second acquire returns `LockError::Busy`, bad release
  token returns `LockError::TokenInvalid`, and successful idempotent replay
  preserves `already_released` behavior.

Source: risk report § SHORT-02; ticket AC-3 at
`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:103-106`;
problem map § 6 — Risk hotspots, item 1.

Level: particular-integration. Assumptions: A1, A2, A7, A8.

### AC-4

Intent: `session_replace::import_replace` keeps its atomicity/recovery
contract while the lock implementation becomes cross-platform.
Source: ticket AC-4 at
`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:108-112`;
current transaction contract in problem map § 1 — Current import-replace
transaction contract details.

Test/evidence: existing `initiative_06_import_replace.rs` stays green on Unix,
especially busy-lock, crash-after-rename recovery, lock-held orphan retention,
concurrent import-replace, and postimage-failure tests. No Windows runtime
`import_replace` test is required for this WU because the existing Initiative
06 import-replace fixtures are Unix-gated and not a mechanical portable subset.
The AC-4 Windows evidence is instead build-only: run exactly
`cd src-tauri && cargo check --target x86_64-pc-windows-msvc --tests`. This is
the runnable substitute for the ticket's sample
`cargo check --target x86_64-pc-windows-msvc -p agent-runner-tauri --tests`
because the manifest package is `oulipoly-agent-runner`, not
`agent-runner-tauri`.
The AC-3 cross-process `SessionLock` runtime test supplies the Windows runtime
lock evidence; the Unix Initiative 06 runtime tests supply the
import-replace atomicity/recovery evidence.
Source: risk report § AUDIT-03 and § SHORT-03; ticket AC-4 at
`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:108-112`;
problem map § 6 — Risk hotspots, item 6; package-name mismatch in assumption A9.

Level: existing particular-integration tests plus Windows build/lock tests.
Assumptions: A2, A3, A4.

### AC-5

Intent: release upload path produces target-distinguishable bare binaries.
Source: ticket AC-5 at
`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:114-117`;
current collision surface at `.github/workflows/release.yml:140-174`.

Structural YAML invariants for `src-tauri/tests/release_yml_contract.rs`:

- `jobs.build.strategy.matrix.include` length is exactly `3`.
- The three matrix entries, in any order, are exactly
  `{ os: ubuntu-latest, target: x86_64-unknown-linux-gnu, bundles: deb }`,
  `{ os: macos-latest, target: aarch64-apple-darwin, bundles: dmg }`, and
  `{ os: windows-latest, target: x86_64-pc-windows-msvc, bundles: msi,nsis }`.
- `Collect artifacts (Linux)` is guarded by `runner.os == 'Linux'`, copies
  the `.deb` bundle from the Linux bundle directory into `artifacts/`, and has
  a `cp` command that writes the bare binary to
  `artifacts/oulipoly-agent-runner-${{ matrix.target }}`.
- `Collect artifacts (macOS)` is guarded by `runner.os == 'macOS'`, copies the
  `.dmg` bundle from the macOS bundle directory into `artifacts/`, and has a
  `cp` command that writes the bare binary to
  `artifacts/oulipoly-agent-runner-${{ matrix.target }}`.
- `Collect artifacts (Windows)` is guarded by `runner.os == 'Windows'`, copies
  the `.msi` bundle and NSIS `.exe` bundle into `artifacts\` or `artifacts/`,
  and has a PowerShell `Copy-Item` or equivalent command that writes the bare
  binary to
  `artifacts\oulipoly-agent-runner-${{ matrix.target }}.exe`.
- The `actions/upload-artifact@v4` step uses
  `name: ${{ matrix.target }}` and `path: artifacts/*`.
- The release job's `actions/download-artifact@v4` step uses
  `merge-multiple: true` and `path: artifacts`.
- The `softprops/action-gh-release@v2` step uses `files: artifacts/*`,
  preserving filenames produced by the collect steps.
- Bundle globs are tied to their matrix targets and are not bare-binary-
  suffixed: Linux `.deb` only for `x86_64-unknown-linux-gnu`, macOS `.dmg`
  only for `aarch64-apple-darwin`, and Windows `.msi` plus NSIS `.exe` only
  for `x86_64-pc-windows-msvc`.

Source: risk report § AUDIT-02; ticket AC-5 at
`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:114-117`;
problem map § 4 — Current release workflow contract details and § 4 —
Pre-#24 Windows row.

Level: particular-integration structural test. Assumption: A5.

### AC-6

Intent: a trial release run publishes Linux x86-64, macOS aarch64, and Windows
x86-64 release assets; bare binaries are suffixed and bundle assets keep
conventional names.
Source: ticket AC-6 at
`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:119-122`.

Test/evidence: AC-6 commits to a real GitHub Actions `workflow_dispatch` run of
the `Release` workflow against a temporary pre-release or one-off release tag,
for example `v0.1.X-rc1` when the implementation branch's Cargo version is set
accordingly. This is chosen over `act` because the risk being closed is the
actual GitHub release handoff: runner toolchain setup, per-matrix artifact
upload, `download-artifact` flattening, and `softprops/action-gh-release`
asset publication. A local `act -j build` run can be diagnostic only; it cannot
substitute for AC-6 closure.
Source: risk report § AUDIT-01 and § SHORT-01; release dispatch trigger at
`.github/workflows/release.yml:3-16`; version/tag resolution and release upload
at `.github/workflows/release.yml:68-99` and `.github/workflows/release.yml:157-174`.

The AC-6 evidence record must contain these fields:

- `workflow_run_url` and `workflow_run_id`.
- `release_url` and the release tag used for the trial.
- `asset_filename_inventory`, listing each visible GitHub release asset:
  Linux bare binary, Linux `.deb`, macOS bare binary, macOS `.dmg`, Windows
  bare binary `.exe`, Windows `.msi`, and Windows NSIS `.exe`.
- `linux_bare_binary_sha256`,
  `macos_bare_binary_sha256`, and `windows_bare_binary_sha256`; hashes must be
  computed from the downloaded release assets and must differ except for an
  explicitly explained impossible collision.
- `matrix_artifacts_listing`, preserving the `artifacts/` directory listing
  per matrix target from the run log or an attached artifact.
- `windows_bundle_filenames`, copied from the Windows matrix artifact listing
  and/or release asset list. If NSIS emits a conventional name such as
  `oulipoly-agent-runner_<v>_x64-setup.exe` rather than a target-suffixed setup
  filename, that is acceptable; only the bare binary needs target-suffixed
  naming.

The structural workflow test and ordinary build logs cannot substitute for
this release evidence.
Source: risk report § AUDIT-01, § AUDIT-05, and § SHORT-01; ticket AC-6 at
`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:119-122`;
problem map § 4 — Filename preservation and collision.

Level: release-pipeline integration evidence. Assumptions: A5, A6.

### AC-7

Intent: existing CI remains green and the release build includes a clean
Windows job.
Source: ticket AC-7 at
`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:124-126`.

Existing test files at risk:

- `src-tauri/tests/initiative_06_pause_handshake.rs`
- `src-tauri/tests/initiative_06_import_replace.rs`
- `src-tauri/tests/initiative_06_locate.rs`
- `src-tauri/tests/initiative_06_export.rs`
- `src-tauri/tests/initiative_09_internal_unification.rs`
- `src-tauri/tests/session_metadata_component.rs`

Source: problem map § 1 — Current test inventory inside the blast radius.

Test/evidence: AC-7 local evidence is pinned to these commands:

- Rust: `cd src-tauri && cargo fmt --check && cargo clippy -- -D warnings && cargo test --no-fail-fast`.
- Frontend regression: `bun run check && bunx tsc --noEmit && bun run test`.
- Windows compile evidence: the AC-2/AC-4 command recorded after applying the
  A9 package-name correction, plus the clean Windows matrix row in the AC-6
  `workflow_dispatch` run.

Level: component and integration verification.
Source: risk report § AUDIT-06; ticket AC-7 at
`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:124-126`;
package-name mismatch in assumption A9.

### AC-8

Intent: replace D-006's "Windows is not a supported target" decision with a
Windows-supported decision.
Source: ticket AC-8 at
`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:128-132`;
current D-006 at `DECISIONS.md:122-162`.

New D-006 content should say exactly this in substance:

- Windows is a supported release target for the `agents` binary.
- The lock implementation uses a cross-platform sentinel-file abstraction based
  on `fs4`, which maps to Unix `flock(2)` and Windows `LockFileEx`.
- Unix preserves `0o700` lock directories and `0o600` lock metadata files.
- Windows relies on default current-user profile/app-data ACL inheritance for
  lock metadata privacy in this single-user developer deployment; explicit DACL
  hardening is not part of WU-13-01.
- `session_replace` publication continues to use same-root/sibling
  `std::fs::rename` paths; no hard-link publication is currently part of the
  mapped implementation.
- Release assets use platform-suffixed bare binary names while `.deb`, `.dmg`,
  `.msi`, and NSIS bundles keep conventional names.
- D-006 must not retain the old "Unix-only by design", "No Windows shim", or
  "Windows removed from Release workflow matrix" framing.

Level: documentation contract plus grep/structural review.
Assumptions: A3, A4, A8.

## 5. Qualitative net-value statement

This WU delivers clear user value: Windows users regain a working `agents`
binary in the release matrix, and Linux users stop receiving a macOS aarch64
bare binary mislabeled by collision as the generic Linux download. These are
current release-surface failures, not speculative polish.
Source: ticket symptom at
`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:20-27`.

The cost is bounded but real: `SessionLock` now depends on a cross-platform file
locking crate, the Cargo dependency tree grows slightly through `fs4`, and the
release workflow gains a Windows build row that consumes runner minutes. The
review surface is kept positive by avoiding a hand-rolled FFI module, avoiding
explicit DACL code, and renaming artifacts at collect-time.
Source: supported-surface choices in sections 2.a, 2.b, and 2.d.

Residual risk remains around Windows rename behavior on unusual reparse or
cross-volume layouts, Windows ACL expectations beyond single-user defaults, and
flaky Windows runners/toolchain installation. This proposal mitigates them by
keeping rename sources and destinations as siblings/same-root paths, documenting
the Windows ACL choice in D-006 instead of implying Unix-identical modes, and
requiring Windows target build/release evidence plus a structural workflow test.
Source: problem map assumptions A3 and A6; problem map § 6 — Risk hotspots.

value: positive

## 6. Implementation outline

1. Modify `src-tauri/Cargo.toml`: add `fs4` as the cross-platform locking
   dependency, using default sync support; avoid adding direct Windows security
   dependencies unless Phase 6 proves they are already required.
   Source: ticket code boundary at
   `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:143-145`.
2. Modify `src-tauri/src/session_lock/mod.rs`: replace direct
   `nix::fcntl::flock` / `AsRawFd` locking with `fs4::FileExt::lock` /
   `unlock`; preserve public lock API and Unix mode setup; avoid a POSIX-only
   compatibility shim.
   Source: current code at `src-tauri/src/session_lock/mod.rs:1-10` and
   `src-tauri/src/session_lock/mod.rs:223-242`.
3. Modify `src-tauri/src/session_replace/mod.rs` only if Phase 6 verification
   finds a compile blocker or a necessary same-root invariant note in code;
   otherwise leave rename behavior unchanged.
   Source: ticket code boundary at
   `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:139-140`;
   problem map § 2 — `session_replace` rename and hard-link calls.
4. Modify `src-tauri/src/session_metadata/mod.rs` only if Phase 6 verifies a
   supported Windows provider path reaches Claude path-hash decomposition and
   requires hardening; otherwise leave it unchanged and document residual risk.
   Source: ticket code boundary at
   `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:141-142`;
   problem map § 6 — Risk hotspots, item 5; deferred body-storage RCA named
   `research/12-empty-bodies-ref-rca.md` at
   `/home/nes/projects/agent-runner/worktrees/rca-empty-bodies-ref/research/12-empty-bodies-ref-rca.md:1-40`.
5. Modify `.github/workflows/release.yml`: restore the Windows matrix row and
   Windows collect step; rename bare binaries at collect-time using
   `${{ matrix.target }}` and `.exe` for Windows; leave bundle names conventional.
   Source: current workflow at `.github/workflows/release.yml:100-174`;
   pre-#24 row/collect evidence in problem map § 4.
6. Modify `DECISIONS.md`: replace D-006 with the Windows-supported decision
   described in AC-8's test-intent track.
   Source: current D-006 at `DECISIONS.md:122-162`.
7. Add `src-tauri/tests/session_lock_cross_platform.rs` or equivalent:
   portable `SessionLock` acquire/release/exclusivity tests for Unix and
   Windows.
   Source: ticket test boundary at
   `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:166-168`.
8. Add `src-tauri/tests/release_yml_contract.rs` or equivalent: structural
   YAML contract test for release matrix and artifact naming.
   Source: ticket test boundary at
   `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:169-172`.
9. Add the Phase 6 verification/audit record without broadening production
   scope: record the A3 constructor invariant assertion or diagnostic evidence,
   the AC-4 Windows build-only command and A9 package-name mapping, the AC-6
   `workflow_dispatch` evidence record, and a final non-interference audit.
   The non-interference audit must include `git diff --name-only` compared
   against the anti-scope list and targeted `rg` checks showing no edits or
   references were added under `src-tauri/src/balancer/`,
   `src-tauri/src/quota/`, `src-tauri/src/state/db.rs`,
   `src-tauri/tests/routing_fanout_rca*`, frontend `src/`, e2e/Playwright, or
   body-storage/canonical-record/session_turns schema surfaces.
   Source: risk report § AUDIT-07; problem map § 7 — Cross-WU
   non-interference; ticket anti-scope at
   `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:182-196`.
