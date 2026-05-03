# WU-13-01 release-restore — Phase 8 justification review

Reviewer: claude-opus
Phase: 8 justification gate
Branch: `impl/wu-13-01`
Base: `main`
Commit under review: `bff6a69 fix(release): restore Windows port + per-platform bare-binary names`

Inputs:

- `git diff main..HEAD` (16 files; +4219/-87)
- `product-strategy/contracts/wu-13-01-release-restore.md`
- `proposals/13-release-restore.md`
- `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md`
- `risk/13-release-restore-{audit,scope,shortcut,supported-surface}.md`
  (all four LOW per `risk/13-release-restore-audit.md:1`,
  `risk/13-release-restore-scope.md:15`,
  `risk/13-release-restore-shortcut.md:17`,
  `risk/13-release-restore-supported-surface.md:12`).

## 1. Verdict

```
verdict: LOW_CONCERN
```

## 2. Per-file justification

### Product code

#### `src-tauri/src/session_lock/mod.rs`

- justification: AC-2 (Windows build) + AC-3 (cross-platform locking)
  per contract §1 lines 21-31 and the private-helper recipe at
  `product-strategy/contracts/wu-13-01-release-restore.md:134-176`.
- evidence path: diff has three edits, all traceable.
  L1-10 drops `use nix::fcntl::{FlockArg, flock}` and `std::os::fd::AsRawFd`
  (AC-2 blockers per `tmp/scratch/wu-13-01/ticket.md:35-37`).
  L108 + L167 rename `with_flock` → `with_lock` (contract line 140).
  L220-237 replaces the body with `fs4::FileExt::lock` / `unlock`,
  preserving the "closure error wins, unlock error replaces success"
  precedence (contract lines 148-167) and the verbatim error wording
  `"acquire sentinel lock: {err}"` / `"release sentinel lock: {err}"`
  from contract lines 143-147. `OpenOptionsExt` / `PermissionsExt`
  calls remain gated under pre-existing `#[cfg(unix)]` at
  `src-tauri/src/session_lock/mod.rs:86-99,294-300` (contract lines
  170-176).
- evaluation: `justified`.

#### `src-tauri/Cargo.toml`

- justification: AC-2 / AC-3 dependency contract at
  `product-strategy/contracts/wu-13-01-release-restore.md:178-195`.
- evidence path: diff adds exactly
  `fs4 = { version = "1.1", default-features = false, features = ["sync"] }`
  at `src-tauri/Cargo.toml:20` (shape verbatim from contract lines
  181-184; latest-stable bump authorized at lines 186-190; active
  version per `proposals/13-release-restore.md:96-111`) and removes
  the now-unused `nix = { version = "0.29", features = ["fs"] }`
  per contract lines 193-195. No other `[dependencies]` change; no
  async/wrapper-file features (forbidden at contract line 190).
- evaluation: `justified`.

#### `src-tauri/Cargo.lock`

- justification: generated; must be consistent with `fs4` add /
  `nix` remove.
- evidence path: lockfile diff removes `cfg_aliases` (line 335-340)
  and `nix v0.29.0` (line 1983-1994), adds `fs4 v1.1.0` with
  transitives `rustix` + `windows-sys 0.61.2` (line 870-880) as the
  proposal predicts at `proposals/13-release-restore.md:103-107`,
  and swaps `nix` for `fs4` in the workspace package list at line
  2272-2280. No unrelated package-version bumps.
- evaluation: `justified`.

### Tests

#### `src-tauri/tests/session_lock_cross_platform.rs` (new)

- justification: AC-3 portable `SessionLock` integration test per
  `product-strategy/contracts/wu-13-01-release-restore.md:212-252`.
- evidence path: all four required test cases present —
  `test_single_process_busy` (lines 14-48 ↔ contract 219-227),
  `test_release_idempotency` (50-77 ↔ 228-230),
  `test_cross_process_exclusivity` (79-118 ↔ 231-247),
  `test_acquire_after_release_succeeds` (120-140 ↔ 248-249).
  Cross-process helper uses `env::current_exe()` + `Command` with
  an `--ignored` re-entry test (142-182), authorized by contract
  240-247. Wait loops bounded by `WAIT_TIMEOUT = 30s` (contract
  line 246). No `#![cfg(unix)]` attribute (contract line 250).
