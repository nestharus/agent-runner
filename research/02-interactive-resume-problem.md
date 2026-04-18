# Interactive Sessions + Cross-Provider Resume: Problem Research

This document describes the problem space for Initiative 02 without
selecting a design. It is grounded in the current codebase, current local
model configuration, and live CLI behavior observed on 2026-04-18 with
Claude Code `2.1.114`, `codex-cli 0.121.0`, and `opencode 1.3.3`.

## 1. Use cases

The initiative brief defines two coupled asks: interactive launch with no
prompt and cross-provider resume by bare `session_id`
(`initiatives/02-interactive-and-resume.md:30-78`). The distinct user
scenarios under that umbrella are:

| Use case | Data needed | Captured today | Qualitative frequency |
| --- | --- | --- | --- |
| Accidental exit: “drop me back into the same Claude/Codex session” | Bare `session_id`, provider hosting it, native resume syntax | Yes for provider lookup via `session_turns(provider_name, session_id, turn_id, timestamp, ...)` (`src-tauri/src/state/db.rs:448-459`, `599-605`) | High for interactive CLIs; both Claude and Codex expose resume affordances in their own UX |
| Interactive exploration without remembering which account is hottest | Model -> provider pool, quota state, provider startup args | Mostly yes: current balancer already picks providers at invocation time (`src-tauri/src/main.rs:326-347`); provider args already encode startup flags in model TOML, e.g. Claude includes `--dangerously-skip-permissions` and Codex includes `--dangerously-bypass-approvals-and-sandbox` (`~/.config/oulipoly-agent-runner/models/claude-opus.toml:1-14`, `~/.config/oulipoly-agent-runner/models/gpt-high.toml:1-9`) | High; this is the direct user request |
| “My agent crashed mid-task; resume and tell it to continue” | `session_id`, provider, optional original model, native resume path that can accept a follow-up prompt | Provider lookup exists in `session_turns`; model provenance exists only if `invocations.session_id` was populated (`src-tauri/src/state/db.rs:558-596`, `893-928`) | Medium |
| Tutorials / pair programming / shell-driven REPL use | A real TTY handoff, not one-shot stdin/stdout capture | Not implemented: current CLI refuses “no prompt” when stdin is a terminal (`src-tauri/src/main.rs:148-171`), and executor always captures child stdio (`src-tauri/src/executor/cli.rs:262-307`) | Medium to high |
| Resume a session id copied from another machine | Local lookup corpus plus proof the underlying provider CLI still has resumable state for that session | Only partially. `session_turns` can know that a session id existed locally, but the DB does not prove the wrapped CLI can still reopen it | Low, but high-friction when it happens |
| Scripts that run non-interactively and then drop into an interactive CLI | TTY inheritance, exit-code propagation, optional parent invocation propagation | Parent propagation exists today via `OULIPOLY_PARENT_INVOCATION` (`src-tauri/src/main.rs:343-370`, `431-439`; `src-tauri/src/executor/cli.rs:253-258`), but the execution path is still one-shot | Low to medium |

Read-only queries against `~/.local/share/oulipoly-agent-runner/state.db`
on 2026-04-18 showed `635,381` ingested turns, `13,891` distinct
provider-scoped sessions, and `5` providers in `session_turns`. That
means the lookup corpus already exists at useful scale. The same DB had
only `116` `invocations` rows and `0` non-null `invocations.session_id`
rows on this machine, so the session corpus is much richer than the
invocation back-links right now.

## 2. Existing wrapped-CLI mechanics

### Claude Code

`claude --help` says Claude “starts an interactive session by default”
and uses `-p/--print` for non-interactive mode. It exposes:

- `-r, --resume [value]`: resume by session id, or open an interactive picker with optional search term.
- `-c, --continue`: continue the most recent conversation in the current directory.
- `--fork-session`: create a new session id when resuming or continuing.
- `--session-id <uuid>`: force a specific session id.

