# Phase 8 multi-concern review — WU-13-01 release-restore

Reviewer: `claude-opus` (Phase 8 multi-concern gate)
Branch: `impl/wu-13-01`
Base: `main` @ `6b9509e`
Single commit on branch: `bff6a69 fix(release): restore Windows port +
per-platform bare-binary names`

## 1. Verdict

```
verdict: MULTI_CONCERN_ACCEPTABLE
```

## 2. Concern enumeration

The diff touches three concerns. Planning-artifact files
(`product-strategy/contracts/`, `proposals/`, `research/`, `risk/`)
are excluded from concern counting because they are
documentation-of-decisions, not code surfaces.

### C1 — Locking primitive abstraction (P1, ticket §"P1 — Windows port")

- One-line summary: replace POSIX-only `nix::fcntl::flock` /
  `AsRawFd` sentinel locking in `SessionLock` with cross-platform
  `fs4::FileExt::lock` / `unlock`, so the workspace compiles on
  `x86_64-pc-windows-msvc`.
- Files touched:
  - `src-tauri/src/session_lock/mod.rs` (+13/-19): drops
    `nix::fcntl::{FlockArg, flock}`, drops `std::os::fd::AsRawFd`,
    renames `with_flock` → `with_lock`, swaps the two `flock(...)`
    calls for `fs4::FileExt::lock` / `unlock`, restructures the
    error-precedence match.
  - `src-tauri/Cargo.toml` (+1/-1): adds
    `fs4 = { version = "1.1", default-features = false, features
    = ["sync"] }`, removes `nix = { version = "0.29", features
    = ["fs"] }`.
  - `src-tauri/Cargo.lock` (+12/-18): mechanical lockfile churn
    (adds `fs4` 1.1.0, drops `nix` 0.29.0 and `cfg_aliases` 0.2.1).
  - `src-tauri/tests/session_lock_cross_platform.rs` (+217, new):
    portable acquire/release/idempotency/cross-process
    exclusivity coverage for AC-3.
- Coupling: tightly coupled to C2 (release.yml Windows row) — the
  Windows row would fail `cargo build` without C1, since the
  pre-PR `nix::fcntl` import is the original Windows-build blocker
  cited in `DECISIONS.md` D-006 (old) and the ticket symptom (line
  11-13).

### C2 — Release pipeline matrix + asset naming (P1 row + P2 rename)

- One-line summary: add `windows-latest` row to
  `jobs.build.strategy.matrix.include`, restore the "Collect
  artifacts (Windows)" step, and suffix all three platforms' bare
  binaries with `-${{ matrix.target }}` (`.exe` on Windows) so
  `softprops/action-gh-release` no longer overwrites Linux's bare
  binary with macOS's at publish time.
- Files touched:
  - `.github/workflows/release.yml` (+14/-5): drops the
    "Windows is intentionally absent" comment, adds the
    `windows-latest` matrix include, suffixes the Linux and macOS
    `cp` lines, adds the PowerShell Windows collect step.
  - `src-tauri/tests/release_yml_contract.rs` (+287, new):
    structural YAML test asserting (a) 3-row matrix shape, (b)
    bare-binary suffix on Linux/macOS/Windows collect steps, (c)
    bundle names stay conventional, (d) upload/download/release
    job invariants (AC-5).
