# WU-14-01 Shortcut-Risk Gate — session-migration-cwd

Phase: 4 shortcut gate
Inputs:
- `proposals/14-session-migration-cwd.md`
- `research/14-problem-map.md`
- `research/14-session-migration-rca.md`
- `~/ai/conventions/no-backwards-compatibility.md`
- `~/ai/conventions/no-deferred-stubs.md`

Scope: detect shortcuts that would let Phase 6 produce only the
appearance of WU-14-01's value (migration writes the target JSONL
where the spawned Claude `--resume` child actually looks) instead of
the value itself. Auditability, supported-surface, and scope-creep
dimensions are out of scope here.

## 1. Verdict

```
verdict: LOW
```

## 2. Findings

Each finding records the claim under review, where the proposal
addresses it, and the closure expectation Phase 6 must hold to keep
the LOW verdict honest. No `medium` or `high` shortcut was identified;
the entries below are either `info` closures (non-shortcut) or `low`
residuals already disclosed in the proposal.

### SHORT-01 — RC-1 mechanism is actually fixed, not papered over

- severity: info
- location: proposal § 1
  (`proposals/14-session-migration-cwd.md:13-48`); RCA RC-1
  (`research/14-session-migration-rca.md:38-72`); problem map
  (`research/14-problem-map.md:8-9`).
- summary: The migrated JSONL is moved to the spawn-cwd-derived
  project directory by changing the migration target derivation, not
  by patching the executor argv or copying files at spawn time.
- evidence:
  - The proposal replaces the source-derived `cwd_hash` computed at
    `src-tauri/src/migration/mod.rs:155` (`source_path.parent().file_name()`)
    with `target_dir = projects_dir.join(cwd_project_dir)` where
    `cwd_project_dir = claude_project_dir_for(provider, resume_working_dir)`
    (`proposals/14-session-migration-cwd.md:13-36`). This is the same
    location identified by the RCA as RC-1
    (`research/14-session-migration-rca.md:43-45`).
  - `migrate_chain_segment` gains an explicit
    `resume_working_dir: &Path` argument supplied by both production
    call sites — `run_repl` at `src-tauri/src/main.rs:1606` and
    `run_resume` at `src-tauri/src/main.rs:1830`
    (`proposals/14-session-migration-cwd.md:50-72`). Both call sites
    already know the spawn cwd via `working_dir` or `std::env::current_dir()`
    before invoking `execute_interactive` /
    `execute_resume`, so the fix is at the producing side, not at the
    spawn-side via argv.
  - The RC-1 reproduction harness
    `rc1_migrated_transcript_must_be_honorable_from_resume_working_dir`
    is kept and turned RED-to-GREEN; the post-fix assertion is named:
    `migrated.target_jsonl_path == resume_project_target` and child
    `result.exit_code == 0` from the resume working dir, with no
    `target_jsonl_path` passed via `ResumePayload`
    (`proposals/14-session-migration-cwd.md:87-96`,
    `proposals/14-session-migration-cwd.md:317`).
- closure expectation: Phase 6 must place the cwd-encoding call
  inside `migrate_chain_segment` so the on-disk target path is
  produced from `resume_working_dir`, not from `source_path.parent()`.
  Any Phase 6 implementation that re-derives the target dir from the
  source transcript and patches argv/file placement at executor time
  would re-open RC-1 and convert this to HIGH.

### SHORT-02 — Test-pass claim names the failure mode it covers

- severity: info
- location: proposal § 5 Test-Intent track
  (`proposals/14-session-migration-cwd.md:315-326`); RCA red-run log
  (`research/14-session-migration-rca.md:101-125`).
- summary: The proposal does not claim "tests pass" abstractly; it
  identifies the specific assertion that flips post-fix (the
  pre-existing pre-fix assertion at
  `src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs:39`)
  and names the new contract assertions and their fixture sources.
