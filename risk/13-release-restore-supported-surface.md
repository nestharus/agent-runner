# WU-13-01 — Phase 4 Supported-Surface Risk Gate

Phase: 4 supported-surface risk
Work unit: `release-restore`
Subject under review: `proposals/13-release-restore.md`
Inputs cross-checked: `research/13-release-restore-problem-map.md`,
`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-13-01/ticket.md`.

## 1. Verdict

```
verdict: LOW
supported_surface: positive
```

The proposal preserves the approved Phase 2.5 framing, every assumption
either stands or stands with a recorded Phase 6 verification dependency,
all four hotspot decisions resolve to `sound` or `acceptable-with-caveat`,
and the qualitative value statement names concrete users and concrete
losses currently observable on the supported surface
(`proposals/13-release-restore.md:732-755`;
`research/13-release-restore-problem-map.md:397-430`).

## 2. Assumption review

The proposal carries A1..A9; the problem map § 5 carries A1..A6
(`research/13-release-restore-problem-map.md:397-430`). The proposal
extends the register with A7..A9 to cover MSRV, Windows ACL, and the
package-name procedural delta. Each is reviewed below.

### A1 — `fs2`/`fs4` availability and equivalent exclusive lock/unlock

- Statement (paraphrased): both crates exist on crates.io, both wrap
  `flock(2)` on Unix and `LockFileEx` on Windows; `fs4`'s exclusive
  method is named `lock`, not `lock_exclusive`
  (`proposals/13-release-restore.md:326-334`).
- Proposal status: `confirmed` (with method-name nuance recorded).
- Gate verdict: `stands`. The problem map cites the local `cargo info`
  output, `fs4-1.1.0/src/lib.rs:298-322`, and the Unix/Windows
  implementation files
  (`research/13-release-restore-problem-map.md:399-403`;
  `proposals/13-release-restore.md:96-122`).
- Evidence cited: `cargo info fs4` v1.1.0; `fs4-1.1.0/src/lib.rs:298-322`;
  `fs4-1.1.0/src/unix.rs:13-30`; `fs4-1.1.0/src/windows.rs:19-33`.
- Impact if invalidated: AC-2 + AC-3 fail; section 2.a retracts; Phase 3
  must reselect between `fs2` and a hand-rolled `cfg`-gated module.

### A2 — Windows `LockFileEx` advisory + per-handle equivalence to `flock`

- Statement: Windows file locks via `LockFileEx` are advisory enough and
  per-handle enough that they match Unix `flock` semantics for the
  sentinel-file `SessionLock` use case
  (`proposals/13-release-restore.md:340-349`).
- Proposal status: `confirmed` with nuance.
- Gate verdict: `stands`. `fs4` documents advisory whole-file locks and
  release-on-handle-close, and its tests assert that an exclusive lock
  blocks another handle's exclusive and shared try-lock until unlock
  (`research/13-release-restore-problem-map.md:405-409`;
  `fs4-1.1.0/src/lib.rs:252-277`;
  `fs4-1.1.0/src/file_ext/sync_impl.rs:132-165`).
- Evidence cited: `fs4-1.1.0/src/lib.rs:252-277`;
  `src-tauri/src/session_lock/mod.rs:223-242`.
- Impact if invalidated: AC-3 fails; section 2.a retracts; the WU
  returns to research because Windows lock semantics would not support
  the current product contract.

### A3 — `session_replace` renames stay same-volume

- Statement: rename source and destination paths are siblings or share
  `journal_root`, so renames do not cross volumes under normal app-data
  layouts (`proposals/13-release-restore.md:353-376`).
- Proposal status: `confirmed for current constructors, unconfirmed for
  unusual mount/reparse-point layouts`.
- Gate verdict: `stands` with caveat. The problem map enumerates each
  rename call site and confirms same-root construction, while flagging
  that no Windows volume-identity probe exists
  (`research/13-release-restore-problem-map.md:411-414`;
  `src-tauri/src/session_replace/mod.rs:438-445`,
  `src-tauri/src/session_replace/mod.rs:498-506`,
  `src-tauri/src/session_replace/mod.rs:536-548`,
  `src-tauri/src/session_replace/mod.rs:1045-1064`,
  `src-tauri/src/session_replace/mod.rs:1170-1176`).