- Coupling:
  - The Windows matrix row (P1) is structurally dependent on C1 —
    landing the row without C1 would re-introduce the
    `error[E0432]: unresolved import nix::fcntl` build break that
    motivated the original `9df5603` Windows removal.
  - The bare-binary rename (P2) is *not* technically dependent on
    C1: the Linux + macOS rename hunks on
    `release.yml:144`/`release.yml:151` would compile and produce
    correct artifacts on their own. Ticket §"Two coupled problems"
    bundles them as organizational coupling ("avoid two
    release.yml PRs in flight").

### C3 — DECISIONS.md D-006 rewrite

- One-line summary: replace the "Unix-only by design" D-006 with a
  "Windows is a supported release target" entry that names the
  `fs4` abstraction, the default-Windows-ACL choice, and the
  bare-binary suffix asset shape.
- Files touched:
  - `DECISIONS.md` (+30/-46): single entry rewrite under the
    `## D-006` heading.
- Coupling: tightly coupled to C1 (cites the `fs4` abstraction as
  the rationale) and to C2 (cites the per-platform suffixed
  bare-binary asset shape). Landing C3 before C1+C2 would document
  state that does not exist; landing it after as a separate PR
  would generate doc churn for trivially zero implementation
  benefit.

## 3. Coupling analysis

Confirming the ticket's coupled-WU rationale against diff evidence.

> "The Windows row in `release.yml` cannot land without the
> locking abstraction (the Windows build would fail without
> `fs4`)."

**Confirmed.** Diff evidence:

- `src-tauri/src/session_lock/mod.rs:1-10` (pre-PR) imports
  `nix::fcntl::{FlockArg, flock}` and `std::os::fd::AsRawFd`. These
  are unconditional `use` statements with no `#[cfg(unix)]` gate.
  `nix` 0.29's `fcntl` module is not compiled on Windows
  (`Cargo.lock` shows `nix` resolves a `libc` dependency only on
  unix-family targets).
- `release.yml` (pre-PR) explicitly comments out Windows because
  "session_lock and session_replace use POSIX-only primitives".
  The new diff drops that comment block (`release.yml:99-103`
  removed) at the same instant it adds the matrix row, which is
  the only safe ordering: the comment was a load-bearing claim
  about C1.
- The new structural test
  `src-tauri/tests/release_yml_contract.rs` asserts the 3-row
  matrix shape; it would fail green on any branch that contains
  C2 but not C1 (the Windows build would fail before the structural
  test runs in pre-merge CI).

> "The Linux/macOS bare-binary rename can land independently, but
> the ticket couples it to avoid two release.yml PRs in flight."

**Confirmed and accepted.** Diff evidence:

- The rename hunks are minimal:
  `cp …/oulipoly-agent-runner artifacts/` →
  `cp …/oulipoly-agent-runner artifacts/oulipoly-agent-runner-${{ matrix.target }}`.
  Two lines on Linux + two on macOS. Carving these into a separate
  PR would be roughly 4 production lines + a slimmer YAML
  structural test, against the cost of a second
  `.github/workflows/release.yml` review pass and a second
  trial-release run for AC-6 evidence.
- The structural test
  `src-tauri/tests/release_yml_contract.rs` already encodes both
  invariants (matrix shape + suffix shape) in one file. Splitting
  C2 would mean either (a) two structural tests touching the same
  YAML, or (b) the test is owned by Split B and Split A only
  asserts matrix shape, leaving the bundle/suffix invariants
  unguarded inside Split A's CI window.

> "DECISIONS.md D-006 rewrite is a documentation update tightly
> bound to the locking abstraction's value statement."

**Confirmed.** Diff evidence:

- New D-006 body explicitly names "the cross-platform `fs4`
  sentinel-file locking abstraction, which maps to Unix `flock(2)`
  and Windows `LockFileEx`" — that is C1.
- New D-006 also names "platform-suffixed bare binary names, while
  `.deb`, `.dmg`, `.msi`, and NSIS bundles keep conventional
  package names" — that is C2.
- The old D-006 was the *justification* the orchestrator used to
  remove Windows in `9df5603`; restoring Windows without rewriting
  D-006 would leave the repo internally contradictory.

## 4. Decomposition assessment

The ticket bundles P1 + P2 + the doc rewrite into one WU. The
diff confirms a hard dependency between C1 and the Windows row of
C2, a soft dependency between C1 and the rename portion of C2,
and a hard documentation dependency between C3 and C1+C2. The
question is whether the soft dependency justifies a split.

### Why decomposition would create more churn than value

- **AC-6 evidence record is single-shot.** The contract's AC-6
  field set (`workflow_run_url`, `release_url`,
  `linux_bare_binary_sha256`, `macos_bare_binary_sha256`,
  `windows_bare_binary_sha256`, `asset_filename_inventory`,
  `windows_bundle_filenames`) is naturally produced by a single
  trial release. A split would either (a) require two trial
  releases against two pre-release tags, doubling release-infra
  cost, or (b) capture partial evidence in Split A and re-do the
  full evidence in Split B, wasting Split A's run.
- **Structural test owns both invariants.** Splitting forces a
  choice between two tests-on-one-YAML or partial coverage
  during the in-flight window.
- **The Windows-row hunk is the change that exercises C1
  end-to-end.** Without C2's matrix row in the same PR, C1 ships
  as "code that should compile on Windows but no CI evidence."
  The repository's `ci.yml` runs only `ubuntu-latest`; the Windows
  build path lives exclusively in `release.yml`. Splitting
  defers the only signal that proves C1 works.
- **D-006 churn.** Splitting C3 alone would mean an extra PR with
  ~76 lines of pure documentation churn and no executable
  consequence. Trivially separable, but high-churn-per-value.
- **Total code surface is small.** Outside planning artifacts:
  ~60 net production lines (`Cargo.toml` 1, `Cargo.lock` 12,
  `release.yml` 14, `session_lock/mod.rs` 13, `DECISIONS.md`
  30) + 504 new test lines. Below typical split-worth thresholds
  in this repository's review history (compare initiative-05's
  ~5,000 net lines accepted as single-concern in
  `review/05-multi-concern.md`).

