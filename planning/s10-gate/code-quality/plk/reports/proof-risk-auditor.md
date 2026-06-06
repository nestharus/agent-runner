# Proof-risk audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| mode | phase-6 | n/a | n/a | Phase 6 per-component code-quality invocation; contract was read before scoring. |
| planning_dir | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate` | n/a | n/a | Used for artifact location context. |
| wu_id | s10 | n/a | n/a | Work unit identifier. |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | n/a | n/a | Used to resolve relative proof-plan evidence paths. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/proposal.md` | 5549 | `2c64dfa0661e` | Exact `## Proof plan` parsed. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/contracts/plk.contract.md` | 23830 | `efe5a4f3866a` | Read for Phase 6 declarations, carried PLK claims, and test-harness scope. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/gates/diff.patch` | 32072 | `a3a7b8ae83c6` | Read to confirm touched runtime/test surfaces and S10 external-launch/config-carrier changes. |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/gates/touched-files.txt` | 504 | `8c6b25292b55` | Read to confirm Phase 6 touched surface set. |
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/evidence/runtime-tests.log` | 75837 | `0db69381a388` | Read for named proof-method pass evidence. |
| code-quality convention | `/home/nes/ai/conventions/code-quality.md` | n/a | n/a | Read before scoring, as requested. |
| proof-risk operator | `/home/nes/ai/agents/proof-risk-auditor.md` | n/a | n/a | Read to apply report shape and proof-risk decision rules. |

## Proof-plan parse
| Field | Present | Evidence |
|---|---:|---|
| `## Proof plan` | yes | `proposal.md:7` starts the exact section. |
| Evidence log path | yes | `proposal.md:9` names `planning/s10-gate/evidence/runtime-tests.log`. |
| Runtime claim fields | yes | Five `Runtime claim:` entries at `proposal.md:11`, `proposal.md:17`, `proposal.md:23`, `proposal.md:29`, and `proposal.md:35`. |
| Proof method fields | yes | Five `Proof method:` entries at `proposal.md:13`, `proposal.md:19`, `proposal.md:25`, `proposal.md:31`, and `proposal.md:37`. |
| Evidence-class match fields | yes | Five `Evidence-class match:` entries at `proposal.md:15`, `proposal.md:21`, `proposal.md:27`, `proposal.md:33`, and `proposal.md:39`. |

