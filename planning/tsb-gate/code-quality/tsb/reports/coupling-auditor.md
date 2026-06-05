# Coupling Audit

## Inputs Read

| Input | Path | Notes |
|---|---|---|
| `worktree_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Source inspection resolved from this worktree. |
| `repo_root` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Same as worktree path. |
| `planning_dir` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate` | Planning artifact root. |
| `wu_id` | `tsb` | Used for report identity. |
| `proposal_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/proposal.md` | Read lines 1-69. |
| `contract_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/contracts/tsb.contract.md` | Read lines 1-190 before scoring. |
| `diff_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/gates/diff.patch` | Read lines 1-1288. |
| `touched_surfaces_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/gates/touched-files.txt` | Read lines 1-5. |
| `output_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/code-quality/tsb/reports/coupling-auditor.md` | This report. |

## References Read

| Reference | Evidence |
|---|---|
| `~/ai/conventions/code-quality.md` | Auditor scope boundary lines 21-25; touched-file ownership lines 143-145; adapter declarations lines 180-204; intrinsic-surface declarations lines 241-249; A1 coupling row lines 291-300. |
| `~/ai/conventions/proposer-critic-pattern.md` | Critic independence and no proposer self-review lines 29-35. |
| `~/ai/conventions/risk-profile.md` | Touched-file ownership and evidence requirement lines 11-16. |
| `~/ai/workflows/implementation-pipeline.md` | Phase 6 contract visibility via `code-quality.md` reference and LOW-only disposition context; Phase 8 confirms actual diff review target at lines 528-538; decision recording does not authorize non-LOW code-quality residual acceptance at lines 627-631. |
| `planning/tsb-gate/proposal.md` | Public-CLI adapter and runtime hard-deadline claims at lines 3-5. |
| `planning/tsb-gate/contracts/tsb.contract.md` | Touched files lines 9-17; adapter declarations lines 133-155; intrinsic-surface declarations lines 159-188. |

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| `crates/oulipoly-runtime/src/quota/process.rs` | Touched surface line 1; diff lines 1-136. | Declared adapter in contract lines 149-155 and intrinsic surface in contract lines 180-188. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | Touched surface line 2; diff lines 137-427. | Declared adapter in contract lines 143-148 and intrinsic surface in contract lines 171-179. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | Touched surface line 3; diff lines 428-443. | No adapter or intrinsic declaration in the resolved contract carrier. Raw non-declared scoring applies. |
| `scripts/opencode-turns` | Touched surface line 4; diff lines 444-1090. | Declared adapter in contract lines 137-142 and intrinsic surface in contract lines 163-170. |
| `scripts/tests/opencode-turns.test.sh` | Touched surface line 5; diff lines 1093-1288. | New touched file. No adapter or intrinsic declaration in the resolved contract carrier. Raw non-declared scoring applies. |

## Per-Pair Coupling

| Source component | Target component | Distinct external symbols/modules referenced | Adapter declaration artifact path | Declared adapter component | `Translates:` contracts | Contract count | Adapter verdict | Intrinsic declaration artifact path | Declared intrinsic component | `Domain:` | `Owns:` set or summary | Domain count | Intrinsic-surface verdict | Final verdict | blocking_or_residual | Evidence |
|---|---:|---|---|---|---|---:|---|---|---|---|---|---:|---|---|---|---|
| `crates/oulipoly-runtime/src/quota/process.rs` | quota/auth shell command and Rust process execution contracts | 3 declared contracts | `planning/tsb-gate/contracts/tsb.contract.md` | `crates/oulipoly-runtime/src/quota/process.rs` | user-configured quota/auth shell command stdout/stderr/exit contract; std process execution contract; std concurrent stream draining contract | 3 | LOW | `planning/tsb-gate/contracts/tsb.contract.md` | `crates/oulipoly-runtime/src/quota/process.rs` | `quota_script_execution_deadline` | `SCRIPT_TIMEOUT_SECS`; `run_script_with_timeout`; `script_timeout`; process-group kill on quota timeout | 1 | LOW | LOW | blocking | Contract lines 149-155 and 180-188; source imports and process use lines 19-21, 78-83, 180-188. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | session script stdout/stderr/exit, session turn JSONL, StateDb ingest, session deadline domain | 3 declared contracts plus 1 declared domain | `planning/tsb-gate/contracts/tsb.contract.md` | `crates/oulipoly-runtime/src/sessions/mod.rs` | user-configured session script stdout/stderr/exit contract; Oulipoly session turn JSONL contract; Oulipoly StateDb session-turn ingest contract | 3 | LOW | `planning/tsb-gate/contracts/tsb.contract.md` | `crates/oulipoly-runtime/src/sessions/mod.rs` | `session_script_execution_deadline` | `SCRIPT_TIMEOUT_SECS`; `run_session_script_with_timeout`; `script_timeout`; process-group kill; degraded marker recognition | 1 | LOW | LOW | blocking | Contract lines 143-148 and 171-179; source session script/degraded/deadline references lines 96-127, 177-204, 512-696. |
| `scripts/opencode-turns` | OpenCode CLI, JSONL/degraded contracts, adapter options, Python process/time modules | 9 raw modules imported, plus undeclared Python process/time symbols | `planning/tsb-gate/contracts/tsb.contract.md` | `scripts/opencode-turns` | OpenCode public CLI surface; Oulipoly session turn JSONL contract; Oulipoly degraded turn-scan marker contract | 3 | HIGH | `planning/tsb-gate/contracts/tsb.contract.md` | `scripts/opencode-turns` | `opencode_turns_adapter_options` | `OPENCODE_TURNS_WINDOW_HOURS`; `OPENCODE_TURNS_MAX_SESSIONS`; `OPENCODE_TURNS_CALL_TIMEOUT`; `OPENCODE_TURNS_DEADLINE` | 1 | LOW for option-domain references only | HIGH | blocking | Contract lines 137-142 and 163-170 do not declare Python process/time contracts. Source imports `json`, `os`, `re`, `shlex`, `signal`, `subprocess`, `sys`, `time`, and `datetime` at lines 31-39; process-group execution and kill references are at lines 439-491; deadline time references are at lines 76-86. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | runtime/config/provider/state/sqlite/serde/std integration-test surface | >= 10 raw external modules/symbol groups | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | HIGH | blocking | No contract declaration for this component. Source imports `chrono`, `oulipoly_config`, `oulipoly_provider`, `oulipoly_runtime::provider_registry`, `oulipoly_runtime::session_metadata`, `oulipoly_runtime::session_provider`, `oulipoly_state`, `rusqlite`, `serde_json`, and multiple `std` modules at lines 4-30. |
| `scripts/tests/opencode-turns.test.sh` | shell proof harness, adapter script, mock OpenCode CLI, Python JSON/time helper, filesystem/env/stdout assertions | >= 7 raw external command/module/surface references | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | HIGH | blocking | No contract declaration for this component. Source references the adapter script at line 8, `cat`/`grep` assertion plumbing at lines 30 and 38-48, embedded `python3` with `json` and `datetime` at lines 60-71, OpenCode mock/export behavior at lines 76-84 and 97-119, and environment/runtime launch variables at lines 124-140. |