- Evidence cited: same-root rename construction, `MoveFileEx`
  `MOVEFILE_REPLACE_EXISTING` cross-filesystem error model, and the
  proposed Phase 6b debug-only assertion at the import-replace
  construction point (`proposals/13-release-restore.md:362-372`).
- Impact if invalidated: AC-4 fails; section 2.c retracts; Phase 6 must
  add a same-volume guard or return to research for a Windows
  publication design.

### A4 — No current `std::fs::hard_link` use in `session_replace`

- Statement: current code is rename-only; hard-link concern is
  historical or outside the mapped surface
  (`proposals/13-release-restore.md:378-390`).
- Proposal status: `unconfirmed / currently not applicable`.
- Gate verdict: `stands`. The problem map runs the verification command
  pattern in advance and confirms zero hits in `session_replace`
  (`research/13-release-restore-problem-map.md:416-419`;
  `src-tauri/src/session_replace/mod.rs:500-506`,
  `:540-548`, `:1051-1064`, `:1170-1176`). Phase 6 still owns a
  pre-edit `rg` step before product changes
  (`proposals/13-release-restore.md:385-387`).
- Evidence cited: problem map § 2 hard-link enumeration; ticket
  reference at `tmp/scratch/wu-13-01/ticket.md:39-40`.
- Impact if invalidated: AC-4/AC-8 limitation text changes; section
  2.c retracts because hard-link publication would need explicit
  cross-volume design.

### A5 — Bare binary is the only artifact-name collision

- Statement: `.deb`, `.dmg`, `.msi`, NSIS `.exe` already have
  platform-distinct names; only the bare `oulipoly-agent-runner`
  collides (`proposals/13-release-restore.md:394-411`).
- Proposal status: `confirmed for current Linux/macOS workflow and
  pre-#24 Windows shape`.
- Gate verdict: `stands`. The problem map cites both the current
  workflow collect lines and the pre-#24 Windows collect block recovered
  from `git show 9df5603^:.github/workflows/release.yml`
  (`research/13-release-restore-problem-map.md:421-425`;
  `.github/workflows/release.yml:140-155`). The proposal commits AC-6
  evidence to record `windows_bundle_filenames` from a real
  `workflow_dispatch` run, distinguishing structural-test scope from
  release-run scope (`proposals/13-release-restore.md:402-410`).
- Evidence cited: workflow lines, ticket symptom at
  `tmp/scratch/wu-13-01/ticket.md:25-27`.
- Impact if invalidated: AC-5/AC-6 fail; section 2.d must broaden
  artifact renaming beyond the bare binary, possibly conflicting with
  ticket anti-scope at `tmp/scratch/wu-13-01/ticket.md:194-196`.

### A6 — `dtolnay/rust-toolchain@stable` installs Windows MSVC target

- Statement: `dtolnay/rust-toolchain@stable` supports
  `targets: x86_64-pc-windows-msvc` on `windows-latest` when the matrix
  row is restored (`proposals/13-release-restore.md:415-432`).
- Proposal status: `unconfirmed for the future release runner, confirmed
  as current workflow shape and local target-list fact`.
- Gate verdict: `stands` with Phase 6 verification dependency. The
  current workflow already passes `${{ matrix.target }}` into the same
  action, and the pre-#24 matrix used the same target triple
  (`research/13-release-restore-problem-map.md:427-430`;
  `.github/workflows/release.yml:126-128`). A6 is closed by the AC-6
  release-run record, not by structural-test evidence.
- Evidence cited: `.github/workflows/release.yml:126-128`; local
  `rustc --print target-list`.
- Impact if invalidated: AC-1 may still be structurally satisfied, but
  AC-2/AC-6 fail; section 2.d remains valid while the runner toolchain
  story needs remediation.

### A7 — `fs4` MSRV compatible with the workspace toolchain

- Statement: `fs4`'s declared MSRV (Rust 1.75.0) is compatible with the
  local toolchain (1.92.0) and with the GitHub Actions stable toolchain
  used by release jobs (`proposals/13-release-restore.md:436-448`).
- Proposal status: `confirmed locally, unconfirmed on Actions until
  release job runs`.
- Gate verdict: `stands`. The proposal cites
  `fs4-1.1.0/Cargo.toml:12-16` and the local `rustc --version` output;
  Phase 6 closes runtime evidence through `cargo check
  --target x86_64-pc-windows-msvc` and the AC-6 `workflow_dispatch`.
