# WU-D Proposal: Proactive Wake + End-to-End Dogfood

## Scope

WU-A records process identity for agent-runner-owned provider invocations. WU-B stores `agent-bash` completions in a per-session mailbox and drains that mailbox on the next headless resume. WU-D closes the remaining one-shot gap: after a background workload completes, agent-runner should wake the owning idle headless session without waiting for a human or another agent turn to issue `agents resume`.

This proposal is design only. It does not implement code.

Hard rules:

- No versioned `state.db` schema change.
- All new persistence lives in the existing sidecar DB, `pid-identity.db`, under `XDG_DATA_HOME/oulipoly-agent-runner/`.
- Tests must isolate `XDG_DATA_HOME`, `XDG_CONFIG_HOME`, and `HOME`.
- The agent must not poll `/proc` to determine whether it is idle or busy.
- Runner-side liveness checks may verify a recorded PID with the WU-A three-part identity: `os_pid`, `os_boot_id`, and `os_pid_starttime_ticks`.
- Mailbox delivery remains at-least-once; delivered marking prevents normal re-delivery, but crash windows can still duplicate a notification envelope.

## Existing Seams

WU-B already provides the durable mailbox pieces:

- `src-tauri/src/commands/notify.rs` resolves `meta.json caller_chain` to the owning provider session and enqueues one `mailbox` row.
- `crates/oulipoly-state/src/mailbox.rs` owns the additive sidecar `mailbox` table and the current `session_runtime` table.
- `src-tauri/src/mailbox_delivery.rs` renders pending mailbox rows into an `[OULIPOLY NOTIFICATIONS]` prefix.
- `src-tauri/src/run/resume/orchestration.rs::prepare_headless_resume_execution` calls `prepare_headless_resume_delivery` after resume resolution and before provider spawn.
- `src-tauri/src/run/resume/orchestration.rs::handle_resume_attempt_terminal_signal` calls `mark_headless_resume_delivered` after a successful completed resume attempt.
- `/home/nes/projects/agent-bash-tool/worktrees/wu-c-spooler-core/src/delivery.rs` shells `agents notify agent-bash-complete --meta <meta>` using `AGENT_BASH_AGENT_RUNNER_BIN` or `PATH`.

WU-D should use those seams rather than inventing a second delivery path. The proactive wake is just a detached `agents resume --session-id <owner>` with no prompt. WU-B then composes the notification prompt during normal resume preparation.

## Sidecar Runtime State

WU-D extends sidecar runtime state. It must not write to `state.db` for liveness, wake claims, or auto-wake counters.

Keep WU-B's existing `session_runtime` role as the current per-session mode/runtime row and add only sidecar-compatible fields. Existing sidecar files can be upgraded by probing `PRAGMA table_info(session_runtime)` and issuing additive `ALTER TABLE` statements when a column is missing.

Recommended sidecar additions:

```sql
-- Existing table, shown with WU-D fields included.
CREATE TABLE IF NOT EXISTS session_runtime (
    session_id                       TEXT PRIMARY KEY,
    mode                             TEXT NOT NULL CHECK(mode IN ('headless', 'pty_interactive')),
    invocation_uuid                  TEXT,
    provider_name                    TEXT,
    model_name                       TEXT,
    pty_control_path                 TEXT,
    updated_at                       TEXT NOT NULL,

    run_state                        TEXT NOT NULL DEFAULT 'idle',
    running_invocation_uuid          TEXT,
    running_os_pid                   INTEGER,
    running_os_boot_id               TEXT,
    running_os_pid_starttime_ticks   INTEGER,
    turn_started_at                  TEXT,
    turn_ended_at                    TEXT,
    turn_start_max_mailbox_seq       INTEGER,
    last_exit_code                   INTEGER,
    models_dir                       TEXT,
    effective_cwd                    TEXT
);

CREATE TABLE IF NOT EXISTS session_wake_claim (
    session_id                       TEXT PRIMARY KEY,
    claim_token                      TEXT NOT NULL,
    claimed_at                       TEXT NOT NULL,
    wake_pid                         INTEGER,
    wake_invocation_uuid             TEXT,
    reason                           TEXT NOT NULL,
    auto_wake_count                  INTEGER NOT NULL,
    min_pending_seq_at_claim         INTEGER,
    max_pending_seq_at_claim         INTEGER
);

CREATE INDEX IF NOT EXISTS idx_session_wake_claim_claimed_at
    ON session_wake_claim(claimed_at);
```

