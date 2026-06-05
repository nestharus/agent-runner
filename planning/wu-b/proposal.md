# WU-B Proposal: Resume-Backed Notification Mailbox + Delivery

## Scope

WU-B implements the receiving side of the agent-bash completion seam:

```text
agents notify agent-bash-complete --caller-ppid <p> --handle <h> --state-dir <d> --meta <d>/meta.json --log <d>/log --rc <d>/rc
```

The spooler owns workload execution and result file retention. Agent-runner owns death-safe resolution from the spooler's captured caller ancestry to a provider session, queues the completion into that session's mailbox, and delivers queued messages at the next headless resume. The v1 core is pull-on-resume for headless sessions; PTY/interactive immediate injection is designed as a follow-up because the current interactive runner inherits stdio and does not keep a writable PTY control handle.

Hard rule: WU-B must not change the versioned `state.db` schema. All persistent mailbox data lives in the independent sidecar database under `XDG_DATA_HOME/oulipoly-agent-runner/`, and all tests must isolate `XDG_DATA_HOME` and `XDG_CONFIG_HOME`.

## Existing Integration Points

The relevant existing files and functions are:

- CLI shape: `src-tauri/src/usage/cli.rs`, `Subcommands` and `SessionSubcommands`.
- CLI dispatch: `src-tauri/src/dispatch.rs`, `dispatch_subcommand`, `dispatch_session_subcommand`, `dispatch_headless_top_level_resume`, `dispatch_interactive_top_level_resume`.
- WU-A sidecar: `crates/oulipoly-state/src/pid_identity.rs`, `PidIdentityDb`, `ProcessIdentity`, `PidIdentityDb::lookup_by_identity`, `PidIdentityDb::lookup_by_invocation_uuid`.
- Existing PID CLI: `src-tauri/src/commands/pid_session.rs`, especially `resolve_row_session_id`, which already falls back from sidecar `session_id` to read-only `state.db` lookup by `invocation_uuid`.
- Headless resume entry: `src-tauri/src/run/resume/orchestration.rs`, `run_resume`, `prepare_headless_resume_execution`, `run_resume_loop`, `execute_resume_attempt_command`.
- Repl resume entry: `src-tauri/src/run/repl/orchestration.rs`, `run_repl`, `prepare_repl_model_and_resume`, `execute_and_finalize_repl_attempt`.
- Resume payload execution: `crates/oulipoly-runtime/src/executor/cli/resume_execution.rs`, `execute_resume_optional_prompt_with_model_identity`.
- Prompt transport: `crates/oulipoly-runtime/src/executor/cli/launch/prompt.rs` and `launch/supervisor_config.rs`; compose the message before the executor call and let existing `PromptMode::Arg` / `PromptMode::Stdin` handling do transport.

## CLI Design

Add top-level command families to `Subcommands` in `src-tauri/src/usage/cli.rs`:

```text
agents notify agent-bash-complete \
  --caller-ppid <pid> \
  --handle <handle> \
  --state-dir <path> \
  --meta <path> \
  --log <path> \
  --rc <path> \
  [--json]

agents mailbox list --session-id <session-id> [--all] [--json]
```

Proposed enum shape:

```rust
Subcommands::Notify { command: NotifySubcommands }
NotifySubcommands::AgentBashComplete { caller_ppid, handle, state_dir, meta, log, rc, json }

Subcommands::Mailbox { command: MailboxSubcommands }
MailboxSubcommands::List { session_id, all, json }
```

`notify agent-bash-complete` reads `meta.json`, parses `caller_chain`, resolves the owner session death-safely, and enqueues one mailbox row. `--caller-ppid` is diagnostic only; it is not trusted for identity because a live PID lookup would break death safety and PID reuse safety.

Recommended exit behavior:

