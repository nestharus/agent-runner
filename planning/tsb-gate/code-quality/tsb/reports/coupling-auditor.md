# Coupling Audit

## Inputs Read

| Input | Path | Notes |
|---|---|---|
| `worktree_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Source inspection resolved from this worktree. |
| `repo_root` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Same as worktree path. |
| `planning_dir` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate` | Planning artifact root. |
| `wu_id` | `tsb` | Used for report identity. |
| `proposal_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/proposal.md` | Read lines 1-69 before scoring. |
| `contract_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/contracts/tsb.contract.md` | Read lines 1-232 before scoring. |
| `diff_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/gates/diff.patch` | Read lines 1-1578; used to resolve delta ownership. |
| `touched_surfaces_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/gates/touched-files.txt` | Read lines 1-5. |
| `output_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/code-quality/tsb/reports/coupling-auditor.md` | This report. |

## References Read

| Reference | Evidence |
|---|---|
| `~/ai/conventions/code-quality.md` | Auditor scope boundary lines 21-25; touched-file ownership lines 143-149; adapter declarations lines 180-204; intrinsic-surface declarations lines 212-253; A1 coupling row lines 291-300. |
| `~/ai/conventions/proposer-critic-pattern.md` | Critic independence and no proposer self-review lines 29-35. |
| `~/ai/conventions/risk-profile.md` | Evidence and touched-file/component ownership lines 11-16. |
| `~/ai/workflows/implementation-pipeline.md` | Phase 6 coupling/cohesion split lines 403-417; Step 6a contract role lines 426-433; per-component code-quality fanout and contract-read requirement lines 489-491. |
| `planning/tsb-gate/proposal.md` | Public-CLI adapter and runtime hard-deadline claims lines 3-5; proof-plan claims lines 11-69. |
| `planning/tsb-gate/contracts/tsb.contract.md` | Touched files lines 9-17; adapter declarations lines 144-168; intrinsic-surface declarations lines 172-205; test-harness declarations lines 209-232. |

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| `scripts/opencode-turns` | Touched surface line 4; diff lines 570-1247; source file lines 1-656. | Declared adapter in contract lines 148-155 and declared intrinsic surface in contract lines 176-187. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | Touched surface line 2; diff lines 137-491; source file lines 1-1138. | Declared adapter in contract lines 156-161 and declared intrinsic surface in contract lines 188-196. |
| `crates/oulipoly-runtime/src/quota/process.rs` | Touched surface line 1; diff lines 1-136; source file lines 1-301. | Declared adapter in contract lines 162-168 and declared intrinsic surface in contract lines 197-204. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | Touched surface line 3; diff lines 492-569; source file lines 1-1421. | Context-only proof harness for this coupling pass. Contract lines 209-222 declare it as test-harness, not adapter or intrinsic-surface. This operator contract excludes test review. |
| `scripts/tests/opencode-turns.test.sh` | Touched surface line 5; diff lines 1247-1578; source file lines 1-342. | Context-only proof harness for this coupling pass. Contract lines 223-232 declare it as test-harness, not adapter or intrinsic-surface. This operator contract excludes test review. |

## Per-Pair Coupling

