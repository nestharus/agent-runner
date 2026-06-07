# Proof-risk audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| mode | `phase-3-proposal` | n/a | n/a | Well-formed mode. |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | n/a | n/a | Used to resolve repo-relative evidence refs. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wu-b/proposal.md` | 29094 | `ba94f2013986` | Read before scoring. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/abd-gate/contracts/abd.contract.md` | 68373 | `3c2d65f79112` | Read before scoring as required for Phase 6. |
| code-quality convention | `/home/nes/ai/conventions/code-quality.md` | 30798 | `fa8b6499cc2e` | Read before scoring; Phase 6 contract visibility requirement is at `/home/nes/ai/conventions/code-quality.md:169-174`. |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/abd-gate/gates/touched-surfaces.md` | 1262 | `0418aa14870c` | Read for new-code vs touched-existing-file triage context. |
| shipped spawn test | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/crates/oulipoly-runtime/tests/age_pid_sidecar_spawn.rs` | 5652 | `751ce664c773` | Read to classify runtime-artifact vs proxy evidence. |
| shipped mailbox test | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/src-tauri/tests/wu_b_mailbox_integration.rs` | 29099 | `cf93aa3b62e7` | Read to classify runtime-artifact vs proxy evidence. |
| shipped wake test | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/src-tauri/tests/wu_d_proactive_wake_integration.rs` | 23572 | `a8f80ad073c5` | Read to classify runtime-artifact vs proxy evidence. |

## Proof-plan parse
| Field | Present | Evidence |
|---|---:|---|
| Exact `## Proof plan` section | Yes | `planning/wu-b/proposal.md:362` contains the exact required heading. |
| `Runtime claim` | Yes | The table header includes `Runtime claim` at `planning/wu-b/proposal.md:364`, with four claim rows at `planning/wu-b/proposal.md:366-369`. |
| `Proof method` | Yes | The table header includes `Proof method` at `planning/wu-b/proposal.md:364`, with named test methods at `planning/wu-b/proposal.md:366-369`. |
| `Evidence-class match` | Yes | The table header includes `Evidence-class match` at `planning/wu-b/proposal.md:364`, with explicit evidence-class explanations at `planning/wu-b/proposal.md:366-369`. |
| Self-certification only | No | The proof methods name concrete runtime/integration tests rather than asserting that the proof plan or generic test pass status is the validation surface at `planning/wu-b/proposal.md:366-369`. |

## Findings
| Finding ID | Severity | Runtime claim | Proof method | Proxy class | Required runtime artifact | Evidence refs | Blocks pipeline |
|---|---|---|---|---|---|---|---|
| None | LOW | No non-LOW proof-risk finding. | n/a | n/a | n/a | Proof-plan structure is complete and the cited shipped tests exercise production-shaped runtime paths. | No |

## Evidence-class decision

The audited claims are runtime-artifact-bound. The proposal names provider-child spawn identity capture, death-safe owner resolution from persisted caller ancestry, mailbox enqueue/drain through runner CLI resume paths, and proactive detached wake behavior with idle-wake, turn-end recheck, single-flight, and cap semantics at `planning/wu-b/proposal.md:366-369`. The Phase 6 contract confirms matching runtime domains for child spawn identity capture at `planning/abd-gate/contracts/abd.contract.md:630-637`, mailbox sidecar semantics at `planning/abd-gate/contracts/abd.contract.md:621-629`, notify and mailbox delivery adapter contracts at `planning/abd-gate/contracts/abd.contract.md:517-529`, resume lifecycle hooks at `planning/abd-gate/contracts/abd.contract.md:557-564`, and auto-wake lifecycle at `planning/abd-gate/contracts/abd.contract.md:638-645`.

PID identity capture is proven by runtime-artifact evidence, not construction-only evidence. The proof plan names `crates/oulipoly-runtime/tests/age_pid_sidecar_spawn.rs::spawn_capture_writes_verified_sidecar_row_without_state_schema_change` at `planning/wu-b/proposal.md:366`. The shipped test drives `RuntimeExecutorService::default().execute(...)` through the production executor path at `crates/oulipoly-runtime/tests/age_pid_sidecar_spawn.rs:67-80`, then verifies the sidecar row and unchanged `state.db` schema at `crates/oulipoly-runtime/tests/age_pid_sidecar_spawn.rs:83-96`. The fixture script is setup; the asserted sidecar row is produced by the runtime spawn path.