- Valid metadata and an owner session resolved: exit `0`, enqueue or report `already_enqueued` for a retried handle.
- Valid metadata but no owner session resolved: exit `0`, do not enqueue, print `no_owner` in JSON/human output. This avoids retry storms for workloads that did not originate under agent-runner.
- Missing, empty, or malformed `caller_chain`: exit `64`, do not enqueue. This is a permanent producer contract violation, not a transient no-owner case.
- Sidecar/state read errors caused by corruption, permissions, or invalid DB state: exit `74`, do not enqueue.
- Same `(kind, handle)` already exists for a different `session_id`: exit `73`, report an idempotency conflict.

`agents mailbox list` is an operator/debug command. By default it lists `delivered_at IS NULL` rows for one session in delivery order. `--all` includes delivered rows. `--json` emits full rows including `seq`, `kind`, `handle`, `enqueued_at`, `delivered_at`, result paths, and delivery attempts.

## Metadata Contract

The only identity source used for ownership resolution is `meta.json`'s captured caller chain. Canonical shape:

```json
{
  "caller_chain": [
    {
      "pid": 12345,
      "starttime_ticks": 987654321,
      "boot_id": "..."
    }
  ]
}
```

The chain is nearest ancestor first. WU-B may accept aliases such as `os_pid`, `os_pid_starttime_ticks`, and `os_boot_id` for compatibility, but the proposal does not require them. `--rc` is still read from the rc file path so the CLI can verify the passed `--rc` path and store the actual completion code. If `meta.json` also contains extra spooler metadata, WU-B preserves it inside `payload_json` but does not use it for ownership.

## Death-Safe Session Resolution

Resolution is pure DB lookup over WU-A's sidecar identity rows. It never asks `/proc` whether any chain PID is alive and never falls back to `--caller-ppid` live identity checks.

Algorithm:

1. Parse `meta.json` and require a non-empty `caller_chain`.
2. Open `PidIdentityDb` read-only using `PidIdentityDb::open_default_read_only`. If the sidecar file does not exist, treat the result as `no_owner` rather than an error.
3. For each chain triple in nearest-first order, construct `ProcessIdentity { os_pid: pid, os_boot_id: boot_id, os_pid_starttime_ticks: starttime_ticks }`.
4. Call `PidIdentityDb::lookup_by_identity(&identity)`. This is keyed by `(os_pid, os_boot_id, os_pid_starttime_ticks)` and is safe after the caller is dead.
5. If no row exists, continue to the next ancestor.
6. If a row exists and `row.session_id` is present, resolve to that session immediately.
7. If `row.session_id` is absent, open `StateDb::default_path()` read-only if it exists and call `StateDb::get_invocation_by_uuid(&row.invocation_uuid)`.
8. If the invocation row exists, use `provider_session_id.or(session_id)`, matching `src-tauri/src/commands/pid_session.rs::resolved_invocation_session_id`.
9. If the state lookup does not produce a session, continue to the next ancestor.
10. Pick the first ancestor that yields a session. This is the nearest owning agent-runner session.
11. If the loop ends without a session, return `no_owner` and do not enqueue.

This handles the key failure cases:

- Dead caller: succeeds because the lookup uses only the captured triple and sidecar DB.
- PID reuse: a reused PID with a different starttime or boot id does not match the sidecar primary key.
- Cross-boot reuse: a different boot id does not match.
- Missing chain: rejected as malformed input because death-safe resolution is impossible.
- Sidecar row without session: still works when `state.db` has later captured `provider_session_id` for the invocation.

`notify` should include resolution diagnostics in JSON output: `matched_chain_index`, `matched_pid`, `owner_invocation_uuid`, `owner_session_id`, and whether the session came from `sidecar_session_id` or `state_db_invocation_join`.

## Mailbox Store

Use a new table in WU-A's existing sidecar DB file, `pid-identity.db`, rather than `oulipoly-agent-messenger` or `oulipoly-agent-store`.

Rationale:

- The mailbox is session-scoped control-plane state, not an invocation-scoped returned artifact.
- `oulipoly-agent-messenger` and `oulipoly-agent-store` are built around artifact versions and producer invocation UUIDs; they do not provide pending/delivered mailbox semantics or death-safe session ownership.
- The PID sidecar already carries the owner resolution data and is explicitly outside the versioned `state.db` migration contract.
- A single SQLite sidecar gives atomic idempotent enqueue, ordered pending reads, and delivered marking with WAL, without changing `state.db`.

