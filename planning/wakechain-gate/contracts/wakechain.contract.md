# Wakechain contract - consolidated wake-chain fix gate

Audited source range: `fcc0faf..HEAD` plus the behavior-identical #43 split-only carry-over from `2845c30` if it is not already committed on this branch. Functional lineage under review: `549a60e`, `2261cc7`, `71a2b86`, `10fe8d0`, `0eb8665`, `7bd0a62`, `3044a7b`, `5447631`, and `841d404`.

Runtime contract: wake delivery is confirmed only after active session turns are ingested and the delivery nonce is observed in the targeted user-turn evidence; stale wake claims are reclaimable only when their PID owner is not live identity-matched; exhausted unconfirmed rows are suppressed; and the startup/maintenance sweep recovers resumable leaks without letting a large dead-owner backlog starve recent recoverable sessions. Abandoned dead-owner debris is marked with `wake_sweep_abandoned` and its wake claim is released instead of being retried forever.

## Declared Roles

| File | Declared roles | Meaning |
|---|---|---|
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `orchestration`, `accessor`, `filter`, `parser`, `validator`, `mapper`, `formatter`, `predicate` | Session-source adapter execution, targeted `SESSION_ID` scan plumbing, JSONL turn parsing, error classification, transcript locator plumbing, and StateDb ingest. |
| `crates/oulipoly-state/src/db.rs` | `accessor`, `mapper`, `formatter`, `predicate`, `validator`, `parser`, `orchestration`, `filter` | StateDb persistence boundary, session-turn body lookup, exact-text and nonce-substring predicates, chain/turn evidence lookup, and schema/data access. |
| `crates/oulipoly-state/src/mailbox.rs` | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate`, `validator` | PID sidecar mailbox and wake-claim storage owner; reclaimability predicates, bounded candidate selection, newest/oldest backlog scans, and abandoned-row marking. |
| `scripts/opencode-turns` | `orchestration`, `parser`, `mapper`, `filter`, `validator`, `formatter`, `accessor`, `predicate` | OpenCode CLI command orchestration, current export parsing, `SESSION_ID`/discovery target selection, timeout/degraded filtering, and normalized JSONL emission. |
| `scripts/tests/opencode-turns.test.sh` | `orchestration`, `validator`, `formatter`, `parser`, `predicate`, `filter`, `accessor` | Executable adapter test harness, fake CLI generation, JSONL exact-output validation, timeout/process assertions, command/export request filters, and helper access to captured fixture state. |
| `src-tauri/src/dispatch.rs` | `orchestration`, `parser`, `validator`, `accessor`, `formatter`, `mapper`, `predicate`, `filter` | Top-level CLI lifecycle dispatcher; starts the wake reclaim sweep for non-resume entrypoints while preserving resume/repl suppression. |
| `src-tauri/src/lib.rs` | `none` | Functionless module declaration facade; exposes wake/mailbox modules for the coordination path. |
| `src-tauri/src/mailbox_delivery.rs` | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `predicate` | Mailbox delivery prep owner; filters exhausted unconfirmed and abandoned rows, renders prefixes/nonces, and records headless runtime state. |
| `src-tauri/src/run/resume/orchestration.rs` | `orchestration`, `validator`, `accessor`, `mapper`, `filter`, `predicate`, `formatter` | Resume attempt sequencing, delivery-confirmation predicates, pre-unconfirmed session ingest, result/error mapping, and stderr warnings. |
| `src-tauri/src/run_tauri.rs` | `orchestration`, `mapper` | Tauri runtime bootstrap; starts the maintenance sweep driver before app service construction. |
| `src-tauri/src/wake_coordinator.rs` | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate`, `validator` | Wake-chain coordinator; owns notify/turn-end wake starts, startup and maintenance sweeps, startability/disposition planning, live-owner suppression, recoverable-candidate selection, and abandoned-debris reaping. |
| `src-tauri/tests/s11_external_provider_wake.rs` | `orchestration`, `formatter`, `mapper`, `accessor`, `parser`, `validator`, `predicate`, `filter` (TEST) | External-provider wake fixtures; keeps wake delivery confirmation and wake-claim behavior green. |
| `src-tauri/tests/wake_confirm_legacy_opencode.rs` | `orchestration`, `formatter`, `mapper`, `accessor`, `parser`, `validator`, `predicate`, `filter` (TEST) | Legacy opencode wake-confirmation fixtures; fake opencode shaping, targeted turn ingest, mailbox/state assertions, and nonce predicates. |
| `src-tauri/tests/wu_d_proactive_wake_integration.rs` | `orchestration`, `formatter`, `mapper`, `accessor`, `parser`, `validator`, `predicate`, `filter` (TEST) | Proactive wake and reclaim integration fixtures; owns stale-claim reclaim, retry cap, consumed suppression, and #44 backlog/reap coverage. |

