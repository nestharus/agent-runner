# Coupling Audit

## Inputs Read

| Input | Path / value | Notes |
|---|---|---|
| `worktree_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Source inspection root. |
| `repo_root` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Same as worktree. |
| `planning_dir` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate` | Planning artifact root. |
| `wu_id` | `cap` | Used for report context. |
| `proposal_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate/proposal.md` | Read before scoring. |
| `contract_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate/contracts/cap.contract.md` | Read before scoring; preferred declaration carrier. |
| `diff_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate/gates/diff.patch` | Incremental diff evidence over `42200fb..9ba1275`. |
| `touched_surfaces_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate/gates/touched-surfaces.md` | Production and test-only touched surface enumeration. |
| `output_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate/code-quality/cap/reports/coupling-auditor.md` | This report. |
| `mode` | `phase-6` | Phase 6 per-component coupling audit. |

## References Read

| Reference | Evidence |
|---|---|
| `~/ai/conventions/code-quality.md` | Lines 21-25 define auditor scope boundary; lines 143-149 define touched-file ownership; lines 180-204 define adapter scoring; lines 212-249 define intrinsic-surface scoring; line 300 preserves A1 coupling row `Coupling by distinct external symbols/modules referenced`. |
| `~/ai/conventions/proposer-critic-pattern.md` | Lines 29-35 define critic independence and prohibit proposer self-critique. |
| `~/ai/conventions/risk-profile.md` | Lines 13-16 bind non-LOW evidence and touched-file ownership to code-quality auditors. |
| `~/ai/workflows/implementation-pipeline.md` | Lines 403-416 describe Phase 6 implementation and current-layer component-pair coupling evidence; lines 489-491 define Phase 6 per-component code-quality fanout and LOW-only verdict semantics. |
| `planning/cap-gate/contracts/cap.contract.md` | Lines 3 and 44-46 separate the test-only sweep from production function inventory; lines 48-60 declare the `spawn_identity.rs` adapter; lines 62-82 declare the `pty_broker.rs` intrinsic surface. |
| `planning/cap-gate/proposal.md` | Lines 3-7 describe the capture-time backfill hook and sidecar/session-runtime behavior; lines 13-23 define runtime proof claims for sidecar backfill, `session_runtime`, and mid-turn notify owner resolution. |

## Metric Binding

Bound A1 row from `~/ai/conventions/code-quality.md` line 300: `Coupling by distinct external symbols/modules referenced`: LOW = `0-2`; MEDIUM = `3-5`; HIGH = `>= 6`.

The Phase 6 contract was readable and non-blank. The `adapter_declarations:` entry for `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` is well formed: it names a resolvable component, sets `role: adapter`, and lists three non-empty `Translates:` contracts at contract lines 51-58. The `intrinsic_surface_declarations:` entry for `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` is well formed: it names a resolvable component, sets `role: intrinsic-surface`, declares exactly one `Domain:`, and lists a non-empty `Owns:` set at contract lines 65-82.

This invocation is delta-scoped by the supplied prompt and touched-surface line 3, which states prior surfaces were gated LOW at `42200fb`. The production coupling delta is commit `9e00408`; contract lines 3 and 44-46 and touched-surface lines 15-19 classify commit `9ba1275` as test-only isolation evidence with no production behavior.

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| `capture-supervision` | `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs`; touched-surface lines 5-7; contract lines 21 and 30-31; diff lines 207-291. | Production component where the stdout-json `None -> Some` capture observation invokes the late backfill seam. |
| `spawn-identity-runtime-state-adapter` | `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs`; touched-surface lines 8-9; contract lines 22 and 32-38; adapter declaration lines 51-58; diff lines 67-205. | Declared adapter that records child identity, backfills PID sidecar `session_id`, and marks session runtime running. |
| `interactive-callsite-adjustment` | `crates/oulipoly-runtime/src/executor/cli/interactive.rs`; touched-surface lines 10-11; contract lines 23 and 39; diff lines 41-51; source lines 105-108. | Production call-site adjustment for `record_child_identity` returning an optional identity; no new storage fanout. |
| `pty-broker-callsite-adjustment` | `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs`; touched-surface lines 10-11; contract lines 24 and 40; intrinsic declaration lines 65-82; diff lines 54-64; source lines 97-101. | Production PTY call-site adjustment for `record_child_identity` returning an optional identity; the delta adds no new intrinsic-surface domain. |
| `test-only-pin-scrub-sweep` | Touched-surface lines 15-19; contract lines 3 and 44-46; proposal lines 9 and 25-27. | Context only for this production coupling verdict per supplied instruction and contract. Not scored as production function coupling. |

## Per-Pair Coupling

