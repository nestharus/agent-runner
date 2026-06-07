# Proof-risk audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| mode | phase-3-proposal | N/A | N/A | Valid mode. |
| worktree_path | /home/nes/projects/agent-runner/worktrees/age-pid-sidecar | N/A | N/A | Supplied repository worktree; directory readable. Evidence paths resolved against this root. |
| operator | /home/nes/ai/agents/proof-risk-auditor.md | — | — | Read before scoring; content confirmed. |
| proposal_path | /home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/proposal.md | 9,980 | 1468609a | Readable; exact `## Proof plan` begins at line 9. Ten runtime-claim/proof-method/evidence-class-match triples present. |
| contract_path | /home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/contracts/s11.contract.md | 31,782 | 0da6c9e4 | Readable; used for declared roles, adapter declarations, test-harness declarations, intrinsic surfaces, and no-`state.db`-migration scope before scoring. |
| runtime evidence summary | /home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/evidence/runtime-tests.log | — | — | Caller-supplied absolute runtime command evidence summary; read and cross-checked against proof-plan method names. |
| live evidence summary | /home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/evidence/live-smoke.md | — | — | Caller-supplied absolute live evidence summary; read and cross-checked against live-artifact evidence-class match statements. |

## Proof-plan parse
| Field | Present | Evidence |
|---|---:|---|
| `## Proof plan` | Yes | `proposal.md:9` exact heading. |
| `Runtime claim` | Yes | Ten runtime-claim entries at `proposal.md:13`, `:19`, `:25`, `:31`, `:37`, `:43`, `:49`, `:55`, `:61`, `:67`. |
| `Proof method` | Yes | Ten proof-method entries at `proposal.md:15`, `:21`, `:27`, `:33`, `:39`, `:45`, `:51`, `:57`, `:63`, `:69`. |
| `Evidence-class match` | Yes | Ten evidence-class-match entries at `proposal.md:17`, `:23`, `:29`, `:35`, `:41`, `:47`, `:53`, `:59`, `:65`, `:71`. |
| Self-certification only | No | Every claim names concrete CLI integration tests, protocol-capture fixtures, live runtime artifacts, sidecar DB rows, or source schema file inventory rather than treating the plan text itself as validation. |

## Findings
| Finding ID | Severity | Runtime claim | Proof plan ref | Proof method | Proxy class | Required runtime artifact | Evidence refs | Blocks pipeline |
|---|---|---|---|---|---|---|---|---|
| None | None | No proof-risk finding fires. | N/A | N/A | N/A | N/A | N/A | No |

## Evidence-class decision

All ten runtime claims present with proof method and evidence-class match. No missing-field, self-certification, proxy-only, or mismatch condition fires.

**Claim 1 (sidecar identity capture, `proposal.md:13-17`).** Runtime-artifact-bound: captures provider child PID and session id at launch time for later notification dispatch. Proof method binds to `s11_external_provider_wake.rs::external_provider_launch_notify_uses_captured_sidecar_owner_and_wakes` — a full CLI/external-provider integration test that launches through the provider fixture, captures sidecar identity, enqueues a notification, and asserts wake spawn. Paired live `/tmp/s11-e2e/initial10.log` and `fresh10-notify2.stdout` provide secondary runtime smoke. The contract declares `src-tauri/tests/s11_external_provider_wake.rs` as the CLI launch/notify/wake/resume integration surface and `spawn_identity.rs` as the intrinsic process-identity surface, confirming the evidence class matches the claim. **LOW.**

**Claim 2 (delivery not marked until confirmed, `proposal.md:19-23`).** Runtime-artifact-bound: sidecar delivery state must distinguish resume ran from delivery confirmed. Proof method binds to five named tests in `s11_external_provider_wake.rs` and `wu_b_mailbox_integration.rs` covering the negative no-turn path, positive submitted-turn marker, wrong-payload rejection, and exact ingested user-turn confirmation. The contract declares both files as CLI launch/notify/wake/resume and mailbox integration test-harness surfaces. The evidence-class match explicitly says the tests exercise the exact resume/mailbox delivery path and state assertions, not a proxy. **LOW.**

**Claim 3 (failed wake retry, `proposal.md:25-29`).** Runtime-artifact-bound: failed or rate-limited wake must increment `delivery_attempts`, leave `delivered_at` absent, record `delivery_error`, release the claim, and remain pending for retry. Proof method uses two wake-level integration tests plus the sidecar unit `mailbox.rs::tests::mark_delivery_failed_records_attempt_without_delivery`. Mixed evidence is explicitly scoped: integration tests cover the full wake/release/retry path; the unit test covers sidecar state mutation. The evidence-class match names the scoped assertion targets explicitly. **LOW.**

**Claim 4 (transport rotation, `proposal.md:31-35`).** Runtime-artifact-bound: rotatable transport/unavailable failures retry the pool; schema/protocol/policy failures remain terminal. Proof method uses `age246_external_transport_rotation.rs` with four named cases that create isolated provider-pool fixtures, force timeout/heartbeat-gap/unavailable classes, assert successful rotation to the next account, and assert bounded terminal failure when all accounts are slow. The contract declares this file as the runtime pool-rotation assertion and transport-timeout fixture surface. Runtime-tests.log records `age246_external_transport_rotation 4 passed`. The TDD commit reference `a1a3ca1` is secondary; shipped test evidence is primary. **LOW.**