## Focused Production Inventory

| File | Function or symbol | A1 class | Meaning |
|---|---|---|---|
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `scan_provider_session`, `scan_provider_with_timeout`, turn-script execution helpers, turn parse/ingest helpers | single-purpose helpers under declared roles | Prior #43 gate-proven surface; targeted scan passes `SESSION_ID`, parses normalized JSONL/degraded records, and ingests active turns before wake-confirm finalization. |
| `crates/oulipoly-state/src/db.rs` | `has_session_user_turn_containing` | `predicate` | Non-empty delivery nonce substring predicate scoped to provider/session user turns. |
| `crates/oulipoly-state/src/db.rs` | `has_session_user_text_turn` and exact-text body helpers | `predicate`, with parser/accessor helpers under prior gate declarations | Exact submitted-turn confirmation path remains separate from nonce-substring confirmation. |
| `crates/oulipoly-state/src/mailbox.rs` | `WakeSweepCandidate` | `mapper` | Session id, auto-wake count, and min/max pending seq DTO returned by sidecar candidate selection. |
| `crates/oulipoly-state/src/mailbox.rs` | `try_acquire_or_renew_wake_claim` | `orchestration` | Claim acquisition transaction; delegates pending/busy and freshness decisions to named helpers. |
| `crates/oulipoly-state/src/mailbox.rs` | `wake_claim_is_reclaimable` | `predicate` | PID-backed claims use live identity; PID-less claims use stale-time TTL. |
| `crates/oulipoly-state/src/mailbox.rs` | `wake_claim_pid_is_reclaimable` | `predicate` | PID-backed reclaimability predicate; reclaimable when recorded PID is not live identity-matched. |
| `crates/oulipoly-state/src/mailbox.rs` | `wake_claim_pid_is_live_identity_matched` | `validator` | Reads live process identity and validates it against the sidecar identity row for the claim. |
| `crates/oulipoly-state/src/mailbox.rs` | `wake_claim_live_identity_has_matching_sidecar_row` | `predicate` | SQL-side identity match over PID, boot id, start ticks, invocation/claim token, provider, and session. |
| `crates/oulipoly-state/src/mailbox.rs` | `wake_sweep_candidates` | `filter` | Bounded candidate selector; skips busy sessions, sessions without pending bounds, and non-reclaimable claims. |
| `crates/oulipoly-state/src/mailbox.rs` | `wake_sweep_candidate` | `mapper` | Constructs `WakeSweepCandidate` DTOs from already-selected session, auto-wake count, and pending seq values. |
| `crates/oulipoly-state/src/mailbox.rs` | `pending_wake_session_ids` | `orchestration` | Merges oldest and newest pending-session scans with deduplication under the caller limit. |
| `crates/oulipoly-state/src/mailbox.rs` | `oldest_pending_wake_session_ids`, `newest_pending_wake_session_ids` | `mapper` | Select scan direction for pending-session candidate lookup. |
| `crates/oulipoly-state/src/mailbox.rs` | `pending_wake_session_ids_by_oldest_seq` | `filter` | SQL filter over non-delivered, non-abandoned mailbox rows grouped by session and ordered by pending seq. |
| `crates/oulipoly-state/src/mailbox.rs` | `mark_pending_abandoned` | `orchestration` | Marks bounded pending rows abandoned and releases the wake claim when rows changed. |
| `src-tauri/src/mailbox_delivery.rs` | `deliverable_pending_rows` | `filter` | Removes rows suppressed by exhausted unconfirmed delivery state or abandoned sweep state. |
| `src-tauri/src/mailbox_delivery.rs` | `mailbox_row_is_deliverable_pending` | `predicate` | Allows rows unless they are abandoned or at/above the unconfirmed retry cap. |
| `src-tauri/src/dispatch.rs` | `startup_wake_reclaim_sweep_enabled` | `predicate` | Suppresses startup sweep for resume/repl resume entrypoints to avoid recursive wake loops. |
| `src-tauri/src/run_tauri.rs` | `run_tauri` maintenance-driver call | `orchestration` | Starts the once-only maintenance driver before Tauri service construction. |
| `src-tauri/src/wake_coordinator.rs` | `run_startup_wake_reclaim_sweep` | `orchestration` | One-shot process-start sweep wrapper; no-ops inside an auto-wake child. |
| `src-tauri/src/wake_coordinator.rs` | `start_wake_reclaim_maintenance_driver` | `orchestration` | Once-only Tauri maintenance driver starter. |
| `src-tauri/src/wake_coordinator.rs` | `wake_reclaim_maintenance_loop` | `orchestration` | Periodic maintenance loop for long-lived Tauri process recovery. |
| `src-tauri/src/wake_coordinator.rs` | `run_wake_reclaim_sweep` | `orchestration` | Opens sidecar, loads bounded candidates, computes a plan, reaps abandoned rows, and starts wake chains for selected recoverable sessions. |
| `src-tauri/src/wake_coordinator.rs` | `wake_sweep_plan` | `orchestration` | Sequences partitioning, recoverable selection, and plan construction through named helpers. |
| `src-tauri/src/wake_coordinator.rs` | `partition_wake_sweep_candidates` | `filter` | Classifies bounded candidates into recoverable, abandoned, or skip buckets and applies the abandoned reap cap. |
| `src-tauri/src/wake_coordinator.rs` | `wake_sweep_plan_from_selected` | `mapper` | Constructs the final `WakeSweepPlan` from already-selected start and reap vectors. |
| `src-tauri/src/wake_coordinator.rs` | `select_recoverable_sweep_candidates` | `filter` | Selects recoverable sessions from both oldest and newest pending sequence windows under the wake sweep cap. |
| `src-tauri/src/wake_coordinator.rs` | `wake_sweep_candidate_disposition` | `orchestration` | Sequences consumed-marker suppression, resumability, live-owner suppression, and abandoned fallback. |
| `src-tauri/src/wake_coordinator.rs` | `resumable_wake_sweep_disposition` | `predicate` | Converts cap and deliverable-pending checks into recoverable or skip disposition. |
| `src-tauri/src/wake_coordinator.rs` | `reap_abandoned_sweep_candidates` | `orchestration` | Applies bounded abandoned-row marking to sessions classified as debris. |
| `src-tauri/src/wake_coordinator.rs` | `wake_sweep_candidate_has_deliverable_pending` | `predicate` | Uses the mailbox delivery production filter to avoid futile wake attempts. |
| `src-tauri/src/wake_coordinator.rs` | `wake_sweep_candidate_is_resumable` | `predicate` | Requires headless idle runtime plus durable StateDb resume evidence. |
| `src-tauri/src/wake_coordinator.rs` | `wake_sweep_runtime_can_resume` | `predicate` | Checks runtime mode, non-running state, and provider identity presence. |
| `src-tauri/src/wake_coordinator.rs` | `wake_sweep_runtime_has_resume_evidence` | `predicate` | Checks chain membership or session-turn evidence in StateDb. |
| `src-tauri/src/wake_coordinator.rs` | `wake_sweep_candidate_has_live_owner` | `predicate` | Scans pending rows for any live owner identity that must block reaping/re-wake. |
| `src-tauri/src/wake_coordinator.rs` | `mailbox_row_has_live_owner_identity` | `predicate` | Reads the live process identity for a mailbox row owner and compares it with the recorded identity. |
| `src-tauri/src/wake_coordinator.rs` | `mailbox_row_owner_identity` | `mapper` | Maps optional mailbox owner PID/boot/start-tick fields into `ProcessIdentity`. |
| `src-tauri/src/wake_coordinator.rs` | `wake_sweep_candidate_reached_cap` | `predicate` | Applies auto-wake retry cap for sweep candidates. |
| `src-tauri/src/wake_coordinator.rs` | `pending_mailbox_consumed_marker_present` | `predicate` | Suppresses futile re-wake when all pending handles already appear in consumed user-turn evidence. |
| `src-tauri/src/wake_coordinator.rs` | `mailbox_handle_marker` | `formatter` | Formats the consumed-turn handle marker string used by the pending-mailbox predicate. |

