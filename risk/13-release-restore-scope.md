# WU-13-01 Release Restore — Phase 4 Scope-Risk Gate

Reviewer: claude-opus
Phase: Phase 4 scope gate
Inputs:

- `proposals/13-release-restore.md` (proposal under review)
- `research/13-release-restore-problem-map.md` (Phase 2.5 problem map)
- `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md`
- `/home/nes/ai/conventions/no-backwards-compatibility.md`

## 1. Verdict

```
verdict: LOW
```

## 2. Findings

### SCOPE-01 — Code-boundary touch points enumerated, all in-scope

- ID: `SCOPE-01`
- Severity: `info`
- Location: `proposals/13-release-restore.md:759-813` (implementation outline)
- Summary: Every file the implementation outline names — `src-tauri/Cargo.toml`,
  `src-tauri/src/session_lock/mod.rs`, `src-tauri/src/session_replace/mod.rs`,
  `src-tauri/src/session_metadata/mod.rs`, `.github/workflows/release.yml`,
  `DECISIONS.md`, plus two new files under `src-tauri/tests/` — appears in
  the ticket's in-scope list at
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:138-150`.
- Evidence:
  - `proposals/13-release-restore.md:759-800` enumerates the eight touch
    points; each maps 1:1 to ticket lines 138-149.
  - The two new test files (`session_lock_cross_platform.rs` and
    `release_yml_contract.rs`) are explicitly authorized by the ticket's
    test boundary at lines 162-175.
- Closure expectation: none. The proposal does not name a touch point
  outside the in-scope list.

### SCOPE-02 — Anti-scope items enumerated and respected

- ID: `SCOPE-02`
- Severity: `info`
- Location: `proposals/13-release-restore.md:8-69` (section 1, Anti-scope)
- Summary: The proposal restates every anti-scope clause from the ticket
  verbatim (lines 10-24) and adds the problem-map adjacencies as further
  exclusions. Specifically:
  - No edits to `src-tauri/src/balancer/`, `src-tauri/src/quota/`, or
    `src-tauri/src/state/db.rs` (proposal lines 31-36; ticket
    lines 151-154; problem map § 3).
  - No `session_turns` / canonical-record / body-storage edits
    (proposal lines 37-45; ticket lines 155-157; problem map § 7).
  - No backwards-compat shim for the old POSIX-only `SessionLock`
    internals (proposal lines 46-51; ticket lines 190-191).
  - No frontend (`src/`) edits (proposal lines 59-61; ticket lines
    158-160).
  - No e2e / Playwright edits (proposal lines 62-64; ticket lines
    177-180).
  - Bundle artifact names (`.deb`, `.dmg`, `.msi`) keep conventional
    naming; only the bare binary is target-suffixed (proposal lines
    254-292; ticket lines 194-196).
- Evidence:
  - `proposals/13-release-restore.md:14-24` mirrors ticket lines 184-196
    word-for-word, including the bundle-artifact carve-out.
  - `proposals/13-release-restore.md:31-45` cites
    `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:151-157`
    and the problem map § 3 / § 7 for the routing/body-storage exclusions.
  - The non-interference audit step in the implementation outline
    (proposal lines 801-813) explicitly grep-checks for inadvertent
    edits under `src-tauri/src/balancer/`, `src-tauri/src/quota/`,
    `src-tauri/src/state/db.rs`, `src-tauri/tests/routing_fanout_rca*`,
    frontend `src/`, e2e/Playwright, and body-storage surfaces.
- Closure expectation: none. The anti-scope is not breached at the
  proposal layer; Phase 6 should preserve the same boundary.

### SCOPE-03 — `session_replace` and `session_metadata` touches gated by need

- ID: `SCOPE-03`
- Severity: `info`
- Location: `proposals/13-release-restore.md:770-783` (implementation
  outline steps 3 and 4); `proposals/13-release-restore.md:199-252`
  (section 2.c)
- Summary: Both `session_replace/mod.rs` and `session_metadata/mod.rs`
  appear in the ticket's in-scope list with the qualifier "minor
  adjustments only if needed" (ticket lines 139-142). The proposal
  preserves that gating: edits happen only if Phase 6 verification
  finds a compile blocker, a same-root constructor invariant assertion,
  or a Windows path-hash hardening requirement.
- Evidence:
  - `proposals/13-release-restore.md:770-775` says modify
    `session_replace/mod.rs` "only if Phase 6 verification finds a
    compile blocker or a necessary same-root invariant note".
  - `proposals/13-release-restore.md:776-783` says modify
    `session_metadata/mod.rs` "only if Phase 6 verifies a supported
    Windows provider path reaches Claude path-hash decomposition".
  - The optional `debug_assert!` constructor invariant
    (proposal lines 364-370) is contained within `session_replace`,
    which is in-scope; it does not extend into anti-scope adjacencies
    (`state/db.rs`, balancer, quota, session_turns, canonical-record).
- Closure expectation: Phase 6 must keep these edits genuinely minor;
  any change that begins to alter `ReplaceError` mapping, DB update
  sequencing, canonical-record shape, or `session_turns` schema would
  cross into anti-scope under
  `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:155-157`
  and problem map § 7 line "lock or rename changes must not alter
  canonical record shape, body fields, or session_turns schema".

### SCOPE-04 — No POSIX-only compatibility shim

- ID: `SCOPE-04`
- Severity: `info`
- Location: `proposals/13-release-restore.md:46-51`,
  `proposals/13-release-restore.md:81-92`,
  `proposals/13-release-restore.md:764-769`
- Summary: The proposal replaces the private `SessionLock::with_flock`
  and the direct `nix::fcntl::flock` / `AsRawFd` paths with the `fs4`
  primitive in a single migration. There is no parallel Unix-only
  module, no `lock_exclusive_posix` alias, and no `cfg(unix)` fallback
  function kept "just in case". The public surface (`SessionLock`,
  `Lease`, `ReleaseReceipt`, `LockError`) is preserved by replacement,
  not by re-export.
- Evidence:
  - `proposals/13-release-restore.md:48-51` cites the
    no-compatibility convention at
    `/home/nes/ai/conventions/no-backwards-compatibility.md:1-35`.
  - `proposals/13-release-restore.md:87-89` says "there is no
    compatibility alias or parallel POSIX-only API".
  - `proposals/13-release-restore.md:766-769` says "preserve public
    lock API and Unix mode setup; avoid a POSIX-only compatibility
    shim".
  - The forbidden patterns enumerated at
    `/home/nes/ai/conventions/no-backwards-compatibility.md:12-22`
    (deprecated aliases, dual implementations, transitional adapter
    layers) are absent from the proposal's section 2.a and from the
    implementation outline.
- Closure expectation: none. Phase 6 must not reintroduce a
  Unix-only shim during implementation.

### SCOPE-05 — Dependency add is minimal

- ID: `SCOPE-05`
- Severity: `info`
- Location: `proposals/13-release-restore.md:81-131`,
  `proposals/13-release-restore.md:759-763`,
  `proposals/13-release-restore.md:294-323` (section 2.e)
- Summary: The proposal adds exactly one direct dependency, `fs4`, with
  default `sync` features only; the optional async and `fs4-rs` wrapper
  features are not enabled. No new `windows-sys::Win32::Security`
  application code is added; the Windows ACL story is deferred to
  default current-user inheritance and recorded in `DECISIONS.md`. The
  structural release-workflow test reuses the existing `serde_yml`
  dependency rather than introducing `serde_yaml`.
- Evidence:
  - `proposals/13-release-restore.md:103-107` documents the single
    direct dependency and that default `sync` is the only feature
    enabled, citing `fs4-1.1.0/Cargo.toml:52-70` and
    `fs4-1.1.0/Cargo.toml:127-137`.
  - `proposals/13-release-restore.md:182-188` documents that no direct
    `windows-sys::Win32::Security` dependency is added.
  - `proposals/13-release-restore.md:296-305` says the YAML test should
    prefer the manifest's existing `serde_yml = "0.0.12"` from
    `src-tauri/Cargo.toml:10-17`.
- Closure expectation: Phase 6 must not enable additional `fs4` features
  (async, wrapper-file) and must not add `windows-sys::Win32::Security`
  unless a verified compile/runtime blocker forces it; either would be
  scope creep under the section 2.b rationale.

### SCOPE-06 — Test boundary respected

- ID: `SCOPE-06`
- Severity: `info`
- Location: `proposals/13-release-restore.md:294-323`,
  `proposals/13-release-restore.md:520-548`,
  `proposals/13-release-restore.md:679-702`,
  `proposals/13-release-restore.md:792-800`
- Summary: New tests live under `src-tauri/tests/`
  (`session_lock_cross_platform.rs` and `release_yml_contract.rs`),
  matching ticket lines 162-172. Existing `initiative_06_*` tests stay
  green on Linux/macOS (proposal AC-4 and AC-7); Windows gets new
  portable lock/release tests rather than a wholesale fixture port.
  Routing tests under `src-tauri/tests/routing_fanout_rca*` are not
  edited.
- Evidence:
  - `proposals/13-release-restore.md:521-547` describes the new
    `session_lock_cross_platform.rs` test under `src-tauri/tests/`,
    using `#[cfg(any(unix, windows))]` rather than touching Unix
    fixtures.
  - `proposals/13-release-restore.md:558-575` (AC-4) and
    `proposals/13-release-restore.md:679-702` (AC-7) confirm the
    existing `initiative_06_*` tests stay green and lists the at-risk
    files; no edits to those files are proposed.
  - The implementation outline step 9
    (`proposals/13-release-restore.md:801-813`) requires a final
    `git diff --name-only` audit to confirm `routing_fanout_rca*`
    is untouched.
  - Problem map § 6 item 6
    (`research/13-release-restore-problem-map.md:456-458`) is honored:
    the proposal does not propose a wholesale Initiative 06 fixture
    rewrite.
