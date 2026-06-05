# Push/Pull Coupling Audit

## Inputs Read

- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `repo_root=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/proposal.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/contracts/tsb.contract.md`
- `diff_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/gates/diff.patch`
- `touched_surfaces_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/gates/touched-files.txt`
- `output_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/code-quality/tsb/reports/push-pull-auditor.md`

## References Read

- `/home/nes/ai/conventions/code-quality.md` lines 21-27, 106-131, 143-149, 169-173, 291-310. A1 Push-vs-pull system coupling, Auditor Scope Boundary, Touched-file ownership, Phase 6 contract visibility, Numerical thresholds, and `uncontrolled-source coupler` failure mode are present and non-contradictory.
- `/home/nes/ai/conventions/agent-questions-and-session-graph.md` lines 230-242. Session-graph Pull-vs-Push Policy disambiguator is present and distinct from this system-coupling audit.
- `planning/tsb-gate/proposal.md` lines 3-5. Proposal declares the intended public-CLI adapter shape, bounded discovery/export calls, degraded marker, and runtime-owned script deadlines.
- `planning/tsb-gate/contracts/tsb.contract.md` lines 133-158 and 159-188. Contract declares adapter surfaces and intrinsic ownership for `scripts/opencode-turns`, `crates/oulipoly-runtime/src/sessions/mod.rs`, and `crates/oulipoly-runtime/src/quota/process.rs`.
- `scripts/README.md` lines 16-46 and 119-127. Repository contract for turn-script JSONL and transcript-locator stdout.
- `README.md` lines 465-476. Repository contract for optional `transcript_locator` script behavior.

## Pull Sites Inspected