- evaluation: `justified`.

#### `src-tauri/tests/release_yml_contract.rs` (new)

- justification: AC-5 structural YAML contract test per
  `product-strategy/contracts/wu-13-01-release-restore.md:254-294`.
- evidence path: every contract YAML invariant has a matching
  assertion — matrix length 3 (lines 19-23 ↔ contract 267); exact
  3-tuple entries (25-46 ↔ 268-270); Linux step (49-76 ↔
  271-274); macOS step (78-96 ↔ 275-277); Windows step + `.exe`
  (98-116 ↔ 278-280); upload-artifact name/path (118-136 ↔
  281-283); download-artifact merge-multiple/path (138-161 ↔
  284-285); release files (163-172 ↔ 286-287); bare-binary token
  appears only in the three collect steps (174-183 ↔ 288-290).
  Loader is the pre-existing `serde_yml` workspace dep
  (`src-tauri/Cargo.toml:16`); no new dev-dependency added
  (contract 261-262). Path resolves via
  `../.github/workflows/release.yml` from `src-tauri/` (contract
  292-293).
- evaluation: `justified`.

### Workflows

#### `.github/workflows/release.yml`

- justification: AC-1 (Windows row + collect step) + AC-5 (bare
  binary suffixing) per contract lines 21-23 / 37-40 and the Step 6c
  plan at lines 343-352.
- evidence path: each YAML edit ties to a single AC. L102-105
  deletes the "Windows is intentionally absent" comment block
  (AC-8 alignment; Step 6c bullet at contract 350-351). L112-114
  adds the `{ os: windows-latest, target: x86_64-pc-windows-msvc,
  bundles: msi,nsis }` matrix row (AC-1; ticket 93-97). L141-148
  suffixes Linux + macOS bare binaries with
  `oulipoly-agent-runner-${{ matrix.target }}` (AC-5; ticket
  114-117). L149-158 restores the Windows collect step via
  PowerShell `Copy-Item` writing
  `oulipoly-agent-runner-${{ matrix.target }}.exe` (AC-1 + AC-5;
  contract 345-348). Bundle copy lines keep conventional `.deb` /
  `.dmg` / `.msi` / NSIS `.exe` names (anti-scope ticket 194-196).
- evaluation: `justified`.

### Decision record

#### `DECISIONS.md`

- justification: AC-8 (rewrite D-006 as Windows-supported) per
  ticket line 128-132 and contract §1 AC-8 at line 52-54.
- evidence path: diff replaces only the D-006 block; both `main`
  and HEAD have 6 `## D-00` headings (verified), confirming
  D-001..D-005 + D-007 untouched. New D-006 body
  (`DECISIONS.md:122-149`) declares Windows a supported release
  target, names `fs4` mapping to `flock(2)` / `LockFileEx`,
  documents `0o700` / `0o600` Unix modes, calls out the Windows
  default-ACL choice, and notes platform-suffixed bare-binary
  names — matching the AC-8 test-intent substance at
  `proposals/13-release-restore.md:711-727`. The old "Unix-only by
  design" / "No Windows shim" / "Windows removed from Release
  workflow matrix" framing is removed (forbidden by ticket line
  132 and proposal 725-727).
- evaluation: `justified`.

### Process artifacts (research, proposal, contract, risk)

All process artifacts below are new files added by this WU; each
exists because a downstream phase requires it. None contain
product code.

- `proposals/13-release-restore.md` (813 lines) — Phase 3 proposal,
  cited as a contract input at
  `product-strategy/contracts/wu-13-01-release-restore.md:5`.
  Header `Phase: 3 proposal` at line 1-7. **justified**.
- `research/13-release-restore-problem-map.md` (475 lines) —
  Phase 2.5 problem map, contract input at line 6. Header
  `Phase: 2.5 existing-state risk profile`. **justified**.
- `research/13-release-restore-hookpoints.md` (673 lines) —
  Phase 5 hookpoint research, contract input at line 7 and
  required Step 6c reading at line 333. **justified**.
- `risk/13-release-restore-audit.md` (59 lines) — Phase 4 audit
  gate; `verdict: LOW` at line 1. **justified**.