## Adapter Declarations

```yaml
adapter_declarations:
  - component: crates/oulipoly-runtime/src/sessions/mod.rs
    role: adapter
    Translates:
      - sessions.toml session-source and transcript-locator configuration contract
      - host SESSION_ID targeted-scan environment contract
      - provider script stdout contract for turn JSONL, transcript paths, and degraded markers
      - StateDb session_turns persistence contract
      - provider metadata timeout/error contract
  - component: crates/oulipoly-state/src/db.rs
    role: adapter
    Translates:
      - StateDb public API and query contract
      - SQLite schema, migration, storage, and lifecycle contract
      - domain record encoding contract for invocations, session turns, artifacts, quota, and mailbox evidence
      - JSON body text-chunk and result envelope contract
      - external identity, time, configuration, and sidecar integration contract
  - component: crates/oulipoly-state/src/mailbox.rs
    role: adapter
    Translates:
      - PID identity sidecar contract
      - SQLite sidecar table contract
      - mailbox pending/delivered/failed row contract
      - wake claim acquisition and reclaim contract
      - wake sweep candidate and abandoned-row contract
  - component: scripts/opencode-turns
    role: adapter
    Translates:
      - OpenCode CLI session list/export contract
      - OpenCode info-nested export contract
      - host SESSION_ID turn-script contract
      - normalized session-turn JSONL contract
      - process timeout/degraded marker contract
  - component: scripts/tests/opencode-turns.test.sh
    role: adapter
    Translates:
      - fake opencode CLI fixture contract
      - opencode-turns executable contract
      - normalized JSONL assertion contract
      - process-timeout cleanup contract
  - component: src-tauri/src/dispatch.rs
    role: adapter
    Translates:
      - CLI entrypoint contract
      - wake reclaim startup scheduling contract
      - top-level resume/repl suppression contract
      - runtime services dispatch contract
  - component: src-tauri/src/run_tauri.rs
    role: adapter
    Translates:
      - Tauri application runtime contract
      - wake reclaim maintenance-driver startup contract
      - production AgentRuntimeServices construction contract
  - component: src-tauri/src/mailbox_delivery.rs
    role: adapter
    Translates:
      - mailbox sidecar row contract
      - notification prefix rendering contract
      - delivery confirmation retry contract
      - wake sweep abandoned-row contract
      - headless resume delivery contract
  - component: src-tauri/src/run/resume/orchestration.rs
    role: adapter
    Translates:
      - resume CLI attempt lifecycle contract
      - mailbox wake-delivery notification contract
      - session-turn confirmation evidence contract
      - executor ExecutionResult submitted-turn contract
      - invocation finalization contract
  - component: src-tauri/tests/s11_external_provider_wake.rs
    role: adapter
    Translates:
      - external-provider-runtime-cli-contract
      - wake-notification-delivery-contract
      - pid-identity-sidecar-contract
      - invocation-state-db-contract
      - test-fixture-process-contract
  - component: src-tauri/tests/wake_confirm_legacy_opencode.rs
    role: adapter
    Translates:
      - legacy-opencode-cli-contract
      - wake-confirmation-transcript-contract
      - mailbox-sidecar-contract
      - invocation-state-db-contract
      - test-fixture-process-contract
  - component: src-tauri/tests/wu_d_proactive_wake_integration.rs
    role: adapter
    Translates:
      - runtime-cli-dispatch-contract
      - wake-claim-sidecar-contract
      - pid-identity-sidecar-contract
      - mailbox-delivery-contract
      - test-fixture-process-contract
```

