# WU-13-01 Shortcut-Risk Gate — release-restore

Phase: 4 shortcut gate
Inputs:
- `proposals/13-release-restore.md`
- `research/13-release-restore-problem-map.md`
- `tmp/scratch/wu-13-01/ticket.md`

Scope: detect shortcuts that would let Phase 6 produce only the
appearance of WU-13-01's value (Windows port + bare-binary
distinct-name) instead of the value itself. Auditability,
supported-surface, and scope-creep dimensions are out of scope here.

## 1. Verdict

```
verdict: LOW
```

## 2. Findings

Each finding records the claim under review, where the proposal
addresses it, what evidence closes it, and the closure expectation
Phase 6 must hold to keep the LOW verdict honest. No `medium` or
`high` shortcut was identified; the entries below are either `info`
closures (non-shortcut) or `low` residuals already disclosed in the
proposal.

### SHORT-01

- severity: info
- location: proposal § 2.a "Locking primitive provider"
  (`proposals/13-release-restore.md:81-122`); ticket AC-2/AC-3
  (`tmp/scratch/wu-13-01/ticket.md:98-106`).
- summary: The Windows lock primitive is a real `LockFileEx` call,
  not a `Ok(())` stub.
- evidence:
  - The proposal replaces `nix::fcntl::flock` with `fs4::FileExt::lock`
    / `unlock` over the existing sentinel `File`, not with a no-op
    Windows path (`proposals/13-release-restore.md:86-89`).
  - Justification cites the `fs4` Windows implementation calling
    `LockFileEx` with the whole-file byte range and being released
    on handle close (`proposals/13-release-restore.md:113-122`;
    `research/13-release-restore-problem-map.md:228`,
    `research/13-release-restore-problem-map.md:407-409`).
  - The blocking-vs-try-lock contract is preserved — `fs4::FileExt::lock`
    is the blocking exclusive operation, mirroring the current
    `FlockArg::LockExclusive` use (`proposals/13-release-restore.md:120-122`;
    `src-tauri/src/session_lock/mod.rs:223-230`).
- closure expectation: Phase 6 must wire the Windows path to
  `fs4::FileExt::lock` directly; any Windows-only branch that
  short-circuits the lock without calling into `fs4` would convert
  this to a HIGH shortcut.

### SHORT-02

- severity: info
- location: proposal § 4 "AC-3" test plan
  (`proposals/13-release-restore.md:513-548`); ticket AC-3
  (`tmp/scratch/wu-13-01/ticket.md:103-106`).
- summary: AC-3 is not single-process-only and is not Unix-gated.
- evidence:
  - The new test file lives under `src-tauri/tests/` with
    `#[cfg(any(unix, windows))]` and explicitly forbids
    `#![cfg(unix)]` (`proposals/13-release-restore.md:525-527`).
  - The exclusivity proof is a sibling helper process, not a
    same-process double-acquire — the parent attempts
    `SessionLock::acquire` while the helper holds the lease and
    asserts `LockError::Busy` before signalling the helper to
    release (`proposals/13-release-restore.md:531-539`). The
    same-process double-acquire is retained only as an additional
    metadata-state assertion, not the AC-3 proof
    (`proposals/13-release-restore.md:537-540`).
  - Error-shape assertions cover `LockError::Busy`, bad-token
    `LockError::TokenInvalid`, and idempotent-replay
    `already_released` behavior
    (`proposals/13-release-restore.md:541-543`).
- closure expectation: Phase 6 must keep both the cross-process
  exclusivity assertion and the second-acquire `LockError::Busy`
  assertion compiled on Windows. Replacing the sibling-process
  approach with a same-process-only assertion, or relocating the
  test under `#![cfg(unix)]`, would convert this to a HIGH shortcut.

### SHORT-03

- severity: info
- location: proposal § 2.e and § 4 "AC-5"
  (`proposals/13-release-restore.md:295-322`,
  `proposals/13-release-restore.md:582-625`); ticket AC-5
  (`tmp/scratch/wu-13-01/ticket.md:114-117`).