**Claim 5 (policy/launch request shaping, `proposal.md:37-41`).** Runtime-artifact-bound: policy and launch requests must carry selected provider settings identity, hybrid shape, host linkage env, account-specific auth, and must exclude ambient leakage. Proof method uses six named cases in `age217_s6a_policy_launch_dispatch.rs` that capture actual protocol requests and environment maps emitted by the executor. The contract declares this file as the external-provider policy/launch fake-provider fixture and runtime dispatch request environment assertion surface. Fixture-captured protocol evidence is the matching class for a request-shaping/environment-boundary claim. **LOW.**

**Claim 6 (wake registry/session reload, `proposal.md:43-47`).** Runtime-artifact-bound: detached wake resume must reload provider registry from launch-time model/config roots and must use ingested external sessions when launch capture is absent. Proof method uses two named tests with isolated model/config/data roots and an ingested-session fallback case. The evidence-class match explains that the tests prove the wake path supplies the correct models dir and session source under both the capture-present and capture-absent branches. **LOW.**

**Claim 7 (S10 compatibility, `proposal.md:49-53`).** Runtime-artifact-bound: S11 must preserve S10 external provider launch/resume semantics. Proof method is mixed with explicit scoping. Shipped tests `external_provider_resume_without_rotate_uses_external_launch_and_recorded_cwd` and `external_launch_session_id_alias_persists_external_capture_method_without_session_capability` are declared the binding resume proof; live `/tmp/s10-e2e/final*.log` smoke is cited only for external launch success. The evidence-class match explicitly states that unavailable `S10-FINAL-OK` / `S10-RESUME-OK` live logs are not substituted for the shipped resume tests. The live-smoke.md corroborates that only `S10-EXTERNAL-OK` launch smoke is currently present in the retained `/tmp/s10-e2e` files, and that S10 resume remains bound to shipped tests. This avoids the proxy-substitution risk. Runtime-tests.log records `s10_external_provider_resume 2 passed`. **LOW.**

**Claim 8 (xhigh routing, `proposal.md:55-59`).** Runtime-artifact-bound: the external xhigh route must still dispatch through the external provider path after S11. Proof method is the retained live task output at `/tmp/claude-1000/-home-nes-projects-agent-runner/45ccb26a-8bb6-4486-9b1e-2226e29292a0/tasks/bvnh4nja0.output`. The evidence-class match identifies this as live runtime smoke and names the recorded markers `gpt-xhigh flipped`, `XHIGH-EXTERNAL-OK`, and `"status":"succeeded"`. The live-smoke.md independently corroborates the artifact at that path with those contents. For a routing-regression claim, live task execution output qualifies as runtime-artifact evidence, not proxy-only evidence. No shipped test for xhigh routing is present; the proposal does not claim otherwise. The live smoke is the only offered evidence and it matches the runtime-artifact evidence class of the claim. **LOW.**

**Claim 9 (live S11 delivery confirmed, `proposal.md:61-65`).** Runtime-artifact-bound: live S11 wake delivery must have reached the resumed provider and the sidecar must have recorded confirmed delivery in one attempt. Proof method names five live sidecar/runtime artifacts: argv log, `pid-identity.db` mailbox rows, invocation result, exported session JSON, and workload marker files. The evidence-class match describes the specific rows and values: `delivered_at` set, `delivered_by_invocation_uuid='7a46a1a5-844d-45e8-bd67-aecaf9cf9194'`, `delivery_attempts=1`, `delivery_error=NULL`, `"success":true`, `WOKE 0`, and `S11-WAKE-OK`. The live-smoke.md independently corroborates each artifact with matching content. These are direct sidecar/runtime artifacts for the behavior asserted. **LOW.**

**Claim 10 (no state.db schema migration, `proposal.md:67-71`).** Source/schema invariant claim, not a runtime execution claim. The claim is that S11 does not add a durable `state.db` schema migration. The proof method names the touched-file inventory, `schema.rs`, `migrations.rs`, and existing state/mailbox unit tests showing the behavior uses existing session-turn body reads and existing sidecar mailbox columns. The evidence-class match correctly identifies this as a source invariant: the S11 touched list does not include schema or migration files; the schema version is unchanged; new behavior uses existing columns. The contract also states that no `state.db` schema migration is declared or required. Static touched-file evidence is the matching class for a negative absence claim about schema files. **LOW.**

## Residual ambiguity / stop-condition notes

No stop condition fired. No `NEEDS_INPUT` trigger: no claim/evidence identity conflict exists that a human decision is required to resolve.

Claim 8 (xhigh routing) has no shipped deterministic test; it is covered only by a live `/tmp` task output. The proposal is transparent about this. Because the claim is a regression-check that the route still fires (not a new behavioral assertion), and because live task output is runtime-artifact-class evidence that directly matches the claim class, this does not rise to a HIGH condition under the operator rules. The live-smoke.md independently corroborates the artifact.

The worktree used in this audit cycle (`/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`) differs from the prior report's worktree (`/tmp/s11-gate-worktree-a1a3ca1`). Both the proposal (9,980 bytes, SHA `1468609a`) and contract (31,782 bytes, SHA `0da6c9e4`) are larger than in the prior cycle, reflecting the current working-tree S11 function-classification remediation. This audit is performed against the current state and supersedes the prior report.

LOW