### Hypothetical split (rejected)

For completeness, a candidate split would have been:

- **Split A — locking + Windows build + D-006**: C1 + Windows
  row of C2 + C3. Files: `src-tauri/Cargo.toml`, `Cargo.lock`,
  `src-tauri/src/session_lock/mod.rs`, `release.yml` (Windows
  row + Windows collect step + D-006 entry rewrite),
  `src-tauri/tests/session_lock_cross_platform.rs`,
  `release_yml_contract.rs` partial. Closes AC-1, AC-2, AC-3,
  AC-7, AC-8.
- **Split B — bare-binary suffix**: rename portion of C2.
  Files: `release.yml` (Linux + macOS `cp` suffix lines),
  `release_yml_contract.rs` extended assertions. Closes AC-5.
  Depends on Split A for the Windows collect-step rename
  contract assertion to make sense.

The split is feasible. It is rejected because (a) `release.yml`
would receive two consecutive edits within a single sprint, (b)
the structural test would be edited twice, (c) AC-6 trial-release
evidence would be duplicated or partially captured, and (d) the
ticket explicitly anticipated this question and bundled the two
concerns. The ticket's organizational coupling argument holds on
diff inspection.

## 5. Findings

### MC-01 — informational — diff is dominated by planning artifacts

- Severity: `info`
- Summary: `git diff --stat main..HEAD` reports 4,219 inserted
  lines, but ~3,728 of those are planning-track files
  (`product-strategy/contracts/wu-13-01-release-restore.md` 388,
  `proposals/13-release-restore.md` 813,
  `research/13-release-restore-*.md` 1,148,
  `risk/13-release-restore-*.md` 1,302,
  `tests/release_yml_contract.rs` 287,
  `tests/session_lock_cross_platform.rs` 217). The actual
  product-code surface is ~30 net inserted lines and ~24 net
  removed lines. Reviewers reading the diff stat without the
  planning-artifact context may misread blast radius.
- Evidence: `git diff --stat main..HEAD`; file enumeration in §2.
- Closure expectation: PR description should call out the small
  production surface explicitly so reviewers focus on
  `session_lock/mod.rs`, `Cargo.toml`, and `release.yml`.

### MC-02 — informational — `release.yml` carries both Windows-row and rename concerns

