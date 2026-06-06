# Push/Pull Coupling Audit

## Inputs Read

| Input | Path / Value |
|---|---|
| mode | `phase-6` |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/gates/diff.patch` |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/gates/touched-files.txt` |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/proposal.md` |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/contracts/plk.contract.md` |
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/evidence/runtime-tests.log` |
| output_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/code-quality/plk/reports/push-pull-auditor.md` |

## References Read

| Reference | Evidence Used |
|---|---|
| `/home/nes/ai/conventions/code-quality.md` | Auditor scope boundary, touched-file ownership, A1 push-vs-pull system coupling, `uncontrolled-source coupler` failure mode, disposition policy, Phase 6 contract visibility, and numerical threshold context are present. |
| `/home/nes/ai/agents/push-pull-auditor.md` | A4 operational binding: source-control proof, common-interface proof, HIGH private-source recipe, report schema, and final verdict rules. |
| `/home/nes/ai/conventions/agent-questions-and-session-graph.md` | `## Pull-vs-Push Policy` exists and is only the session-graph terminology disambiguator, not this system-coupling rule. |
| `planning/plk-gate/proposal.md` | PLK intent for same-StateDb UUID parent lookup, conservative PID sidecar stale reconciliation, and runtime proof plan. |
| `planning/plk-gate/contracts/plk.contract.md` | Component roles, adapter declarations, intrinsic-surface declarations, and test-harness declarations for the PLK touched surfaces. |
| `planning/plk-gate/evidence/runtime-tests.log` | Runtime evidence for nested `agent-bash`, trace reconciliation, and parent-resolution unit tests. |
| `README.md` | Stable invocation marker, trace, and cross-invocation parent-env contracts. |
| `AGENTS.md` | StateDb schema ownership and migration/source-of-truth guidance. |
| `crates/oulipoly-state/src/invocation_marker.rs` | Canonical `CompositeInvocationId` marker and `OULIPOLY_PARENT_INVOCATION` grammar owner. |
| `crates/oulipoly-state/src/pid_identity.rs` | PID identity sidecar path/schema/API owner and live process identity interface. |
| `/home/nes/projects/agent-bash-tool/trunk/docs/DESIGN.md` | `agent-bash run`/`status` output and handle interface. |
| `/home/nes/projects/agent-bash-tool/trunk/planning/wu-c/proposal.md` | `agent-bash run` JSON, `status --full` text, and state-root/per-handle artifact contract. |

## Pull Sites Inspected