- summary: The release-workflow contract test is structural YAML
  parsing, not a string grep.
- evidence:
  - The test parses `.github/workflows/release.yml` via the
    existing `serde_yml = "0.0.12"` dependency rather than
    `grep`/`rg` over the raw text
    (`proposals/13-release-restore.md:296-305`;
    `src-tauri/Cargo.toml:10-17`).
  - Structural invariants enumerated include: matrix length is
    exactly 3; the three `(os, target, bundles)` triples; per-OS
    collect-step guards (`runner.os == 'Linux' | 'macOS' |
    'Windows'`) with literal bare-binary destination paths
    `artifacts/oulipoly-agent-runner-${{ matrix.target }}` and
    `artifacts\oulipoly-agent-runner-${{ matrix.target }}.exe`;
    `actions/upload-artifact@v4` with `name: ${{ matrix.target }}`
    and `path: artifacts/*`; `actions/download-artifact@v4` with
    `merge-multiple: true`; `softprops/action-gh-release@v2` with
    `files: artifacts/*`; bundle globs tied to the matrix target
    (Linux `.deb` / macOS `.dmg` / Windows `.msi` + NSIS `.exe`)
    (`proposals/13-release-restore.md:587-616`).
- closure expectation: Phase 6 must implement the test against the
  parsed YAML tree (jobs/strategy/matrix/steps), not against a flat
  text scan. A `String::contains("windows-latest")` style assertion
  would not satisfy AC-5 and would re-open this finding as a MEDIUM
  shortcut.

### SHORT-04

- severity: info
- location: proposal § 4 "AC-6"
  (`proposals/13-release-restore.md:626-670`); ticket AC-6
  (`tmp/scratch/wu-13-01/ticket.md:119-122`).
- summary: AC-6 commits to a real `workflow_dispatch` trial release,
  not an `act` simulation, and not "we'll figure it out later."
- evidence:
  - The proposal explicitly chooses `workflow_dispatch` against a
    temporary pre-release tag and rejects `act` as an AC-6
    substitute, naming runner-toolchain setup, per-matrix artifact
    upload, `download-artifact` flattening, and
    `softprops/action-gh-release` publication as the risks
    `act` cannot close (`proposals/13-release-restore.md:633-643`).
  - The evidence-record contract enumerates `workflow_run_url`,
    `workflow_run_id`, `release_url`, release tag,
    `asset_filename_inventory` (Linux/macOS/Windows bare and bundle
    assets), three platform-specific `*_bare_binary_sha256` fields
    that must differ except for an explicitly explained collision,
    `matrix_artifacts_listing`, and `windows_bundle_filenames`
    (`proposals/13-release-restore.md:644-668`).
  - The proposal explicitly states the structural workflow test and
    ordinary build logs cannot substitute for AC-6 release evidence
    (`proposals/13-release-restore.md:664-665`).
- closure expectation: Phase 6 must produce a real GitHub Actions
  release run with the listed evidence fields. Substituting `act`
  output, an internal log dump, or the AC-5 structural test for
  AC-6 would re-open this as a HIGH shortcut.

### SHORT-05

- severity: info
- location: proposal § 4 "AC-8" and § 6 step 6
  (`proposals/13-release-restore.md:704-729`,
  `proposals/13-release-restore.md:789-791`); ticket AC-8
  (`tmp/scratch/wu-13-01/ticket.md:128-132`).
- summary: D-006 is rewritten, not preserved-with-an-addendum.
- evidence:
  - The proposal explicitly says: "D-006 must not retain the old
    'Unix-only by design', 'No Windows shim', or 'Windows removed
    from Release workflow matrix' framing"
    (`proposals/13-release-restore.md:726-727`).
  - The replacement contents enumerate Windows-supported, `fs4`
    abstraction with `flock(2)` / `LockFileEx` mapping, preserved
    Unix `0o700` / `0o600`, default-ACL Windows strategy, rename-only
    publication, and platform-suffixed bare-binary naming with
    conventional bundle names
    (`proposals/13-release-restore.md:712-725`).
  - Implementation outline step 6 names "replace D-006 with the
    Windows-supported decision," not "append a note"
    (`proposals/13-release-restore.md:789-791`).