Proposed sidecar schema extension in `crates/oulipoly-state/src/pid_identity.rs` or a sibling sidecar module that shares the same default path:

```sql
CREATE TABLE IF NOT EXISTS mailbox (
    seq                         INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id                  TEXT    NOT NULL,
    kind                        TEXT    NOT NULL,
    handle                      TEXT    NOT NULL,
    payload_json                TEXT    NOT NULL,
    enqueued_at                 TEXT    NOT NULL,
    delivered_at                TEXT,
    delivered_by_invocation_uuid TEXT,
    delivery_attempts           INTEGER NOT NULL DEFAULT 0,
    delivery_error              TEXT,
    owner_invocation_uuid       TEXT,
    matched_os_pid              INTEGER,
    matched_os_boot_id          TEXT,
    matched_os_pid_starttime_ticks INTEGER,
    matched_chain_index         INTEGER,
    state_dir                   TEXT    NOT NULL,
    meta_path                   TEXT    NOT NULL,
    log_path                    TEXT    NOT NULL,
    rc_path                     TEXT    NOT NULL,
    rc                          INTEGER NOT NULL,
    UNIQUE(kind, handle)
);

CREATE INDEX IF NOT EXISTS idx_mailbox_pending
    ON mailbox(session_id, delivered_at, seq);

CREATE TABLE IF NOT EXISTS session_runtime (
    session_id                  TEXT PRIMARY KEY,
    mode                        TEXT NOT NULL CHECK(mode IN ('headless', 'pty_interactive')),
    invocation_uuid             TEXT,
    provider_name               TEXT,
    model_name                  TEXT,
    pty_control_path            TEXT,
    updated_at                  TEXT NOT NULL
);
```

`session_runtime` is not required for headless v1 delivery, but it is the place to record/derive mode without touching `state.db`. `run_resume` upserts `mode='headless'` for `resolved.active_session_id`. `run_repl` upserts `mode='pty_interactive'` once `resume_session_id` is known. Existing state columns such as `session_capture_method` are insufficient because both headless resume and repl resume can use `resumed`; mode is known from the launch path, not reliably from persisted invocation rows.

Payload JSON for `agent_bash_complete` should be stable and small:

```json
{
  "schema_version": 1,
  "kind": "agent_bash_complete",
  "handle": "...",
  "rc": 0,
  "state_dir": "/...",
  "meta_path": "/.../meta.json",
  "log_path": "/.../log",
  "rc_path": "/.../rc",
  "owner": {
    "session_id": "...",
    "invocation_uuid": "...",
    "matched_chain_index": 0
  },
  "caller_chain": []
}
```

Do not inline untrusted log content into the prompt by default. Store paths and rc; the agent can inspect the log path if needed.

## Mailbox Operations

`enqueue_agent_bash_complete`:

- Opens the sidecar DB read-write and ensures the sidecar schema with `CREATE TABLE IF NOT EXISTS` statements.
- Executes one transaction.
- Inserts a row with `kind='agent_bash_complete'`, the resolved `session_id`, `handle`, result paths, rc, owner diagnostics, and `payload_json`.
- Uses `UNIQUE(kind, handle)` for retry idempotency.
- On conflict, reads the existing row. If `session_id` matches, returns `already_enqueued` without modifying `seq` or duplicating delivery. If `session_id` differs, returns an idempotency conflict.

`list_pending(session_id)`:

- Reads rows where `session_id = ?` and `delivered_at IS NULL`.
- Orders by `seq ASC`.
- Does not mutate rows.

`mark_delivered(session_id, seqs, delivered_by_invocation_uuid)`:

- Runs in one transaction after a successful delivery attempt.
- Sets `delivered_at = now`, `delivered_by_invocation_uuid = ?`, and `delivery_attempts = delivery_attempts + 1` for rows matching the session and seq list with `delivered_at IS NULL`.
- Leaves already-delivered rows unchanged so the operation is idempotent.