| ID | Puller | Source | Pull mechanism | Ownership/interface evidence | Verdict | Evidence |
|---|---|---|---|---|---|---|
| PP-001 | `src-tauri/src/commands/trace/accessor.rs::load_trace_environment` | Default `StateDb` and `sessions.toml` | `StateDb::open_default`, `default_config_root().join("sessions.toml")`, `SessionsConfig::load` | LOW source-control proof: trace command, config path helper, StateDb, and config loader are workspace-owned; contract declares the trace pre-render reconciliation surface. | LOW | `accessor.rs:11-23`; `plk.contract.md:13,174-184` |
| PP-002 | `src-tauri/src/commands/trace/accessor.rs::load_trace_environment` | Stale-running reconciliation before trace rendering | Calls `invocation::stale_reconcile::reconcile_stale_running_invocations(&state)` | LOW common-interface proof: PLK contract declares this trace-time handoff and the stale reconciliation module as the owner of sidecar-backed reconciliation. | LOW | `accessor.rs:11-16`; `diff.patch:11-18`; `plk.contract.md:46-72,174-184` |
| PP-003 | `src-tauri/src/dispatch/parent_invocation.rs::resolve_parent_invocation_id` | `OULIPOLY_PARENT_INVOCATION` env value and `CompositeInvocationId` grammar | Reads env, delegates parsing, looks up UUID in supplied `StateDb` | LOW common-interface proof: `CompositeInvocationId` is the canonical grammar owner; README and PLK contract declare parent env semantics and same-StateDb UUID lookup. | LOW | `parent_invocation.rs:5-19`; `invocation_marker.rs:5-20,46-65`; `README.md:716-720`; `plk.contract.md:32-38,157-164` |
| PP-004 | `src-tauri/src/dispatch.rs::tests` parent-env helpers | Process env value `OULIPOLY_PARENT_INVOCATION` | Locked test helpers read/set/remove env around resolver tests | LOW source-control proof: the unit harness controls the process env source in-process; contract declares serialized `CompositeInvocationId` env values and locked process-env mutation. | LOW | `dispatch.rs:520-581,971-1052`; `plk.contract.md:243-249` |
| PP-005 | `src-tauri/src/dispatch/predicate.rs` | App config diagnostics model setting | `agent_runner_lib::load_app_config().diagnostics_model` | LOW source-control proof: this is an existing same-workspace app-config accessor and remains subordinate to the dispatch predicate intrinsic surface. | LOW | `predicate.rs:6-24`; `plk.contract.md:206-213` |
| PP-006 | `src-tauri/src/invocation/mod.rs` | Invocation module namespace | Adds `stale_reconcile` child module export | LOW source-control proof: module namespace is owned by the same crate; contract declares the child module export. | LOW | `mod.rs:1-3`; `plk.contract.md:214-220` |
| PP-007 | `src-tauri/src/invocation/stale_reconcile.rs::open_pid_sidecar_read_only_optional` | PID identity sidecar path and file existence | `PidIdentityDb::default_path`, `Path::exists`, `PidIdentityDb::open_read_only` | LOW common-interface proof: `oulipoly_state::pid_identity` owns the sidecar path/schema/API; contract declares read-only sidecar open through `PidIdentityDb::default_path`. | LOW | `stale_reconcile.rs:50-60`; `pid_identity.rs:5-7,66-117,195-197`; `plk.contract.md:165-173` |
| PP-008 | `src-tauri/src/invocation/stale_reconcile.rs::running_invocation_row_values` | `state.db` `invocations` running-row shape | Raw SQL selects `id`, `invocation_uuid`, `created_at` for unfinished running rows | LOW source-control proof: this is a private storage-shape pull, but the consumer and StateDb schema owner are inside the same controlled workspace boundary; contract declares translation over `StateDb running invocation rows`. | LOW | `stale_reconcile.rs:62-98`; `AGENTS.md` State DB Schema Migrations section; `plk.contract.md:120-126,165-173` |
| PP-009 | `src-tauri/src/invocation/stale_reconcile.rs::invocation_has_dead_pid_evidence` | PID sidecar rows for an invocation UUID | `PidIdentityDb::lookup_by_invocation_uuid`, typed `PidIdentityRow`, `PidIdentityRow::identity` | LOW common-interface proof: PID sidecar rows are exposed through the `PidIdentityDb` typed API and declared as a PLK adapter-translated surface. | LOW | `stale_reconcile.rs:176-225`; `pid_identity.rs:23-43,174-192`; `plk.contract.md:120-126,165-173` |
| PP-010 | `src-tauri/src/invocation/stale_reconcile.rs::live_process_identity_state` | OS live process identity | `read_live_process_identity(os_pid)` returns live/dead/unknown | LOW common-interface proof: stale reconciliation pulls from the repo-owned `pid_identity` API, not directly from `/proc` private layout; contract declares conservative live/dead/unknown process identity handling. | LOW | `stale_reconcile.rs:206-225`; `pid_identity.rs:215-217,341-419`; `plk.contract.md:165-173` |
| PP-011 | `src-tauri/src/invocation/stale_reconcile.rs::finalize_stale_invocation` | StateDb terminal invocation finalization fields and already-finalized race signal | `StateDb::finalize_invocation`, stale constants, benign error-text predicate | LOW source-control proof: finalization and its error string are same-workspace owned; contract declares the stale terminal fields and already-finalized tolerance as stale reconciliation responsibilities. | LOW | `stale_reconcile.rs:228-244`; `plk.contract.md:65-72,165-173` |
| PP-012 | `src-tauri/tests/pr_a_invocation_integration.rs::Fixture` and helpers | Isolated filesystem/config/state layout | Writes provider script, model TOML, `providers.toml`, DB path, XDG env | LOW source-control proof: test harness creates and controls the fixture files; contract declares isolated XDG roots, fixture provider/model config, and StateDb assertion surfaces. | LOW | `pr_a_invocation_integration.rs:25-113`; `plk.contract.md:135-142,223-234` |
| PP-013 | `src-tauri/tests/pr_a_invocation_integration.rs::run_agent_bash_nested_child` and `wait_for_agent_bash_done` | External `agent-bash run` JSON and `agent-bash status --full` text | Executes `agent-bash`, parses JSON `handle`, polls status until `DONE`, asserts `DONE rc=0` | LOW common-interface proof: `agent-bash` owning docs/proposal declare `run` JSON fields and non-blocking `status` output beginning `RUNNING` or `DONE rc=<n>`; PLK contract declares the real `agent-bash` binary supplied by `AGENT_BASH_BIN`. | LOW | `pr_a_invocation_integration.rs:115-196,343-372`; `DESIGN.md:90-94`; `wu-c/proposal.md:40-53,66-105,142-164`; `plk.contract.md:88-98,135-142,223-234` |
| PP-014 | `src-tauri/tests/pr_a_invocation_integration.rs::parse_invocation` and `parse_valid_invocations` | Runner stderr `OULIPOLY_INVOCATION=<json>` marker | Scans lines by prefix and parses `CompositeInvocationId` | LOW common-interface proof: README declares the stable invocation marker and the state crate owns the marker grammar. | LOW | `pr_a_invocation_integration.rs:198-218`; `README.md:479-525`; `invocation_marker.rs:5-20,28-65` |
| PP-015 | `src-tauri/tests/pr_a_invocation_integration.rs` StateDb assertions and fixture SQL | `state.db` invocation rows | Opens `StateDb`, calls `get_invocation_by_uuid`, inserts fixture rows by raw SQL, counts rows | LOW source-control proof: the harness controls the fixture DB and the StateDb schema is owned inside the same workspace; contract declares this StateDb assertion surface. | LOW | `pr_a_invocation_integration.rs:231-260,262-700`; `plk.contract.md:135-142,223-234` |
| PP-016 | `src-tauri/tests/pr_b_trace_integration.rs::Fixture` and sidecar seed helpers | Isolated trace fixture files, `state.db`, `pid-identity.db`, and PID identity row shape | Writes config/model files, maps sidecar path, opens `StateDb`/`PidIdentityDb`, records dead PID identity | LOW source-control proof plus common-interface proof: test controls fixture files and `PidIdentityDb` owns sidecar schema/API; contract declares the trace harness and PID sidecar fixture surface. | LOW | `pr_b_trace_integration.rs:23-210`; `pid_identity.rs:5-7,66-117`; `plk.contract.md:143-150,235-242` |
| PP-017 | `src-tauri/tests/pr_b_trace_integration.rs` trace validators | Trace CLI JSON/human output and durable StateDb terminal state | Executes compiled runner `trace`, parses JSON fields, checks status strings and DB row fields | LOW source-control proof: trace producer and test consumer are in the same repo-controlled boundary; proposal and contract declare trace JSON and durable StateDb terminal-state assertions. | LOW | `pr_b_trace_integration.rs:212-438`; `proposal.md:31-47`; `plk.contract.md:143-150,235-242` |
| PP-018 | `src-tauri/src/dispatch.rs` production dispatcher | CLI enum fields, runtime services, command-handler boundaries | Routes parsed `Cli`/`Subcommands` to named handlers and runtime services | LOW source-control proof: CLI structs, command handlers, runtime service ports, and dispatch helper modules are repo/workspace-owned; file-local and Phase 6 declarations identify dispatch-owned CLI lifecycle and translated service/DB/module surfaces. | LOW | `dispatch.rs:1-23,83-449`; `plk.contract.md:14,185-205` |