- closure expectation: Phase 6 must remove or rewrite the existing
  D-006 block at `DECISIONS.md:122-162` rather than appending a
  D-007 / errata while leaving the "Unix-only by design" framing
  in place.

### SHORT-06

- severity: info
- location: proposal § 2.c "Atomic-replace verification"
  (`proposals/13-release-restore.md:199-252`); ticket AC-4
  (`tmp/scratch/wu-13-01/ticket.md:108-112`).
- summary: `session_replace` atomicity is not weakened to make
  Windows tests easier; rename calls and same-root invariants are
  preserved.
- evidence:
  - Section 2.c keeps `std::fs::rename` as-is and does not introduce
    a `cfg`-gated rename wrapper that would abstract or skip
    behavior on Windows (`proposals/13-release-restore.md:201-203`,
    `proposals/13-release-restore.md:236-247`).
  - Existing Initiative 06 tests for busy-lock, crash-after-rename
    recovery, lock-held orphan retention, concurrent import-replace,
    and postimage-failure remain green on Unix and are not relaxed
    (`proposals/13-release-restore.md:559-564`).
  - Hard-link punt is bounded: the proposal records that the current
    code already has no `std::fs::hard_link` call (problem map § 2;
    `research/13-release-restore-problem-map.md:270-276`) and
    requires Phase 6 to halt and return to research if a hard-link
    call is rediscovered outside the mapped surface
    (`proposals/13-release-restore.md:206-211`).
- closure expectation: Phase 6 must not introduce a Windows-only
  shortcut around `import_replace`'s rename → DB-update sequence
  (e.g. skipping `fsync_dir`, swapping `rename` for a non-atomic
  copy, or relaxing preimage protection). Any such weakening would
  re-open this as a HIGH shortcut.

### SHORT-07

- severity: low
- location: proposal § 4 "AC-4"
  (`proposals/13-release-restore.md:551-579`); ticket AC-4
  (`tmp/scratch/wu-13-01/ticket.md:108-112`).
- summary: AC-4 declines a Windows-runtime `import_replace` test
  and substitutes (a) build-only `cargo check --target
  x86_64-pc-windows-msvc --tests` plus (b) the AC-3 cross-process
  `SessionLock` Windows runtime test plus (c) the existing Unix
  Initiative 06 atomicity/recovery suite.
- evidence:
  - The proposal states explicitly: "No Windows runtime
    `import_replace` test is required for this WU" and reasons
    that the Initiative 06 fixtures are Unix-gated and not a
    mechanical portable subset
    (`proposals/13-release-restore.md:561-567`;
    `research/13-release-restore-problem-map.md:303-306`,
    `research/13-release-restore-problem-map.md:456-458`).
  - The substitution legs are named: AC-3 supplies the Windows
    runtime lock evidence; the Unix Initiative 06 runtime tests
    supply atomicity/recovery; AC-4 Windows evidence is build-only
    via `cd src-tauri && cargo check --target x86_64-pc-windows-msvc
    --tests` (with the A9 package-name correction)
    (`proposals/13-release-restore.md:564-572`,
    `proposals/13-release-restore.md:469-482`).
- closure expectation: this is the closest the proposal comes to a
  shortcut — the ticket says "at least the platform-portable subset
  runs green on Windows," and the proposal narrows that to
  build-only on Windows for `import_replace` while running the
  portable lock subset (AC-3) at runtime. The narrowing is
  disclosed and reasoned (Unix-only fixtures), not hidden, which is
  why the verdict stays LOW. Phase 6 evidence must explicitly cite
  both substitution legs (AC-3 runtime lock + Unix Initiative 06
  atomicity) so a reader can see that AC-4's atomicity contract
  was not silently traded for a build-only check. If Phase 6
  surfaces a portable Initiative 06 subset that compiles on
  Windows without the Unix fixtures and the proposal/contract
  fails to add it, this finding promotes to MEDIUM.