If a fresh sidecar creates `session_runtime`, include all fields in the `CREATE TABLE`. If an older WU-B sidecar already has the smaller table, add columns by compatibility code. SQLite cannot add every desired `CHECK` constraint after table creation, so code should validate `run_state` values even if an upgraded table has no SQL-level check.

The `session_wake_claim` table is separate because it has different lifecycle semantics from liveness. A session can be idle while a wake is claimed but not yet started. A stale claim can be stolen without mutating the runtime liveness row.

## Idle And Busy Detection

Definition: a provider session is busy when agent-runner has a non-stale sidecar runtime row for that `session_id` with `run_state='running'`, a `running_invocation_uuid`, and a live process whose current three-part identity matches the stored `running_os_pid`, `running_os_boot_id`, and `running_os_pid_starttime_ticks`.

Definition: a provider session is idle when any of these is true:

- There is no `session_runtime` row for the session.
- The row exists and `run_state='idle'`.
- The row says `run_state='running'`, but the recorded PID is no longer live with the same three-part identity; this is a stale runtime row and should be atomically changed to `idle` before returning idle.
- The row says `run_state='running'`, but `running_invocation_uuid` no longer matches the active running row being finalized; the finalizer must not clear a newer invocation's runtime state.

Agent code does not self-poll. The only `/proc` verification is inside agent-runner when `notify` or the wake coordinator evaluates sidecar liveness. That verification uses WU-A's PID reuse guard:

1. Read the live identity for `running_os_pid` with `pid_identity::read_live_process_identity`.
2. If no live identity exists, the runtime row is stale.
3. If the live identity differs in boot id or starttime ticks, the PID was reused and the runtime row is stale.
4. If all three fields match, the session is busy.

Runtime state updates:

- On provider child spawn, after WU-A records the child identity in `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs::record_child_identity`, also mark the owning session `running` when `SpawnIdentityContext.session_id` is present.
- The running update records `mode`, `invocation_uuid`, provider/model names, child PID identity, `turn_started_at`, `turn_start_max_mailbox_seq`, optional `models_dir`, and optional effective cwd.
- On provider process exit and normal finalization, mark the same session `idle` with `turn_ended_at` and `last_exit_code`.
- The idle update must be conditional on `(session_id, running_invocation_uuid)` so an old finalizer cannot clear a newer turn.
- Spawn errors should mark idle when a runtime row was created before the error. If no child identity was recorded, there may be no runtime row to clear.
- Process crashes are recovered by the stale identity check on the next notify/wake/liveness query.

Known-session limitation:

- `run_resume` always has a provider session id, so it can mark liveness precisely.
- A new headless `run` only has a session id at spawn if the provider supports start-known session capture, such as `forced_flag_verified`. If a new unpinned session dispatches `agent-bash` and the background job completes before the first provider session id is captured into `state.db`, WU-B can still return `no_owner`. WU-D does not solve pre-session ownership buffering.
- The dogfood proof should use an existing session or a fixture provider with start-known session capture so the owner is known at spawn.

## Wake Trigger A: Notify When Idle

Trigger A runs after `notify agent-bash-complete` successfully enqueues or idempotently observes an existing row.

Algorithm:

1. Finish the WU-B enqueue transaction first. A wake must never launch for data that was not durably committed.
2. Open the sidecar mailbox DB and list pending rows for the resolved owner session.
3. If no `delivered_at IS NULL` rows exist, do not wake. This handles duplicate `already_enqueued` notifications for rows already delivered.
4. Evaluate session liveness from `session_runtime` and the recorded PID identity.
5. If the session is busy, do not wake. The turn-end recheck handles the pending rows when the active turn exits.
6. If the session is idle, attempt to acquire a single-flight wake claim for that session.
7. If another non-stale claim exists, do not wake. The in-flight wake is responsible for draining all currently pending rows.
8. If the claim is acquired, spawn a detached wake resume and return from `notify` without waiting for the resume to finish.

The wake command shape is:

```text
<current agents binary> resume --session-id <owner-session-id> [--model <model-name>] [--models-dir <models-dir>]
```

Do not pass `--prompt` or `--file`. The mailbox prefix is generated by `prepare_headless_resume_delivery` inside the resumed process. If `model_name` is known from `session_runtime` or the owner PID identity row, pass `-m <model_name>` to avoid ambiguous model inference. If the original invocation used a non-default models directory, record it in `session_runtime.models_dir` and pass `--models-dir <models_dir>`.

