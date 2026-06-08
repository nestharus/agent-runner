# Wakechain proposal - consolidated wake-chain fix gate

## Problem

The full wake-chain fix lineage spans three closed sub-deltas plus #44 hardening. #43 fixes wake delivery confirmation for OpenCode by ingesting active turns before the unconfirmed branch, parsing current `info`-nested exports, honoring targeted `SESSION_ID`, and confirming delivery by nonce substring rather than exact quote-wrapped text. #42 fixes stale wake-claim lock leaks by reclaiming dead PID claims before TTL and limiting unconfirmed mailbox wake retries. #44 hardens the sweep so a large dead-owner backlog does not starve recent recoverable leaks and dead-owner debris is marked abandoned instead of retried forever.

## Design

The consolidated tip preserves the three runtime invariants: confirmation requires real submitted or ingested user-turn evidence; wake claims block new work only while their owner identity is plausibly live; and the sweep only starts wake chains for resumable, deliverable sessions. #44 adds a plan phase that separates recoverable candidates from abandoned debris, selects recoverable sessions across oldest/newest pending windows, skips live-owner rows, and reaps bounded abandoned rows with `wake_sweep_abandoned`.

No state.db schema changes, no prod DB/config touch, and no push/install are part of this gate.

## Audited Range

Base: `fcc0faf`.

Head: current branch tip plus the prior #43 split-only carry-over if needed before auditor dispatch.

Generated inputs:

| Artifact | Path |
|---|---|
| Diff | `planning/wakechain-gate/gates/diff.patch` |
| Touched files | `planning/wakechain-gate/gates/touched-files.txt` |
| Contract | `planning/wakechain-gate/contracts/wakechain.contract.md` |
| Runtime evidence | `planning/wakechain-gate/evidence/runtime-tests.log` |

## Proof plan

Evidence log: `planning/wakechain-gate/evidence/runtime-tests.log` (XDG-isolated, `OULIPOLY_DATA_DIR` scrubbed; Cargo/Rustup caches only are reused for toolchain access).

| Runtime claim | Proof method | Evidence-class match |
|---|---|---|
| Resume wake delivery ingests active session turns before the unconfirmed branch and confirms delivered mail when the exported user turn contains the delivery nonce. | `cargo test -p oulipoly-agent-runner --test wake_confirm_legacy_opencode`, especially `legacy_opencode_resume_confirms_delivery_after_targeted_turn_ingest` plus nonce-omitted/export-omitted negative cases. | Runtime CLI integration: compiled runner, fake provider/OpenCode fixture, sidecar mailbox state, and state.db session-turn evidence. |
| The OpenCode adapter handles current `info`-nested exports, targeted `SESSION_ID`, and quote-wrapped prompt bodies. | `bash scripts/tests/opencode-turns.test.sh` for adapter parsing/targeting, plus the wake-confirm integration suite for end-to-end confirmation behavior. | Executable adapter tests plus runtime CLI integration; adapter tests exercise the committed script with fake OpenCode commands and exact normalized JSONL assertions. |
| `scripts/opencode-turns` parses current exports and uses `SESSION_ID` without positional session args while preserving discovery, timeout, and command-selection behavior. | `bash scripts/tests/opencode-turns.test.sh`. | Executable shell/Python adapter evidence, including current export shape, `SESSION_ID` targeting, bounded discovery, deadline/degraded output, and process-group cleanup. |
| `StateDb::has_session_user_turn_containing` confirms non-empty nonce substrings only for the requested provider/session user turns. | `cargo test --workspace`, including `oulipoly-state` unit coverage for exact user body and substring lookup. | Runtime unit evidence over the StateDb API and real SQLite-backed test DB. |
| Dead-PID wake claims are reclaimable before TTL and can be stolen by a fresh notifier. | `cargo test --workspace`, including `crates/oulipoly-state/src/mailbox.rs::tests::wake_dead_pid_claim_can_be_stolen_before_ttl`. | Runtime unit evidence over the sidecar mailbox DB and PID identity reclaimability predicate. |
| Startup/sweep recovery delivers a pending mailbox row from a dead-pid claim leak. | `cargo test -p oulipoly-agent-runner --test wu_d_proactive_wake_integration`, especially `wake_sweep_reclaims_dead_claim_and_delivers_pending_mailbox`. | Runtime CLI integration: compiled runner, sidecar wake claim, state.db resume evidence, and mailbox delivery assertions. |
| Live identity-matched wake owners are not stolen by the sweep. | `wake_sweep_does_not_disturb_live_identity_matched_claim` in `wu_d_proactive_wake_integration`. | Runtime CLI/sidecar integration with a real live process identity recorded in the PID identity sidecar. |
| Consumed notification markers suppress futile re-wake attempts. | `wake_sweep_does_not_rewake_consumed_pending_mailbox` in `wu_d_proactive_wake_integration`. | Runtime integration over mailbox pending rows plus StateDb ingested user-turn marker lookup. |
| Twice-unconfirmed rows do not re-wake forever, but newer deliverable rows still deliver. | `wake_sweep_does_not_rewake_twice_unconfirmed_pending_mailbox` and `wake_sweep_skips_twice_unconfirmed_rows_and_delivers_newer_pending_mailbox`. | Runtime CLI integration: mailbox rows with retry state exercise production `mailbox_delivery` filtering. |
| #44 backlog hardening recovers a recent leak and reaps dead-owner debris under backlog. | `wu_d_proactive_wake_integration::wake_sweep_backlog_recovers_recent_leak_and_reaps_dead_owner_debris`. | Runtime CLI integration: compiled runner scans a mixed backlog, resumes recoverable idle sessions, marks dead-owner rows `wake_sweep_abandoned`, releases their claims, and asserts no dead-owner prompt was started. |
| Existing proactive wake behavior remains green. | `cargo test -p oulipoly-agent-runner --test wu_d_proactive_wake_integration` inside the workspace run; adjacent notify, busy, cap, race, batch, and no-pending tests remain green. | Runtime CLI integration and unit evidence across wake-start, claim, delivery, and sweep paths. |
| Workspace behavior remains green at the audited source head. | `cargo test --workspace`. | Full Rust workspace runtime test evidence across state, runtime, provider, and Tauri runner crates. |

## Residual

The sweep remains bounded. A pathological backlog can require more than one sweep cycle, but #44 proves the relevant production regression is closed: recent recoverable work is selected despite older dead-owner debris, and dead-owner debris is marked abandoned instead of consuming wake capacity indefinitely.