Empirical behavior:

- `script -qefc "timeout 6 claude" /dev/null` opened the full interactive
  REPL with alternate-screen UI and a prompt. This confirms that a real
  TTY is sufficient for interactive mode.
- `printf 'hello from pipe\n' | timeout 6 claude` produced a one-shot
  text response (`Hello! How can I help you today?`) instead of entering
  the REPL. Claude therefore distinguishes TTY-attached vs piped stdin.
- `claude -p --resume 9e69e8cc-... --output-format stream-json --verbose "reply with the single word gamma"`
  succeeded, and both the `system.init` event and final `result` event
  reported the same `session_id`. `claude -p -c ...` reused that same
  session id as well.
- `claude -p --resume 9e69e8cc-... --model sonnet ...` also succeeded and
  kept the same `session_id`, but changed `model` in the `system.init`
  event from Opus to Sonnet. Model switching on resume is therefore
  permitted by the CLI, at least in print mode.
- A synthetic session created with `claude -p --session-id aaaaaaaa-...`
  did emit that id in `system.init`, but `claude -p --resume aaaaaaaa-...`
  immediately afterward failed with “No conversation found with session
  ID ...”. That leaves an unresolved question: print-mode sessions with a
  forced id are not obviously equivalent to persisted resumable sessions.

### Codex

`codex --help` says that if no subcommand is specified, arguments are
forwarded to the interactive CLI. Resume is not a top-level `--resume`
flag. Instead:

- `codex resume [SESSION_ID] [PROMPT]` resumes an interactive session.
- `codex exec resume [SESSION_ID] [PROMPT]` resumes non-interactively.
- `codex fork` exists as a separate interactive fork path.
- `codex exec` is the one-shot path used by current runner models.

Empirical behavior:

- `printf 'hello from pipe\n' | timeout 6 codex` failed immediately with
  `Error: stdin is not a terminal`. Top-level Codex does not downgrade to
  one-shot piped execution the way Claude does.
- `codex --no-alt-screen` in a PTY stayed interactive, rendered the TUI,
  and on Ctrl-C printed `To continue this session, run codex resume <id>`.
- `printf 'hello\n' | codex exec --skip-git-repo-check` ran
  non-interactively, printed `session id: 019da003-...`, and completed.
- `codex exec resume 019da003-... ping` reused the same `session id`.
- `codex resume 019da003-...` in a PTY showed the prior conversation
  history (`hello`, `ping`, `pong`) and on exit again pointed at the same
  id.
- `codex exec resume 019da003-... -m gpt-5.4-mini ok` kept the same
  `session id` while changing the model from `gpt-5.4` to
  `gpt-5.4-mini`. Model switching is also permitted here.

### GLM / OpenCode

This installation does not currently route `glm` through OpenCode.
`~/.config/oulipoly-agent-runner/models/glm.toml:1-5` points `glm` at
`forge --provider zai_coding --model glm-5.1 -p`. `opencode --help` does
show a default TUI plus `-c/--continue`, `-s/--session`, and `--fork`,
but that is not the repo’s current GLM path. Resume behavior for the
actual GLM provider remains open.

## 3. Data inventory: what `--resume` would query

Persisted today:

- `session_turns` is already the provider-scoped transcript index. Its
  uniqueness key is `(provider_name, session_id, turn_id)` and it is
  indexed by `(provider_name, session_id, timestamp)`
  (`src-tauri/src/state/db.rs:448-459`, `599-605`). That is sufficient to
  answer “which provider has session `<id>`?” and “which matching copy is
  most recent?”
- `invocations` already has `session_id` and `session_capture_method`
  columns plus an index on `(provider_name, session_id)`
  (`src-tauri/src/state/db.rs:558-596`). `update_session_capture()`
  writes those fields, and `get_invocation_by_uuid()` reads them
  (`src-tauri/src/state/db.rs:893-928`).