- Evidence cited: `fs4-1.1.0/Cargo.toml:12-16`;
  `.github/workflows/release.yml:126-128`.
- Impact if invalidated: AC-2/AC-7 fail; section 2.a retracts and the
  dependency choice must be revisited.

### A8 — Windows default ACL inheritance sufficient for single-user privacy

- Statement: Windows default ACL inheritance on per-user
  profile/app-data paths is sufficient for this WU's single-user lock
  metadata privacy story (`proposals/13-release-restore.md:451-466`).
- Proposal status: `unconfirmed by automated tests; accepted as a
  documented product assumption`.
- Gate verdict: `stands` with caveat. The ticket explicitly authorizes
  platform-necessary ACL semantic differences
  (`tmp/scratch/wu-13-01/ticket.md:187-189`,
  `:209-212`), and the problem map flags this as a Phase 3 decision
  point rather than a settled current behavior
  (`research/13-release-restore-problem-map.md:235-241`,
  `:439-443`). The proposal closes A8 through D-006 documentation in
  AC-8 rather than through an automated DACL test, which is consistent
  with the ticket's allowance for documented platform differences.
- Evidence cited: ticket anti-scope clause at
  `tmp/scratch/wu-13-01/ticket.md:187-189`; problem map § 6 item 2.
- Impact if invalidated: AC-8 documentation is inadequate and section
  2.b retracts; a future security WU or this WU after research would
  need explicit DACL implementation.

### A9 — Package-name mismatch is procedural, not product

- Statement: the manifest package is `oulipoly-agent-runner`, not
  `agent-runner-tauri` as written in the ticket's AC-2 sample command
  (`proposals/13-release-restore.md:469-481`).
- Proposal status: `confirmed`.
- Gate verdict: `stands`. The problem map enumerated `Cargo.toml`'s
  `[package]` table and recorded the same delta
  (`research/13-release-restore-problem-map.md:90-91`;
  `src-tauri/Cargo.toml:1-4`). AC-4's runnable substitute command
  carries this correction (`proposals/13-release-restore.md:565-570`).
- Evidence cited: `src-tauri/Cargo.toml:1-4`.
- Impact if invalidated: AC-2 evidence would be mis-specified;
  implementation does not change, but Phase 6a contract must record
  the runnable command mapping.

No assumption is invalidated. The verdict therefore does not escalate
to `assumption-blocked`.

## 3. Value review

The proposal's net-value statement reads
"value: positive" and lists Windows users regaining a working `agents`
binary, plus Linux users no longer receiving a macOS aarch64 bare binary
mislabeled as the generic Linux download
(`proposals/13-release-restore.md:733-755`).

- **Concrete user.** The beneficiaries are identifiable: Windows users
  blocked by issue #16 and the unauthorized #24 removal, plus Linux
  users who download the unsuffixed bare binary asset
  (`tmp/scratch/wu-13-01/ticket.md:14-27`,
  `:31-32`). The supported surface in section 2 of the proposal frames
  the audience as "a local desktop/control-plane CLI that must publish
  working release binaries for Linux, macOS, and Windows"
  (`proposals/13-release-restore.md:71-79`), which matches the public
  release surface confirmed in `release.yml`.