- Severity: `info`
- Summary: The single file `release.yml` carries P1 (Windows
  matrix row + Windows collect step) and P2 (bare-binary
  suffix on Linux/macOS) hunks interleaved. A line-level
  reviewer should mentally separate them.
- Evidence: `release.yml` diff hunks at lines 99-118 (matrix
  row + comment removal) and 141-159 (collect-step renames +
  Windows collect step).
- Closure expectation: PR description should list the file's
  hunks against AC-1 (Windows row), AC-5 (suffix), and AC-8
  (D-006) for review traceability. No code change required.

### MC-03 — low — `Cargo.lock` churn extends beyond the C1 swap

- Severity: `low`
- Summary: The `Cargo.lock` diff drops both `nix` 0.29.0 and
  `cfg_aliases` 0.2.1 (a transitive of `nix`). Both removals
  are mechanically correct under the `nix → fs4` swap, but a
  `cfg_aliases` removal is the kind of transitive that
  occasionally surprises reviewers reading lock diffs. The
  contract §4 anticipated this ("If `nix` is no longer
  referenced outside removed code paths, remove it").
- Evidence: `Cargo.lock` hunks for `cfg_aliases` (lines 335-340
  removed) and `nix` (lines 1979-1996 removed); contract
  guidance at `wu-13-01-release-restore.md:192-195`.
- Closure expectation: confirm via `rg "use nix" src-tauri/`
  and `rg "nix::" src-tauri/` that no other `src-tauri` module
  used `nix` (Phase 6c is responsible for this; Phase 8 only
  flags it for the line-level review pass). No code change
  expected if grep is clean.

### MC-04 — low — Windows AC-6 evidence is residual, not in-diff

- Severity: `low`
- Summary: AC-6 (trial release evidence with per-platform
  `_sha256` fields) is captured outside the PR's automated
  gates. The diff itself produces no AC-6 artifact. This is a
  documented residual under R4 in the contract and is
  acknowledged in `risk/13-release-restore-test-residuals.md`
  (29 lines). The multi-concern verdict does not turn on AC-6,
  but reviewers should know AC-6 will close out-of-band.
- Evidence: contract `wu-13-01-release-restore.md:386-388`;
  residuals file at
  `risk/13-release-restore-test-residuals.md`.
- Closure expectation: the trial-release evidence record is
  attached to the PR comment thread post-merge or in a
  follow-up commit. No multi-concern action.

## 6. Verdict justification

WU-13-01 is a `MULTI_CONCERN_ACCEPTABLE` PR. The diff touches
three identifiable concerns — the locking primitive abstraction,
the release pipeline matrix and asset naming, and the D-006
documentation rewrite — but the ticket's coupling argument holds
on diff inspection. The Windows matrix row in `release.yml`
cannot land without the `fs4` swap in `session_lock/mod.rs`
because the pre-PR `nix::fcntl` import is itself the original
Windows-build blocker; the D-006 rewrite is a value statement
that names both the locking abstraction and the per-platform
suffixed asset shape and so is incoherent landed alone; and the
bare-binary rename, while technically separable from C1, is a
two-line hunk per platform inside the same `release.yml` file
that the Windows row edits, naturally guarded by the same
structural test, and naturally evidenced by the same
trial-release run that proves the Windows row works. Splitting
would generate two `release.yml` PRs, two structural-test edits,
and either two trial releases or partial AC-6 evidence — a strict
churn-cost regression against the bundled WU. Net production code
surface is ~30 lines; the ticket explicitly anticipated this
question at §"Two coupled problems" and chose to bundle. The
choice is sound and is accepted under the
`MULTI_CONCERN_ACCEPTABLE` gate semantics.

---

Artifact: `/home/nes/projects/agent-runner/worktrees/impl-wu-13-01/review/13-release-restore-multi-concern.md`
Verdict: `MULTI_CONCERN_ACCEPTABLE`