### SHORT-08

- severity: info
- location: proposal § 2.d and § 4 "AC-5"
  (`proposals/13-release-restore.md:254-292`,
  `proposals/13-release-restore.md:582-616`); ticket
  bundle-naming anti-scope
  (`tmp/scratch/wu-13-01/ticket.md:194-196`).
- summary: Bare-binary renaming happens at collect-time on all
  three platforms with `.exe` for Windows, and bundle artifacts
  keep conventional names — i.e. the AC-5 contract change is the
  bare binary, not the bundles.
- evidence:
  - Linux and macOS collect steps copy the bare binary to
    `artifacts/oulipoly-agent-runner-${{ matrix.target }}`; the
    Windows collect step copies to
    `artifacts\oulipoly-agent-runner-${{ matrix.target }}.exe`;
    bundle globs (`*.deb`, `*.dmg`, `*.msi`, NSIS `*.exe`) keep
    conventional names tied to their matrix targets
    (`proposals/13-release-restore.md:258-267`,
    `proposals/13-release-restore.md:594-616`).
  - The structural test asserts those literal destinations as
    invariants of the workflow file
    (`proposals/13-release-restore.md:594-606`).
- closure expectation: Phase 6 must implement the rename at the
  collect step in `release.yml` (Linux/macOS `cp`, Windows
  `Copy-Item`) so the structural test sees the literal target-suffix
  paths. Renaming after `download-artifact` flattening would leave
  same-name files colliding inside `artifacts/` until publish time
  and was explicitly rejected
  (`proposals/13-release-restore.md:286-291`); doing it that way
  anyway would re-open this as a MEDIUM shortcut.

### SHORT-09

- severity: low
- location: proposal § 3 A6 / A7 (`proposals/13-release-restore.md:417-448`).
- summary: Windows runner toolchain — `dtolnay/rust-toolchain@stable`
  installing `x86_64-pc-windows-msvc` and the `windows-latest`
  default MSVC build tools — is treated as an assumption verified
  by AC-6 release-run rather than by a separate explicit
  "install MSVC tooling" step.
- evidence:
  - A6 records that the current workflow already passes
    `targets: ${{ matrix.target }}` to
    `dtolnay/rust-toolchain@stable` and that
    `x86_64-pc-windows-msvc` is in the local target list, and
    asks Phase 6 to verify on the actual runner via a release
    build or `cargo check`
    (`proposals/13-release-restore.md:417-432`;
    `research/13-release-restore-problem-map.md:108`,
    `research/13-release-restore-problem-map.md:427-430`).
  - A7 records `fs4` MSRV 1.75 against the local 1.92.0 stable and
    asks Phase 6 to confirm against the Actions toolchain
    (`proposals/13-release-restore.md:435-448`).
- closure expectation: Phase 6 must run AC-6 with the Windows row
  enabled; if the runner toolchain fails to provide MSVC link.exe
  / lib.exe by default, the implementation must add an explicit
  MSVC-tooling install step rather than dropping the Windows row
  again. If the row is silently disabled or replaced by a
  Linux-only build of a "Windows" binary, this finding promotes to
  HIGH.

### SHORT-10

- severity: low
- location: proposal § 2.b and § 3 A8
  (`proposals/13-release-restore.md:148-197`,
  `proposals/13-release-restore.md:451-466`); ticket allowance for
  platform ACL differences
  (`tmp/scratch/wu-13-01/ticket.md:187-189`).
- summary: Windows ACL strategy is "rely on the default per-user
  profile/app-data ACL inheritance," documented in D-006, with no
  test that asserts ACL layout.
