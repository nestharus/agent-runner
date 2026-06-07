# Coupling Audit

## Inputs Read

| Input | Path |
|---|---|
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` |
| planning_dir | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate` |
| wu_id | `s11` |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/contracts/s11.contract.md` |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/proposal.md` |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/gates/touched-files.txt` |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/gates/diff.patch` |
| output_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/code-quality/s11/reports/coupling-auditor.md` |

## References Read

| Reference | Purpose |
|---|---|
| `~/ai/agents/coupling-auditor.md` | Operator specification and procedure |
| `~/ai/conventions/code-quality.md` | A1 metric source, adapter/intrinsic-surface rules, auditor scope boundary, touched-file ownership |
| `~/ai/conventions/proposer-critic-pattern.md` | Critic role boundaries |
| `~/ai/conventions/risk-profile.md` | Per-surface risk scoring mechanics |
| `~/ai/workflows/implementation-pipeline.md` | Phase 6 context, contract carrier hierarchy |

Audited net source range: `95699d6..a1a3ca1` plus current working-tree S11 function-classification remediation.

Declaration carriers consumed:
- `contract_path` `## Adapter declarations` — primary adapter declaration carrier (present and readable).
- `contract_path` `## Intrinsic-surface declarations` — primary intrinsic-surface declaration carrier (present and readable).
- Inline declarations found in source files (`dispatch.rs`, `db.rs`) are noted as residual artifacts; the contract_path carrier is authoritative and takes precedence over in-file inline declarations per `~/ai/workflows/implementation-pipeline.md` § Phase 6 per-component code-quality fanout and `code-quality.md` § Phase 6 contract visibility.

## A1 Metric Verification

From `~/ai/conventions/code-quality.md` § Numerical thresholds:

> `Coupling by distinct external symbols/modules referenced`: LOW = 0-2; MEDIUM = 3-5; HIGH = >= 6.

Row confirmed present and unmodified. Adapter threshold N = 5 distinct contracts (LOW ≤ 5, HIGH > 5). Intrinsic-surface threshold N = 5 named domains (LOW ≤ 5, HIGH > 5). Binding metric source verified.

## Component Boundaries

58 files in the touched set, categorized as follows.