Detached launch requirements:

- Use `std::env::current_exe()` from the `notify` process, not a string lookup of `agents`, so `AGENT_BASH_AGENT_RUNNER_BIN=/path/to/test-or-installed/agents` is honored transitively.
- Inherit `XDG_DATA_HOME`, `XDG_CONFIG_HOME`, `HOME`, and normal provider environment so the wake sees the same sidecar, config, and credentials.
- Set `OULIPOLY_AUTO_WAKE=1`, `OULIPOLY_AUTO_WAKE_SESSION_ID=<session>`, `OULIPOLY_AUTO_WAKE_TOKEN=<claim-token>`, and `OULIPOLY_AUTO_WAKE_COUNT=<n>` in the child environment.
- Remove `OULIPOLY_PARENT_INVOCATION` from the child environment so the wake resume is not incorrectly parented to the spooler notify process.
- Set stdin, stdout, and stderr to null by default. Optional future debug logging can write to a sidecar wake log, not to the producer's stdio.
- On Unix, detach with `setsid` or an equivalent command setup so the wake child does not share the producer's process group or terminal.
- On Windows, use the platform equivalent detached process flags.
- `notify` must not wait for the provider resume. It may wait only for a tiny launcher setup step if a double-fork helper is used to avoid zombies.
- Record `wake_pid` in `session_wake_claim` when available. This makes the process detached from stdio, but not untracked by the control plane.

Do not reuse `agent-bash` to launch wake resumes. Reusing `agent-bash` would produce another background workload with its own completion notification and can create feedback loops. A runner-owned detached spawn is smaller and keeps wake lifecycle state in one sidecar.

Suggested notify JSON additions for operator diagnostics:

```json
{
  "wake": {
    "attempted": true,
    "status": "spawned | busy | no_pending | already_in_flight | spawn_error",
    "claim_token": "...",
    "wake_pid": 12345
  }
}
```

The spooler currently suppresses notify stdout/stderr, so this is primarily useful for direct CLI debugging and tests.

## Wake Trigger B: Turn-End Recheck

Trigger B runs after a headless turn exits and the runner has marked that session idle.

Exact hook points:

- Resume path: in `src-tauri/src/run/resume/orchestration.rs`, after `CompletedAttemptControl::Return(exit_code)` for a successful completed attempt, after `mark_headless_resume_delivered`, and before returning from the attempt/loop.
- New headless run path: in `src-tauri/src/run/balancing/finalization.rs`, after successful session ingestion has produced or confirmed a provider session id, after final invocation persistence, and before returning the final success output path.
- Spawn-error and failed-completion paths should mark runtime idle but should not auto-wake by default. This avoids an automatic failure loop. Pending mailbox rows remain durable for the next manual or successful automatic resume.

Trigger B algorithm:

1. Mark the session idle using the completed invocation UUID as a compare key.
2. If this attempt delivered mailbox rows, commit `mark_headless_resume_delivered` before checking for more pending rows.
3. List pending rows for the active provider session.
4. If no undelivered rows remain, release the wake claim if this was an auto-wake and return normally.
5. If pending rows remain and the consecutive auto-wake count is below the cap, try to acquire or renew the session wake claim and spawn another detached `agents resume --session-id <owner>`.
6. If the cap is reached, leave rows pending, release the claim, and emit a diagnostic marker to stderr or a sidecar wake log.

Why a recheck is required:

- If a background workload completes while the owner is mid-turn, Trigger A observes `busy` and does not wake.
- When that turn exits, Trigger B observes the newly pending rows and launches the wake resume.
- If a wake resume drains rows and no new rows appear during that wake turn, the post-delivery pending check is empty and the wake chain terminates.
- If a wake resume itself dispatches more background work and those workloads finish before the wake turn exits, Trigger B may schedule another wake, bounded by the cap.
- If a wake resume drains only a batch cap, such as WU-B's 20-row limit, Trigger B can schedule the next wake to drain the remainder until empty or capped.

The `turn_start_max_mailbox_seq` field is diagnostic and useful for tests, but eligibility should be "pending exists after delivered marking" rather than strictly "seq greater than turn start." Delivered marking and batch caps are enough to prevent re-delivery, and strict high-water logic would strand old rows left by a batch cap.

## Loop, Storm, And Concurrency Safety

Only wake on undelivered rows:

- Every wake decision must re-read `mailbox WHERE session_id=? AND delivered_at IS NULL` inside or immediately before the claim transaction.
- A duplicate notify for an already delivered handle must not spawn a wake.
- A turn-end recheck after a wake that delivered all rows must not spawn another wake.

Single-flight per session:

- `session_wake_claim.session_id` is the single-flight key.
- Claim acquisition is a transaction: read pending rows, check liveness, insert or replace only if no claim exists or the existing claim is stale.
- Claim staleness is time-bounded, for example `claimed_at` older than 10 minutes, and can also be confirmed by `wake_pid` no longer matching a live process identity if a wake PID identity was recorded.
- Manual resumes are allowed. If a manual turn starts while a wake claim exists, the wake child should validate the claim token and liveness before executing. If it finds the session busy, it exits and releases its claim; the manual turn-end recheck handles pending rows.

Consecutive auto-wake cap:

- Use `OULIPOLY_AUTO_WAKE_COUNT` as the chain counter.
- Default cap: 5 consecutive auto-wakes for the same session.
- Tests may override with `OULIPOLY_AUTO_WAKE_MAX`.
- A manual headless run/resume resets the chain.
- A notify-triggered wake from an idle session starts a new chain at count 1.
- A turn-end recheck spawned by an auto-wake increments the count.
- When the cap is reached, leave rows pending and report `auto_wake_cap_reached`; do not delete mailbox rows.

Claim lifecycle:

- Trigger A or B creates the claim before spawning the wake child.
- The wake child validates `OULIPOLY_AUTO_WAKE_TOKEN` before it starts provider execution.
- The wake child releases the claim at normal completion after the turn-end recheck decides not to schedule a successor.
- If a successor wake is scheduled, the claim can be renewed with a new token and incremented count before the current child exits.
- If spawn fails, release the claim immediately and leave mailbox rows pending.
- If the process crashes, a later notify/manual run can steal the stale claim after TTL.

At-least-once mailbox safety:

- WU-B already lists pending rows without mutating them and marks delivered only after successful resume completion.
- Concurrent wakes can still duplicate prompts if a crash occurs after the provider sees a prompt but before delivered marking. This is accepted at-least-once behavior.
- The notification envelope includes stable handles so the agent can deduplicate semantically.

Busy completion race:

- If notify enqueues while a turn is running, it does not wake.
- If the active turn exits between liveness check and claim, the notify may skip wake. Trigger B covers the pending row.
- If notify sees idle and claims, but a manual resume starts before the wake child, the wake child exits on busy/invalid claim and leaves rows pending. The manual turn-end recheck covers the pending row.

Rows produced by wake-resume itself:

- If the wake-resume dispatches new background tasks and they finish after the wake turn exits, their notify call sees the session idle and starts a new Trigger A chain.
- If they finish before the wake turn exits, Trigger A sees busy and skips, and Trigger B may schedule another wake.
- The cap bounds pathological self-replication. A provider can still create more background work, but WU-D will not resume forever in one autonomous chain.

## End-To-End Wiring And Install Step

This feature only works in the real environment once the installed `agents` binary contains all WU-A, WU-B, and WU-D behavior:

- WU-A: records provider child PID identity at spawn.
- WU-B: handles `notify agent-bash-complete`, enqueues mailbox rows, and drains on resume.
- WU-D: records session liveness, triggers wake-on-idle, and performs turn-end rechecks.

There is no `state.db` migration. Installing the binary is safe with respect to the versioned state schema. Sidecar schema additions are additive and live in `pid-identity.db`.

Build and install plan:

```bash
cargo build --release -p oulipoly-agent-runner
install -d "$HOME/.local/bin"
if [ -x "$HOME/.local/bin/agents" ]; then cp -a "$HOME/.local/bin/agents" "$HOME/.local/bin/agents.pre-wu-d.$(date +%Y%m%d%H%M%S)"; fi
install -m 0755 target/release/oulipoly-agent-runner "$HOME/.local/bin/agents"
"$HOME/.local/bin/agents" session schema-probe
```

`agent-bash` invocation path:

- `agent-bash` already shells `agents notify agent-bash-complete --meta <meta>` from its spooler delivery seam.
- It selects the binary from `AGENT_BASH_AGENT_RUNNER_BIN` when set.
- Otherwise it resolves `agents` from `PATH`.
- For dogfood and controlled installs, export `AGENT_BASH_AGENT_RUNNER_BIN="$HOME/.local/bin/agents"` in the agent environment.
- If agents are launched with a sanitized environment, also ensure `$HOME/.local/bin` is on `PATH` or explicitly set `AGENT_BASH_AGENT_RUNNER_BIN` in the provider config/session environment.