- evidence:
  - The proposal preserves Unix `0o700` / `0o600` and explicitly
    declines DACL construction in this WU, citing single-user
    developer deployment and the ticket's explicit allowance for
    platform-necessary semantic differences
    (`proposals/13-release-restore.md:150-161`,
    `proposals/13-release-restore.md:163-180`).
  - A8 records the Windows-default ACL claim as "unconfirmed by
    automated tests; accepted as a documented product assumption"
    and routes verification to D-006 documentation rather than a
    test (`proposals/13-release-restore.md:451-466`).
- closure expectation: This is not a shortcut against AC-3 / AC-4
  / AC-5 / AC-6 (the locking and release-publication value), but
  it does narrow the privacy contract on Windows. Phase 6 must
  ensure the rewritten D-006 names this narrowing explicitly so
  the trade is visible; D-006 silently inheriting the old Unix
  privacy claim while Windows uses default ACLs would re-open
  this as MEDIUM.

### SHORT-11

- severity: low
- location: proposal § 3 A3 verification step
  (`proposals/13-release-restore.md:362-372`); ticket AC-4
  atomicity contract
  (`tmp/scratch/wu-13-01/ticket.md:108-112`).
- summary: A3's same-volume invariant for `session_replace` rename
  publication is verified by a debug-only constructor assertion
  ("scratch path is under sessions root parent"), not by a
  Windows runtime test of cross-volume / reparse-point behavior.
- evidence:
  - The proposal's verification step is a `debug_assert!` style
    constructor invariant, with the residual recorded as
    "same-subtree constructor invariant, volume identity not
    probed" (`proposals/13-release-restore.md:364-371`;
    `research/13-release-restore-problem-map.md:411-414`).
- closure expectation: This residual is explicitly disclosed and
  kept inside the AC-4 evidence trail, so it does not collapse
  AC-4's atomicity contract. Phase 6 must keep the residual in
  the evidence record. If the debug-only assertion is removed
  outright (turning A3 into an unwritten assumption), this
  finding promotes to MEDIUM.

## 3. Verdict justification

The proposal commits to delivering WU-13-01's actual value rather
than its appearance: the Windows lock primitive is `fs4::FileExt::lock`
mapping to `LockFileEx` rather than a Windows stub
(`proposals/13-release-restore.md:86-122`); the AC-3 lock test is
explicitly portable and uses a sibling helper process plus a
second-acquire `LockError::Busy` assertion rather than a
`#![cfg(unix)]` test or single-process simulacrum
(`proposals/13-release-restore.md:525-543`); the AC-5 release
contract is enforced by a `serde_yml` structural test that asserts
matrix shape, per-OS collect destinations, upload/download names,
release `files` glob, and bundle-glob target tying, not by string
grep (`proposals/13-release-restore.md:587-616`); AC-6 commits to a
real `workflow_dispatch` run with a fully-enumerated evidence
record (URLs, IDs, three distinct bare-binary sha256s, asset
inventory) and explicitly rejects `act` and the structural test as
substitutes (`proposals/13-release-restore.md:633-665`); D-006 is
rewritten with explicit instructions not to retain the "Unix-only
by design" framing (`proposals/13-release-restore.md:726-727`); and
`session_replace` atomicity is preserved without a `cfg`-gated
rename wrapper or other Windows-only weakening
(`proposals/13-release-restore.md:201-247`,
`proposals/13-release-restore.md:559-564`). The two narrowings the
proposal does take — AC-4 Windows runtime evidence is build-only
plus AC-3-runtime-lock + Unix-Initiative-06-atomicity (SHORT-07),
and Windows ACL is default-inherited rather than DACL-constructed
(SHORT-10) — are explicitly disclosed, reasoned against the
ticket's allowance for platform-necessary semantic differences and
the Initiative 06 fixtures' Unix-only construction, and routed
into the rewritten D-006 / Phase 6 evidence record rather than
hidden. None of the listed shortcut vectors (Windows-test gating,
string-grep YAML, missing cross-process assertion, Windows
`Ok(())` lock stub, atomicity weakening, D-006 addendum-only,
unjustified hard-link punt, deferred trial release) are present.
verdict: LOW.