- evidence:
  - The Test-Intent row for the RCA harness states the acceptance
    condition explicitly: "post-fix migration writes under the resume
    working dir's project directory; fake Claude exits 0 from that
    cwd without receiving any target path"
    (`proposals/14-session-migration-cwd.md:317`). The expected signal
    is the exact assertion flip RCA RC-1 says is red today
    (`research/14-session-migration-rca.md:111-125`).
  - The new helper-contract tests `claude_project_dir_for_maps_absolute_unix_cwd`
    and `claude_project_dir_for_rejects_relative_or_empty_cwd` name
    exact string mappings (`/home/nes/project` → `-home-nes-project`,
    `/` → `-`) and the explicit error variant for unsupported inputs
    (`proposals/14-session-migration-cwd.md:30-33`,
    `proposals/14-session-migration-cwd.md:318`). This avoids the
    "tests pass with a tautological fixture" failure mode the problem
    map flags at `research/14-problem-map.md:13` and
    `research/14-problem-map.md:88`.
  - The Codex deferred-row keeps the pre-existing
    `MigrationError::CodexMigrationDeferred` and `[migrate]`-stderr
    contracts intact under the new signature
    (`proposals/14-session-migration-cwd.md:322`), so a Codex caller
    cannot accidentally turn the new variant into a silent success.
- closure expectation: Phase 6 must keep the RCA harness in the
  `tests/session_migration_rca/` tree and not weaken its acceptance
  to "the file exists somewhere". Replacing the harness with a
  same-cwd substitute or removing the spawn-from-resume-cwd assertion
  would re-open RC-1.

### SHORT-03 — `target_jsonl_path` removal is a clean migration, not a shim

- severity: info
- location: proposal § 1 executor changes
  (`proposals/14-session-migration-cwd.md:74-83`); convention
  (`~/ai/conventions/no-backwards-compatibility.md:1-44`); problem map
  (`research/14-problem-map.md:24-26`,
  `research/14-problem-map.md:39-40`).
- summary: The dead `target_jsonl_path` field in `ResumePayload` and
  the `_target_jsonl_path` parameter in `compose_resume_args` are
  deleted, not preserved with a default or "ignored for compatibility"
  comment. Tests whose only assertion was that the parameter is
  ignored are deleted with the parameter.
- evidence:
  - § 1 executor changes enumerate: remove `ResumePayload.target_jsonl_path`,
    remove the dead `_target_jsonl_path` parameter, update
    `compose_resume_provider_args`, `execute_resume`, and
    `execute_interactive`, and "Delete or replace tests whose only
    assertion is that compose helpers ignore `target_jsonl_path`;
    after Option 1 that parameter no longer exists. This follows
    `~/ai/conventions/no-backwards-compatibility.md`."
    (`proposals/14-session-migration-cwd.md:75-83`).
  - The RCA harness change explicitly says "Remove `target_jsonl_path`
    from `ResumePayload` construction in the harness. The harness
    should prove the executor does not need a file path once
    migration writes the cwd-derived location"
    (`proposals/14-session-migration-cwd.md:91-93`). This forecloses
    the Option 2-shaped shortcut where the executor consumes a
    migrated path as a fallback.
  - Anti-scope reiterates the rule at
    `proposals/14-session-migration-cwd.md:152-154`: "Do not introduce
    backwards-compatibility shims for the old source-derived target
    path computation."
- closure expectation: Phase 6 must drop the field and parameter from
  `src-tauri/src/executor/cli.rs:276`, `:282`, and `:292`, plus all
  call-site `target_jsonl_path: None` lines at
  `src-tauri/src/main.rs:1701` and `:1900`, and update tests in
  `src-tauri/src/executor/cli.rs` accordingly. Re-introducing the
  field "for safety" or keeping a `_target_jsonl_path: Option<...>`
  parameter under any name would re-open this as MEDIUM under the
  no-backwards-compatibility convention.

### SHORT-04 — Windows deferral is authorized, not a hidden stub

- severity: info
- location: proposal § 1 helper contract / § 2 anti-scope / § 4 A5 /
  § 7 open questions
  (`proposals/14-session-migration-cwd.md:21-48`,
  `proposals/14-session-migration-cwd.md:170-176`,
  `proposals/14-session-migration-cwd.md:286-295`,
  `proposals/14-session-migration-cwd.md:378-381`); convention
  (`~/ai/conventions/no-deferred-stubs.md:20-39`); problem map
  (`research/14-problem-map.md:67-73`).