`list_mailbox(session_id, all)`:

- Operator command for `agents mailbox list`.
- Uses the same sidecar table and never opens `state.db` unless a future `--resolve` diagnostic flag is added.

## Headless Delivery Semantics

Headless mode is the WU-B v1 core. It implements queue-if-busy, deliver-at-next-turn semantics:

- `notify` always enqueues at completion time when an owner session resolves.
- The current one-shot headless agent is not interrupted.
- The next headless resume consumes the queue for the resolved active provider session. In this codebase that means `Subcommands::Resume` or top-level `--resume` when `resolve_top_level_resume_prompt_source` chooses the headless path into `run::run_resume`.
- `repl --resume` is interactive here (`Subcommands::Repl` into `run::run_repl`), so it is covered by the PTY plan below rather than the headless prompt-prepending path. If product requirements demand pull-on-resume delivery for `repl --resume` in v1, that requires either a PTY broker injection path or a dispatch behavior change, both outside this proposal's headless core.

Exact integration point:

- In `src-tauri/src/run/resume/orchestration.rs::prepare_headless_resume_execution`, after `resolve_resume_for_headless_execution` returns `resolved` and before returning `PreparedHeadlessResumeExecution`, call the mailbox service for `resolved.active_session_id`.
- Compose a new `answer` from the pending mailbox prefix plus the existing `answer` returned by `resolve_resume_answer(prompt, file)`.
- Store the selected mailbox `seq`s in `PreparedHeadlessResumeExecution`, alongside `answer`, so they can be marked delivered only after the resume attempt succeeds.
- In `run_resume_loop` / `run_resume_attempt`, pass the composed answer exactly as the existing code passes `input.prepared.answer.as_deref()` into `execute_resume_attempt_command`.
- Do not change `crates/oulipoly-runtime/src/executor/cli/resume_execution.rs`; it already supports optional prompt transport and large arg prompts.
- On successful completion of the attempt, after `finalize_completed_attempt` returns a successful `CompletedAttemptControl::Return(0)` and before returning from `run_resume`, call `mark_delivered` for the drained seqs with the resume invocation UUID.
- If spawn fails, quota retry occurs, terminal signal handling returns failure, or the provider exits non-zero, leave the rows pending.

Use `resolved.active_session_id` rather than the raw CLI `session_id` input. The CLI input may be a chain id or legacy session id; `resolved.active_session_id` is the provider session that will actually receive the resume payload.

Prompt composition:

```text
[OULIPOLY NOTIFICATIONS]
The following background agent-bash workloads completed while this session was inactive.

1. kind: agent_bash_complete
   handle: <handle>
   rc: <rc>
   state_dir: <state_dir>
   meta: <meta_path>
   log: <log_path>
   rc_file: <rc_path>

Use the paths above if you need details. Do not assume log content unless you inspect it.
[END OULIPOLY NOTIFICATIONS]

<existing resume answer, if any>
```

Composition rules:

- No pending mailbox and no existing answer: preserve `None`, so existing native resume-without-prompt behavior remains unchanged.
- No pending mailbox and an existing answer: preserve the answer byte-for-byte.
- Pending mailbox and no existing answer: use only the notification prefix as `Some(answer)`.
- Pending mailbox and existing answer: prepend the prefix, then a blank line and `[USER RESUME PAYLOAD]`, then the original answer.
- Respect pending order by `seq ASC`.
- Apply a v1 batch cap, for example 20 messages or 64 KiB rendered prefix, whichever comes first. Leave the remainder pending for the next resume and include a truncation line in the prefix.

This preserves existing resume behavior when the mailbox is empty and routes all prompt transport through the existing executor path.

## PTY / Interactive Delivery Plan

Current interactive execution in `crates/oulipoly-runtime/src/executor/cli/interactive.rs::execute_interactive_with_result_and_model_identity` builds a child command, sets stdin/stdout/stderr to `Stdio::inherit()`, records child PID identity, and waits. There is no retained PTY writer, broker process, or control socket that a later `agents notify` process can use to inject text.

