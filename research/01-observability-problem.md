# Observability Problem Space for Agent-Runner Invocations

This document defines the problem space behind "show me what happened" for `oulipoly-agent-runner`. It is intentionally not a design document. The goal is to identify which questions users want answered, which evidence already exists, which evidence exists only in raw logs, and which relationships are not representable today.

The current shipped context is the April 17, 2026 session-ingestion and multi-window quota work in commit `3775d6f`, which added `session_turns`, `provider_quota_windows`, adapter-script contracts, and density-based selection. The README explicitly states that persistent state is on-disk only in SQLite at `~/.local/share/oulipoly-agent-runner/state.db`, with no daemon or background service (`README.md:181-185`, `README.md:232-279`).

Empirically on this machine on 2026-04-17, the live state DB is about `215M`, contains `61` `invocations` rows, and `626,472` `session_turns` rows. That matters: the dominant observability surface is already "external CLI activity plus ingestion", not just runner-mediated calls.

## 1. Use Cases

| Use case | Data needed | Captured today? | Frequency |
|---|---|---|---|
| "I just ran an agent. What did it actually do?" | Invocation ID, provider/account, session ID, transcript content, tool calls, timing | Partial. `invocations` has success/exit metadata only (`src-tauri/src/state/db.rs:233-245`, `src-tauri/src/state/db.rs:376-433`). `session_turns` has timestamps and roles but no content and no invocation link (`src-tauri/src/state/db.rs:348-361`, `src-tauri/src/state/db.rs:1153-1180`). Raw logs have content. | Common daily workflow |
| "Agent A spawned B which spawned C. Show the call tree." | Parent/child invocation edges, or nested session/subagent markers, plus stable IDs | Mostly not captured. Claude raw logs have `parentUuid` and `isSidechain`, but the adapter drops them (`scripts/claude-code-turns:68-82`). Cross-invocation parentage is not stored anywhere. | Forensic, but likely recurring |
| "Something failed 20 minutes ago. Show me what went wrong." | Per-invocation failure record, stderr, category, timestamps, provider/account | Partial. `invocations` stores `success`, `exit_code`, `error_category`, `created_at`; it does not store stderr (`src-tauri/src/state/db.rs:233-245`). Only the latest failure snippet is retained in `providers.last_error`, and it is overwritten on each new failure (`src-tauri/src/state/db.rs:387-415`). | Common debugging workflow |
| "Show all calls an account made today across all models." | Account/provider name per call, chronological order, direct CLI usage plus runner usage | Partial at best. `session_turns` is keyed by `provider_name` and sees direct CLI usage (`src-tauri/src/state/db.rs:348-361`). But `invocations` records `model_name` + `provider_index`, not provider/account name (`src-tauri/src/state/db.rs:233-245`), so account-level invocation chronology is not a first-class query. | Daily operational workflow |
| "Compare two parallel agents in real time." | Stable run IDs, streaming events, per-run grouping, tool events, parentage | Largely absent for normal CLI execution. Runtime returns only subprocess stdout/stderr and exit code (`src-tauri/src/executor/mod.rs:7-13`, `src-tauri/src/executor/cli.rs:214-290`, `src-tauri/src/main.rs:255-295`). | Less frequent, but interactive |
| "Feed a past run to another LLM for critique or continuation." | Exportable structured transcript, ideally with tool calls and chronology | Partial outside the DB. SQLite lacks content; raw Claude/Codex logs contain enough structure to reconstruct a transcript if the correct session can be found. | Casual but valuable |
| "Was that a runaway loop or just an expensive task?" | Quota window snapshots, call counts, session density over time, transcript/tool loop evidence | Partial. Quota windows and assistant-turn counts exist (`src-tauri/src/quota/mod.rs:65-98`, `src-tauri/src/state/db.rs:246-262`, `src-tauri/src/state/db.rs:569-695`, `src-tauri/src/state/db.rs:1153-1180`), but there is no invocation-to-session join and no transcript/tool data in SQLite. | Operational / forensic |
| "Show me everything your agent did with customer X's data on date Y." | Complete transcript, attachments, tool inputs/outputs, account, timing, customer scoping, export surface | Mostly absent in SQLite. Raw Claude logs can include content and attachments; current adapter contract strips to four fields (`README.md:254-260`, `scripts/README.md:16-31`). Customer scoping is not modeled. | Rare but high-stakes |