- Closure expectation: Phase 6 must not edit the existing Unix-only
  Initiative 06 fixtures or the routing reproduction harnesses; AC-7's
  pinned commands at `proposals/13-release-restore.md:691-697` are the
  enforcement gate.

### SCOPE-07 — DECISIONS.md scope narrowed to D-006

- ID: `SCOPE-07`
- Severity: `info`
- Location: `proposals/13-release-restore.md:704-731`,
  `proposals/13-release-restore.md:789-791`
- Summary: The proposal rewrites only D-006 and the immediately adjacent
  release-workflow narrative. D-001 (lease-renewal API) is not
  rewritten; other DECISIONS entries are not named.
- Evidence:
  - `proposals/13-release-restore.md:706-709` cites ticket AC-8 at
    `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:128-132`
    and current D-006 at `DECISIONS.md:122-162`.
  - `proposals/13-release-restore.md:712-727` enumerates the new D-006
    bullets and explicitly says D-006 must not retain
    "Unix-only by design", "No Windows shim", or "Windows removed
    from Release workflow matrix" framing.
  - `proposals/13-release-restore.md:789-791` says "Modify
    `DECISIONS.md`: replace D-006 with the Windows-supported decision
    described in AC-8's test-intent track" — no other DECISIONS
    entries are listed as targets.
  - The problem map call-out about D-001 constraints
    (`research/13-release-restore-problem-map.md:135` /
    `:138-139`) is honored: the proposal does not introduce
    lease-renewal or token-rotation semantics.
