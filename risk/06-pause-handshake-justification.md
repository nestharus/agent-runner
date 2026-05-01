# Justification Review — 06-pause-handshake

**Verdict:** `LOW_CONCERN`

Every change in `git diff main..HEAD` traces to the proposal/contract scope. The diff is +3811 lines and 0 deletions across 18 files; it is purely additive and does not touch any unrelated subsystem. One minor finding (missing README updates that the proposal explicitly scoped to this PR) is recorded as `LOW` for the next fix pass.

## Scope of review

Asks: does every change in this diff serve `proposals/06-pause-handshake.md` Rev 4 and `research/06-pause-handshake-contract.md`? Justification review does not re-validate the supported-surface or test-firstness gates; those have their own dimensions.

Inputs consulted: contract §1–§10, proposal Rev 4 §1–§13, problem-map §1–§7, audit history (R1–R4 + 4 CodeRabbit passes), and Phase 6 process-tree audit (`PASS-WITH-ADVISORY`).

## Diff inventory and traceability

| File or group | Lines | Justification anchor |
|---|---:|---|
| `proposals/06-pause-handshake.md` | +700 | Phase 3 proposal Rev 4. Required artifact. |
| `research/06-pause-handshake-contract.md` | +187 | Phase 6 Step 6a contract. Required artifact. |
| `research/06-pause-handshake-hookpoints.md` | +388 | Phase 5 hookpoints. Required artifact. |
| `research/06-pause-handshake-problem-map.md` | +152 | Phase 2.5 problem map. Required artifact. |
| `risk/06-pause-handshake-audit.md` | +147 | Phase 4 audit. Required artifact. |
| `risk/06-pause-handshake-scope.md` | +241 | Phase 4 scope. Required artifact. |
| `risk/06-pause-handshake-shortcut.md` | +217 | Phase 4 shortcut. Required artifact. |
| `risk/06-pause-handshake-supported-surface.md` | +289 | Phase 4 supported-surface. Required artifact. |
| `risk/06-pause-handshake-audit-history.md` | +48 | Audit-history per `~/ai/conventions/audit-history.md`. |
| `risk/06-pause-handshake-process-tree-audit.md` | +85 | Phase 6 process-tree audit. Required artifact. |
| `src-tauri/Cargo.toml`, `Cargo.lock` | +24 | Adds `nix = 0.29` (flock(2)), `getrandom = 0.2` (token CSPRNG), `sha2 = 0.10` (token_hash). All three are consumed by `session_lock/mod.rs`. No unused deps. |
| `src-tauri/src/lib.rs` | +1 | `pub mod session_lock;`. |
| `src-tauri/src/main.rs` | +230 | `Subcommands::Session`, `SessionSubcommands::{PauseHandshake, ResumeHandshake}`, `run_pause_handshake`, `run_resume_handshake`, `default_lock_dir`, `emit_resume_resolution_error`, `emit_lock_error`, `emit_json_error`, and the two TTL constants (`60_000`, `600_000`). All trace to contract §1, §5, §6. |
| `src-tauri/src/session_lock/mod.rs` | +373 | Lock primitive per contract §2 and §3 and proposal §4 / §6 (Rev 4 sentinel-flock + atomic rename). |
| `src-tauri/tests/initiative_06_pause_handshake.rs` | +356 | T1–T11 plus T-release-after-expiry-no-marker (proposal §9.1). |
| `src-tauri/tests/fixtures/initiative_06_pause_handshake.rs` | +370 | Test fixtures for the above. |
| `src-tauri/tests/fixtures/mod.rs` | +3 | Registers the new fixture module under `#![cfg(unix)]`. |

## Findings

### Drift / drive-by cleanup
None. The diff has zero deletions and modifies no pre-existing function bodies. New code is added behind `Subcommands::Session` and a new `session_lock` module; existing dispatch paths (`trace`, `repl`, `resume`, `resume-list`, `migrate-db`, `migrate-config`) are untouched.

### Speculative abstractions
None. `Lease`, `ReleaseReceipt`, `LockError`, and `SessionLock` shapes match contract §2 verbatim. The internal `with_flock` helper, `read_json`, `atomic_write_json`, `remove_if_exists`, `fsync_dir`, `validate_version`, `parse_time`, `stored_token_matches`, `marker_token_matches`, `valid_token`, `generate_token`, `random_hex_128`, `token_hash`, and `io_operational` each carry one concrete responsibility called out by contract §3 and proposal §4. No public surface is broader than the contract requires.

### Unrelated fixes
None. The diff is purely additive; it adds no fix to any pre-existing module.

### Behavior changes outside stated purpose
None. `StateDb::open_default()` is invoked from both new commands solely for resolver-only access (pause) or accepted open-time side effects (resume), per proposal §8 / contract §7, matching 06-locate / 06-export precedent.

### Polish / fix-pass items

- **F-J1 (LOW): README updates promised in proposal §10 are not in this diff.** Proposal §10 lists six README obligations (synopses, receipt fields, token format and TTL bounds, exit codes, sibling release marker, advisory-scope note, and `~/.local/share/oulipoly-agent-runner/locks/` path documentation). `git diff main..HEAD -- README.md` is empty, and `grep` for `pause-handshake|resume-handshake|locks/|sentinel.lock` in `README.md` returns nothing. This is the only stated-purpose item in the proposal that the diff does not deliver. Recommend folding the README block into the next fix pass; it is a documentation finding, not a behavior or design finding, and does not block the PR on the justification dimension. (Supported-Surface Verification may re-classify it under their own gate.)

### Items previously evaluated and not re-raised here

These were classified during the four CodeRabbit passes and the four Phase-4 rounds. Justification review does not re-litigate them:

- TTL proposal/contract numerical drift (R3-F03 closed by aligning the contract; R4-F02/F03 skipped as churn). Implementation uses the contract values 60_000 / 600_000.
- `flock` deprecated-API preference (R2-F08, R3-F07, R4-F07) skipped as design preference; the `#[allow(deprecated)]` is intentional and contained behind `with_flock`.
- Resume-handshake `StateDb::open` side-effect comment (R4-F06) skipped as already-accepted by proposal §8.

### Commit-shape note (informational, not a justification verdict)

Six of the fourteen new commits are research/risk artifacts that landed before product code, matching the implementation-pipeline phase ordering. Commit `7a4e3e7` is a fixture-only fix-pass (Stdio::piped()) confirmed by Phase-6 process-tree audit as fixture-infrastructure correction with no contract or product-behavior change. This shape is consistent with single-feature PR scope; commit-hygiene gate owns the granular per-commit verdict.

## Verdict (restated)

`LOW_CONCERN`. The diff is on-purpose for the stated feature: adding the `agents session pause-handshake` and `agents session resume-handshake` CLI surfaces, the file-backed session-lock primitive (sentinel-flock + atomic rename per Rev 4), the three required dependencies, and the Phase 2.5 / 3 / 4 / 5 / 6 review artifacts that justify them. The only justification-shaped finding is the missing README block (F-J1, LOW), which is appropriate for the next fix pass and does not warrant escalation.