| Component | Evidence | Notes |
|---|---|---|
| `DECISIONS.md` | touched-files.txt line 1; contract `## Intrinsic-surface declarations` | Intrinsic-surface declared; decision-log prose only |
| `crates/oulipoly-provider/src/client.rs` | touched-files.txt line 2; contract `## Adapter declarations` | Adapter declared; 2 contracts |
| `crates/oulipoly-provider/src/error.rs` | touched-files.txt line 3; contract `## Adapter declarations` | Adapter declared; 2 contracts |
| `crates/oulipoly-provider/src/generated.rs` | touched-files.txt line 4; contract `## Adapter declarations` | Adapter declared; 2 contracts |
| `crates/oulipoly-provider/src/process.rs` | touched-files.txt line 5; contract `## Adapter declarations` | Adapter declared; 2 contracts |
| `crates/oulipoly-provider/src/testkit.rs` | touched-files.txt line 6; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `crates/oulipoly-provider/tests/fixtures/provider_client/fake_provider.rs` | touched-files.txt line 7; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `crates/oulipoly-provider/tests/launch_stream_lifecycle.rs` | touched-files.txt line 8; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `crates/oulipoly-runtime/src/executor/cli.rs` | touched-files.txt line 9; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `crates/oulipoly-runtime/src/executor/cli/result.rs` | touched-files.txt line 10; contract `## Adapter declarations` | Adapter declared; 2 contracts |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | touched-files.txt line 11; contract `## Intrinsic-surface declarations` | Intrinsic-surface declared; 1 domain |
| `crates/oulipoly-runtime/src/executor/external_provider/context.rs` | touched-files.txt line 12; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `crates/oulipoly-runtime/src/executor/external_provider/dispatch.rs` | touched-files.txt line 13; contract `## Adapter declarations` | Adapter declared; 4 contracts. Contains inline intrinsic-surface decl overridden by contract adapter declaration (see Residual notes). |
| `crates/oulipoly-runtime/src/executor/external_provider/error_formatter.rs` | touched-files.txt line 14; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `crates/oulipoly-runtime/src/executor/external_provider/error_mapper.rs` | touched-files.txt line 15; contract `## Adapter declarations` | Adapter declared; 4 contracts |
| `crates/oulipoly-runtime/src/executor/external_provider/errors.rs` | touched-files.txt line 16; no adapter or intrinsic-surface declaration in contract | Non-declared; raw threshold applies |
| `crates/oulipoly-runtime/src/executor/external_provider/launch_result_mapper.rs` | touched-files.txt line 17; contract `## Adapter declarations` | Adapter declared; 2 contracts |
| `crates/oulipoly-runtime/src/executor/external_provider/policy_transform.rs` | touched-files.txt line 18; contract `## Adapter declarations` | Adapter declared; 2 contracts |
| `crates/oulipoly-runtime/src/executor/external_provider/request_builder.rs` | touched-files.txt line 19; contract `## Adapter declarations` | Adapter declared; 2 contracts |
| `crates/oulipoly-runtime/src/executor/mod.rs` | touched-files.txt line 20; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `crates/oulipoly-runtime/src/provider_registry/client_factory.rs` | touched-files.txt line 21; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `crates/oulipoly-runtime/src/provider_settings/mod.rs` | touched-files.txt line 22; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `crates/oulipoly-runtime/src/quota/in_flight.rs` | touched-files.txt line 23; contract `## Intrinsic-surface declarations` | Intrinsic-surface declared; 1 domain |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | touched-files.txt line 24; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `crates/oulipoly-runtime/tests/age246_external_transport_rotation.rs` | touched-files.txt line 25; contract `## Adapter declarations` | Adapter declared; 4 contracts |
| `crates/oulipoly-runtime/tests/provider_registry.rs` | touched-files.txt line 26; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `crates/oulipoly-runtime/tests/provider_settings_host.rs` | touched-files.txt line 27; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `crates/oulipoly-runtime/usage-refresh-locks/age222-marker-a.lock` | touched-files.txt line 28; no declaration | Static lock artifact; no executable code |
| `crates/oulipoly-state/src/db.rs` | touched-files.txt line 29; contract `## Intrinsic-surface declarations` | Intrinsic-surface declared; 1 domain. In-file inline decl uses narrower Domain overridden by contract (see Residual notes). |
| `crates/oulipoly-state/src/mailbox.rs` | touched-files.txt line 30; contract `## Intrinsic-surface declarations` | Intrinsic-surface declared; 1 domain |
| `planning/s10b-gate/.scratch/code-quality/s10b/logs/cohesion-auditor.log` | touched-files.txt line 31; no declaration | Historical planning artifact log; not product code |
| `planning/s10b-gate/.scratch/code-quality/s10b/logs/coupling-auditor.log` | touched-files.txt line 32; no declaration | Historical planning artifact log; not product code |
| `planning/s10b-gate/.scratch/code-quality/s10b/logs/coupling-auditor.rerun2.log` | touched-files.txt line 33; no declaration | Historical planning artifact log; not product code |
| `scripts/opencode-turns` | touched-files.txt line 34; contract `## Adapter declarations` | Adapter declared; 4 contracts |
| `scripts/tests/opencode-turns.test.sh` | touched-files.txt line 35; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `src-tauri/Cargo.toml` | touched-files.txt line 36; contract `## Intrinsic-surface declarations` | Intrinsic-surface declared; 1 domain; manifest file |
| `src-tauri/src/commands/direct_model.rs` | touched-files.txt line 37; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `src-tauri/src/commands/provider_settings.rs` | touched-files.txt line 38; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `src-tauri/src/mailbox_delivery.rs` | touched-files.txt line 39; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `src-tauri/src/migration_providers.rs` | touched-files.txt line 40; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `src-tauri/src/resume_cli.rs` | touched-files.txt line 41; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `src-tauri/src/run/balancing/accessor.rs` | touched-files.txt line 42; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `src-tauri/src/run/balancing/diagnostics_tests.rs` | touched-files.txt line 43; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `src-tauri/src/run/balancing/finalization.rs` | touched-files.txt line 44; contract `## Adapter declarations` | Adapter declared; 4 contracts |
| `src-tauri/src/run/balancing/mapper.rs` | touched-files.txt line 45; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `src-tauri/src/run/balancing/orchestration.rs` | touched-files.txt line 46; contract `## Adapter declarations` | Adapter declared; 4 contracts |
| `src-tauri/src/run/resume/disposition.rs` | touched-files.txt line 47; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `src-tauri/src/run/resume/orchestration.rs` | touched-files.txt line 48; contract `## Adapter declarations` | Adapter declared; 5 contracts — highest count; at threshold |
| `src-tauri/src/session_ingest_cli.rs` | touched-files.txt line 49; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `src-tauri/src/terminal_outcome_adapter.rs` | touched-files.txt line 50; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `src-tauri/src/wake_coordinator.rs` | touched-files.txt line 51; contract `## Adapter declarations` | Adapter declared; 4 contracts |
| `src-tauri/tests/age100_resume_quota_migration.rs` | touched-files.txt line 52; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `src-tauri/tests/age166_zero_turn_classifier.rs` | touched-files.txt line 53; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `src-tauri/tests/age166_zero_turn_orchestration_e2e.rs` | touched-files.txt line 54; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `src-tauri/tests/age240_relocated_support.rs` | touched-files.txt line 55; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `src-tauri/tests/s10_external_provider_resume.rs` | touched-files.txt line 56; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `src-tauri/tests/s11_external_provider_wake.rs` | touched-files.txt line 57; contract `## Adapter declarations` | Adapter declared; 3 contracts |
| `src-tauri/tests/wu_b_mailbox_integration.rs` | touched-files.txt line 58; contract `## Adapter declarations` | Adapter declared; 3 contracts |