| ID | Puller | Source | Pull mechanism | Ownership/interface evidence | Verdict | Evidence |
|---|---|---|---|---|---|---|
| PP-001 | `crates/oulipoly-runtime/src/quota/process.rs` quota/auth script runner | User-configured shell command stdout/stderr/exit status plus std process/pipe/process-group APIs | `sh -c`, piped stdout/stderr drain, `try_wait`, timeout formatting, Unix process-group kill | LOW common-interface proof: Phase 6 contract declares this file as an adapter translating user-configured quota/auth shell command stdout/stderr/exit and std process/stream contracts; intrinsic declaration owns quota script deadline, timeout token, and process-group kill behavior. | LOW | `crates/oulipoly-runtime/src/quota/process.rs` lines 44-61, 78-83, 117-129, 171-188, 235-242; `planning/tsb-gate/contracts/tsb.contract.md` lines 149-157 and 180-188. |
| PP-002 | `crates/oulipoly-runtime/src/sessions/mod.rs` turn-script scanner | User-configured session `turn_script` stdout JSONL, degraded marker JSONL, and `StateDb` ingest API | Runs configured script with `STATE_DIR`, parses one JSON object per line into `ScriptTurn`, recognizes `{"degraded":true,"count":...}`, writes through `StateDb` methods | LOW common-interface proof: Phase 6 contract declares session script stdout/stderr/exit, Oulipoly session turn JSONL, Oulipoly degraded marker, and StateDb ingest as translated contracts; repository docs define the turn-script JSONL fields. | LOW | `crates/oulipoly-runtime/src/sessions/mod.rs` lines 24-34, 96-128, 154-204, 234-249, 388-417, 551-566; `planning/tsb-gate/contracts/tsb.contract.md` lines 143-148, 157, 171-179; `scripts/README.md` lines 16-46. |
| PP-003 | `crates/oulipoly-runtime/src/sessions/mod.rs` transcript locator path resolution | User-configured `transcript_locator` stdout single-line path | Runs configured locator script with `STATE_DIR` and `SESSION_ID`, requires exactly one non-empty stdout line, maps line to `PathBuf` | LOW common-interface proof: repository documentation declares the locator contract and the Phase 6 contract includes transcript locator execution under the session runtime deadline owner note. | LOW | `crates/oulipoly-runtime/src/sessions/mod.rs` lines 453-471, 481-494, 497-532, 551-566, 674-696; `README.md` lines 465-476; `scripts/README.md` lines 119-127; `planning/tsb-gate/contracts/tsb.contract.md` line 157. |
| PP-004 | `scripts/opencode-turns` OpenCode adapter | OpenCode public CLI output from `opencode session list --json` and `opencode export <sessionID>` | Spawns OpenCode through configured command, parses public CLI JSON/text values into normalized Oulipoly JSONL records, applies recent-window/cap filtering, emits degraded marker on timeout | LOW common-interface proof: proposal explicitly requires public CLI use rather than private storage; Phase 6 contract declares `scripts/opencode-turns` as an adapter translating the OpenCode public CLI surface, Oulipoly session turn JSONL, and Oulipoly degraded marker contract. | LOW | `scripts/opencode-turns` lines 8-28, 195-238, 285-336, 409-432, 439-491, 501-509, 512-576, 588-629; `planning/tsb-gate/proposal.md` lines 3-5; `planning/tsb-gate/contracts/tsb.contract.md` lines 137-142 and 157. |
| PP-005 | `scripts/opencode-turns` option and launch configuration | Process environment variables and CLI argv (`OPENCODE_TURNS_*`, `OPENCODE_BIN`, `BASE_DIR`, explicit session IDs) | Reads env options, shell-splits `OPENCODE_BIN`, accepts explicit session args, ignores `BASE_DIR` for compatibility | LOW source-control/common-interface proof: Phase 6 contract declares the adapter owns `OPENCODE_TURNS_*` option parsing; the OpenCode command boundary is the declared public CLI adapter surface; explicit session IDs are caller-pushed argv into the adapter interface. | LOW | `scripts/opencode-turns` lines 5-19, 66-108, 339-344, 409-411, 614-629; `planning/tsb-gate/contracts/tsb.contract.md` lines 137-142 and 163-170. |
| PP-006 | `scripts/tests/opencode-turns.test.sh` shell proof harness | Same-repo adapter path, test-owned temp files, mock OpenCode CLI output, stdout/stderr/export logs | Uses `$PWD/scripts/opencode-turns`, creates mock `opencode`, reads temp stdout/stderr/log files with `grep` and `cat` | LOW source-control proof: all pulled layouts are same-repo test harness conventions or temp artifacts produced by the harness itself; mock output implements the declared public CLI adapter boundary. | LOW | `scripts/tests/opencode-turns.test.sh` lines 8, 52-88, 90-122, 124-143, 145-185; `planning/tsb-gate/contracts/tsb.contract.md` lines 13-14 and 137-142. |
| PP-007 | `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` integration fixture | Same-workspace `StateDb`, same-repo `scripts/opencode-turns`, test-owned fake provider/OpenCode scripts, fixture SQLite rows | Builds fixture paths, reads provider record JSONL, snapshots SQLite via SQL queries, invokes runtime session scanner with fake OpenCode public CLI | LOW source-control proof: integration test, state schema, fake provider scripts, and adapter path are within the same repository/workspace controlled boundary; the OpenCode path is test-owned and implements the declared public CLI contract. | LOW | `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` lines 77-167, 203-213, 423-478, 594-659, 940-1028, 1334-1411; `planning/tsb-gate/contracts/tsb.contract.md` lines 17 and 137-142. |

## Uncontrolled-Source Coupler Findings

| ID | Puller | Source | Implicit contract evidence | Missing proof | Decoupling direction | Failure mode |
|---|---|---|---|---|---|---|
| None | None | None | No touched pull site reads private storage shape, private file layout, unstable generated output, incidental naming convention, private endpoint, or uncontrolled deployment topology without ownership/common-interface proof. | None | None | None |

## Residual Ambiguity / Stop-Condition Notes

- No stop condition fired. Required inputs, Phase 6 contract, and proposal were readable before scoring.
- The proposal notes proof gaps for max-session cap enforcement and descendant cleanup (`planning/tsb-gate/proposal.md` lines 53-69). Those are proof-risk/testing concerns, not A4 uncontrolled-source-coupler findings, because the touched pull sites still route through declared adapter or same-controlled-boundary interfaces.
- Deployment-level pull sites inspected here are script/process execution, process environment, filesystem temp fixtures, SQLite fixture/state access, and public OpenCode CLI calls. No touched deployment edge pulls from OpenCode private storage, a private endpoint, or undeclared service topology.

Verdict: LOW

VERDICT: LOW