Death-safe owner resolution is proven by runtime-artifact evidence. The proof plan names the caller-chain notify tests at `planning/wu-b/proposal.md:367`. The shipped harness invokes the compiled `oulipoly-agent-runner notify agent-bash-complete` command at `src-tauri/tests/wu_b_mailbox_integration.rs:93-109`, and the tests cover nearest-ancestor selection, state fallback, fake/dead PID resolution, and PID-reuse mismatch at `src-tauri/tests/wu_b_mailbox_integration.rs:341-409`. Those cases exercise persisted identity triples and isolated sidecar/state DBs rather than live `/proc` probing or trusted `--caller-ppid` identity.

Mailbox enqueue and drain-on-resume are proven by runtime-artifact evidence with setup-only row construction for drain cases. The proof plan names notify and resume tests at `planning/wu-b/proposal.md:368`. Notify idempotency runs through the compiled runner at `src-tauri/tests/wu_b_mailbox_integration.rs:445-463`. Resume delivery runs through compiled resume commands and fixture provider scripts at `src-tauri/tests/wu_b_mailbox_integration.rs:126-147`, with prompt prepend, success marking, failure non-marking, batch cap, and active-session selection asserted at `src-tauri/tests/wu_b_mailbox_integration.rs:530-737`. Direct mailbox seeding at `src-tauri/tests/wu_b_mailbox_integration.rs:219-244` is a fixture application point for pending rows; the drain, prompt transport, delivery marking, and failure retention are still asserted through the production resume path.

Proactive wake is proven by runtime-artifact evidence. The proof plan names the proactive wake tests and explicitly includes single-flight diagnostics at `planning/wu-b/proposal.md:369`. The shipped tests invoke the compiled runner for initial agent and resume paths at `src-tauri/tests/wu_d_proactive_wake_integration.rs:80-99`, while the fixture provider captures real process identity and calls production `notify agent-bash-complete` at `src-tauri/tests/wu_d_proactive_wake_integration.rs:621-636`. Idle wake is covered at `src-tauri/tests/wu_d_proactive_wake_integration.rs:255-292`; busy/turn-end recheck delivery is covered at `src-tauri/tests/wu_d_proactive_wake_integration.rs:295-326`; no-pending loop termination is covered at `src-tauri/tests/wu_d_proactive_wake_integration.rs:329-350`; the bounded auto-wake cap is covered at `src-tauri/tests/wu_d_proactive_wake_integration.rs:353-385`; manual race handling is covered at `src-tauri/tests/wu_d_proactive_wake_integration.rs:431-466`; and batch follow-up wake is covered at `src-tauri/tests/wu_d_proactive_wake_integration.rs:469-502`. Single-flight is not proxy-only: `concurrent_notify_single_flight` spawns two notify commands at `src-tauri/tests/wu_d_proactive_wake_integration.rs:402-409`, asserts one spawned wake and one claim token at `src-tauri/tests/wu_d_proactive_wake_integration.rs:410-412` and `src-tauri/tests/wu_d_proactive_wake_integration.rs:559-577`, then verifies exactly one provider-side wake launch artifact at `src-tauri/tests/wu_d_proactive_wake_integration.rs:423-425` and `src-tauri/tests/wu_d_proactive_wake_integration.rs:579-582`.

No proxy-only proof or evidence-class mismatch remains for the requested runtime claims. The tests use isolated fixture data and seeded rows where appropriate, but the asserted behaviors are exercised through production-shaped executor, CLI, SQLite sidecar, subprocess, resume, and wake paths rather than static parsing, mocks, relaxed schemas, or final-state-only proxies.

## Residual ambiguity / stop-condition notes

No `BLOCKED` condition fired: `mode`, `proposal_path`, `report_path`, `worktree_path`, and `contract_path` were supplied and readable, and this report path was writable.

No `NEEDS_INPUT` condition fired. The platform scope is Unix/Linux-shaped, matching the PID identity and wake evidence surfaces cited above; no supplied artifact claims a broader non-Unix proof obligation for these runtime behaviors.

VERDICT: LOW