| Source component | Target component | Distinct external symbols/modules referenced | Adapter declaration artifact path | Declared adapter component | `Translates:` contracts | Contract count | Adapter verdict | Intrinsic declaration artifact path | Declared intrinsic component | `Domain:` | `Owns:` set or summary | Domain count | Intrinsic-surface verdict | Final verdict | blocking_or_residual | Evidence |
|---|---|---:|---|---|---|---:|---|---|---|---|---|---:|---|---|---|---|
| `capture-supervision` | `spawn-identity-runtime-state-adapter` | 1 seam contract: `executor spawn-identity seam contract` containing `SpawnIdentityContext`, `record_child_identity`, `backfill_captured_session_id` | `planning/cap-gate/contracts/cap.contract.md` | `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | `pid_identity sidecar contract`; `mailbox session_runtime running contract`; `executor spawn-identity seam contract` | 3 | LOW | n/a | n/a | n/a | n/a | 0 | n/a | LOW | blocking | Contract lines 51-58 declare `spawn_identity.rs` as an adapter and line 60 states `supervision/mod.rs` consumes only `SpawnIdentityContext`, `record_child_identity`, and `backfill_captured_session_id`. Source file `supervision/mod.rs` lines 41-43 import only those seam symbols; lines 132, 142-148, 175-181, 185-190, and 211-230 thread the recorded identity and call the backfill seam. Diff lines 215-218, 225-230, 239-245, 254-260, 265-270, and 275-289 show the delta-owned hook. |
| `capture-supervision` | `oulipoly_state::pid_identity` | 1 type symbol: `ProcessIdentity` | n/a | n/a | n/a | 0 | n/a | n/a | n/a | n/a | n/a | 0 | n/a | LOW | blocking | `supervision/mod.rs` line 46 imports `ProcessIdentity`; lines 215-216 use it only as the optional recorded identity parameter passed to the declared adapter seam. The storage operations remain in `spawn_identity.rs`, not in supervision. |
| `spawn-identity-runtime-state-adapter` | `pid_identity sidecar contract` | Adapter-scored as 1 contract; raw subordinate references include `pid_identity`, `LiveProcessIdentityRecord`, `PidIdentityDb`, `ProcessIdentity`, `record_live_process_identity`, and `set_session_id` | `planning/cap-gate/contracts/cap.contract.md` | `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | `pid_identity sidecar contract`; `mailbox session_runtime running contract`; `executor spawn-identity seam contract` | 3 | LOW | n/a | n/a | n/a | n/a | 0 | n/a | LOW | blocking | `spawn_identity.rs` lines 12-14 import the pid-identity module and types; lines 79-90 record and return `ProcessIdentity`; lines 100-109 accept the captured identity for backfill; lines 182-190 open `PidIdentityDb` and call `set_session_id`. These references are subordinate to the declared `pid_identity sidecar contract` at contract lines 55 and 51-58. |
| `spawn-identity-runtime-state-adapter` | `mailbox session_runtime running contract` | Adapter-scored as 1 contract; raw subordinate references include `MailboxDb`, `SessionRuntimeRunningUpdate`, `open_default`, and `mark_session_running` | `planning/cap-gate/contracts/cap.contract.md` | `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | `pid_identity sidecar contract`; `mailbox session_runtime running contract`; `executor spawn-identity seam contract` | 3 | LOW | n/a | n/a | n/a | n/a | 0 | n/a | LOW | blocking | `spawn_identity.rs` lines 10-11 import mailbox symbols; lines 140-152 open the mailbox and call `mark_session_running`; lines 155-172 construct `SessionRuntimeRunningUpdate`. These references are subordinate to the declared `mailbox session_runtime running contract` at contract line 56 and the adapter declaration at lines 51-58. |
| `interactive-callsite-adjustment` | `spawn-identity-runtime-state-adapter` | 0 newly introduced external symbols/modules; pre-existing call-site now discards the optional return | `planning/cap-gate/contracts/cap.contract.md` | `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | `executor spawn-identity seam contract` | 1 | LOW | n/a | n/a | n/a | n/a | 0 | n/a | LOW | blocking | Diff lines 45-50 change `record_child_identity(child.id(), spawn_identity.as_ref())` to `let _ = record_child_identity(child.id(), spawn_identity.as_ref())`. Source `interactive.rs` lines 35-38 and 105-108 show the same seam call; the delta does not add a new module or symbol reference. |
| `pty-broker-callsite-adjustment` | `spawn-identity-runtime-state-adapter` | 0 newly introduced external symbols/modules; pre-existing call-site now discards the optional return | `planning/cap-gate/contracts/cap.contract.md` | `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | `executor spawn-identity seam contract` | 1 | LOW | `planning/cap-gate/contracts/cap.contract.md` | `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `unix_pty_termios_signal_kernel_surface` | `/dev/tty` open; `openpty`; `tcgetattr`; `cfmakeraw`; `tcsetattr`; `TIOCSCTTY`; `tcsetpgrp`; `TIOCGWINSZ`; `TIOCSWINSZ`; `SIGWINCH`; `setsid`; PTY relay `poll/read/write` | 1 | LOW | LOW | blocking | Diff lines 58-64 change `record_child_identity(child.id(), recorded_context.as_ref().or(context))` to `let _ = record_child_identity(child.id(), recorded_context.as_ref().or(context))`. Source `pty_broker.rs` lines 3 and 97-101 show the same seam call; the delta does not add a new external reference outside the declared intrinsic `Owns:` set at contract lines 69-82. |

## Evidence For Non-LOW Scores

| Score | blocking_or_residual | Ownership proof or residual basis | Evidence | Why it supports the verdict |
|---|---|---|---|---|
| none | n/a | n/a | n/a | No MEDIUM or HIGH component-pair score was found in the production delta. |

## Residual Ambiguity / Stop-Condition Notes

No stop condition fired. `contract_path` was readable and non-blank. A1 still contains the required `Coupling by distinct external symbols/modules referenced` row. The adapter declaration for `spawn_identity.rs` and intrinsic-surface declaration for `pty_broker.rs` are well formed and resolve to production touched components.

The test-only 51-file `OULIPOLY_DATA_DIR` isolation sweep is not scored as production coupling because the supplied contract separates it from production function inventory at lines 3 and 44-46 and the touched-surface file states it has no production changes at lines 15-19. If a later gate asks to audit test-code coupling, those files need their own touched-component pass.

VERDICT: LOW
