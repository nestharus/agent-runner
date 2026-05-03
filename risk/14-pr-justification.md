Verdict: LOW

# WU-14-01 Phase 8 — Justification Review

Scope: every change in `git diff main..HEAD` (HEAD `8a89207`) checked against
AC-1..AC-7 (contract §1), the contract §2 in-scope code surfaces, the contract
§3 anti-scope, the proposal §1 design, and the ticket Code Boundary
authorizations called out in the prompt (target_jsonl_path deletion,
SpawnCwdUnsupported variant, claude_project_dir_for signature, base-command
`current_dir` injection).

## Code changes

### `src-tauri/src/migration/mod.rs`

- `MigrationError::SpawnCwdUnsupported { provider, cwd }` (`:27-30`) — contract
  §4 mandated variant; explicitly authorized by the prompt.
- `claude_project_dir_for(provider, cwd)` (`:256-265`) — contract §4
  signature; produces AC-2's cwd-derived encoding and AC-2/AC-1's rejection
  path for non-absolute or empty cwd.
- `migrate_chain_segment` 8th param `resume_working_dir: &Path` (`:88`) —
  contract §4. Slot order matches contract spec (between `resolved` and
  `target_provider_index`).
- `#[allow(clippy::too_many_arguments)]` on `migrate_chain_segment` (`:83`) —
  required because the contract-specified 8-arg signature trips the default
  clippy threshold of 7. Smallest accommodation; not a refactor.
- Replacement of the source-derived `cwd_hash` block at old `:155-161` with
  `let cwd_project_dir = claude_project_dir_for(&target.name, resume_working_dir)?;`
  (`:161`) — contract §2 hookpoint; the AC-2 fix.
- `target_dir = projects_dir.join(&cwd_project_dir)` (`:188`) — propagates
  the helper's output to the same hookpoint specified in contract §4.
- Inline test split: `migration_reuses_source_session_id_on_target_side`
  removed and replaced with
  `migration_reuses_source_session_id_when_source_and_spawn_cwd_match` and
  `migration_writes_target_under_spawn_cwd_when_source_and_spawn_cwd_differ`
  (`:333-456`) — contract §5 mandated split; same-cwd half preserves the
  original A1 risk, different-cwd half encodes RC-1.
- Three `claude_project_dir_for_*` unit tests (`:458-498`) — contract §5
  helper-unit-test set (encodes A2).
- New test helpers `claude_project_dir_name`, `seed_source_jsonl`,
  `seed_resolved` (`:319-372`) — extracted from the original monolithic test
  so the same setup serves both halves of the split. Used only by the three
  new test functions; not drive-by additions.

### `src-tauri/src/main.rs`

- `effective_spawn_cwd(working_dir: Option<&Path>)` helper (`:1047-1056`) —
  contract §4 effective-cwd derivation. Lifted out of both call sites to
  avoid duplication.
- Call at `run_repl` migration site (`:1617`, `:1624`) and `run_resume`
  migration site (`:1842`, `:1849`) — contract §2 mandated insertion.
- Removal of `target_jsonl_path: None` initializer at the two
  `ResumePayload` construction sites (`:1715`, `:1915`) — explicit ticket
  Code Boundary authorization (field deletion).

### `src-tauri/src/executor/cli.rs`

- Deletion of `target_jsonl_path: Option<&'a Path>` field from
  `ResumePayload` (`:279`) and the `_target_jsonl_path` parameter from
  `compose_resume_args` (`:282-285`) — explicit Code Boundary authorization;
  the field was dead per Phase 0 RCA.
- Deletion of `compose_resume_args_ignores_target_jsonl_for_flag_strategy`
  and `compose_resume_args_ignores_target_jsonl_for_subcommand_strategy`
  along with the only-callers `resume_strategy_flag_fixture` /
  `resume_strategy_subcommand_fixture` helpers — explicit Code Boundary
  authorization. Tests asserted dead behavior; helpers had no remaining
  consumers (verified via grep — zero hits outside the deleted block).
- Removal of `target_jsonl_path: None` from the four executor unit tests at
  `:1095`, `:1142`, `:1189`, `:1238` — contract §2 propagation.
- Four `// risk: Executor resume payload/argv without target JSONL path; ...`
  annotation comments added to the four updated executor tests (`:1062`,
  `:1107`, `:1154`, `:1201`) — Phase 6b traceability comments matching the
  annotation style used elsewhere in the file (e.g., the original `risk:`
  annotation on the deleted `compose_resume_args_ignores_*` tests). One-line
  documentation only; no behavior change.

### `src-tauri/tests/initiative_05_migration.rs`

- New helper `claude_project_dir_name` (`:635-637`) — needed at two
  assertion sites (`:679` and `:845`) to derive the new expected target path
  per contract §2 (do NOT relax to `starts_with`). Used only by those two
  tests.