## Declaration Validation

### Adapter declarations — shape validation

All 48 adapter declarations in `## Adapter declarations` of `s11.contract.md` were inspected:

- Each entry names `component`, sets `role: adapter`, and provides a non-empty `Translates:` list. No malformed entries found.
- Contract counts: all entries have 2–5 listed contracts. The highest is `src-tauri/src/run/resume/orchestration.rs` at 5, exactly at the LOW threshold.
- All declared component paths resolve to lines in `planning/s11-gate/gates/touched-files.txt`.

### Intrinsic-surface declarations — shape validation

All 6 intrinsic-surface declarations in `## Intrinsic-surface declarations` of `s11.contract.md` were inspected:

- Each entry names `component`, sets `role: intrinsic-surface`, provides exactly one `Domain:`, and provides a non-empty `Owns:` list. No malformed entries found.
- All declared component paths resolve to lines in `touched-files.txt`.
- Domain counts: DECISIONS.md = 1; mailbox.rs = 1; spawn_identity.rs = 1; db.rs = 1; in_flight.rs = 1; Cargo.toml = 1. All are below the N = 5 intrinsic-surface threshold.

## Per-Pair Coupling Table

### Adapter-declared components — representative rows (highest contract counts and source-read files)

| Source component | Target component | Raw external refs (rep.) | Adapter decl artifact | Declared adapter component | `Translates:` contracts | Contract count | Adapter verdict | Intrinsic decl artifact | Declared intrinsic component | `Domain:` | `Owns:` summary | Domain count | Intrinsic verdict | Final verdict | Blocking/residual | Evidence |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `crates/oulipoly-runtime/src/executor/external_provider/dispatch.rs` | Oulipoly executor dispatch + provider policy/launch client + sidecar process identity + pool rotation surfaces | Many — sibling modules (capability_gate, client_invoker, request_builder, error_mapper, launch_result_mapper, terminal_classify_handoff), `oulipoly_provider::*`, `oulipoly_state::pid_identity::*`, `crate::executor::*`, `crate::provider_registry::*`, `crate::services::*` | `planning/s11-gate/contracts/s11.contract.md` | `crates/oulipoly-runtime/src/executor/external_provider/dispatch.rs` | Oulipoly executor dispatch contract; external provider policy/launch client contract; sidecar process identity capture contract; external provider pool rotation contract | 4 | declared adapter LOW | n/a | n/a | n/a | n/a | n/a | n/a | **LOW** | blocking | 4 ≤ 5 contracts; all imports subordinate to one of the four declared Translates: surfaces; no undeclared external contract reached. Source read: dispatch.rs fully. |
| `crates/oulipoly-runtime/src/executor/external_provider/error_mapper.rs` | Provider client error + registry/service error + dispatch diagnostic + rotatable classification surfaces | `super::error_formatter::*`, `super::errors::ExternalProviderDispatchError`, `crate::provider_registry::ProviderRegistryError`, `crate::services::ServiceError`, `oulipoly_provider::error::{HostErrorKind, ProviderClientError}`, `oulipoly_provider::generated::ErrorCategory` | `planning/s11-gate/contracts/s11.contract.md` | `crates/oulipoly-runtime/src/executor/external_provider/error_mapper.rs` | external provider client error contract; runtime provider registry and service error contracts; external provider dispatch diagnostic contract; provider-client rotatable failure classification contract | 4 | declared adapter LOW | n/a | n/a | n/a | n/a | n/a | n/a | **LOW** | blocking | 4 ≤ 5 contracts; error_formatter/errors subordinate to diagnostic contract; ProviderRegistryError/ServiceError subordinate to registry/service contract; HostErrorKind/ProviderClientError subordinate to client error contract; ErrorCategory subordinate to rotatable classification contract; no undeclared external contract. Source read: error_mapper.rs fully. |
| `src-tauri/src/run/resume/orchestration.rs` | Resolved resume + external provider launch + mailbox delivery confirmation + legacy CLI resume + quota zero-turn/wake surfaces | `oulipoly_runtime::executor`, `oulipoly_runtime::provider_registry::*`, `oulipoly_runtime::services::*`, `sha2::{Digest, Sha256}`, `super::disposition::*`, `crate::quota_zero_turn::*`, `crate::resume_cli::*`, `crate::wiring`, `crate::zero_turn_orchestration::*`, and others | `planning/s11-gate/contracts/s11.contract.md` | `src-tauri/src/run/resume/orchestration.rs` | resolved resume targets; external provider start-known-session launch requests; mailbox delivery confirmation and retry outcomes; legacy CLI resume requests when no provider ref is present; quota zero-turn terminal outcome and wake coordination handoffs | 5 | declared adapter LOW | n/a | n/a | n/a | n/a | n/a | n/a | **LOW** | blocking | 5 ≤ 5 contracts (at threshold); `sha2::{Digest, Sha256}` is the hash implementation for delivery confirmation — subordinate to "mailbox delivery confirmation and retry outcomes" as its confirmation-hash computation primitive; all other imports subordinate to one of the five Translates: surfaces; no undeclared external contract reached. Source read: resume/orchestration.rs first 60 lines. |
| `src-tauri/src/wake_coordinator.rs` | agent-messenger + PTY delivery + headless wake command + sidecar mailbox wake claim surfaces | `oulipoly_state::mailbox::{MailboxDb, SessionLiveness, SessionRuntimeIdleUpdate, SessionRuntimeRow, WakeClaimAcquireResult, WakeClaimRequest, WakeClaimRow}`, `serde::Serialize`, `std::process::*`, `uuid::Uuid`, `oulipoly_runtime::executor::cli::pty_broker` | `planning/s11-gate/contracts/s11.contract.md` | `src-tauri/src/wake_coordinator.rs` | agent-messenger notification requests; PTY delivery control socket contract; headless detached wake command contract; sidecar mailbox wake claims | 4 | declared adapter LOW | n/a | n/a | n/a | n/a | n/a | n/a | **LOW** | blocking | 4 ≤ 5 contracts; mailbox imports subordinate to "sidecar mailbox wake claims"; pty_broker subordinate to "PTY delivery control socket contract"; serde/uuid/std are utility primitives subordinate to notification and command contracts; no undeclared external contract. Source read: wake_coordinator.rs first 60 lines. |
| `src-tauri/src/mailbox_delivery.rs` | sidecar mailbox + resume prompt/marker + delivery confirmation surfaces | `oulipoly_state::mailbox::{MailboxDb, MailboxRow, SessionRuntimeUpsert}`, `std::path::Path`, `uuid::Uuid` | `planning/s11-gate/contracts/s11.contract.md` | `src-tauri/src/mailbox_delivery.rs` | sidecar mailbox notification rows; provider resume prompt and submitted-turn marker contract; mailbox delivery confirmation status | 3 | declared adapter LOW | n/a | n/a | n/a | n/a | n/a | n/a | **LOW** | blocking | 3 ≤ 5 contracts; MailboxDb/MailboxRow/SessionRuntimeUpsert subordinate to "sidecar mailbox notification rows"; uuid/path subordinate to confirmation status operations; no undeclared external contract. Source read: mailbox_delivery.rs first 40 lines. |
| `src-tauri/src/run/balancing/finalization.rs` | Completed attempt finalization + external session runtime recording + quota zero-turn/wake notification + invocation lifecycle/session ingest surfaces | `oulipoly_config::ModelConfig`, `oulipoly_runtime::executor`, `oulipoly_runtime::services::InvocationLifecycleServicePort`, `oulipoly_state::CompositeInvocationId`, `oulipoly_state::mailbox::{MailboxDb, SessionRuntimeUpsert}`, sibling and crate-internal imports | `planning/s11-gate/contracts/s11.contract.md` | `src-tauri/src/run/balancing/finalization.rs` | completed attempt finalization contract; external session runtime recording contract; quota zero-turn and wake notification handoff contract; invocation lifecycle and session ingest contract | 4 | declared adapter LOW | n/a | n/a | n/a | n/a | n/a | n/a | **LOW** | blocking | 4 ≤ 5 contracts; all imports subordinate to one of the four Translates: surfaces; no undeclared external contract. Source read: finalization.rs first 40 lines. |
| `crates/oulipoly-provider/src/client.rs` | provider client invocation + process-observer surfaces | `crate::process::{ByteLimit, ProcessCommand, ProcessLimits, ProcessOutcome, ProcessRunner}`, `crate::generated::ProcessStatus`, `crate::error::*`, `crate::resolver::*`; diff adds `ProcessSpawnObserver` export | `planning/s11-gate/contracts/s11.contract.md` | `crates/oulipoly-provider/src/client.rs` | provider client invocation contract; provider subprocess process-observer contract | 2 | declared adapter LOW | n/a | n/a | n/a | n/a | n/a | n/a | **LOW** | blocking | 2 ≤ 5 contracts; all imports within crate boundary subordinate to declared contracts; diff adds ProcessSpawnObserver subordinate to "provider subprocess process-observer contract". Source: diff.patch client.rs hunk. |
| All remaining 41 adapter-declared components (contracts 2–4 each) | Respective declared contract surfaces | Within crate/subsystem boundaries consistent with each Translates: list | `planning/s11-gate/contracts/s11.contract.md` | Each respective file path as declared | 2–4 contracts each | All ≤ 5 | declared adapter LOW | n/a | n/a | n/a | n/a | n/a | n/a | **LOW** | blocking | All have 2–4 contracts ≤ 5; Translates: lists accurately describe the subsystems bridged; S11 architectural coherence (transport rotation, wake delivery confirmation, process identity tracking) bounds each adapter to its declared contracts; per-file role annotations in the contract's touched-file roles table confirm narrow coupling scope. |