## Uncontrolled-Source Coupler Findings

| ID | Puller | Source | Implicit contract evidence | Missing proof | Decoupling direction | Failure mode |
|---|---|---|---|---|---|---|
| None | n/a | n/a | n/a | n/a | n/a | n/a |

## Residual Ambiguity / Stop-Condition Notes

No `BLOCKED` or `NEEDS_INPUT` condition was reached. The Phase 6 contract and proposal were readable before scoring, the A1 metric source contains the required push-vs-pull rule text and failure mode, and the touched surfaces were inspectable from the supplied diff and touched-file list.

The raw SQL read of `invocations` in `stale_reconcile.rs` is the strongest coupling point, but it is LOW under A1 source-control proof because the StateDb schema owner and consumer live inside this controlled workspace and PLK explicitly declares `StateDb running invocation rows` as an adapter-translated surface. If that schema were externalized to an independently owned service or artifact, this would need a producer-owned typed accessor/common interface.

The test harness polls `agent-bash status --full`, but A4 scores the pull from the status text as LOW because `agent-bash` has an owning common-interface contract for `RUNNING` and `DONE rc=<n>` output. Any separate concern about polling strategy is outside this push-vs-pull coupling verdict.

No deployment-level service, cache, private endpoint, or service-topology pull site beyond local repo-owned SQLite/state filesystem surfaces was found in the PLK touched files.

VERDICT: LOW