- Closure expectation: Phase 6 must keep DECISIONS edits limited to
  D-006; touching D-001 or unrelated entries would be scope creep.

### SCOPE-08 — Bundle naming preserved; only bare binary suffixed

- ID: `SCOPE-08`
- Severity: `info`
- Location: `proposals/13-release-restore.md:254-292`,
  `proposals/13-release-restore.md:580-624`
- Summary: The proposal renames only the bare binary at collect-time
  using `${{ matrix.target }}` (with `.exe` for Windows). `.deb`,
  `.dmg`, `.msi`, and NSIS `.exe` bundle assets keep conventional names.
  This matches the ticket's anti-scope clause at lines 194-196.
- Evidence:
  - `proposals/13-release-restore.md:259-263` says bundle globs for
    `.deb`, `.dmg`, `.msi`, and NSIS `.exe` "remain conventionally
    named".
  - The structural YAML invariants in the AC-5 test
    (`proposals/13-release-restore.md:585-616`) explicitly assert that
    bundle globs are tied to their matrix targets and are *not*
    bare-binary-suffixed.
  - AC-6 evidence (`proposals/13-release-restore.md:657-662`) accepts
    conventional NSIS names like
    `oulipoly-agent-runner_<v>_x64-setup.exe` rather than forcing a
    target-suffixed bundle name.
