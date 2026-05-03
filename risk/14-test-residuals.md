# WU-14-01 Test Residuals

Phase: 6b encode-tests-first

## Residuals

### Real Claude Code cwd lookup

- Unverified risk: RC-1 fixture validates the observed cwd lookup contract with a fake Claude process, not the real Claude Code binary.
- Residual class: `integration-hidden`
- Technique considered: chaos
- Scope: real Claude Code invocation against a live config directory.
- Budget or bound: out of Step 6b scope; contract requires in-repo tests only.
- Result: encoded by `src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs` using the existing RCA fixture.
- Remaining residual: real Claude could diverge from the fixture despite RCA evidence.
- Invalidating inputs: a real Claude Unix run that does not map cwd with `/` replaced by `-`.
- Net-value impact: does not change the approved net-value case; the RCA already observed the same lookup behavior.

### Windows Claude project directory hashing

- Unverified risk: A5, Windows cwd hashing is not defined by this repository.
- Residual class: `bounded-model`
- Technique considered: symbolic
- Scope: Windows absolute paths, drive-letter normalization, and backslash handling.
- Budget or bound: deferred by contract to `WU-14-02-windows-claude-path-hash`.
- Result: no Step 6b test emitted for Windows hashing.
- Remaining residual: the Unix helper tests do not prove Windows placement semantics.
- Invalidating inputs: authoritative Windows Claude Code hashing contract or an in-repo Windows encoder.
- Net-value impact: does not change the approved net-value case; Windows behavior is explicitly out of scope.

### Symlink and canonicalization behavior

- Unverified risk: A3, Claude Code may canonicalize symlinked cwd paths before hashing.
- Residual class: `combinatorial/path-state`
- Technique considered: property-based
- Scope: symlinked workspaces and relative paths resolved through symlink components.
- Budget or bound: out of WU-14-01 scope; contract forbids symlink canonicalization changes.
- Result: no Step 6b symlink test emitted.
- Remaining residual: tests cover absolute path encoding and relative rejection, not symlink equivalence.
- Invalidating inputs: a real-Claude harness showing canonicalized symlink cwd hashing is required.
- Net-value impact: does not change the approved net-value case; the proposal names this as a future investigation.

### `working_dir = None` production call-site behavior

- Unverified risk: production `run_repl` and `run_resume` must use `std::env::current_dir()` when no working dir is supplied.
- Residual class: `integration-hidden`
- Technique considered: graph
- Scope: top-level migration path through `src-tauri/src/main.rs` with `working_dir = None`.
- Budget or bound: Step 6b is constrained to the listed test files and must not modify `src-tauri/src/main.rs`.
- Result: no new main-level test emitted.
- Remaining residual: type checking will force the call-site signature update, but tests do not prove the exact default-cwd value selected in `main.rs`.
- Invalidating inputs: a supported migration route that launches the child from a different cwd than the value passed to migration.
- Net-value impact: does not change the approved net-value case; helper behavior is deterministic and main.rs derivation is specified for Step 6c.

### README semantic coverage

- Unverified risk: AC-7 documentation could be omitted or worded incorrectly.
- Residual class: `integration-hidden`
- Technique considered: bounded-model
- Scope: README content around resume/session storage documentation.
- Budget or bound: Step 6b is test-only and the contract assigns README changes to Step 6c.
- Result: no doc checker emitted.
- Remaining residual: README compliance depends on Step 6c review/gate evidence, not an automated test.
- Invalidating inputs: README lacks the child-cwd re-anchor paragraph after Step 6c.
- Net-value impact: does not change the approved net-value case; this is a documentation acceptance criterion, not runtime behavior.

### Full-suite and CI verification

- Unverified risk: aggregate Rust/frontend gates may catch interactions beyond the focused Step 6b tests.
- Residual class: `emergent-interaction`
- Technique considered: graph
- Scope: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test --no-fail-fast`, `bun run check`, `bunx tsc --noEmit`, `bun run test`.
- Budget or bound: Step 6b intentionally leaves tests failing to compile until Step 6c lands production code.
- Result: post-edit targeted RCA compile check failed as expected on missing production changes.
- Remaining residual: full green verification is Step 6c responsibility after implementation.
- Invalidating inputs: post-Step 6c gate failures.
- Net-value impact: does not change the approved net-value case; this is the normal test-first handoff state.