## 2. Data Inventory

### What exists in SQLite

- `invocations` records per-call outcome: `id`, `model_name`, `provider_index`, `success`, `exit_code`, `error_category`, `created_at` (`src-tauri/src/state/db.rs:233-245`). This is enough for counts, recent failures, and basic timelines. It is not enough for transcript replay, stderr replay, duration analysis, or account-level grouping.
- `providers` is not a provider-account ledger; it is per-`(model_name, provider_index)` aggregate stats: invocation count, error count, last error snippet, last invocation time (`src-tauri/src/state/db.rs:222-231`, `src-tauri/src/state/db.rs:387-415`). This means the table named `providers` does not answer "what did account `claude2` do?" across models.
- `provider_quotas` stores per-account aggregate quota metadata: `calls_since_refresh`, `refreshed_at`, and learned deltas (`src-tauri/src/state/db.rs:246-254`, `src-tauri/src/state/db.rs:498-528`). The `used_percent`/`resets_at` columns are backward-compat copies of the longest window, not the full quota picture (`src-tauri/src/state/db.rs:605-638`).
- `provider_quota_windows` stores the actual multi-window quota rows: one row per `(provider_name, window_id)` with `used_percent` and `resets_at` (`src-tauri/src/state/db.rs:256-262`, `src-tauri/src/state/db.rs:530-567`).
- `session_turns` stores normalized session-log events across CLIs: `provider_name`, `session_id`, `turn_id`, `timestamp`, `role`, `source_file`, `ingested_at` (`src-tauri/src/state/db.rs:348-361`). Counting assistant turns since a timestamp is first-class (`src-tauri/src/state/db.rs:1153-1180`).

### Important gaps inside the schema

- There is no parent-child invocation relationship. Every `invocations` row is standalone.
- There is no invocation-to-session correlation. The runner records an invocation, but does not persist which CLI `session_id` it produced. `run_with_balancing` executes, records the invocation, increments quota ticks, and exits; nothing extracts or stores session identifiers for ordinary model calls (`src-tauri/src/main.rs:255-295`, `src-tauri/src/executor/cli.rs:214-290`).
- There is no parent-child turn relationship in `session_turns`. The adapter contract only allows `session_id`, `turn_id`, `timestamp`, `role` (`README.md:254-260`, `scripts/README.md:16-31`, `src-tauri/src/sessions/mod.rs:8-18`, `src-tauri/src/sessions/mod.rs:32-39`).
- Claude Code's `parentUuid` and `isSidechain` fields are dropped by the current reference adapter because `claude-code-turns` filters raw JSONL down to `session_id`, `turn_id`, `timestamp`, and `role` (`scripts/claude-code-turns:68-82`).
- `source_file` exists in the schema but is effectively empty on the normal ingestion path. `scan_provider` batches only `(session_id, turn_id, timestamp, role)` (`src-tauri/src/sessions/mod.rs:81-111`), and `ingest_session_turns_batch` inserts `''` for `source_file` (`src-tauri/src/state/db.rs:1127-1144`). The live DB confirms blank `source_file` values.
- Per-invocation stderr is not retained. Only the latest provider-slot error snippet survives in `providers.last_error` (`src-tauri/src/state/db.rs:401-415`).
- `invocations.id` is an auto-increment row id, but the runner does not return it to the caller; `record_invocation` returns `Result<(), String>`, not the inserted id (`src-tauri/src/state/db.rs:376-433`).

### What exists outside SQLite but is recoverable

- Claude Code raw logs under `~/.claude*/projects/.../*.jsonl` contain richer structure than the DB: `uuid`, `parentUuid`, `sessionId`, `isSidechain`, `message`, and in sampled files also `agentId`/`slug` for sidechains. On this machine, a sampled sidechain file under `subagents/agent-*.jsonl` carried `isSidechain: true` and `agentId`.
- Codex raw logs under `~/.codex*/sessions/.../rollout-*.jsonl` contain more than user/assistant turns. Sampled files contained `session_meta`, `turn_context`, `event_msg`, and `response_item` payload types including `message`, `reasoning`, `function_call`, `function_call_output`, and `custom_tool_call`.
- The adapter surface is intentionally extensible and format-agnostic: scripts may read JSONL, SQLite, or remote APIs and emit normalized JSONL (`scripts/README.md:16-31`, `scripts/README.md:67-91`).