## Intrinsic-Surface Declarations

```yaml
intrinsic_surface_declarations:
  - component: crates/oulipoly-runtime/src/sessions/mod.rs
    role: intrinsic-surface
    Domain: session_metadata_ingest_and_lookup
    Owns:
      - provider session-source lookup and state-dir resolution
      - targeted scan execution with SESSION_ID in environment
      - turn-script stdout parsing into SessionTurnIngest
      - degraded/error reporting without wedging resume
      - StateDb session-turn batch persistence handoff
      - transcript-locator script resolution, stdout validation, and path mapping
  - component: crates/oulipoly-state/src/db.rs
    role: intrinsic-surface
    Domain: state_db_persistence_boundary
    Owns:
      - SQLite open, schema, migration, and connection lifecycle
      - state record CRUD and query APIs for invocations, transitions, session turns, mailbox evidence, quota, artifacts, and provider accounts
      - JSON body parsing and text-chunk confirmation predicates
      - domain result-envelope, sidecar identity, lifecycle-log, model/config, time, UUID, and messenger integrations
      - StateDb caller-facing error mapping and formatting
  - component: crates/oulipoly-state/src/mailbox.rs
    role: intrinsic-surface
    Domain: pid-identity-sidecar mailbox and wake-claim storage
    Owns:
      - session_wake_claim ownership and acquisition semantics
      - wake claim PID identity freshness predicate
      - wake sweep candidate selection over sidecar state
      - pending mailbox row bounds for wake claims
      - abandoned-row marking and claim release for dead-owner debris
      - sidecar-only mailbox schema helpers, not versioned state.db schema
  - component: scripts/opencode-turns
    role: intrinsic-surface
    Domain: opencode_turn_adapter
    Owns:
      - current OpenCode export metadata unwrapping (`info`, nested `message.info`)
      - SESSION_ID environment target selection when positional sessions are absent
      - OpenCode command derivation and OPENCODE_BIN override handling
      - recent-window discovery fallback and max-session cap
      - normalized JSONL record emission and degraded marker semantics
  - component: scripts/tests/opencode-turns.test.sh
    role: intrinsic-surface
    Domain: opencode_turn_adapter_executable_tests
    Owns:
      - fake OpenCode current-export/session-list fixtures
      - exact normalized record assertions for info-nested exports
      - SESSION_ID-env export assertions
      - timeout/degraded and process-group cleanup assertions
  - component: src-tauri/src/lib.rs
    role: intrinsic-surface
    Domain: crate-root module facade
    Owns:
      - public module declaration boundary
      - crate-root re-export surface for wake and mailbox coordination modules
      - no executable runtime behavior
  - component: src-tauri/src/wake_coordinator.rs
    role: intrinsic-surface
    Domain: proactive wake and wake-reclaim orchestration
    Owns:
      - notify and turn-end wake chain starts
      - startup wake reclaim sweep
      - Tauri maintenance wake reclaim loop
      - sweep disposition planning and recoverable candidate selection
      - live-owner suppression before reaping or re-wake
      - abandoned dead-owner debris marking
      - auto-wake cap enforcement and consumed-notification suppression
  - component: src-tauri/src/mailbox_delivery.rs
    role: intrinsic-surface
    Domain: mailbox delivery preparation
    Owns:
      - deliverable pending row filtering
      - unconfirmed delivery retry suppression
      - abandoned row suppression
      - notification prefix and delivery nonce construction
  - component: src-tauri/src/dispatch.rs
    role: intrinsic-surface
    Domain: cli_lifecycle_orchestration
    Owns:
      - startup wake reclaim sweep suppression for resume entrypoints
      - top-level CLI dispatch sequencing
      - runtime service dispatch selection
  - component: src-tauri/src/run_tauri.rs
    role: intrinsic-surface
    Domain: tauri_runtime_bootstrap
    Owns:
      - wake reclaim maintenance-driver startup
      - production runtime service construction
      - Tauri command registration and app run boundary
```

## Residual Accepted For This Gate

The sweep still has a fixed scan/start cap. #44 expands scan breadth and reaps dead-owner debris, but a pathological backlog can still require multiple sweep cycles. This is accepted because the normal single-leak path recovers immediately, #44 proves a recent recoverable leak is no longer starved by dead-owner debris under backlog, and abandoned rows are marked instead of retried indefinitely.