## Dogfood Demonstration

The dogfood proof has two layers: a deterministic XDG-isolated fixture proof and a real installed-environment proof.

### Deterministic Fixture Proof

Goal: exercise the real process chain without a remote model:

```text
agents -m fixture-model --models-dir <fixture-models-dir> "dispatch background work"
  -> fixture provider process starts under agent-runner
  -> WU-A records provider PID identity with owner session id
  -> fixture provider calls agent-bash run -- <cmd>
  -> fixture provider exits, runner marks session idle
  -> agent-bash supervisor finishes workload
  -> agent-bash calls agents notify agent-bash-complete --meta <meta>
  -> notify resolves caller_chain to the owner session and enqueues
  -> Trigger A sees idle and launches detached agents resume --session-id <owner>
  -> WU-B drains mailbox into the resume prompt
  -> fixture provider receives [OULIPOLY NOTIFICATIONS]
  -> runner marks mailbox row delivered
```

Fixture setup:

```bash
tmp=$(mktemp -d)
export XDG_DATA_HOME="$tmp/data"
export XDG_CONFIG_HOME="$tmp/config"
export HOME="$tmp/home"
export AGENT_BASH_AGENT_RUNNER_BIN="/absolute/path/to/target/debug/oulipoly-agent-runner"
export PATH="/absolute/path/to/agent-bash/target/debug:$PATH"
mkdir -p "$XDG_CONFIG_HOME/oulipoly-agent-runner/models" "$HOME"
```

Use a fixture provider script configured with start-known session capture, for example `forced_flag_verified` with `flag = "--session-id"`. The script should accept the injected session id, print the expected session-start JSON/event for capture, and write every received prompt to a known file.

Initial fixture turn behavior:

```text
1. Parse --session-id <sid> from argv.
2. Emit the provider session capture event for <sid>.
3. Call: agent-bash run -- sh -c 'sleep 1; printf dogfood-idle-wake-ok'.
4. Record the returned agent-bash handle/state paths for the test.
5. Exit 0 without waiting for the workload.
```

Auto-resume fixture behavior:

```text
1. Parse --resume <sid> and the final prompt argument.
2. Write the final prompt argument to $tmp/resumed-input.txt.
3. Exit 0.
```

Assertions for idle-wake proof:

- `resumed-input.txt` exists within a bounded timeout.
- `resumed-input.txt` starts with `[OULIPOLY NOTIFICATIONS]`.
- It contains `kind: agent_bash_complete`, the workload handle, `rc: 0`, `state_dir:`, `meta:`, `log:`, and `rc_file:`.
- `agents mailbox list --session-id <sid> --all --json` returns one row for the handle.
- That row has `delivered_at != null` and `delivered_by_invocation_uuid != null`.
- `agents mailbox list --session-id <sid> --json` returns no pending rows.
- `pid-identity.db` has `session_runtime.run_state='idle'` for the session after completion.
- `session_wake_claim` has no non-stale claim for the session.
- `$HOME/.local/share/oulipoly-agent-runner` and `$HOME/.config/oulipoly-agent-runner` do not exist; all state stayed in the XDG temp dirs.

### Multi-Task Busy Variant

Goal: prove Trigger A queues while busy and Trigger B wakes at turn end.

Initial fixture turn behavior:

```text
1. Parse and emit the provider session id.
2. Dispatch three workloads:
   agent-bash run -- sh -c 'sleep 0.1; printf task-a'
   agent-bash run -- sh -c 'sleep 0.2; printf task-b'
   agent-bash run -- sh -c 'sleep 0.3; printf task-c'
3. Sleep for 1 second before exiting, keeping the owner session busy while all three notify calls happen.
4. Exit 0.
```

Expected behavior:

- Each notify enqueues a mailbox row.
- Each notify sees `run_state='running'` with a live matching PID identity and does not spawn a wake.
- At turn end, the runner marks the session idle and Trigger B sees three undelivered rows.
- Trigger B launches one detached resume.
- The auto-resume prompt contains all three handles in `seq ASC` order.
- All three rows become delivered by the wake resume invocation.
- Only one wake claim is active at a time.

Assertions:

- The initial turn's stderr/log records no wake spawned by Trigger A while busy.
- `resumed-input.txt` contains exactly one notification block with all three handles.
- The handle order in the prompt matches mailbox `seq` order.
- `mailbox list --all` reports three delivered rows and zero pending rows.
- The wake count for the chain is 1.

### Real Installed Dogfood

After installing the WU-D binary to `~/.local/bin/agents`:

```bash
export AGENT_BASH_AGENT_RUNNER_BIN="$HOME/.local/bin/agents"
export PATH="$HOME/.local/bin:$PATH"
```

Use an existing real provider session and ask it to dispatch a background workload without waiting:

```text
Run this command and then stop without polling for completion:
agent-bash run -- sh -c 'sleep 5; printf wu-d-dogfood-ok'
```

Expected proof points:

- The initial provider turn exits normally.
- The `agent-bash` state directory records delivery attempted with exit code 0.
- `agents mailbox list --session-id <owner> --all --json` shows an `agent_bash_complete` row.
- The row becomes delivered after the detached wake resume.
- The resumed turn's input/transcript contains the `[OULIPOLY NOTIFICATIONS]` envelope with the handle and result paths.
- The agent can inspect the `log` path from the envelope and observe `wu-d-dogfood-ok`.

For real dogfood, use the provider transcript locator or provider-native transcript file to confirm the resumed turn input contains the notification. If the provider does not expose prompts cleanly, use the deterministic fixture proof for the prompt assertion and the real dogfood for operational confidence.

## Test Plan

All tests must set isolated `XDG_DATA_HOME`, `XDG_CONFIG_HOME`, and `HOME`. Tests must assert default user paths are untouched.

Sidecar liveness unit tests:

- `runtime_mark_running_records_pid_identity`: mark a session running with a fake/live identity and verify the sidecar row fields.
- `runtime_mark_idle_is_invocation_guarded`: a stale invocation UUID cannot clear a newer running row.
- `liveness_live_matching_identity_is_busy`: a runtime row with the current process identity reports busy.
- `liveness_dead_or_reused_identity_is_idle_and_cleared`: a dead PID or mismatched starttime reports idle and changes the stale row to idle.
- `runtime_schema_additions_do_not_touch_state_db`: opening/upgrading mailbox runtime state leaves `PRAGMA user_version` and `state.db` columns unchanged.

Wake coordinator unit tests:

- `wake_no_undelivered_no_claim_no_spawn`: no pending mailbox rows means no claim and no child launch.
- `wake_idle_pending_acquires_claim`: pending rows plus idle liveness creates one claim with min/max pending seq.
- `wake_busy_pending_skips_claim`: pending rows plus live running identity does not claim.
- `wake_existing_claim_is_single_flight`: a second wake attempt for the same session does not spawn while the claim is fresh.
- `wake_stale_claim_can_be_stolen`: stale claim is replaced and a new wake can launch.
- `wake_spawn_failure_releases_claim`: spawn failure leaves mailbox pending and removes/marks the claim failed.

XDG-isolated integration tests:

- `idle_wake_delivers`: deterministic fixture provider dispatches one `agent-bash` workload, exits, notify wakes the idle session, the resumed input contains the notification, and the mailbox row is delivered.
- `busy_then_turn_end_delivers`: fixture provider dispatches three short workloads and stays busy until all notify calls happen; no Trigger A wake occurs; Trigger B launches one wake; all three rows are delivered.
- `no_undelivered_no_wake`: run notify idempotently after a row was already delivered or run turn-end recheck with no pending rows; assert no wake claim and no resume child.
- `loop_terminates_when_no_new_rows`: a wake resume drains pending rows and dispatches no new workloads; assert no successor wake is spawned.
- `auto_wake_cap_stops_self_replicating_session`: fixture provider dispatches a new completing workload on every auto-resume; with `OULIPOLY_AUTO_WAKE_MAX=2`, assert two wakes occur, the cap diagnostic is recorded, and remaining rows stay pending.
- `concurrent_notify_single_flight`: launch two or more `notify agent-bash-complete` processes for the same idle session concurrently; assert one wake claim, one wake child, all rows delivered, and no duplicate sidecar corruption.
- `manual_resume_race_is_safe`: acquire a wake claim, start a manual resume before the wake child starts, assert the wake child exits without delivery, manual turn-end recheck delivers pending rows.
- `batch_cap_followup_wake`: seed more rows than WU-B's batch cap, launch wake, assert first batch delivered and a bounded successor wake drains the remainder.
- `xdg_isolation`: each integration test asserts no default `$HOME/.local/share/oulipoly-agent-runner` or `$HOME/.config/oulipoly-agent-runner` was created.