## Evidence For Non-LOW Scores

| Score | blocking_or_residual | Ownership proof or residual basis | Evidence | Why it supports the verdict |
|---|---|---|---|---|
| HIGH | blocking | `scripts/opencode-turns` is touched by touched-files line 4 and diff lines 444-1090; `code-quality.md` lines 21-25 and 143-145 make whole touched files/components in scope. | Contract adapter declaration lines 137-142 lists 3 translated contracts; intrinsic declaration lines 163-170 owns only option env names. Source imports nine Python modules at lines 31-39, uses time deadline calls at lines 76-86, and uses `subprocess.Popen`, `subprocess.PIPE`, `subprocess.DEVNULL`, `TimeoutExpired`, `os.killpg`, and `signal.SIGKILL` at lines 439-491. | The declared adapter is under the contract-count threshold, but the component reaches Python process/time module contracts that are not named in `Translates:` and are not subordinate to the option `Owns:` set. Per adapter rule in `code-quality.md` lines 196-204, undeclared external contracts keep the component from a LOW adapter verdict. |
| HIGH | blocking | `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` is touched by touched-files line 3 and diff lines 428-443; no matching adapter or intrinsic declaration appears in contract lines 133-188. | Source imports `chrono`, `oulipoly_config`, `oulipoly_provider`, `oulipoly_runtime::provider_registry`, `oulipoly_runtime::session_metadata`, `oulipoly_runtime::session_provider`, `oulipoly_state`, `rusqlite`, `serde_json`, and multiple `std` modules at lines 4-30. | Non-declared coupling preserves the raw A1 threshold from `code-quality.md` line 300. The file references at least ten distinct external symbol/module groups, exceeding the HIGH threshold `>= 6`. |
| HIGH | blocking | `scripts/tests/opencode-turns.test.sh` is touched by touched-files line 5 and is a new file in diff lines 1093-1288; no matching adapter or intrinsic declaration appears in contract lines 133-188. | Source references `scripts/opencode-turns` at line 8, `cat`/`grep` assertions at lines 30 and 38-48, embedded `python3` with `json` and `datetime` at lines 60-71, OpenCode mock behavior at lines 76-84 and 97-119, and env-driven adapter launch at lines 124-140. | Non-declared coupling preserves the raw A1 threshold from `code-quality.md` line 300. The shell proof harness references more than six distinct external modules/commands/surfaces, so the pair scores HIGH. |

## Residual Ambiguity / Stop-Condition Notes

No stop condition fired. The Phase 6 contract and proposal were readable before scoring. Adapter and intrinsic declarations in `planning/tsb-gate/contracts/tsb.contract.md` are syntactically well formed.

The caller requested incremental delta scoring for `9545ff8..c5f57f5`. I used the diff to resolve touched ownership, but applied the canonical whole touched-file/component boundary from `code-quality.md` lines 21-25 and 143-145. The non-LOW findings above are therefore blocking, not residual.

VERDICT: HIGH
