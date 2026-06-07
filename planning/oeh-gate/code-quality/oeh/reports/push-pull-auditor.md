# Push/Pull Coupling Audit

## Inputs Read

| Input | Path / Value |
|---|---|
| mode | phase-6 |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/gates/diff.patch` |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/gates/touched-files.txt` |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/proposal.md` |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/contracts/oeh.contract.md` |
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/evidence/runtime-tests.log` |
| output_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/code-quality/oeh/reports/push-pull-auditor.md` |
| base | `549daaa` |
| head | `HEAD` / `bdbb9e3` |
| original_head | `3515d31` |
| wu_id | `oeh` |

## References Read

| Reference | Evidence |
|---|---|
| A1 metric source | `/home/nes/ai/conventions/code-quality.md` lines 106-132 define push-vs-pull system coupling, canonical-doc-as-schema, and HIGH private-source recipes. |
| Auditor scope and touched-file ownership | `/home/nes/ai/conventions/code-quality.md` lines 21-27 and 143-149. |
| Phase 6 contract visibility | `/home/nes/ai/conventions/code-quality.md` lines 169-173. |
| Numerical thresholds and failure modes | `/home/nes/ai/conventions/code-quality.md` lines 291-310. |
| Terminology disambiguator | `/home/nes/ai/conventions/agent-questions-and-session-graph.md` lines 230-242. |
| OEH proposal | `planning/oeh-gate/proposal.md` lines 3-9 and 15-31. |
| OEH Phase 6 contract | `planning/oeh-gate/contracts/oeh.contract.md` lines 3-25 and 66-124. |
| Touched files | `planning/oeh-gate/gates/touched-files.txt` lines 1-5. |
| Diff evidence | `planning/oeh-gate/gates/diff.patch` lines 1-433. |
| Runtime evidence | `planning/oeh-gate/evidence/runtime-tests.log` lines 114-180 record the relevant runtime tests passing; lines 1-990 were available as runtime artifact evidence. |

A1 preservation check passed: the metric source includes the Push-vs-pull system coupling section, the session-graph Pull-vs-Push Policy disambiguator exists, the `uncontrolled-source coupler` failure mode exists, and the Numerical thresholds section exists.

## Pull Sites Inspected