- `invocations.parent_invocation_id` already exists and can relate a
  resumed run back to a parent runner invocation
  (`src-tauri/src/state/db.rs:558-596`, `760-881`).
- `count_session_turns()` already computes total, assistant, and
  sidechain counts for a `(provider_name, session_id)` pair
  (`src-tauri/src/state/db.rs:1736-1759`).

Not reliably captured today:

- Original model at session scope. `session_turns` has no model column.
  `invocations.model_name` can answer that only if an invocation row was
  correlated to the session, and the live DB on this machine currently
  has no populated `invocations.session_id` rows.
- Whether the provider CLI can still reopen the session. The DB proves
  turns were ingested, not that the native CLI’s local persistence still
  exists or is resumable.
- Whether a session originated from interactive mode, print mode, direct
  CLI use, or a resumed run. That distinction matters because Claude’s
  synthetic print-mode `--session-id` test did not behave like a normal
  resumable session.

## 4. TTY handoff considerations

The current executor is structurally one-shot:

- `resolve_prompt()` errors when no prompt is present and stdin is a TTY
  (`src-tauri/src/main.rs:148-171`).
- `run()` always resolves a prompt before calling `run_with_balancing()`
  for model or agent execution (`src-tauri/src/main.rs:210-256`).
- `execute_provider()` sets stdin to either `null` or `piped`, always
  sets stdout/stderr to `piped`, and waits with `wait_with_output()`
  (`src-tauri/src/executor/cli.rs:262-307`).

Interactive mode inverts those assumptions. The child wants the user’s
terminal, not captured pipes. That creates three problem areas:

1. Session capture today is post-hoc parsing over captured stdout. Both
   existing capture strategies (`forced_flag_verified` and
   `stdout_json_event`) depend on parsing output after `wait_with_output()`
   (`src-tauri/src/executor/cli.rs:247-307`, `313-360`). Direct TTY
   handoff removes that channel.
2. `OULIPOLY_INVOCATION=...` is currently emitted unconditionally on
   stderr before spawn (`src-tauri/src/main.rs:360-362`). In interactive
   mode that line lands in the user’s terminal unless explicitly handled.
3. Terminal mode restoration becomes the child CLI’s problem. Both Claude
   and Codex switch to alternate-screen / richer terminal modes in
   interactive mode. Nested terminals, signal delivery, and Ctrl-C
   behavior matter more than in the current captured subprocess model.

Rust’s obvious primitive here is `Stdio::inherit()`, but the research
point is not the API call itself; it is that interactive handoff changes
what data the runner can observe, when stderr is visible, and how parent
env propagation interacts with a long-lived child process.

## 5. Resume mechanics: per-CLI matrix

| CLI | Interactive entry | Resume entry | Non-interactive resume | Same session id on normal resume? | Model switch on resume? |
| --- | --- | --- | --- | --- | --- |
| Claude Code | `claude` with a TTY | `claude --resume [id]` or `claude --continue` | Yes: `claude -p --resume <id> "..."` and `claude -p -c "..."` both worked | Yes, empirically | Yes, empirically (`--model sonnet` kept the same id) |
| Codex | `codex` with a TTY | `codex resume [id] [prompt]` | Yes: `codex exec resume <id> [prompt]` | Yes, empirically | Yes, empirically (`-m gpt-5.4-mini` kept the same id) |
| OpenCode | `opencode [project]` per help | `--session` / `--continue` per help | Unverified | Open | Open |

Two subtleties matter:

- Codex’s resume mechanism is command-shaped, not flag-shaped. The
  initiative brief’s “Codex: `--resume <id>`” is not true for the current
  installed CLI.
- Both Claude and Codex expose explicit fork behavior (`--fork-session`
  for Claude, `fork` for Codex). That implies “resume” and “fork” are
  distinct native semantics, and the runner cannot treat them as the same
  thing.