- summary: Windows path-hash is deferred with a named follow-up WU
  and a named future harness file; the production code path raises
  an explicit, parameterized error variant for non-Unix cwd inputs
  rather than silently writing to the wrong location.
- evidence:
  - The follow-up work unit and harness path are both named:
    "Future WU: `WU-14-02-windows-claude-path-hash`, with
    reproduction harness
    `src-tauri/tests/session_migration_rca/rc2_windows_cwd_project_dir_hash.rs`"
    (`proposals/14-session-migration-cwd.md:171-173`,
    `proposals/14-session-migration-cwd.md:378-381`). This matches
    the no-deferred-stubs "name the follow-up work unit" requirement
    (`~/ai/conventions/no-deferred-stubs.md:27-30`).
  - The production helper raises a typed error rather than returning
    `Ok` with a wrong directory. `claude_project_dir_for` returns
    `MigrationError::SpawnCwdUnsupported { provider, cwd }` for
    relative, empty, or absolute non-Unix inputs
    (`proposals/14-session-migration-cwd.md:21-29`). This matches
    "Raise an explicit error on use, not a silent stub"
    (`~/ai/conventions/no-deferred-stubs.md:31-33`).
  - The deferred behavior is asserted as deferred by
    `claude_project_dir_for_rejects_relative_or_empty_cwd`
    (`proposals/14-session-migration-cwd.md:30-33`,
    `proposals/14-session-migration-cwd.md:318`), so removing the
    rejection branch later breaks the test
    (`~/ai/conventions/no-deferred-stubs.md:34-37`).
  - A5 records the evidence base for the deferral — production
    decoder is Unix-shaped, no production encoder exists, and the
    problem map's Cross-platform section confirms unknowns
    (`proposals/14-session-migration-cwd.md:286-295`;
    `research/14-problem-map.md:67-73`). The deferral is reasoned,
    not asserted.
- closure expectation: Phase 6 must implement
  `claude_project_dir_for` with the rejection branch wired to
  `MigrationError::SpawnCwdUnsupported` and a unit test that asserts
  the rejection. A Phase 6 implementation that silently treats a
  Windows path as a Unix one (`replace('\\', '/').replace('/', '-')`),
  or that returns a fallback `String` instead of an error, would
  collapse the authorized deferral into a hidden Windows stub and
  promote this to HIGH. The follow-up WU label
  `WU-14-02-windows-claude-path-hash` and harness path must remain
  in proposal/anti-scope/open-questions text so the deferral remains
  enforceable at review time.

### SHORT-05 — cwd-hash logic is centralized in migration, not duplicated

- severity: info
- location: proposal § 1
  (`proposals/14-session-migration-cwd.md:13-72`); tradeoffs § 1
  (`proposals/14-session-migration-cwd.md:131-141`); problem map
  (`research/14-problem-map.md:53-57`).
- summary: Option 1 places the production cwd-to-Claude-project-dir
  helper in exactly one production location (`src-tauri/src/migration/mod.rs`)
  and threads only the `Path` (not an encoded string) through
  `main.rs`. The executor performs no encoding; `cmd.current_dir(dir)`
  remains the only spawn-time cwd touchpoint.
- evidence:
  - The helper is declared as
    `pub(crate) fn claude_project_dir_for(provider: &str, cwd: &Path) -> Result<String, MigrationError>`
    inside `src-tauri/src/migration/mod.rs`
    (`proposals/14-session-migration-cwd.md:18-21`); no parallel
    helper is added to `src-tauri/src/executor/cli.rs` or
    `src-tauri/src/main.rs`.
  - The `main.rs` change passes `&Path` only — it absolutizes a
    relative `working_dir` against `std::env::current_dir()` but does
    not touch separator-replacement or the `-` prefix
    (`proposals/14-session-migration-cwd.md:52-58`). The encoding
    contract lives in the helper, not in the call sites.
  - The tradeoffs section explicitly rejects Option 2 because it
    would split cwd-hash/write logic between migration and executor,
    and Option 3 because it would write both source-derived and
    cwd-derived paths
    (`proposals/14-session-migration-cwd.md:134-141`).
  - The pre-existing decoder remains in
    `src-tauri/src/session_metadata/mod.rs:338`
    (`research/14-problem-map.md:54`); the new encoder is its dual.
    No second encoder is introduced.