Existing WU-B tests remain relevant and should continue to pass:

- Notify resolution, idempotency, no-owner, malformed metadata, and conflict tests.
- Resume prompt composition and delivered marking tests.
- Batch cap and resolved active session id tests.

## Proof plan

| Runtime claim | Proof method | Evidence-class match |
|---|---|---|
| Notify-triggered idle wake launches a detached resume that drains the mailbox and records delivery. | `src-tauri/tests/wu_d_proactive_wake_integration.rs::idle_wake_delivers`. | Invokes the compiled runner with isolated XDG/HOME, a fixture provider script, real sidecar state, and production notify/resume subprocesses; the resumed prompt and delivered mailbox row are runtime artifacts. |
| Busy sessions defer wake until turn-end recheck, with one wake chain for queued work. | `src-tauri/tests/wu_d_proactive_wake_integration.rs::busy_then_turn_end_delivers`. | The provider remains live while notify calls run, so liveness and turn-end recheck are exercised through production runtime state rather than seeded-only assertions. |
| Wake claims are single-flight and safe against manual-race overlap. | `src-tauri/tests/wu_d_proactive_wake_integration.rs::concurrent_notify_single_flight` and `manual_resume_race_is_safe`. `concurrent_notify_single_flight` asserts exactly one notify response reports `wake.status="spawned"`, all non-null wake claim tokens collapse to one token, and the provider-side wake launch log has exactly one entry. | Concurrent compiled-runner notify/resume processes contend on the real `session_wake_claim` table; the assertions observe the wake diagnostic claim token artifact, final claim cleanup, delivered rows, and a detached wake-child launch log. This proves one claim winner and one wake child for concurrent notify rather than only eventual delivery. |
| Auto-wake chains terminate on no pending rows, respect batch follow-up, and stop at the configured cap. | `src-tauri/tests/wu_d_proactive_wake_integration.rs::no_undelivered_no_wake_and_loop_terminates`, `batch_cap_followup_wake`, and `auto_wake_cap_stops_self_replicating_session`. | The tests set production auto-wake environment variables and observe real detached resume behavior, pending/delivered mailbox rows, and cap diagnostics through sidecar state. |

## Risks And Follow-Ups

- New sessions without a known provider session id at spawn can still lose completions that notify before session capture exists. This is a separate pre-session ownership buffering problem.
- Detached auto-resume output is not user-visible by default. That is acceptable for autonomous wake, but debug logs should be available through sidecar diagnostics.
- Prompt injection risk remains bounded by WU-B's envelope design: store paths and rc, do not inline arbitrary log content.
- Auto-wake can spend provider quota. The cap and single-flight claim bound runaway loops, but product UX may later need a per-session opt-out or max daily auto-wake budget.
- PTY interactive immediate injection remains out of scope. PTY sessions can still queue mailbox rows, but WU-D proactive wake is for headless sessions.

## 12-Line Summary

1. WU-D adds proactive headless wake on top of WU-A PID identity and WU-B mailbox delivery.
2. No versioned `state.db` schema changes are allowed; all new runtime and wake state lives in `pid-identity.db`.
3. A session is busy only when sidecar runtime says running and the stored PID still matches the three-part identity.
4. Stale running rows are treated as idle after runner-side identity verification.
5. Trigger A runs after notify enqueue: if pending rows exist and the owner is idle, claim and detach `agents resume --session-id`.
6. Trigger A skips busy sessions because Trigger B handles completions that arrive mid-turn.
7. Trigger B runs at successful headless turn end after delivered marking, then wakes again only if undelivered rows remain.
8. Single-flight wake claims prevent duplicate concurrent wake resumes for the same session.
9. A consecutive auto-wake cap prevents self-replicating background-task loops.
10. The installed `agents` binary must include WU-A, WU-B, and WU-D; `agent-bash` finds it via `AGENT_BASH_AGENT_RUNNER_BIN` or `PATH`.
11. Dogfood uses an XDG-isolated fixture plus a real installed run to prove idle wake and busy turn-end delivery.
12. Tests cover idle wake, busy recheck, no-pending no-wake, loop termination, single-flight concurrency, and XDG isolation.