Therefore v1 keeps PTY fully queued and documents immediate injection as a follow-up. The headless path is complete and reliable; PTY notifications remain pending unless/until a future PTY broker can forward them.

Follow-up design for forward-whenever PTY mode:

- Replace direct inherited stdio for interactive sessions with a PTY broker owned by agent-runner.
- Record `session_runtime(mode='pty_interactive', pty_control_path=..., invocation_uuid=...)` in the sidecar when `run_repl` launches or resumes a session.
- `notify` still enqueues first, preserving durability and idempotency.
- After enqueue, if `session_runtime` says `pty_interactive` and `pty_control_path` is present, `notify` sends the rendered notification envelope to the control socket.
- The broker writes the envelope to the PTY input stream and returns success/failure.
- On successful PTY injection, `notify` marks the row delivered immediately.
- On socket failure, missing runtime row, dead process, or stale runtime record, `notify` leaves the row pending and reports `queued_no_immediate_delivery`.

Mode recording/derivation:

- Headless mode is derived from dispatch through `dispatch_resume_subcommand` or `dispatch_headless_top_level_resume` into `run::run_resume`.
- Interactive mode is derived from dispatch through `dispatch_interactive_top_level_resume` or `Subcommands::Repl` into `run::run_repl`.
- Because `notify` runs in a separate process, WU-B should persist that derived mode in the sidecar `session_runtime` table. Do not infer mode from `state.db` capture columns.

## Crash Safety and Delivery Guarantees

Enqueue crash safety:

- Enqueue is a single SQLite transaction.
- A crash before commit leaves no row.
- A crash after commit leaves exactly one row.
- A retried notify with the same `(kind, handle)` returns the existing row and does not double-enqueue.

Resume delivery crash safety:

- Listing pending rows does not mutate them.
- Rows are marked delivered only after the provider resume attempt completes successfully.
- If agent-runner crashes before marking delivered, rows remain pending and are delivered again on the next resume.
- This is at-least-once delivery. Duplicate notification is possible after a crash where the provider saw the prompt but agent-runner crashed before marking delivered. The stable `handle` is included so the agent can deduplicate semantically.
- If the provider exits non-zero, spawn fails, or quota rotation retries, rows remain pending.
- If a mark-delivered transaction partially fails, SQLite rollback preserves pending state.

Ordering:

- `seq INTEGER PRIMARY KEY AUTOINCREMENT` defines total enqueue order.
- Delivery reads `ORDER BY seq ASC` per session.
- Retries keep the original `seq`.
- Rows beyond a batch cap remain pending and retain order for later resumes.

Concurrent resumes:

- V1 assumes one headless resume per provider session at a time, matching normal usage.
- If two resumes concurrently drain the same session, at-least-once semantics may duplicate delivery. The delivered marking is idempotent, so no data corruption occurs.
- If stricter exactly-once-ish behavior becomes necessary, add sidecar claim columns later (`claimed_by_invocation_uuid`, `claimed_at`) with stale-claim recovery. Do not add this complexity to v1 unless tests demonstrate concurrent resumes are common.

## Test Plan

All CLI/integration tests must set `XDG_DATA_HOME` and `XDG_CONFIG_HOME` to a `tempfile::TempDir`, following the pattern in `src-tauri/tests/age_pid_sidecar_cli.rs`. Tests must assert that no default user data path is touched.

Notify resolution and enqueue tests:

- `notify_resolves_nearest_ancestor_sidecar_session`: seed sidecar rows for multiple fake dead process identities; make `caller_chain` nearest first; verify the nearest row with `session_id` wins and one mailbox row is inserted.
- `notify_resolves_from_state_when_sidecar_session_null`: seed sidecar with `session_id = NULL`, seed `state.db` invocation with `provider_session_id`, run notify, verify mailbox `session_id` is the provider session.
- `notify_works_after_caller_dead`: use fake PIDs and boot/start triples not present in `/proc`; verify resolution succeeds purely from sidecar DB.
- `notify_rejects_reuse_mismatch`: seed sidecar with same pid and boot id but different `starttime_ticks`; notify with mismatched triple; verify `no_owner` and no mailbox row.
- `notify_no_owner_valid_chain_returns_zero`: valid chain with no matching sidecar rows exits `0`, reports `no_owner`, and does not enqueue.
- `notify_missing_chain_is_usage_error`: missing, empty, or malformed `caller_chain` exits `64` and does not enqueue.
- `notify_idempotent_retried_handle`: run the same notify twice; verify one row, original `seq`, second output says `already_enqueued`.
- `notify_handle_conflict_different_session`: preinsert same `(kind, handle)` for another session; notify resolves to a different session; verify exit `73` and no mutation.
- `notify_ordering_by_seq`: enqueue handles A, B, C and verify pending list order is A, B, C.
- `mailbox_isolation`: enqueue for sessions A and B; `agents mailbox list --session-id A` returns only A.

Headless resume drain tests:

- `resume_with_pending_mailbox_prepends_notifications`: seed a resumable chain/session and mailbox rows, run `agents resume --session-id <sid> --prompt <payload>` against a fixture provider that records received stdin/argv, and assert notifications precede the original payload.
- `resume_without_mailbox_preserves_payload`: run the same fixture with no pending mailbox and assert the provider receives exactly the original prompt.
- `resume_without_mailbox_and_without_prompt_preserves_native_resume`: run resume without prompt or file and no mailbox; assert provider receives resume args and no prompt payload.
- `resume_with_only_mailbox_sends_notification_prompt`: no user prompt, pending mailbox exists; assert provider receives the notification envelope as the prompt.
- `resume_marks_delivered_after_success`: successful fixture provider exits `0`; assert `delivered_at` and `delivered_by_invocation_uuid` are set.
- `resume_failure_leaves_pending`: fixture provider exits non-zero or fails spawn; assert `delivered_at IS NULL`.
- `resume_redelivers_after_unmarked_crash_simulation`: list pending and compose a batch without calling mark-delivered, then run resume again; assert rows are still delivered on the next successful resume. This can be a lower-level mailbox unit test if simulating process crash in integration is too heavy.
- `resume_drains_in_order_and_respects_batch_cap`: seed more than the cap; assert first batch order and remaining rows stay pending.
- `resume_uses_resolved_active_session_id`: resume by chain id; assert drain uses `resolved.active_session_id`, not the raw CLI string.

Mailbox store unit tests:

- `enqueue_transaction_rollback_has_no_partial_row`: inject an error inside the enqueue transaction before commit and verify no row is visible.
- `mark_delivered_is_idempotent`: call mark-delivered twice for the same seq and verify no error and stable delivered state.
- `list_pending_excludes_delivered`: mark one row delivered and verify pending only returns the remaining rows.

PTY tests for v1:

- No immediate injection test is required in WU-B v1 because PTY injection is a documented follow-up.
- Add a pending/future test note for a PTY broker: notify enqueues first, successful broker injection marks delivered, broker failure leaves pending.

## Proof plan