| Source component | Target component | Distinct external symbols/modules referenced | Adapter declaration artifact path | Declared adapter component | `Translates:` contracts | Contract count | Adapter verdict | Intrinsic declaration artifact path | Declared intrinsic component | `Domain:` | `Owns:` set or summary | Domain count | Intrinsic-surface verdict | Final verdict | blocking_or_residual | Evidence |
|---|---|---:|---|---|---|---:|---|---|---|---|---|---:|---|---|---|---|
| `scripts/opencode-turns` | OpenCode public CLI, Oulipoly session-turn JSONL/degraded marker, Python stdlib process/time/data surfaces | 5 declared contracts | `planning/tsb-gate/contracts/tsb.contract.md` | `scripts/opencode-turns` | OpenCode public CLI surface; Oulipoly session turn JSONL contract; Oulipoly degraded turn-scan marker contract; Python stdlib process/time surface; Python stdlib data/argv parsing surface | 5 | LOW | `planning/tsb-gate/contracts/tsb.contract.md` | `scripts/opencode-turns` | `opencode_turns_adapter_runtime` | OPENCODE_TURNS env options; Python process spawn/kill; deadline/timestamp; data/argv parsing | 1 | LOW | LOW | blocking | Contract lines 148-155 and 176-187. Source uses public CLI only in doc and calls (`opencode session list --json`, `opencode export`) at `scripts/opencode-turns` lines 8-15 and 330-336, env options at lines 66-71, process/time surfaces at lines 74-86 and 439-491, and JSONL/degraded emission at lines 607-617. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | User-configured session script contract, session-turn JSONL contract, StateDb ingest contract, session-script deadline domain | 3 declared contracts | `planning/tsb-gate/contracts/tsb.contract.md` | `crates/oulipoly-runtime/src/sessions/mod.rs` | user-configured session script stdout/stderr/exit contract; Oulipoly session turn JSONL contract; Oulipoly StateDb session-turn ingest contract | 3 | LOW | `planning/tsb-gate/contracts/tsb.contract.md` | `crates/oulipoly-runtime/src/sessions/mod.rs` | `session_script_execution_deadline` | SCRIPT_TIMEOUT_SECS; run_session_script_with_timeout timeout_secs; script_timeout token; process-group kill; degraded marker recognition | 1 | LOW | LOW | blocking | Contract lines 156-161 and 188-196. Source ingests script JSONL and degraded markers at `crates/oulipoly-runtime/src/sessions/mod.rs` lines 91-127 and 154-205, persists through StateDb at lines 392-421, and owns script timeout/process-group handling at lines 501-705. |
| `crates/oulipoly-runtime/src/quota/process.rs` | User-configured quota/auth shell command contract, Rust process execution, Rust stream-draining, quota deadline domain | 3 declared contracts | `planning/tsb-gate/contracts/tsb.contract.md` | `crates/oulipoly-runtime/src/quota/process.rs` | user-configured quota/auth shell command stdout/stderr/exit contract; std process execution contract; std concurrent stream draining contract | 3 | LOW | `planning/tsb-gate/contracts/tsb.contract.md` | `crates/oulipoly-runtime/src/quota/process.rs` | `quota_script_execution_deadline` | SCRIPT_TIMEOUT_SECS; run_script_with_timeout timeout_secs; script_timeout token; process-group kill | 1 | LOW | LOW | blocking | Contract lines 162-168 and 197-204. Source owns quota/auth spawning and stream draining at `crates/oulipoly-runtime/src/quota/process.rs` lines 36-62 and 64-115, timeout polling at lines 117-178, process-group kill at lines 180-195, and timeout formatting at lines 235-244. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | Test-harness context for provider/session dispatch and OpenCode adapter path | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | context-only | Touched surface line 3 and contract test-harness declaration lines 213-222. Not scored as a coupling target because this operator contract is not used for test review. |
| `scripts/tests/opencode-turns.test.sh` | Test-harness context for OpenCode adapter invocation, mocks, env options, stdout/stderr assertions, process deadline proof | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | context-only | Touched surface line 5 and contract test-harness declaration lines 223-232. Not scored as a coupling target because this operator contract is not used for test review. |

## Evidence For Non-LOW Scores

| Score | blocking_or_residual | Ownership proof or residual basis | Evidence | Why it supports the verdict |
|---|---|---|---|---|
| n/a | n/a | n/a | No MEDIUM or HIGH component-pair score was found on the production coupling targets. | n/a |

## Residual Ambiguity / Stop-Condition Notes

No stop condition fired. The Phase 6 contract and proposal were readable before scoring, and `~/ai/conventions/code-quality.md` still contains the bound A1 row `Coupling by distinct external symbols/modules referenced` at line 300.

The adapter declarations in `planning/tsb-gate/contracts/tsb.contract.md` lines 148-168 are well formed: each entry names a touched production component, sets `role: adapter`, and has a non-empty `Translates:` list. The intrinsic-surface declarations in lines 176-204 are well formed: each entry names a touched production component, sets `role: intrinsic-surface`, has exactly one `Domain:`, and has a non-empty `Owns:` list.

The caller requested incremental delta scoring for `9545ff8..c5f57f5`; I used the diff to resolve the current delta-touched production components, then applied the contract-declared adapter and intrinsic rules to the whole production component surfaces. Test files in `touched-files.txt` are recorded as context-only proof surfaces because the supplied operator contract says this auditor is not used for test review; their `test_harness_declarations:` entries are not adapter or intrinsic declarations and were not used to alter production coupling scores.

VERDICT: LOW