- Closure expectation: Phase 6 must not extend the
  `${{ matrix.target }}` suffixing into the bundle copy steps.

### SCOPE-09 — `ci.yml` correctly excluded

- ID: `SCOPE-09`
- Severity: `info`
- Location: `proposals/13-release-restore.md:52-58`
- Summary: The proposal explicitly notes that `.github/workflows/ci.yml`
  is *not* in the code boundary; only `release.yml` is. This matches
  the ticket code boundary at lines 146-149 and AC-7 at lines 124-126.
  The rejected `actionlint`-in-`ci.yml` option is called out and
  rejected on scope grounds.
- Evidence:
  - `proposals/13-release-restore.md:52-58` says "No changes to
    `.github/workflows/ci.yml`. The ticket test boundary requires
    existing CI to stay green, but only `.github/workflows/release.yml`
    is in the code boundary".
  - `proposals/13-release-restore.md:316-322` rejects the workflow-
    level YAML lint option because "it would edit `ci.yml`, which is
    outside this WU".
- Closure expectation: none. Phase 6 must keep `ci.yml` untouched
  unless a hard blocker is discovered; any such discovery should
  return to research, not silently broaden scope.

### SCOPE-10 — No public API renaming or module restructuring

- ID: `SCOPE-10`
- Severity: `info`
- Location: `proposals/13-release-restore.md:86-92`,
  `proposals/13-release-restore.md:124-131`
- Summary: The proposal preserves the `SessionLock`, `Lease`,
  `ReleaseReceipt`, and `LockError` public shapes owned by
  `src-tauri/src/session_lock/mod.rs:14-48`. There is no proposed
  module move (e.g., a top-level `platform/` or `cross_platform_lock/`
  module) and no renaming of methods (`new`, `acquire`, `release`,
  `any_active_for_session`). This avoids the "shared error-type or
  trait refactor" adjacency risk flagged by problem map § 3.
- Evidence:
  - `proposals/13-release-restore.md:86-89` says "The public
    `SessionLock`, `Lease`, `ReleaseReceipt`, and `LockError` shapes
    stay owned by `src-tauri/src/session_lock/mod.rs`".
  - The implementation outline step 2 says "preserve public lock API
    and Unix mode setup".
  - Problem map § 3 line 299
    (`research/13-release-restore-problem-map.md:299`) warns that a
    shared trait refactor "could create compile pressure across
    anti-scope modules"; the proposal's "private locking helper plus
    Cargo dependency review" framing
    (`proposals/13-release-restore.md:130-131`) is consistent with
    avoiding that pressure.
- Closure expectation: Phase 6 must keep public-API surface stable;
  any rename or trait extraction would warrant returning to Phase 3.

## 3. Verdict justification

The proposal stays inside the ticket's scope envelope on every
dimension this gate examines. The implementation outline names
exactly the files the ticket lists as in-scope
(`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md:138-150`),
the anti-scope clauses are restated verbatim and reinforced with the
problem-map adjacencies (proposal section 1), and the no-compatibility
convention is honored by replacing the POSIX-only `SessionLock`
internals rather than aliasing them
(`proposals/13-release-restore.md:46-51`,
`/home/nes/ai/conventions/no-backwards-compatibility.md:1-35`).
Optional touches into `session_replace/mod.rs` and
`session_metadata/mod.rs` are gated by Phase 6 verification needs and
remain inside the ticket's "minor adjustments only if needed"
qualifier; the public lock API is preserved without a parallel module;
the dependency add is exactly one crate (`fs4`) with default sync
features only; the structural workflow test reuses an existing manifest
dependency; the bundle-naming carve-out for `.deb`, `.dmg`, `.msi`,
and NSIS bundles is preserved; `.github/workflows/ci.yml` is correctly
excluded; and the DECISIONS rewrite is narrowed to D-006. None of the
findings rise above `info` severity, no `medium` or `high` scope-creep
issues are present, and no anti-scope clause is breached. The verdict
is therefore `LOW`.

Artifact path: `/home/nes/projects/agent-runner/worktrees/impl-wu-13-01/risk/13-release-restore-scope.md`