| Runtime claim | Proof method | Evidence-class match |
|---|---|---|
| PID identity is captured at provider-child spawn without changing `state.db`. | `crates/oulipoly-runtime/tests/age_pid_sidecar_spawn.rs::spawn_capture_writes_verified_sidecar_row_without_state_schema_change`. | The test drives `RuntimeExecutorService` through the production executor spawn path against a real fixture child process, then reads the sidecar DB row and separately snapshots `state.db` schema. The fixture provider is setup; the asserted sidecar write is produced by the runtime spawn path, not by direct seeding. |
| Death-safe owner resolution comes from `meta.json caller_chain`, not live PID probing or `--caller-ppid`. | `src-tauri/tests/wu_b_mailbox_integration.rs::notify_resolves_nearest_ancestor_sidecar_session`, `notify_resolves_from_state_when_sidecar_session_null`, `notify_works_after_caller_dead`, and `notify_rejects_reuse_mismatch`. | These tests invoke the compiled `oulipoly-agent-runner` notify command with XDG-isolated sidecar/state files and captured caller-chain metadata. The fake/dead PID and mismatch cases prove the production CLI reads persisted identity triples and state fallback rows rather than asking `/proc` or trusting a proxy identity. |
| Mailbox enqueue is idempotent, drains on headless resume, and marks rows delivered only after a successful provider attempt. | `src-tauri/tests/wu_b_mailbox_integration.rs::notify_idempotent_retried_handle`, `resume_with_pending_mailbox_prepends_notifications`, `resume_marks_delivered_after_success`, `resume_failure_leaves_pending`, `resume_drains_in_order_and_respects_batch_cap`, and `resume_uses_resolved_active_session_id`. | The tests invoke the compiled runner for notify, mailbox, and resume commands, with real SQLite sidecar tables and fixture provider scripts that receive the actual prompt transport. Seed rows are setup only; the drain, prompt envelope, success marking, failure non-marking, batch cap, and active-session selection are asserted through the production resume path. |
| Proactive wake performs idle wake, turn-end recheck, single-flight coordination, and bounded auto-wake chaining. | `src-tauri/tests/wu_d_proactive_wake_integration.rs::idle_wake_delivers`, `busy_then_turn_end_delivers`, `concurrent_notify_single_flight`, `manual_resume_race_is_safe`, `batch_cap_followup_wake`, `auto_wake_cap_stops_self_replicating_session`, and `no_undelivered_no_wake_and_loop_terminates`. `concurrent_notify_single_flight` asserts exactly one notify response reports `wake.status="spawned"`, all non-null wake claim tokens collapse to one token, and the provider-side wake launch log has exactly one entry. | These tests run the compiled runner with isolated XDG/HOME, fixture providers, real notify/resume subprocesses, and the sidecar wake-claim/runtime tables. The single-flight test now observes the contended notify wake diagnostics plus a provider-side detached wake launch artifact, so the mechanism is proven by exactly one claim winner and exactly one wake child rather than only by final delivered rows. |

## Risks

- Prompt injection risk: result paths and handles come from an external spooler. The delivery envelope should quote paths, avoid inline log content, and clearly label the block as system notification metadata.
- At-least-once duplicates: a crash after the provider sees the prompt but before `mark_delivered` causes redelivery. The handle is stable and should be included in every envelope so the agent can deduplicate.
- Large mailbox prompts: many completions can bloat the resume payload or hit provider arg limits. Use a batch cap and leave the rest pending.
- Session rotation: mailbox is keyed by provider session. If future workflows need chain-level delivery across migrated active sessions, add an explicit chain alias/index rather than overloading session keys.
- PTY gap: current interactive execution has no writable PTY handle after launch. Immediate injection needs a broker/control socket and should not be faked with unsafe `/proc` or TTY writes.
- Sidecar schema evolution: `pid-identity.db` is not versioned like `state.db`; use additive `CREATE TABLE IF NOT EXISTS` and compatibility tests.
- Result path lifetime: the spooler must retain `state_dir`, `log`, and `rc` until delivery. WU-B stores paths and rc, not copied logs.
- Concurrent headless resumes can duplicate notifications. V1 accepts this under at-least-once semantics; add claims only if concurrency becomes a real issue.

## 12-Line Summary

1. Add `agents notify agent-bash-complete` to receive spooler completions.
2. Resolve ownership from `meta.json caller_chain`, not live PIDs.
3. Match `(pid, boot_id, starttime_ticks)` against WU-A `pid_identity` rows.
4. Pick the nearest ancestor that yields a provider session.
5. Fall back from sidecar `session_id` to read-only `state.db` invocation lookup.
6. Store mailbox rows in `pid-identity.db`, never in versioned `state.db`.
7. Enqueue is idempotent by `(kind, handle)`.
8. Headless v1 delivers at the next successful resume.
9. Resume prepends notification envelopes to the existing resume payload.
10. Empty mailbox preserves current resume behavior exactly.
11. PTY immediate injection is a follow-up requiring a PTY broker/control socket.
12. Tests isolate `XDG_DATA_HOME` and cover resolution, idempotency, ordering, draining, and crash safety.