- `risk/13-release-restore-scope.md` (334 lines) — Phase 4 scope
  gate; `verdict: LOW` at line 15. **justified**.
- `risk/13-release-restore-shortcut.md` (396 lines) — Phase 4
  shortcut gate; `verdict: LOW` at line 17. **justified**.
- `risk/13-release-restore-supported-surface.md` (484 lines) —
  Phase 4 supported-surface gate; `verdict: LOW` at line 12.
  **justified**.
- `risk/13-release-restore-test-residuals.md` (29 lines) —
  Step 6b residuals required by contract §5 line 320-323; encodes
  R1 (Windows ACL) per contract §8 line 377-379, routing the
  mitigation through AC-8's D-006 rewrite. **justified**.
- `product-strategy/contracts/wu-13-01-release-restore.md`
  (388 lines) — Phase 6a contract authored by the orchestrator
  (header line 3). Contains §1 AC list, §2/§3 in/anti-scope code
  surfaces, §4 schemas + Cargo recipe, §5 test boundary, §6 code
  boundary, §7 observable signals, §8 risk annotations.
  **justified**.

## 3. Drive-by drift detection

- whitespace-only / formatting-only edits to files outside the
  declared scope: **none**. `git diff main..HEAD --name-only`
  yields the 16 paths listed above; every path appears in the
  contract's in-scope set
  (`product-strategy/contracts/wu-13-01-release-restore.md:58-74`)
  or is a process artifact named by §1 / §5 / §6 / §8.
- API renames or type-shape adjustments unrelated to the locking
  abstraction: **none**. The only rename is private helper
  `with_flock` → `with_lock`, mandated by contract §4 at
  `product-strategy/contracts/wu-13-01-release-restore.md:140`.
  Public API (`Lease`, `ReleaseReceipt`, `LockError`, `SessionLock`,
  `any_active_for_session`) is byte-identical.
- comment removals / additions in code paths unrelated to the WU:
  **none in product code**. The
  `# Windows is intentionally absent ...` block in
  `.github/workflows/release.yml:101-104` is removed because it
  contradicts the new D-006 framing and AC-1 / AC-8 (Step 6c bullet
  at contract line 350-351). The `#[allow(deprecated)]` attributes
  on `with_flock` and the `nix::fcntl` import are removed because
  the deprecated callee no longer exists in this crate after the
  switch to `fs4` — i.e., the `#[allow(deprecated)]` is now
  vestigial, not an unrelated cleanup.
- `Cargo.toml` feature-flag changes beyond the `fs4` addition:
  **none**. The only other `Cargo.toml` line touched is the `nix`
  removal, which the contract explicitly authorizes at
  `product-strategy/contracts/wu-13-01-release-restore.md:193-195`.
  No other dep, feature, or `[target.*]` block changed.
- `Cargo.lock` unrelated bumps: **none**. Lockfile diff is
  consistent with adding `fs4` (and its `rustix` + `windows-sys`
  transitives, which the proposal predicts at
  `proposals/13-release-restore.md:103-107`) and removing `nix`
  (and its `cfg_aliases` transitive). No package-version bumps to
  unrelated crates.
- routing-fanout / body-storage / frontend / e2e surfaces:
  **untouched**. No path under `src-tauri/src/balancer/`,
  `src-tauri/src/quota/`, `src-tauri/src/state/db.rs`,
  `src-tauri/tests/routing_fanout_rca/`,
  `src-tauri/src/session_export/`,
  `src-tauri/src/session_metadata/`, `src/`, `e2e/`, or
  `playwright.config.ts` appears in
  `git diff --name-only main..HEAD`, satisfying the anti-scope at
  `product-strategy/contracts/wu-13-01-release-restore.md:76-89`
  and ticket lines 151-160 / 182-196.

## 4. Findings

### J-01

- severity: `info`
- location: `src-tauri/Cargo.toml:20`
- summary: `fs4 = "1.1"` is pinned a major version above the
  contract example (`"0.13"`).
