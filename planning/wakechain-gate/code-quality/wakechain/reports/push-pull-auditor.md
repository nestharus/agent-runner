# Push/Pull Coupling Audit

## Inputs Read

| Input | Value |
|---|---|
| repo_root | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness` |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness` |
| base_ref | `fcc0faf` |
| head_ref | `HEAD plus prior #43 split-only carry-over if present in the working tree` |
| planning_dir | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate` |
| wu_id | `wakechain` |
| diff_path | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/gates/diff.patch` |
| changed_files_path | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/gates/touched-files.txt` |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/proposal.md` |
| contract_path | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/contracts/wakechain.contract.md` |
| output_path | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/code-quality/wakechain/reports/push-pull-auditor.md` |

## References Read

| Reference | Evidence |
|---|---|
| `/home/nes/ai/agents/push-pull-auditor.md` | Operator scope, metric binding, touched-file ownership, output contract, and stop conditions read at lines 7-178. |
| `/home/nes/ai/conventions/code-quality.md` | A1 source verified: Auditor Scope Boundary lines 21-28, Push-vs-pull system coupling lines 106-131, Touched-file ownership lines 143-149, Numerical thresholds lines 291-302, Failure modes lines 304-310. |
| `/home/nes/ai/conventions/agent-questions-and-session-graph.md` | Terminology disambiguator verified: Pull-vs-Push Policy lines 230-242. |
| `planning/wakechain-gate/proposal.md` | Wake-chain scope, pull-site intent, and proof plan read at lines 3-49. |
| `planning/wakechain-gate/contracts/wakechain.contract.md` | Declared roles, focused production inventory, adapter declarations, intrinsic-surface declarations, and residual read at lines 7-263. |

A1 preservation check passed: the push-vs-pull system-coupling rule, session-graph terminology disambiguator, `uncontrolled-source coupler` failure mode, and numerical threshold context are present and non-contradictory in the references above.

## Pull Sites Inspected