## 3. Tree-of-Trees: What Nesting Actually Exists

There are two different nesting problems.

### a) Nesting within a CLI session

Claude sessions are not just linear chat logs. The raw JSONL carries a turn-level parent pointer (`parentUuid`) and a sidechain marker (`isSidechain`). In practice this means a session can contain both ordinary back-and-forth turns and subagent/task branches. Those branches can be structurally meaningful even when they belong to the same `sessionId`.

Codex appears to represent nesting differently. The raw event stream is not a simple message list; it includes `reasoning`, tool/function-call records, tool outputs, and contextual events. In sampled local files there was no obvious Claude-style `parentUuid`, but there was clearly more internal structure than the current `codex-turns` adapter preserves.

Today, both CLIs are flattened before ingestion. The runner's normalized contract deliberately erases everything except session, turn id, timestamp, and role (`README.md:254-260`, `src-tauri/src/sessions/mod.rs:8-18`). So "tree inside a session" currently exists in raw logs, not in the DB.

### b) Nesting across agent-runner invocations

This is a separate problem. If a provider CLI shells out to `oulipoly-agent-runner` again, or invokes another tool that eventually invokes it, that second invocation is not a child in any stored sense. The current runtime has no trace context object, no explicit parent id parameter, and no correlation write when recording invocations (`src-tauri/src/main.rs:255-295`).

Mechanically, the only generic cross-process propagation surfaces visible in the current execution path are:

- inherited process environment, because `Command` is used directly and no custom trace environment is set (`src-tauri/src/executor/cli.rs:214-290`)
- CLI arguments and prompt text, because those are what the provider actually receives (`src-tauri/src/executor/cli.rs:221-257`)
- working directory, which is already passed through (`src-tauri/src/executor/cli.rs:239-241`)

But no wrapped CLI is currently required to preserve any parent marker, and the schema has nowhere to store one even if it were observed.

Linear trees happen when one invocation maps to one session with one chronological branch. Branching happens when:

- a CLI creates subagents/sidechains inside one session
- one invocation causes multiple child invocations
- one agent fan-outs to multiple tools or subprocesses in parallel

The key problem is that the system currently has chronology, not causality.

## 4. Existing Observability Ecosystems

- LangSmith models a request as a `trace`, the steps inside it as `runs`, and multi-turn conversation grouping as `threads`; its UI is built around trace inspection plus tags/metadata/feedback. Official docs explicitly describe a trace as a collection of runs and a thread as a sequence of traces linked by `session_id`/`thread_id`/`conversation_id`. Source: <https://docs.langchain.com/langsmith/observability-concepts>.
- Langfuse models `traces`, nested `observations`, and optional `sessions`. Its UI includes trace detail, sessions, timeline, and agent-graph oriented views. Source: <https://langfuse.com/docs/observability/data-model>, <https://langfuse.com/docs>.
- OpenTelemetry models traces as DAGs of spans, with `TraceId`, `SpanId`, parent span id, events, attributes, and explicit propagation of span context across process boundaries. Source: <https://opentelemetry.io/docs/specs/otel/overview/>.
- Honeycomb and Datadog both use span-based distributed tracing with waterfall/flame-graph style inspection. Honeycomb describes each span as an event with `traceID` and `parentID`; Datadog's trace view centers on flame graph, span list, waterfall, and map views over one trace. Sources: <https://docs.honeycomb.io/get-started/basics/observability/concepts/distributed-tracing>, <https://docs.datadoghq.com/tracing/trace_explorer/trace_view/>.
- Aider is much simpler and local-first: its official docs tell users to share transcript markdown from `.aider.chat.history.md`, and render it via a share URL. That is transcript observability without a span model. Source: <https://aider.chat/docs/faq.html>.
- Pydantic AI splits message history from tracing. Its docs emphasize structured message-history objects, caution that tool calls and returns must stay paired, and separately expose OpenTelemetry/Logfire tracing. Sources: <https://pydantic.dev/docs/ai/core-concepts/message-history/>, <https://pydantic.dev/docs/ai/integrations/logfire/>.
- AutoGPT's platform docs expose a graph execution model with `graph_id`, `graph_exec_id`, `node_id`, and `node_exec_id`, plus a Monitor tab and shareable debug logs. That is closer to workflow execution tracing than plain chat history. Sources: <https://agpt.co/docs/platform/new-blocks>, <https://docs.agpt.co/platform/delete-agent/>, <https://docs.agpt.co/classic/share-your-logs/>.