## 6. Conflict resolution semantics

The brief says duplicate `session_id` matches should resolve to the “most
recent sessionid” (`initiatives/02-interactive-and-resume.md:22-25`,
`65-70`). Three different conflict shapes exist:

- Exact full-id conflict across providers. This is rare but not purely
  theoretical. The live DB contains one exact duplicate session id,
  `9e69e8cc-616d-4640-bf1d-96f5391b1a2e`, under both `claude2` and
  `claude3`, each with two turns; the later copy is `claude2`.
- Partial-id conflict. This is common if the user pastes only a prefix.
  The same DB had `988` duplicated 8-character prefixes across distinct
  provider-scoped sessions on 2026-04-18.
- Multiple runner invocations against one native session. Once
  `invocations.session_id` is populated broadly, a resumed session can
  reasonably have several invocation rows pointing at one session id.

The important point is that “conflict” is not only a theoretical UUIDv4
collision problem. In the observed data, exact duplicates exist, and
prefix collisions are abundant.

## 7. Tradeoff axes the proposal phase must resolve

- Provider selection during `--resume`: the user’s language points toward
  lookup by session ownership, while the current balancer only knows how
  to choose among providers before starting a fresh invocation
  (`src-tauri/src/main.rs:326-347`).
- Model arg validation: the native CLIs do not force model continuity.
  Both Claude and Codex allowed a resumed session to keep its session id
  while changing models. Any mismatch policy is therefore runner policy,
  not a CLI limitation.
- Interactive + parent invocation propagation: the propagation channel
  already exists (`src-tauri/src/main.rs:343-370`, `431-439`;
  `src-tauri/src/executor/cli.rs:253-258`), but interactive mode changes
  the lifetime and visibility of that context.
- `agents <model>` ambiguity: the current parser treats the first
  positional as an agent name unless `--model` is used. `run()` falls
  into agent resolution when `cli.model` is absent, and `resolve_agent()`
  errors if that positional is not a known agent
  (`src-tauri/src/main.rs:210-256`, `293-312`). “No prompt means
  interactive model launch” collides with existing positional semantics.
- Quota check during resume: current density scoring happens once, before
  spawn. There is no mid-session rebalance path, and a resumed session is
  already tied to a concrete provider account
  (`initiatives/02-interactive-and-resume.md:103-126`;
  `src-tauri/src/main.rs:326-347`).
- Startup-args mismatch: the installed Claude and Codex model configs
  currently hardcode one-shot entry (`-p` for Claude, `exec` for Codex)
  in provider args (`~/.config/oulipoly-agent-runner/models/claude-opus.toml:1-14`,
  `~/.config/oulipoly-agent-runner/models/gpt-high.toml:1-9`). Interactive
  launch is therefore not just “omit the prompt”; it also conflicts with
  today’s configured provider commands.

## 8. Open questions

The initiative brief’s five open questions remain open
(`initiatives/02-interactive-and-resume.md:101-126`):

1. How quota should be interpreted for long-lived interactive sessions.
2. The per-CLI resume matrix beyond Claude and Codex.
3. What a model mismatch means at runner policy level.
4. How parent invocation propagation should behave for interactive runs.
5. Whether “most available” still means current density scoring.

New questions surfaced by the evidence:

1. Are Claude print-mode sessions created with `--session-id` meant to be
   resumable? Local behavior said “not necessarily.”
2. Why does the local transcript corpus already contain one exact
   cross-provider duplicate session id? Imported logs, forced ids, or
   genuine account overlap each imply different resume semantics.
3. The live DB on this machine has the session corpus needed for lookup,
   but `invocations.session_id` is effectively empty. Is that expected lag
   in adoption, or evidence that Initiative 01 capture is not yet widely
   exercised in practice?
4. What is the actual GLM resume contract for the provider path this repo
   really uses (`forge`), as opposed to the hypothetical OpenCode path?