| ID | Puller | Source | Pull mechanism | Ownership/interface evidence | Verdict | Evidence |
|---|---|---|---|---|---|---|
| PP-001 | `.gitignore` | Repository scratch/artifact file layout | Ignore pattern for `.scratch/` | No runtime pull site. The contract classifies `.gitignore` as artifact hygiene, not product behavior. | LOW | `planning/oeh-gate/contracts/oeh.contract.md` lines 13 and 21; `planning/oeh-gate/gates/diff.patch` lines 1-9. |
| PP-002 | `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | Terminal-signal classification contract and `std::process::ExitStatus` | Reads `TerminalSignal`, optional real child `ExitStatus`, `synthetic_exit_code`, `exit_code_from_status`, and `terminal_reason_from_signal` to build `SupervisedOutput` | Same crate/repo boundary controls the terminal DTO and supervised-output mapping; Phase 6 contract declares this file as an adapter translating `terminal-signal-classification-contract`, `std-process-exit-status-contract`, and `supervised-output-contract`. | LOW | File lines 19-26 and 36-77; contract lines 22, 33-34, and 68-75. |
| PP-003 | `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` tests | OpenCode incident stream fixture and Unix `ExitStatusExt::from_raw` status encoding | Test fixture feeds bytes and real exit status into `supervised_output_from_terminal` | Fixture source is local test data in the touched file; real status encoding is an explicit std/Unix process-status contract covered by the contract adapter declaration. | LOW | File lines 85-138; contract lines 51-52 and 70-75. |
| PP-004 | `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `TerminalSignalKind`, `TerminalSignal`, `TerminalStatusEvidence`, canonical terminal reason vocabulary, Unix signal names | Pattern matches DTO variants and maps process status/signal values into terminal reasons and synthetic exit codes | Terminal DTO is declared in `crates/oulipoly-provider/src/terminal.rs` and re-exported through runtime; same repo controls it. Phase 6 contract declares this file as an adapter for `executor-terminal-signal-dto-contract`, `canonical-terminal-reason-vocabulary`, `std-process-exit-status-contract`, and `provider-error-evidence-carrier`. | LOW | File lines 58-63, 65-80, 110-190, and 354-375; provider contract lines 3-44; OEH contract lines 23, 35-37, and 76-82. |
| PP-005 | `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | Provider-error evidence carrier | `unknown_terminal_reason` trims `signal.evidence` and checks `provider error: ` prefix | The prefix is an in-repo evidence carrier produced by the OpenCode recognizer in the same controlled boundary. Phase 6 contract explicitly declares `provider-error-evidence-carrier` and provider-error evidence preservation for Unknown terminal signals. | LOW | File lines 63 and 172-178; `crates/oulipoly-runtime/src/executor/providers/opencode.rs` lines 79-85; OEH contract lines 35-37, 76-82, and 117-123. |
| PP-006 | `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | OpenCode JSON stream event contract | Converts stdout/stderr bytes to text, pulls the last non-empty line, parses JSON, reads `type`, `error`, `/data/statusCode`, `/data/status_code`, `/statusCode`, `/status`, `/data/message`, `/message`, `/name`, and `/data/name` | Phase 6 contract declares this file as an adapter translating `OpenCode json stream event contract`, `Oulipoly terminal-signal-recognizer contract`, and `Oulipoly terminal-signal evidence contract`; it also declares the intrinsic OpenCode terminal-signal recognition domain, including structured `type:error` terminal events and the last non-empty stream line terminality rule. | LOW | File lines 1-6, 19-54, 67-172; diff lines 148-207; contract lines 24, 38-45, 83-89, and 101-109. |
| PP-007 | `crates/oulipoly-runtime/src/executor/providers/opencode.rs` tests | Recognizer evidence fixture and OpenCode incident/recovered stream fixtures | Constructs `TerminalSignalEvidence` and checks terminal signal kind/evidence | Fixture source is local to the touched file, while `TerminalSignalEvidence` and `TerminalSignalKind` are in-repo DTO contracts controlled by `oulipoly-provider`; Phase 6 contract lists these test functions under the OpenCode adapter/intrinsic surface. | LOW | File lines 174-287; provider contract lines 3-44; OEH contract lines 53-58 and 83-89. |
| PP-008 | `src-tauri/tests/opencode_terminal_error_exit_zero.rs` | Unix CLI integration fixture contract | Uses `Age153Fixture` to write model/provider TOML, shell command bodies, resume chain state, and execute one-shot/resume paths | Test fixture helper is in the same `src-tauri/tests` controlled boundary. Phase 6 contract declares this test component as an adapter translating `Unix CLI integration fixture contract`, `Oulipoly result-envelope JSON contract`, and `StateDb invocation terminal fields`. | LOW | Touched test lines 16-88 and 91-99; helper `src-tauri/tests/age153_support/mod.rs` lines 18-57, 72-157, and 175-180; OEH contract lines 25, 59-64, and 89-95. |
| PP-009 | `src-tauri/tests/opencode_terminal_error_exit_zero.rs` | Oulipoly result-envelope JSON contract | Pulls stdout line with `OULIPOLY_RESULT=`, parses JSON via helper, and asserts `status`, `success`, `exit_code`, and `terminal_reason` | Result envelope shape is asserted through an in-repo helper that declares expected keys. Phase 6 contract declares `Oulipoly result-envelope JSON contract`; runtime `ExecutionResult` and CLI result mapper define the source fields in the same repository boundary. | LOW | Touched test lines 23-31 and 48-56; helper lines 471-509; `crates/oulipoly-runtime/src/executor/mod.rs` lines 55-66; `crates/oulipoly-runtime/src/executor/cli/result.rs` lines 42-99; OEH contract lines 89-95. |
| PP-010 | `src-tauri/tests/opencode_terminal_error_exit_zero.rs` | StateDb `invocations` table fields | Direct SQL query reads `status`, `success`, `exit_code`, and `terminal_reason` from `invocations` by `provider_name` | The pull reads a storage shape, but source-control proof is present: the `oulipoly-state` crate in the same repository owns StateDb schema/migrations, and the Phase 6 contract declares `StateDb invocation terminal fields` as a translated contract for this test component. | LOW | Touched test lines 101-122; `crates/oulipoly-state/migrations/0004_state_db_schema_boundary.sql` lines 1-18; `crates/oulipoly-state/src/schema.rs` lines 8-9; OEH contract lines 89-95. |

## Uncontrolled-Source Coupler Findings

| ID | Puller | Source | Implicit contract evidence | Missing proof | Decoupling direction | Failure mode |
|---|---|---|---|---|---|---|
| None | None | None | No HIGH private-source, uncontrolled source, private endpoint, unstable generated output, incidental naming convention, or private file layout pull lacked source-control or stable common-interface proof inside the touched set. | None | None required. | None |

## Residual Ambiguity / Stop-Condition Notes

No stop condition fired. The direct StateDb SQL assertion is the highest-coupling site because it pulls private storage shape, but it remains LOW under the supplied Phase 6 contract and same-repository schema ownership proof. No deployment-level service, database, cache, filesystem, private endpoint, or service-topology pull site outside the local test fixture was introduced by the touched files.

VERDICT: LOW