The common pattern across these systems is separation of concerns: one identifier for the overall request/run, another for nested work units, and a separate inspection surface over the same underlying events. What becomes heavy for a single-user local CLI tool is the hosted-service part: collectors, agents, retention/indexing policies, multi-tenant dashboards, and full APM breadth.

## 5. Constraints and Forces

- Single-binary, no-daemon architecture. Inspection has to work from SQLite plus whatever raw session logs still exist on disk (`README.md:185`, `README.md:269-279`).
- Heterogeneous and user-extensible adapters. The runner explicitly does not know whether a CLI stores history as JSONL, SQLite, or remote API (`scripts/README.md:30-31`, `scripts/README.md:67-91`). Any observability story has to tolerate adapters that cannot provide rich parentage.
- Storage pressure is already real. The live DB is `215M` with `626,472` `session_turns`. Storing full transcript content and tool payloads would be materially larger than the current role/timestamp-only ingest.
- Privacy and sensitivity. Raw logs can include prompt content, reasoning, tool inputs/outputs, and attachments. That is categorically more sensitive than the current DB.
- Performance. Deep inspection should not require repeatedly scanning hundreds of thousands of flattened turn rows or walking every raw log tree from scratch.
- Historical stability. `invocations` stores `provider_index`, not provider name. If model configs change ordering, historical interpretation becomes weaker unless external config history is available.
- Observability spans both runner-mediated and direct CLI usage. That is the point of session ingestion (`README.md:232-260`), but it means "what happened?" is broader than "what the runner directly launched."
- Composability matters. Users may want machine-readable exports that can be piped into another agent, not just human-readable summaries.

## 6. Tradeoff Axes

- **Source of truth**: SQLite-only summaries vs hybrid SQLite + raw-log reconstruction.
- **Identity granularity**: per-invocation row IDs vs invocation + session + nested turn/span IDs.
- **Correlation method**: explicit parent propagation vs heuristic timestamp/cwd inference.
- **Transcript depth**: role/timestamp metadata only vs content previews vs full messages/tool calls/attachments.
- **Account attribution**: model/provider-index interpretation vs stable provider/account naming.
- **Capture timing**: post-hoc scan of logs vs data emitted at invocation time.
- **Privacy posture**: always-on rich transcript capture vs metadata-only defaults vs explicit user opt-in for sensitive content.
- **Inspection surface**: SQL recipes and raw files vs structured export vs interactive viewer.
- **Verbosity**: tree summary vs full transcript dump vs filterable mixed views.
- **Caller visibility**: internal DB-only invocation IDs vs IDs exposed to callers/logs in normal operation.

## 7. Open Questions

- Do newer Codex session formats emit explicit parent/child markers analogous to Claude's `parentUuid`/`isSidechain`, or is nesting only implicit in event ordering and tool-call references?
- How do other runner-supported CLIs such as Gemini or Droid expose history, if at all? The current shipped reference adapters cover Claude and Codex; `scripts/README.md` explicitly notes some CLIs may expose no history surface.
- Can cross-invocation parentage be inferred well enough from existing evidence alone, such as cwd, near-simultaneous timestamps, and shared raw-session markers, or would that remain probabilistic and fragile?
- Are raw session logs stable enough across CLI versions to be treated as a dependable observability source, or are they only a best-effort fallback?
- Should multimodal passthrough models be considered part of the same observability problem space as text-agent runs, given that their outputs are binary and their "transcript" semantics are different?
- Is `provider_index` historical ambiguity already a practical problem in real usage, or only a latent one if model configs are frequently reordered?