- closure expectation: Phase 6 must implement the encoding inside
  `claude_project_dir_for` and call it once from `migrate_chain_segment`.
  Re-implementing `replace('/', '-')` inline at a `main.rs` or
  executor call site, or duplicating the helper into another module
  to avoid a `pub(crate)` re-export, would convert this to MEDIUM.
  Test-only encoders in `src-tauri/tests/fixtures/initiative_06*.rs`
  and `src-tauri/tests/session_migration_rca/mod.rs:130` may remain
  as fixture support — § 1 explicitly allows that
  (`proposals/14-session-migration-cwd.md:94-96`), but the new
  helper-contract tests must validate the production path
  independently of those fixtures.

### SHORT-06 — Inline-test split adds the missed contract, not a rename

- severity: info
- location: proposal § 1 inline-test correction
  (`proposals/14-session-migration-cwd.md:98-107`); problem map
  (`research/14-problem-map.md:13`,
  `research/14-problem-map.md:43-44`,
  `research/14-problem-map.md:88`).
- summary: The replacement of `migration_reuses_source_session_id_on_target_side`
  with two focused tests adds the spawn-cwd mismatch case the
  problem map flagged as missing — it is not a cosmetic rename.
- evidence:
  - The problem map records that the existing inline test "uses one
    literal `cwd_hash` for the source path and expected target path
    at `src-tauri/src/migration/mod.rs:314` and
    `src-tauri/src/migration/mod.rs:367`; it does not represent
    separate source and spawn cwd hashes"
    (`research/14-problem-map.md:88`). That is a tautological
    fixture: the test would still pass even if migration produced
    the wrong target dir whenever source and spawn cwd differ.
  - The split keeps the same-cwd invariant in
    `migration_reuses_source_session_id_when_source_and_spawn_cwd_match`
    (the original assertion's substantive content survives) and adds
    `migration_writes_target_under_spawn_cwd_when_source_and_spawn_cwd_differ`,
    which materially exercises the case the old fixture could not
    distinguish (`proposals/14-session-migration-cwd.md:99-106`).
  - The Test-Intent table acceptance condition for this row is
    explicit: "Same-cwd case preserves session id and writes existing
    shape; different-cwd case reuses session id but writes under
    spawn-cwd hash, not source parent hash"
    (`proposals/14-session-migration-cwd.md:319`). The different-cwd
    leg matches the failure mode at
    `research/14-problem-map.md:43-44`.
- closure expectation: Phase 6 must implement both tests with
  fixtures whose source-transcript parent and supplied
  `resume_working_dir` differ in the second test. A Phase 6
  implementation that reuses the same-cwd fixture for both tests —
  changing names and assertion strings without making source/spawn
  cwd actually differ — would collapse this back into the cosmetic
  rename SHORT-06 was screened against and promote this to HIGH.

### SHORT-07 — Symlink-canonicalization narrowing is disclosed, not hidden

- severity: low
- location: proposal § 1 main.rs changes / § 4 A3 / § 7 open
  questions (`proposals/14-session-migration-cwd.md:55-58`,
  `proposals/14-session-migration-cwd.md:269-275`,
  `proposals/14-session-migration-cwd.md:382-385`); problem map
  (`research/14-problem-map.md:73`,
  `research/14-problem-map.md:86-87`).
- summary: The proposal absolutizes a relative `working_dir` against
  `std::env::current_dir()` but explicitly does not call
  `canonicalize`, because the problem map records Claude Code's own
  symlink hashing behavior as unknown. The narrowing is disclosed and
  routed to a future symlink-question WU.
- evidence:
  - § 1 main.rs notes: "If `working_dir` is relative, absolutize it
    relative to `std::env::current_dir()` before passing it into
    migration. Do not canonicalize symlinks in this WU because
    `build_command` forwards the caller cwd via `cmd.current_dir(dir)`
    and the problem map says Claude's symlink canonicalization
    behavior is unknown."
    (`proposals/14-session-migration-cwd.md:55-58`).
  - A3 records the same evidence — `build_command` at
    `src-tauri/src/executor/cli.rs:348` forwards `working_dir`
    directly to `cmd.current_dir`, no production spawn-time
    canonicalization exists today, and Claude's symlink behavior is
    not knowable from this repo
    (`proposals/14-session-migration-cwd.md:269-275`;
    `research/14-problem-map.md:86-87`).
  - § 7 enumerates the symlink question as an open future-WU
    candidate alongside the Windows path-hash question
    (`proposals/14-session-migration-cwd.md:382-385`), so the
    deferral is visible at the proposal level.
- closure expectation: This is a disclosed residual rather than a
  silent shortcut, so the verdict stays LOW. Phase 6 must absolutize
  via `std::env::current_dir()` (so the path passed to migration
  matches the path passed to `cmd.current_dir`) without inserting a
  `canonicalize()` call. If Phase 6 silently canonicalizes — or, the
  inverse, fails to absolutize a relative `working_dir` and lets
  migration receive a path different from what
  `cmd.current_dir(dir)` will see — this finding promotes to MEDIUM
  because the producer-side and consumer-side cwds would diverge.

### SHORT-08 — `working_dir = None` coverage is verified at Phase 5, not silently assumed

- severity: low
- location: proposal § 1 main.rs / § 7 open questions
  (`proposals/14-session-migration-cwd.md:50-72`,
  `proposals/14-session-migration-cwd.md:373-377`); Test-Intent row
  (`proposals/14-session-migration-cwd.md:324`).
- summary: The proposal commits to handling the `working_dir = None`
  case by falling back to `std::env::current_dir()` in `main.rs`, but
  acknowledges that pre-existing Rust tests may not exercise this leg
  and routes verification to Phase 5 with a Phase-6a helper-contract
  fallback if missing.
- evidence:
  - § 1 main.rs requires both call sites to compute the effective
    spawn cwd as "`working_dir` when supplied, otherwise
    `std::env::current_dir()`" before calling
    `migrate_chain_segment`
    (`proposals/14-session-migration-cwd.md:52-54`).
  - § 7 open question 2: "Confirm whether `working_dir = None` has
    existing coverage; if not, Phase 6a should specify a small helper
    contract for effective cwd derivation."
    (`proposals/14-session-migration-cwd.md:373-376`). The Phase-5/6a
    routing makes the verification work scheduled, not handwaved.
  - The `run_repl`/`run_resume` Test-Intent row records a residual
    risk: "Existing tests may not cover `working_dir = None`; add
    unit/helper coverage if Phase 5 finds no existing path"
    (`proposals/14-session-migration-cwd.md:324`).
- closure expectation: Phase 5 must inspect existing tests for
  `working_dir = None` coverage. If absent, Phase 6 must add a unit
  test on the effective-cwd derivation helper that asserts the
  fallback to `std::env::current_dir()`. Shipping the change without
  either an existing test or a new one would convert the disclosed
  residual into a real shortcut and promote this to MEDIUM.

### SHORT-09 — Locator script left unchanged is a stated expectation, not a stub

- severity: info
- location: proposal § 1 / § 7
  (`proposals/14-session-migration-cwd.md:118-121`,
  `proposals/14-session-migration-cwd.md:377-378`); problem map
  (`research/14-problem-map.md:31`,
  `research/14-problem-map.md:42`).
- summary: The proposal claims `scripts/claude-code-locate-transcript`
  needs no code change because it locates by exact session-id
  filename across a projects tree, which remains a superset of
  Claude Code's cwd-scoped resume lookup; Phase 6 verifies this
  rather than the proposal asserting it without a check.
- evidence:
  - § 1 reads: "No planned code change. Phase 6 should verify it
    still finds the migrated file by exact `<session_id>.jsonl`
    filename under the provider projects tree."
    (`proposals/14-session-migration-cwd.md:118-121`).
  - The problem map confirms the locator's broader semantics at
    `research/14-problem-map.md:31` and
    `research/14-problem-map.md:42`. Migration narrowing the target
    placement does not reduce the locator's discovery space; it only
    affects which directory the file lives in.
  - § 7 open question 3 routes the verification to Phase 6 with the
    expected answer named: "expected answer is no script change"
    (`proposals/14-session-migration-cwd.md:377-378`).
- closure expectation: Phase 6 must run the locator against a
  cwd-derived fixture path and confirm session-id discovery still
  succeeds. If discovery breaks, the proposal's unchanged-locator
  claim is wrong and the WU must add the script change rather than
  be merged with a known regression. A Phase 6 that ships without
  running the locator at all would reopen this as MEDIUM.

## 3. Verdict justification

The proposal commits to delivering WU-14-01's actual value — moving
the migration target JSONL to the spawn-cwd-derived project directory
the Claude `--resume` child actually inspects — rather than its
appearance. The change is at the producing side
(`migrate_chain_segment` accepts `resume_working_dir`, replaces the
`source_path.parent().file_name()` derivation, and writes under
`projects_dir.join(claude_project_dir_for(provider, cwd))`), not a
patch on the executor argv or a post-spawn file copy
(`proposals/14-session-migration-cwd.md:13-48`,
`research/14-session-migration-rca.md:43-72`). The dead executor
parameter `ResumePayload.target_jsonl_path` and its
`compose_resume_args` companion are deleted under the
no-backwards-compatibility convention rather than preserved as a
shim, and tests whose only assertion was "ignored parameter" are
deleted with them
(`proposals/14-session-migration-cwd.md:75-83`,
`~/ai/conventions/no-backwards-compatibility.md:1-44`). The Windows
narrowing is an authorized deferral by the no-deferred-stubs
convention: a named follow-up WU (`WU-14-02-windows-claude-path-hash`)
with a named future harness file
(`src-tauri/tests/session_migration_rca/rc2_windows_cwd_project_dir_hash.rs`),
an explicit production error variant
(`MigrationError::SpawnCwdUnsupported { provider, cwd }`) raised on
non-Unix input rather than a silent fallback, and a test that
asserts the rejection so removing the rejection later breaks the
test (`proposals/14-session-migration-cwd.md:21-48`,
`proposals/14-session-migration-cwd.md:170-173`,
`proposals/14-session-migration-cwd.md:318`,
`~/ai/conventions/no-deferred-stubs.md:20-39`). Encoding logic lives
in exactly one production location
(`src-tauri/src/migration/mod.rs::claude_project_dir_for`) — the
executor performs no encoding and `main.rs` only chooses the cwd to
pass — and Options 2 and 3 are explicitly rejected for splitting or
duplicating logic (`proposals/14-session-migration-cwd.md:134-141`).
The inline-test replacement is substantive: the
`...when_source_and_spawn_cwd_differ` leg covers exactly the
spawn-cwd-mismatch case the problem map flagged as missing in the
old same-hash fixture (`research/14-problem-map.md:88`,
`proposals/14-session-migration-cwd.md:98-106`,
`proposals/14-session-migration-cwd.md:319`). The two narrowings the
proposal does take — symlink canonicalization deferred (SHORT-07)
and `working_dir = None` coverage routed to Phase 5/6a verification
(SHORT-08) — are explicitly disclosed in § 4 A3 and § 7 open
questions and bounded into Phase 6 evidence rather than hidden.
None of the listed shortcut vectors (executor-argv patch instead of
producing-side fix, retained dead `target_jsonl_path` parameter,
silent Windows fallback, duplicated encoding helper, cosmetic
inline-test rename, hidden symlink canonicalization, or untested
`working_dir = None` fallback) are present.
verdict: LOW.