## Claim Decisions
| Runtime claim | Required evidence class | Proof method | Evidence refs | Decision |
|---|---|---|---|---|
| Nested `agent-bash` children inherit `OULIPOLY_PARENT_INVOCATION` and record `parent_invocation_id` as the parent's StateDb row id. | Runtime-artifact integration over real `agent-bash`, runner process dispatch, inherited environment, and StateDb row assertion. | `src-tauri/tests/pr_a_invocation_integration.rs::nested_agent_bash_chain_records_parent_id_from_inherited_env`. | Proposal `proposal.md:11-15`; contract carried claim `plk.contract.md:315-317`; evidence `runtime-tests.log:833-847`; source assertion `src-tauri/tests/pr_a_invocation_integration.rs:344-372`. | Match. The method exercises the real nested `agent-bash` path and durable DB parent linkage, not a parser-only proxy. |
| Trace reconciles stale `running` rows only when PID sidecar evidence conclusively proves the recorded process identity is dead, and persists failed terminal fields. | Runtime-artifact integration over trace CLI, stale StateDb rows, PID sidecar evidence, positive mutation, and no-sidecar non-mutation control. | `trace_reconciles_liveness_stale_running_row_with_dead_pid` plus `trace_json_stale_running_row_is_lifted_without_mutating_db`. | Proposal `proposal.md:17-21`; contract carried claim `plk.contract.md:318-320`; evidence positive `runtime-tests.log:849-863` and post-proof `runtime-tests.log:1684-1695`; evidence control `runtime-tests.log:1698-1709`; source assertions `src-tauri/tests/pr_b_trace_integration.rs:275-342`. | Match. The paired tests cover conclusive dead-PID mutation and non-conclusive/no-sidecar non-mutation. |
| Same-DB UUID parent resolution tolerates provider/source-name drift while preserving malformed or unknown parent values as root-invocation cases. | Unit-level StateDb resolver evidence is appropriate because the claim targets the resolver's same-DB UUID lookup semantics and malformed/unknown root handling, not a deployed process artifact. | `resolve_parent_invocation_id_uses_same_db_uuid_despite_source_name_drift` plus existing malformed/unknown resolver tests in the same module. | Proposal `proposal.md:23-27`; contract carried claim `plk.contract.md:312-314`; evidence source-drift `runtime-tests.log:865-879`; source assertions `src-tauri/src/dispatch.rs:1003-1025` and malformed/unknown tests beginning `src-tauri/src/dispatch.rs:1028`. | Match. The proof method binds the claim to real `StateDb` resolver behavior and explicitly covers source-name drift. Malformed/unknown controls are named and present in the same resolver module. |
| External provider launch exit session metadata populates runtime session capture and is carried into the next external provider resume request. | Runtime executor integration with an executable external-provider fixture, provider registry dispatch, launch stream exit-session metadata, `SessionCaptureResult`, and resume request JSON capture. | `crates/oulipoly-runtime/tests/s10_external_launch_session.rs::external_launch_exit_session_populates_capture_and_resume_request`. | Proposal `proposal.md:29-33`; contract test harness `plk.contract.md:327-334`; evidence initial `runtime-tests.log:7-22` and post-FC `runtime-tests.log:883-894`; source assertions `crates/oulipoly-runtime/tests/s10_external_launch_session.rs:69-109`; diff implementation `diff.patch:1-85` and test fixture `diff.patch:98-411`. | Match. The method runs the production `RuntimeExecutorService` external-provider path against an executable provider fixture and verifies both capture and resume handoff. |
| S10 moved-provider setup/config carriers backfill the external-provider binary ref without regressing runtime config separation or source-guard thresholds. | Mixed unit plus source-guard integration evidence is appropriate: setup prompt/config migration claims are covered by production-shaped temp filesystem and prompt unit tests; source-guard claims are covered by integration source-guard suites. | `crates/oulipoly-setup/src/context.rs::tests::*`; exact config migration tests; `age244_s7b_export_replace_dispatch`; `age245_s7c_rotation_source_guard`; `age246_s8_setup_dispatch_source_guard`. | Proposal `proposal.md:35-39`; setup evidence `runtime-tests.log:60-82` and post-FC `runtime-tests.log:931-948`; exact config evidence `runtime-tests.log:1712-1735`; export/replace source guard `runtime-tests.log:24-58` and post-FC `runtime-tests.log:897-928`; rotation source guard `runtime-tests.log:786-803` and post-FC `runtime-tests.log:1649-1663`; setup dispatch source guard `runtime-tests.log:805-823` and post-FC `runtime-tests.log:1666-1681`; diff implementation `diff.patch:412-865`. | Match. Earlier broad config-migration commands filtered out the exact tests, but the later exact-filter section records the three named config-migration proofs as `ok`, so the final evidence artifact supports the claim. |

## Findings
| Finding ID | Severity | Runtime claim | Proof method | Proxy class | Required runtime artifact | Evidence refs | Blocks pipeline |
|---|---|---|---|---|---|---|---|
| None | LOW | n/a | n/a | n/a | n/a | All five claims have named methods, explicit evidence-class matches, and supporting evidence refs. | no |

## Evidence-class decision
The proof plan is structurally complete: every runtime claim has an adjacent proof method and an adjacent evidence-class match statement. The highlighted PLK carry-forward claims use evidence classes that match their surfaces: the nested `agent-bash` claim uses a real binary and durable StateDb assertion; stale-running reconciliation uses trace CLI integration with positive and no-sidecar controls; same-DB UUID source drift uses focused resolver/StateDb unit coverage appropriate to an internal lookup-semantic claim. The S10 external-provider launch session claim uses an executable provider fixture through `RuntimeExecutorService`, which is runtime-path integration for the external-provider adapter boundary rather than a static schema proxy.

The moved-provider setup/config claim uses mixed evidence, but the proxy/runtime split is explicit and appropriate: setup prompt formatting is unit-level text generation, config migration uses production migration APIs against real temp TOML files, and source-guard thresholds use integration-style source scans. The evidence log contains an earlier broad config-migration filter that ran zero matching tests, but the later exact-filter block at `runtime-tests.log:1712-1735` supersedes that ambiguity by recording all three named config-migration tests as passing.

## Residual ambiguity / stop-condition notes
No stop condition fired. The only residual limitation is that the external-provider launch proof uses an executable local provider fixture rather than a third-party provider binary; this is acceptable for the stated claim because the contract scopes the target to the runner-owned external-provider launch adapter boundary and request/response contract, not a deployed provider implementation.

VERDICT: LOW