- evidence: contract §4 example shows
  `fs4 = { version = "0.13", default-features = false, features = ["sync"] }`
  at `product-strategy/contracts/wu-13-01-release-restore.md:184`,
  but the same paragraph at
  `product-strategy/contracts/wu-13-01-release-restore.md:186-190`
  authorizes Step 6c to "pin to the latest stable" if the example
  version is no longer current. The proposal at
  `proposals/13-release-restore.md:96-99` records `cargo info fs4`
  reporting `fs4 v1.1.0` as the actively-maintained release. The
  `default-features = false, features = ["sync"]` shape is
  preserved verbatim, satisfying the dep-tree minimisation
  requirement at contract line 190. MSRV remains compatible
  (`fs4` 1.1 still declares 1.75; project toolchain is 1.92 per
  `proposals/13-release-restore.md:108-111`).
- closure expectation: none — the bump is explicitly authorized.

### J-02

- severity: `info`
- location: `src-tauri/src/session_lock/mod.rs:217-237`
- summary: Helper uses `fs4::FileExt::lock(&self.sentinel)` rather
  than the `use fs4::fs_std::FileExt;` path shown in the contract
  recipe.
- evidence: contract §4 at
  `product-strategy/contracts/wu-13-01-release-restore.md:158-161`
  states "If the cargo registry's `fs4` exports `FileExt` directly
  under the crate root, prefer that path." `fs4 v1.1.0` exports
  `FileExt` at the crate root per `Cargo.lock:870-880` and the
  proposal's evidence at `proposals/13-release-restore.md:113-122`,
  so the chosen path is the contract-preferred one. Blocking
  semantics, error precedence, and error-message wording are
  preserved per contract lines 162-167.
- closure expectation: none.

### J-03

- severity: `info`
- location: `src-tauri/tests/session_lock_cross_platform.rs:142-166`
- summary: Cross-process helper is implemented as an `#[ignore]`d
  test re-entry of the same test binary rather than a separate
  `[[bin]]` entry under `src-tauri/`.
- evidence: contract §5 cross-process helper guidance at
  `product-strategy/contracts/wu-13-01-release-restore.md:240-247`
  authorizes either `[[bin]]` / `examples/` / `tests/bin/` or
  `std::env::current_exe()` introspection. The implementation
  uses the latter (`env::current_exe()` +
  `Command::new(...).arg(HELPER_TEST_NAME).arg("--exact").arg("--ignored")`),
  which is the lighter-weight option and avoids touching
  `Cargo.toml` `[[bin]]` (test residuals R3 at contract §8
  line 382-385 mentions both shapes).
- closure expectation: none.

### J-04

- severity: `info`
- location: `.github/workflows/release.yml:101-104` (deletion)
- summary: A 4-line comment block was removed from the workflow.
- evidence: the deleted comment cites D-006's "Unix-only by
  design" rationale. AC-8 forces D-006 to be rewritten so the
  comment becomes false; Step 6c bullet at contract line 350-351
  explicitly directs removal: "Removes the 'Windows is
  intentionally absent' comment block in the build-job header."
- closure expectation: none.

No `medium` or `high` findings.

## 5. Verdict justification

Every modified or added file in this diff traces to a numbered
acceptance criterion (AC-1 through AC-8), to a contract section
(§4 dependency / private-helper / file-mode recipe; §5 test
boundary; §6 code boundary), or to a Phase 6a contract / Phase 4
risk-gate / Phase 5 hookpoint artifact required by the pipeline.
The two product-code edits — `session_lock/mod.rs` and
`Cargo.toml` — are byte-for-byte aligned with the contract recipe
at `product-strategy/contracts/wu-13-01-release-restore.md:134-195`,
including the helper rename, error-message wording, error
precedence ordering, dep features, and `nix` removal. The release
workflow edits restore exactly the matrix row + collect step shape
that AC-1 / AC-5 require, and bundle artifact names stay
conventional per the anti-scope. The D-006 rewrite covers all
substance the AC-8 test-intent enumerates and removes the forbidden
old framing. The two new tests assert one invariant per contract
bullet and use only contract-authorized helpers. Lockfile churn is
fully accounted for by the `fs4` add / `nix` remove. No file
outside the in-scope set is touched, no unrelated rename or
formatting drift appears, and the four upstream Phase 4 risk gates
all carry LOW verdicts. The verdict is therefore LOW_CONCERN; no
CodeRabbit fix-pass is required and the orchestrator does not need
to revise the diff.