| ID | Puller | Source | Pull mechanism | Ownership/interface evidence | Verdict | Evidence |
|---|---|---|---|---|---|---|
| PP-001 | `crates/oulipoly-runtime/src/sessions/mod.rs` session ingest | `sessions.toml` session-source config, adapter script stdout JSONL, adapter `STATE_DIR`/`SESSION_ID` environment | `provider_session_source`, `run_session_script_with_timeout`, JSONL parse into `ScriptTurn`, persist into `StateDb` | LOW common-interface proof: file declares the script contract inline at `crates/oulipoly-runtime/src/sessions/mod.rs:19-34`; contract declares this adapter translates sessions config, targeted `SESSION_ID`, provider stdout, and StateDb persistence at `wakechain.contract.md:73-82`. | LOW | `crates/oulipoly-runtime/src/sessions/mod.rs:104-177`, `crates/oulipoly-runtime/src/sessions/mod.rs:204-472`, `diff.patch:9-64` |
| PP-002 | `scripts/opencode-turns` OpenCode adapter | OpenCode CLI `session list --json` and `export <sessionID>` output, `SESSION_ID`, `OPENCODE_BIN`, timeout environment | Spawns OpenCode CLI, parses declared/listed export shapes including `info` metadata, emits normalized JSONL | LOW common-interface proof: adapter doc states it uses the public OpenCode CLI and not private storage layout at `scripts/opencode-turns:8-18`; contract declares OpenCode CLI, info-nested export, host `SESSION_ID`, normalized JSONL, and degraded marker contracts at `wakechain.contract.md:99-106`. | LOW | `scripts/opencode-turns:446-463`, `scripts/opencode-turns:511-560`, `scripts/opencode-turns:592-760`, `proposal.md:34-36` |
| PP-003 | `crates/oulipoly-state/src/db.rs` StateDb API | `session_turns` table body text for targeted provider/session user turns | SQL `SELECT EXISTS` with provider/session/role/body filters and non-empty substring needle | LOW source-control proof: `StateDb` is the persistence boundary owner for state schema/query APIs and JSON body predicates under `wakechain.contract.md:83-90` and `wakechain.contract.md:183-191`. | LOW | `diff.patch:427-453`, `proposal.md:37`, `wakechain.contract.md:31-32` |
| PP-004 | `crates/oulipoly-state/src/mailbox.rs` MailboxDb sidecar API | PID identity sidecar tables, `mailbox`, `session_runtime`, and `session_wake_claim` tables | Public sidecar methods `mark_pending_abandoned`, `wake_sweep_candidates`, `record_wake_claim_pid_identity`, claim validation/reclaimability predicates | LOW source-control proof: `mailbox.rs` owns sidecar mailbox and wake-claim storage, candidate selection, PID freshness, and abandoned-row marking at `wakechain.contract.md:91-98` and `wakechain.contract.md:192-201`. | LOW | `crates/oulipoly-state/src/mailbox.rs:333-372`, `crates/oulipoly-state/src/mailbox.rs:538-602`, `crates/oulipoly-state/src/mailbox.rs:653-706`, `diff.patch:573-699` |
| PP-005 | `src-tauri/src/wake_coordinator.rs` wake reclaim sweep | Mailbox sidecar candidate DTOs and sidecar mutation API | Opens optional sidecar, pulls `WakeSweepCandidate` list, partitions candidates, pushes abandoned-row mutation through `MailboxDb::mark_pending_abandoned`, starts wake chains for selected recoverable sessions | LOW common-interface/source-control proof: `WakeSweepCandidate` and abandoned-row API are declared sidecar contracts at `wakechain.contract.md:33-44`; coordinator owns sweep planning and abandoned-debris reaping at `wakechain.contract.md:49-59` and `wakechain.contract.md:226-237`. | LOW | `src-tauri/src/wake_coordinator.rs:141-160`, `src-tauri/src/wake_coordinator.rs:169-287`, `crates/oulipoly-state/src/mailbox.rs:563-602` |
| PP-006 | `src-tauri/src/wake_coordinator.rs` resumability and consumed-notification suppression | Default `state.db`, StateDb chain/turn APIs, mailbox pending rows, notification marker strings | Opens read-only StateDb via `StateDb::default_path`, checks `chain_id_for_segment`/`count_session_turns`, verifies consumed markers through `has_session_user_turn_containing` and `handle: {row.handle}` marker | LOW within declared boundary: coordinator owns resumability, consumed-notification suppression, and mailbox-handle marker predicate at `wakechain.contract.md:60-69`; mailbox delivery owns notification prefix/nonce rendering and abandoned-row suppression at `wakechain.contract.md:127-134` and `wakechain.contract.md:237-244`. | LOW | `src-tauri/src/wake_coordinator.rs:289-412`, `src-tauri/src/mailbox_delivery.rs:269-303`, `proposal.md:40-43` |
| PP-007 | `src-tauri/src/mailbox_delivery.rs` mailbox delivery prep | Mailbox sidecar pending rows and delivery error/attempt state | Pulls `MailboxDb::list_pending`, filters exhausted unconfirmed rows and `wake_sweep_abandoned`, renders delivery prefix/nonce, marks delivered/failed via sidecar API | LOW source-control/common-interface proof: mailbox delivery is declared owner of deliverable pending filtering, unconfirmed suppression, abandoned suppression, and notification prefix construction at `wakechain.contract.md:45-46` and `wakechain.contract.md:237-244`; sidecar row contract is declared at `wakechain.contract.md:127-134`. | LOW | `src-tauri/src/mailbox_delivery.rs:50-89`, `src-tauri/src/mailbox_delivery.rs:151-177`, `src-tauri/src/mailbox_delivery.rs:269-303`, `diff.patch:1617-1643` |
| PP-008 | `src-tauri/src/run/resume/orchestration.rs` resume delivery confirmation | ExecutionResult submitted-turn evidence, sessions adapter targeted scan, StateDb user-turn predicates | Before unconfirmed branch, scans targeted provider session when confirmation is needed; then checks nonce or exact text via StateDb APIs | LOW common-interface proof: resume lifecycle, mailbox wake-delivery notification, session-turn confirmation evidence, `ExecutionResult` submitted-turn, and invocation finalization are declared adapter surfaces at `wakechain.contract.md:135-142`. | LOW | `src-tauri/src/run/resume/orchestration.rs:721-755`, `src-tauri/src/run/resume/orchestration.rs:892-973`, `proposal.md:34-37` |
| PP-009 | `src-tauri/src/dispatch.rs` and `src-tauri/src/run_tauri.rs` startup/maintenance topology | CLI resume/repl flags and Tauri runtime bootstrap edge | Dispatch starts startup sweep for non-resume CLI entrypoints; Tauri bootstrap starts once-only maintenance driver | LOW source-control proof: CLI lifecycle orchestration and wake reclaim startup scheduling are declared at `wakechain.contract.md:114-125` and `wakechain.contract.md:245-258`; no private service topology is pulled. | LOW | `src-tauri/src/dispatch.rs:83-132`, `diff.patch:1728-1737` |
| PP-010 | `src-tauri/src/lib.rs` module facade | Crate-local module declarations | Adds `mailbox_delivery` and `wake_coordinator` modules; no runtime read/pull site | LOW source-control proof: contract declares `src-tauri/src/lib.rs` as functionless crate-root facade at `wakechain.contract.md:17` and `wakechain.contract.md:219-225`. | LOW | `diff.patch:1569-1589` |
| PP-011 | Integration and adapter tests in `scripts/tests/opencode-turns.test.sh`, `src-tauri/tests/s11_external_provider_wake.rs`, `src-tauri/tests/wake_confirm_legacy_opencode.rs`, `src-tauri/tests/wu_d_proactive_wake_integration.rs` | Fixture-owned temp files, fake CLI scripts, test XDG directories, sidecar DB rows, state DB rows, process state | Tests generate fake provider/OpenCode scripts, seed sidecar/state rows, read fixture outputs and DB assertions | LOW source-control proof: tests own their fake CLI, fixture filesystem, sidecar, and assertion contracts under `wakechain.contract.md:143-166`; direct SQL/file reads are fixture-local test setup/assertions inside isolated temp/XDG roots. | LOW | `scripts/tests/opencode-turns.test.sh:97-170`, `scripts/tests/opencode-turns.test.sh:459-783`, `src-tauri/tests/wake_confirm_legacy_opencode.rs:65-321`, `src-tauri/tests/wu_d_proactive_wake_integration.rs:71-483`, `src-tauri/tests/s11_external_provider_wake.rs:61-237` |

## Uncontrolled-Source Coupler Findings

| ID | Puller | Source | Implicit contract evidence | Missing proof | Decoupling direction | Failure mode |
|---|---|---|---|---|---|---|
| None | None | None | No HIGH pull site found in the touched files/components. | None | None | None |

## Residual Ambiguity / Stop-Condition Notes

The consumed-notification suppression path is intentionally string-based (`[OULIPOLY NOTIFICATIONS]` and `handle: ...`), but it remains inside the declared mailbox-delivery/wake-coordinator common boundary for this WU. If that prompt format becomes an external producer surface, the marker shape should be promoted to an explicit schema/API so the coordinator continues pulling from a common interface rather than private rendered text.

No deployment-level private endpoint, cache, database, filesystem, or service-topology pull was found without source-control or common-interface proof. Tests use fixture-owned temp files and databases, not production private layout.

Verdict: LOW

LOW