- `let resume_working_dir = ...` declaration plus `&resume_working_dir`
  argument added to all 14 `migrate_chain_segment(...)` call sites listed
  in contract §2 (`:644`, `:723`, `:802`, `:883`, `:910`, `:958`, `:988`,
  `:1012`, `:1042`, `:1068`, `:1094`, `:1120`, `:1155`, `:1185`) —
  mechanical signature propagation per contract.
- Exact-target-path assertions at `:679` and `:845` updated to derive from
  `claude_project_dir_name(&resume_working_dir)` — contract §2
  ("update the expected path; do NOT relax to `starts_with`").

### `src-tauri/tests/pr_f_resume_integration.rs`

- `cmd.current_dir(self.dir.path());` added to `base_repl_command` (`:335`)
  and the two resume base-command builders (`:355`, `:370`) — explicit
  prompt authorization (Phase 6b deterministic-spawn-cwd contract
  requirement).
- Migration target-path assertion at `repl_resume_migrates_to_least_loaded_provider:945-963`
  updated to derive `expected_target_dir` from `fixture.dir.path()` and
  added negative-existence assertion against the source-cwd-derived path
  — contract §5 end-to-end AC-2 lock-in.
- Source-side fixture string `"cwd-hash-fixture"` retained for the
  `stage_claude_jsonl` source-side helper, as contract §2 directs ("Do NOT
  rename the helper or remove `cwd-hash-fixture` from the source-side
  fixture").

### `src-tauri/tests/session_migration_rca.rs` (+ `mod.rs`, `rc1_*.rs`)

- New `#[cfg(unix)]` test crate file plus `session_migration_rca/mod.rs`
  shared fixture and `rc1_cwd_project_dir_mismatch.rs` reproduction harness
  — Phase 0 RCA artifact; AC-1's load-bearing regression test. Unix gating
  matches the contract's anti-scope on Windows path-hash (deferred to
  WU-14-02). Helper `claude_project_dir_name` mirrors the inline test-only
  encoder pattern called out in contract §3.

### `README.md`

- One-paragraph addition (`:654`) describing cwd-derived re-anchoring after
  migration — implements AC-7.

## Doc/artifact changes

Each is the orchestrator-mandated output of its phase, not drift:

- `proposals/14-session-migration-cwd.md` — Phase 3 proposal.
- `product-strategy/contracts/wu-14-01-session-migration-cwd.md` — Phase 6a
  orchestrator contract.
- `research/14-{problem-map,hookpoints,session-migration-rca}.md` — Phase 0
  RCA, Phase 2.5 problem map, Phase 5 hookpoints.
- `risk/14-{audit,scope,shortcut,supported-surface,test-residuals}.md` —
  Phase 4 risk gates and Phase 6b residual artifact.
- `risk/14-process-tree-audit-phase{4,6}.md` — Phase 4 / Phase 6 process-tree
  audits.

## Drift / drive-by check

Specifically searched for changes that don't trace to AC-1..AC-7 or contract
§2/§4/§5. None found:

- No edits to anti-scope surfaces (contract §3): no `balancer/` change, no
  `state/db.rs` schema change, no `sessions/`, `session_metadata/`,
  `routing_fanout_rca/`, `release_yml_contract`, or
  `session_lock_cross_platform` edits, no `src/` or `e2e/` edits, no
  `scripts/claude-code-locate-transcript` change, no Codex migration
  extension. Test-only Claude project-dir encoders in
  `tests/session_migration_rca/mod.rs:129` and the `tests/fixtures/`
  encoders are untouched (contract §3 directive); the new
  `session_migration_rca/mod.rs::claude_project_dir_name` is the test-only
  encoder the contract anticipated.
- No new abstractions, services, DI shims, or backwards-compatibility
  shims for the old source-derived target path (contract §3 anti-scope:
  "No backwards-compatibility shim").
- No `Cargo.toml` / `Cargo.lock` change.
- No Windows path-hash handling beyond the `SpawnCwdUnsupported` rejection
  required by contract §4 (deferred to WU-14-02 per contract §3).
- No symlink canonicalization (contract §3).
- The `#[allow(clippy::too_many_arguments)]` is forced by the contract's
  8-arg signature; the only alternative would be a struct refactor that the
  contract does not authorize.
- Helper extraction in `migration/mod.rs#tests` (`claude_project_dir_name`,
  `seed_source_jsonl`, `seed_resolved`) is required to express the
  contract's mandated test split without duplicating ~30 lines of identical
  setup across three tests; helpers are private to the test module.

## Verdict justification

Every code, test, doc, and artifact change traces to AC-1..AC-7,
contract §2/§4/§5, or to an explicit prompt-authorized boundary
(target_jsonl_path field/test deletion, base-command `current_dir`
injection, `SpawnCwdUnsupported` variant, `claude_project_dir_for`
signature). No speculative abstraction, unrelated refactor, or anti-scope
violation observed.

Verdict: **LOW**.