- **Concrete loss avoided.** The losses are observable, not
  speculative. v0.1.23 already published a macOS aarch64 bare binary
  under the unsuffixed `oulipoly-agent-runner` asset name, so Linux
  downloads return `Exec format error`
  (`tmp/scratch/wu-13-01/ticket.md:14-17`,
  `:25-27`). The Windows release row was removed in commit `9df5603`
  (#24) under a "Unix-only" framing the user explicitly rejects
  (`tmp/scratch/wu-13-01/ticket.md:10-13`,
  `DECISIONS.md:122-162`).

- **Cost justification.** The costs are bounded and proportional:
  - One direct dependency (`fs4`) added to `src-tauri/Cargo.toml`, with
    Cargo handling its `rustix`/`windows-sys` platform splits internally
    (`proposals/13-release-restore.md:104-109`).
  - One Windows runner row consuming GitHub Actions minutes during
    release-only `workflow_dispatch` runs (the workflow is not on every
    push; `release.yml:3-16`).
  - ACL story is intentionally bounded: keep Unix `0o700`/`0o600` and
    rely on default Windows app-data ACL inheritance, documented as a
    platform-necessary semantic difference rather than reimplemented in
    bespoke DACL code (`proposals/13-release-restore.md:148-197`;
    ticket allowance at `tmp/scratch/wu-13-01/ticket.md:187-189`,
    `:209-212`).
  - No backwards-compat shim, no hand-rolled FFI, no rename wrapper, no
    explicit DACL builder; review surface stays close to the actual
    fix.

- **Net positive.** The proposal's `value: positive` claim is
  defensible. The current failure mode is concrete and reproducible
  through the v0.1.23 release listing, the cost lines are itemized, and
  the proposal discloses the residuals (Windows mount/reparse layouts,
  ACL beyond single-user defaults, runner toolchain flakiness) in its
  own §5 instead of hiding them
  (`proposals/13-release-restore.md:747-753`). Nothing in this review
  surfaces a hidden cost that would tip the balance non-positive.

The value statement is therefore not non-positive; the gate does not
escalate to `not-positive` and no NEEDS_INPUT new-value-question is
warranted.

## 4. Hotspot decision review

Problem map § 6 ranks six hotspots
(`research/13-release-restore-problem-map.md:432-458`). The four named
in the gate prompt are reviewed here; the residual two (path-hash
Windows behavior, Unix-only fixtures vs. portable subset) are surfaced
under § 5 findings.

### Hotspot 1 — Locking primitive provider

- Decision summary: replace `nix::fcntl::flock` /
  `AsRawFd` / `OpenOptionsExt` use in `SessionLock` with
  `fs4::FileExt::lock` and `unlock` over the existing sentinel `File`,
  preserving the public `SessionLock` / `Lease` / `ReleaseReceipt` /
  `LockError` surface (`proposals/13-release-restore.md:81-146`).
- Evaluation: `sound`. The chosen crate matches problem map
  assumption A1 verbatim, the Windows primitive matches assumption A2,
  and the `lock` (rather than `lock_exclusive`) method-name nuance is
  recorded in A1's evidence cell rather than waved away. The rejection
  of a hand-rolled `cfg`-gated module is justified by file count,
  unsafe/FFI review surface, and platform byte-range ownership
  (`proposals/13-release-restore.md:142-146`;
  `research/13-release-restore-problem-map.md:436-438`).
- Caveat: none beyond the runner-stable-toolchain dependency tracked by
  A6/A7.

### Hotspot 2 — Windows ACL strategy

- Decision summary: keep Unix `0o700`/`0o600` mode calls; on Windows,
  rely on default current-user profile/app-data ACL inheritance and
  document the single-user equivalence in D-006
  (`proposals/13-release-restore.md:148-197`).
- Evaluation: `acceptable-with-caveat`. The decision matches the
  ticket's allowance for platform-necessary semantic differences
  (`tmp/scratch/wu-13-01/ticket.md:187-189`,
  `:209-212`) and avoids pulling
  `windows-sys::Win32::Security` into the lock critical path. The
  caveat is assumption A8: there is no automated test confirming default
  ACL inheritance on real `windows-latest` profile paths. A8 is
  explicitly accepted as a documented product assumption rather than
  as a tested invariant (`proposals/13-release-restore.md:451-466`).
- Caveat: AC-8 documentation is the sole closure mechanism for A8. If
  the user later requires multi-user hardening, a separate security WU
  is named, not folded into this WU
  (`proposals/13-release-restore.md:191-197`).

### Hotspot 3 — Artifact rename location (collect-time vs publish-time)

- Decision summary: rename the bare binary at collect-time inside each
  build matrix step, producing
  `artifacts/oulipoly-agent-runner-${{ matrix.target }}` on Linux/macOS
  and `artifacts/oulipoly-agent-runner-${{ matrix.target }}.exe` on
  Windows; bundle globs (`.deb`/`.dmg`/`.msi`/NSIS `.exe`) keep
  conventional names (`proposals/13-release-restore.md:254-292`).
- Evaluation: `sound`. The rejection of publish-time renaming is
  grounded in the problem map's analysis of `actions/upload-artifact@v4`
  and `actions/download-artifact@v4` `merge-multiple: true` last-writer
  semantics, and in the GitHub release asset API's rejection of
  duplicate filenames
  (`research/13-release-restore-problem-map.md:389-395`,
  `:444-446`). Collect-time naming makes each upload artifact directory
  already final, so the flattened release-job surface needs no extra
  mutation logic. The decision honors ticket anti-scope by leaving
  bundle names alone (`tmp/scratch/wu-13-01/ticket.md:194-196`).
- Caveat: none.

### Hotspot 4 — Structural test for the `release.yml` contract

- Decision summary: a Rust integration test under `src-tauri/tests/`
  (likely `release_yml_contract.rs`) parses the workflow with the
  manifest's existing `serde_yml` dependency and asserts matrix,
  collect-step, upload, download, and release-file invariants
  (`proposals/13-release-restore.md:294-322`,
  `:585-622`).
- Evaluation: `sound`. The decision keeps testing inside the repository
  Rust harness, avoids editing `ci.yml` (anti-scope), and reuses
  `serde_yml = "0.0.12"` already present in `src-tauri/Cargo.toml`,
  adding zero dependencies
  (`research/13-release-restore-problem-map.md:448-450`;
  `src-tauri/Cargo.toml:10-17`). The proposal's enumeration of YAML
  invariants in AC-5 is concrete enough for Phase 6a contract use
  rather than handwaved
  (`proposals/13-release-restore.md:587-617`).
- Caveat: none.

No hotspot resolves to `unsound`, so the verdict does not escalate to
`MEDIUM` on hotspot grounds.

## 5. Findings

### SUPSURF-01 — Unix-only fixture port for AC-3 cross-process exclusivity

- Severity: `low`.
- Location: `proposals/13-release-restore.md:521-547`;
  `research/13-release-restore-problem-map.md:456-458`;
  `src-tauri/tests/fixtures/initiative_06_pause_handshake.rs:1-9`,
  `src-tauri/tests/fixtures/initiative_06_import_replace.rs:1-11`.
- Summary: AC-3 requires cross-process exclusivity through "a sibling
  helper process from the portable test harness (or a dedicated helper
  compiled for the test)" without importing Unix-only fixtures, but the
  current Initiative 06 fixture stack is `#![cfg(unix)]` and uses
  `std::os::unix::fs::PermissionsExt`/`symlink`. The proposal does not
  identify which existing helper or new helper provides this
  cross-platform subprocess scaffolding, and the deferred problem map
  hotspot 6 ("existing Unix-only tests vs. portable subset") is not
  decided in section 2.
- Evidence: AC-3 plan at `proposals/13-release-restore.md:530-540`;
  fixture imports at
  `src-tauri/tests/fixtures/initiative_06_pause_handshake.rs:1-9`;
  problem map § 6 item 6 at
  `research/13-release-restore-problem-map.md:456-458`.
- Closure expectation: Phase 6a contract names the subprocess helper
  (existing or new) used for cross-process exclusivity, or AC-3 falls
  back to a single-process exclusivity probe that the problem map
  explicitly says does not constitute the AC-3 evidence
  (`proposals/13-release-restore.md:537-540`). This is not a
  proposal-level revision request because the proposal already names
  the constraint; it is a contract-phase obligation.

### SUPSURF-02 — Path-hash Windows hotspot deferred without decision

- Severity: `low`.
- Location: `research/13-release-restore-problem-map.md:283-295`,
  `:452-454`; `proposals/13-release-restore.md:776-783`.
- Summary: problem map hotspot 5 asks "whether Windows support requires
  hardening Claude path-hash before release, or whether no supported
  Windows provider path currently reaches Claude Code storage." The
  proposal handles this only as implementation outline step 4: modify
  `session_metadata` "only if Phase 6 verifies a supported Windows
  provider path reaches Claude path-hash decomposition and requires
  hardening; otherwise leave it unchanged and document residual risk."
  Section 2 does not include a labeled "path-hash strategy" decision.
- Evidence: implementation outline step 4 at
  `proposals/13-release-restore.md:776-783`; problem map hotspot 5 at
  `research/13-release-restore-problem-map.md:452-454`; current code
  pinning at `src-tauri/src/session_metadata/mod.rs:338-388`.
- Closure expectation: Phase 6 evidence includes the proposal's named
  verification (whether a supported Windows provider path currently
  reaches `decode_claude_project_dir_candidates`) and either (a) records
  the residual risk in D-006/AC-8 documentation, or (b) returns to
  Phase 3 if hardening is required. The gate does not escalate today
  because the proposal commits Phase 6 to the verification.

### SUPSURF-03 — A4 hard-link verification is a pre-edit obligation

- Severity: `info`.
- Location: `proposals/13-release-restore.md:206-215`,
  `:385-390`.
- Summary: assumption A4 is recorded as `unconfirmed / currently not
  applicable`, with a Phase 6 `rg` step. The problem map already
  executed an equivalent check and recorded zero hits, but if Phase 6
  surfaces a `hard_link` call that the problem map missed (different
  `rg` flags, different file globs), section 2.c retracts. The proposal
  states this dependency explicitly; this finding records it for Phase 5
  hookpoint research rather than as a gap.
- Evidence: proposal § 2.c verification trigger at
  `proposals/13-release-restore.md:206-215`; problem map § 2 hard-link
  enumeration at `research/13-release-restore-problem-map.md:271-275`.
- Closure expectation: Phase 6 implementation evidence records the
  exact `rg` invocation output before product edits land.

### SUPSURF-04 — Bundle name verification deferred from AC-5 to AC-6

- Severity: `info`.
- Location: `proposals/13-release-restore.md:402-414`,
  `:625-668`.
- Summary: A5 is closed structurally for the bare binary by AC-5 but
  the generated Windows bundle filenames (e.g.
  `oulipoly-agent-runner_<v>_x64.msi`,
  `oulipoly-agent-runner_<v>_x64-setup.exe`) are pinned only by the
  AC-6 release-run record. There is no structural test asserting
  bundle-name shape because the ticket constrains bundles to remain
  conventionally named. If the actual emitted bundle filenames differ
  from the proposal's documented expectations, the bare-binary contract
  still holds, but D-006 must absorb the actual names.
- Evidence: proposal verification step at
  `proposals/13-release-restore.md:402-410`; AC-6 evidence schema at
  `:649-662`.
- Closure expectation: AC-6 evidence record contains
  `windows_bundle_filenames` directly copied from the artifact listing,
  and D-006 reflects them.

### SUPSURF-05 — Section 2 hotspot coverage is partial

- Severity: `info`.
- Location: `proposals/13-release-restore.md:81-322`;
  `research/13-release-restore-problem-map.md:432-458`.
- Summary: problem map § 6 enumerates six hotspots; proposal section 2
  has labeled subsections (a)..(e) covering hotspots 1, 2, 3, 4. Items
  5 (path-hash) and 6 (Unix-only fixtures) appear in implementation
  outline steps and AC-3/AC-4 prose but not as labeled supported-surface
  tracks. This is not a defect under the gate prompt's enumeration
  ("locking provider, ACL strategy, rename location, structural test
  location"), but it is recorded so Phase 5/Phase 6 readers do not
  treat hotspots 5 and 6 as silently dropped.
- Evidence: hotspot enumeration at
  `research/13-release-restore-problem-map.md:432-458`; proposal
  outline step coverage at
  `proposals/13-release-restore.md:770-810`.
- Closure expectation: none required at gate level; Phase 6a contract
  may carry these as residual-risk items.

## 6. Verdict justification

The proposal preserves the Phase 2.5 framing with citations to
`research/13-release-restore-problem-map.md` rather than rewriting it,
extends the assumption register with three procedurally necessary items
(A7 MSRV, A8 Windows ACL story, A9 package-name correction) without
invalidating any A1..A6 entry, and resolves the four supported-surface
hotspots named by the gate prompt to `sound`/`acceptable-with-caveat`.
The qualitative value statement names concrete users (Windows users
blocked by #16; Linux users hit by the v0.1.23 collision), concrete
losses ("Exec format error"; Windows release absence), proportional
costs (one dependency, one runner row, documented ACL difference), and
discloses residuals in its own body rather than hiding them. The
findings (SUPSURF-01..SUPSURF-05) record contract-phase or evidence-
phase obligations rather than proposal-level revisions; none rises to
`medium` or `high`. The gate therefore returns `LOW` +
`supported_surface: positive`, and the WU may proceed to Phase 5 once
the other Phase 4 gates concur.