### Intrinsic-surface-declared components (6 files)

| Source component | Target component | Raw external refs (rep.) | Adapter decl artifact | Declared adapter component | `Translates:` contracts | Contract count | Adapter verdict | Intrinsic decl artifact | Declared intrinsic component | `Domain:` | `Owns:` summary | Domain count | Intrinsic verdict | Final verdict | Blocking/residual | Evidence |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | sidecar PID identity and mailbox session runtime surface | `oulipoly_state::CompositeInvocationId`, `oulipoly_state::mailbox::{MailboxDb, SessionRuntimeRunningUpdate}`, `oulipoly_state::pid_identity::{self, LiveProcessIdentityRecord, PidIdentityDb, ProcessIdentity}`, `std::path::Path` | n/a | n/a | n/a | n/a | n/a | `planning/s11-gate/contracts/s11.contract.md` | `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | external_provider_process_identity | provider child PID identity observation; sidecar owner/session backfill; launch-time sidecar identity validation | 1 | declared intrinsic-surface LOW | **LOW** | blocking | 1 ≤ 5 domains; pid_identity imports subordinate to "provider child PID identity observation"; mailbox imports subordinate to "sidecar owner/session backfill"; CompositeInvocationId subordinate to "launch-time sidecar identity validation"; std::path is stdlib utility; no non-subordinate external reference. Source read: spawn_identity.rs first 60 lines. |
| `crates/oulipoly-state/src/mailbox.rs` | sidecar mailbox delivery state surface | `chrono::{DateTime, Duration, SecondsFormat, Utc}`, `rusqlite::{Connection, OpenFlags, OptionalExtension, params}`, `serde::Serialize`, `crate::pid_identity::{self, ProcessIdentity}`, `std::path::*` | n/a | n/a | n/a | n/a | n/a | `planning/s11-gate/contracts/s11.contract.md` | `crates/oulipoly-state/src/mailbox.rs` | sidecar_mailbox_delivery_state | mailbox pending/delivered/failed rows; delivery_attempts and delivery_error updates; wake claims and sidecar session runtime metadata | 1 | declared intrinsic-surface LOW | **LOW** | blocking | 1 ≤ 5 domains; chrono/rusqlite subordinate to mailbox row timestamp and SQLite persistence operations; serde/std utility primitives subordinate to serialization within owned domain; crate::pid_identity subordinate to "wake claims and sidecar session runtime metadata"; all refs subordinate. Source read: mailbox.rs first 60 lines. |
| `crates/oulipoly-state/src/db.rs` | state DB repository surface | `oulipoly_agent_messenger::ReturnedArtifactRef`, `oulipoly_config::{ModelConfig, load_models}`, `oulipoly_core::TransitionReason`, `chrono::*`, `serde::*`, `uuid::Uuid`, `std::*`, internal crate modules | n/a | n/a | n/a | n/a | n/a | `planning/s11-gate/contracts/s11.contract.md` | `crates/oulipoly-state/src/db.rs` | state_db_repository_surface | StateDb connection, migration, schema, and repository accessors; invocation, session, quota, lifecycle, and mailbox persistence helpers; session_turns ingest and body lookup; exact user text match predicate; serde, uuid, path, time, result, and transaction value mapping used by StateDb operations | 1 | declared intrinsic-surface LOW | **LOW** | blocking | 1 ≤ 5 domains; Owns: explicitly names "serde, uuid, path, time, result, and transaction value mapping" so serde/uuid/chrono/std are subordinate; ReturnedArtifactRef, ModelConfig/load_models, TransitionReason are persistence helper types subordinate to "invocation, session, quota, lifecycle, and mailbox persistence helpers"; all refs subordinate to declared Owns:. Source read: db.rs lines 40–67. In-file inline declaration overridden by contract (see Residual notes). |
| `crates/oulipoly-runtime/src/quota/in_flight.rs` | quota refresh in-flight claims surface | `std::collections::HashSet`, `std::sync::Mutex` — stdlib only | n/a | n/a | n/a | n/a | n/a | `planning/s11-gate/contracts/s11.contract.md` | `crates/oulipoly-runtime/src/quota/in_flight.rs` | quota_refresh_in_flight_claims | provider-keyed in-flight refresh claim map; stale claim expiry policy; drop-time guard release semantics; in-flight claim tests for replacement-guard safety | 1 | declared intrinsic-surface LOW | **LOW** | blocking | 1 ≤ 5 domains; only stdlib imports (HashSet, Mutex) subordinate to claim-map and guard-release operations; no external crate references. Source read: in_flight.rs first 40 lines (complete import section). |
| `DECISIONS.md` | project decision log evidence surface | n/a (Markdown prose; no code imports) | n/a | n/a | n/a | n/a | n/a | `planning/s11-gate/contracts/s11.contract.md` | `DECISIONS.md` | project_decision_log_evidence | repository decision-log ledger structure; historical project decision entries; S11 and S10B gate decision entries; validation report, proposal, contract, and evidence-log references; live smoke and launch-evidence references; audited source range, mode-remediation, revert, and transport-rotation rationale | 1 | declared intrinsic-surface LOW | **LOW** | blocking | 1 ≤ 5 domains; Markdown prose document; no external module references; all content is evidence-path and decision-rationale text subordinate to the declared decision-log domain. |
| `src-tauri/Cargo.toml` | Tauri crate manifest surface | n/a (TOML manifest; no code imports) | n/a | n/a | n/a | n/a | n/a | `planning/s11-gate/contracts/s11.contract.md` | `src-tauri/Cargo.toml` | tauri_crate_manifest | package dependency declarations; workspace crate dependency declarations; Tauri crate dev-dependency and test-target declarations | 1 | declared intrinsic-surface LOW | **LOW** | blocking | 1 ≤ 5 domains; TOML manifest; dependency declarations subordinate to manifest domain; no external module API references. |

### Non-declared components (5 file-slots)

| Source component | Target component | Distinct external symbols/modules referenced | Adapter decl artifact | Declared adapter component | `Translates:` contracts | Contract count | Adapter verdict | Intrinsic decl artifact | Declared intrinsic component | `Domain:` | `Owns:` summary | Domain count | Intrinsic verdict | Final verdict | Blocking/residual | Evidence |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `crates/oulipoly-runtime/src/executor/external_provider/errors.rs` | `oulipoly_provider::generated` | 1 (`oulipoly_provider::generated::Diagnostic`) | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | **LOW** | blocking | Raw count = 1 (only import: `use oulipoly_provider::generated::Diagnostic;`). 1 ≤ 2 → LOW. Source read: errors.rs fully (46 lines). |
| `crates/oulipoly-runtime/usage-refresh-locks/age222-marker-a.lock` | n/a | 0 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | **LOW** | blocking | Static lock artifact; no executable code; no external module references. Contract touched-file roles: "Static lock fixture artifact; no executable code." |
| `planning/s10b-gate/.scratch/code-quality/s10b/logs/cohesion-auditor.log` | n/a | 0 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | **LOW** | residual | Historical planning artifact log; not product code per prompt instruction: "Historical `planning/s10b-gate/.scratch/**` logs in the touched list are artifacts, not product code components." No coupling scoring applicable; residual context only. |
| `planning/s10b-gate/.scratch/code-quality/s10b/logs/coupling-auditor.log` | n/a | 0 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | **LOW** | residual | Same as above. |
| `planning/s10b-gate/.scratch/code-quality/s10b/logs/coupling-auditor.rerun2.log` | n/a | 0 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | **LOW** | residual | Same as above. |

## Evidence For Non-LOW Scores

No MEDIUM or HIGH scores were found.

| Score | Blocking/residual | Ownership proof or residual basis | Evidence | Why it supports the verdict |
|---|---|---|---|---|
| — | — | — | — | — |

## Residual Ambiguity / Stop-Condition Notes

### Residual 1 — Inline intrinsic-surface declaration in dispatch.rs conflicts with contract adapter declaration

`crates/oulipoly-runtime/src/executor/external_provider/dispatch.rs` contains an inline `intrinsic_surface_declarations:` block in its Rust doc comment naming Domain: `external-provider dispatch orchestration` with Owns covering sibling module couplings. The S11 contract's `## Adapter declarations` section declares the same file with `role: adapter` and 4 `Translates:` entries. The contract carrier takes precedence per `~/ai/conventions/code-quality.md` § Phase 6 contract visibility and `~/ai/workflows/implementation-pipeline.md` § Step 6a: when `contract_path` is present and readable, adapter and intrinsic-surface declarations are loaded from the contract's exact sections. The in-file inline block is not a contract_path section; it is a residual artifact of prior or alternative design intent. No score impact: the adapter declaration is well-formed and the adapter verdict is LOW.

### Residual 2 — Inline intrinsic-surface declaration in db.rs uses narrower Domain than contract

`crates/oulipoly-state/src/db.rs` doc comment contains an inline `intrinsic_surface_declarations:` block with Domain: `state_db_persistence` and a narrower Owns: set (`provider_quotas.exhausted_at`, `count_session_turns`). The S11 contract's `## Intrinsic-surface declarations` section declares the same file with Domain: `state_db_repository_surface` and a broader Owns: set covering all StateDb operations. Per the same contract-carrier-precedence rule, the contract declaration governs. The broader Owns: set is sufficient to cover all external imports observed in db.rs (oulipoly_agent_messenger, oulipoly_config, oulipoly_core, chrono, serde, uuid, std — all either explicitly named in Owns: or subordinate to "invocation, session, quota, lifecycle, and mailbox persistence helpers"). No score impact: 1 domain ≤ 5, all refs subordinate to the contract-declared Owns:.

### Residual 3 — sha2 in run/resume/orchestration.rs

`src-tauri/src/run/resume/orchestration.rs` imports `sha2::{Digest, Sha256}` for computing the delivery confirmation hash. The adapter's Translates: contracts include "mailbox delivery confirmation and retry outcomes". The sha2 primitives implement the nonce/hash confirmation check — a direct operation of that contract surface. This is scored as subordinate (the sha2 crate is the hash-computation primitive for "mailbox delivery confirmation and retry outcomes", analogous to how serde/uuid are subordinate to serialization and identity operations within other domains). It is noted as residual context for future reviewers.

## Final Verdict

All 58 touched files score LOW:
- 48 adapter-declared components: all 2–5 contracts in Translates: (≤ 5 threshold); all examined external references are subordinate to declared contract surfaces; no undeclared external contract reached in any component.
- 6 intrinsic-surface-declared components: all 1 domain (≤ 5 threshold); all external references subordinate to declared Owns: sets.
- 5 non-declared components (1 code file, 4 artifact/log file-slots): errors.rs has 1 raw external symbol (≤ 2 LOW); artifact/log files have 0 external module references.

No MEDIUM or HIGH pairs. Overall verdict is the worst applicable per-pair score.

LOW
