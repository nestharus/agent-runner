# 1. Summary

This proposal adds a new additive CLI entrypoint,
`oulipoly-agent-runner repl <model> [--resume <session-id>]`, so
interactive launch does not collide with the current flat
first-positional-is-agent parser. Provider-specific interactive launch
and resume syntax remain declarative on `ProviderConfig`, following the
same V1/V2/V3 pattern already used for `session_capture`.

The revision is driven by the risk findings and `VALUES.md`. Three
changes are load-bearing:

- PR-F adds a `session_id`-leading index on `session_turns`, because V8
  means "cheap on use," not "full-scan 635k rows on every resume."
- Resume now treats `<MODEL>` as an explicit provider-pool constraint:
  if the resolved provider is not in that model's provider list, the
  runner errors with a helpful suggestion instead of silently ignoring
  the model argument. That is the V10/V13 answer.
- Resume selection becomes observable at a TTY: the runner always emits
  a short stderr line, `[resume] -> <provider>`, while longer duplicate
  detail stays gated to non-TTY stderr or a future `--verbose`.

The core design remains the same: `repl` gets a dedicated interactive
executor path with inherited stdio and a guarded finalize-on-exit
lifecycle; `--resume` resolves the provider lazily from `session_turns`
and records explicit provenance as `session_capture_method =
"resumed"`.

# 2. CLI shape

The subcommand is `repl`. I am not changing the flat CLI path because
the current parser already reserves the first positional for agent
resolution and errors when no prompt is present. `repl` is the clean
V14-compatible way to add prompt-less interactive launch without
guessing intent.

Proposed surface:

```text
Usage: oulipoly-agent-runner repl [OPTIONS] <MODEL>

Arguments:
  <MODEL>                     Model id to launch interactively

Options:
      --resume <SESSION_ID>   Resume an existing session by full UUID
  -p, --project <PATH>        Working directory for the wrapped CLI
      --models-dir <PATH>     Override models directory
  -h, --help                  Print help
```

Rules:

- `<MODEL>` is always required and is resolved from `models.toml`;
  `repl` does not accept an agent name, `--agent-file`, `--file`, or
  prompt positionals.
- `--resume` takes a bare full UUID only. The runner validates with
  `Uuid::parse_str`; no prefix matching in v1.
- `repl` always inserts an invocation row.
- `Trace` remains unchanged.

Exit codes:

- `1` for runner-side failures: unknown model, missing
  `interactive_args`, missing `[providers.resume]` on `--resume`,
  malformed UUID, provider lookup miss, provider/model mismatch, spawn
  failure, or DB/config error.
- `2` for clap usage errors.
- Child exit codes propagate otherwise; on Unix signal exit, `repl`
  returns `128 + signal`.

# 3. Provider config additions

`ProviderConfig` already carries `args` and `session_capture`. This
proposal adds one field for interactive launch and one block for resume
strategy:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interactive_args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_capture: Option<SessionCapture>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<ResumeStrategy>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResumeStrategy {
    pub kind: ResumeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subcommand: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResumeKind {
    Flag,
    Subcommand,
}
```

Validation mirrors `SessionCapture::validate()`:

- `interactive_args = Some(vec![])` is invalid.
- `ResumeKind::Flag` requires `flag` and rejects `subcommand`.
- `ResumeKind::Subcommand` requires `subcommand` to be present and
  non-empty and rejects `flag`.
- Absence of `interactive_args` or `resume` is valid at load time;
  `repl` errors clearly if it needs them.

Claude example:

```toml
[[providers]]
name = "claude2"
command = "env"
args = ["-u", "CLAUDECODE", "claude2", "-p", "--model", "opus", "--dangerously-skip-permissions"]
interactive_args = ["-u", "CLAUDECODE", "claude2", "--model", "opus", "--dangerously-skip-permissions"]

[providers.resume]
kind = "flag"
flag = "--resume"
```

Codex example:

```toml
[[providers]]
name = "codex"
command = "codex"
args = ["exec", "--dangerously-bypass-approvals-and-sandbox", "-m", "gpt-5.4", "-c", "model_reasoning_effort=high"]
interactive_args = ["--dangerously-bypass-approvals-and-sandbox", "-m", "gpt-5.4", "-c", "model_reasoning_effort=high"]

[providers.resume]
kind = "subcommand"
subcommand = ["resume"]
```

I am explicitly keeping `args` and `interactive_args` as separate
arrays. The risk review is right that this duplicates some prefixes and
creates drift potential. The alternative, `common_args` plus per-mode
delta, is also value-aligned. I am not taking it now because the extra
shape adds structural complexity for a narrow problem and buys no new
capability. Under V1/V3 the important thing is a declarative provider
surface; under V16 the default is the smallest reviewable shape that
ships the feature.

This is therefore a deliberate value-neutral preference. Guardrails:

- PR-E adds parse/round-trip tests for `interactive_args`.
- PR-E adds canonical Claude and Codex config-shape tests so repo-owned
  examples do not drift silently.
- Section 11 carries an explicit followup: if first-party config churn
  shows repeated `args` / `interactive_args` drift, collapse to a shared
  prefix shape in a later proposal rather than accreting ad hoc checks.

Backwards compatibility is strict:

- Existing one-shot execution keeps using `args`.
- This proposal deliberately revises one synthesis contract from
  `research/02-interactive-resume-needs.md`: that document said absent
  `interactive_args` should fall back to `args`. The later problem
  research showed why that is unsafe here: today's repo-owned `args`
  encode one-shot entry points like Claude `-p` and Codex `exec`, which
  are the wrong surface for interactive launch. Under V10, a clear error
  is the better failure than silently launching the wrong mode; under
  V14, once that fallback is known wrong, the proposal should say so
  explicitly rather than preserve it as an implicit compat path.
- `repl` on a provider without `interactive_args` fails with a clear
  error instead of silently reusing one-shot args like Claude `-p` or
  Codex `exec`.
- `repl --resume` on a provider without `resume` fails clearly instead
  of guessing syntax from the command name.

# 4. TTY handoff in executor

Add a new entry point in `src-tauri/src/executor/cli.rs` parallel to
`execute_provider()`:

```rust
pub fn execute_interactive(
    model: &ModelConfig,
    provider_index: usize,
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    resume_session_id: Option<&str>,
) -> Result<i32, String>;
```

Behavior:

- Build the child command the same way `execute_provider()` does today
  from `provider.command` plus `shell_split()`, but use
  `provider.interactive_args` instead of `provider.args`.
- If `resume_session_id` is present, compose the provider's `resume`
  strategy on top of `interactive_args`:
  - `flag`: append `<flag> <session_id>`
  - `subcommand`: append `<subcommand...> <session_id>`
- Set `stdin`, `stdout`, and `stderr` to `Stdio::inherit()`.
- Call `wait()`, not `wait_with_output()`.

Signal and terminal handling on Unix:

- On Unix, the runner installs scoped handlers for `SIGINT`, `SIGTERM`, and
  `SIGHUP` while the child is running.
- `SIGINT` and terminal-close `SIGHUP` are not forwarded manually; with
  inherited TTY, the child already receives terminal-generated signals
  directly. The parent handler exists only so the parent survives long
  enough to reap and finalize.
- `SIGTERM` delivered directly to the runner is forwarded once to the
  child if it is still alive, then the runner continues waiting.
- The runner never restores terminal state itself; the wrapped CLI owns
  that.

This is the Unix behavior, not a cross-platform proof. PR-E test scope
explicitly includes Unix signal-handling integration coverage for this
path, while the Windows console-control story remains the open question
called out in §11.

Lifecycle:

1. Insert a `running` invocation row.
2. Emit `OULIPOLY_INVOCATION=...` only when
   `!std::io::stderr().is_terminal()`, matching the synthesis contract
   that stable trace correlation matters for wrappers while TTY users do
   not need the line painted into the REPL.
3. Spawn the child with `OULIPOLY_PARENT_INVOCATION` set, same as the
   current one-shot execution path.
4. Wait for the child.
5. Finalize the row with success and exit code. `error_category` stays
   `NULL` for `repl`, because inherited stderr means there is no
   captured diagnostic payload to run through `run_diagnostics()`.

Crash safety is best-effort. Use a small RAII finalizer guard around the
invocation row: normal completion marks it finished; early `Err` or
panic unwinding finalizes it as failed; scoped signal handling keeps the
parent alive long enough to finalize. `SIGKILL`, abort, and power loss
can still strand a `running` row, which is acceptable and observable
under V10.

The guard must explicitly no-op after happy-path finalization. That is
not optional wording; it is part of the lifecycle contract because
`finalize_invocation()` already errors on double-call. Concretely, the
guard owns a `finalized: bool` flag or equivalent defensive wrapper, and
`Drop` checks that flag before attempting a fallback finalize. PR-E
tests must cover the "explicit finalize succeeded, `Drop` runs second"
path.

# 5. Provider lookup and resume semantics

The lookup source of truth is `session_turns`, not
`invocations.session_id`: `session_turns` is populated at scale while
invocation back-links are sparse.

Lookup SQL:

```sql
SELECT provider_name, MAX(timestamp) AS latest_timestamp
FROM session_turns
WHERE session_id = ?1
GROUP BY provider_name
ORDER BY latest_timestamp DESC, provider_name ASC;
```

PR-F adds a DB helper that returns the ordered matches for a bare
session id, not just a yes/no answer. The runner then applies the
resume policy:

- No matches: exit `1` with `No session found matching <id>. Check that
  session ingestion is configured and that the provider still has
  resumable local state.`
- One or more matches: select the first row from the ordered query.
- Always emit one short stderr line after selection:
  `[resume] -> <provider>`.
- If more than one provider matched, emit the longer detail line only
  when `stderr` is not a TTY or a future `--verbose` requests it:
  `[resume] session <id> matched claude2, claude3; selected claude2 by latest turn timestamp`.

This is the V10/V15 resolution: the short line makes the choice
observable, and longer detail remains caller-controlled.

`<MODEL>` remains load-bearing even on resume. The runner does not treat
it as decorative. After selecting a provider from `session_turns`, it
checks that the provider appears in the requested model's provider list.
If not, the runner errors and does not launch the mismatched provider.

Error shape:

```text
session <id> belongs to provider <provider>, which is not in model <model>'s provider pool.
Try a model that includes <provider>: <suggestion1>, <suggestion2>, ...
```

That is the correct V13/V10 behavior: a resumed session still lives on
a concrete provider account, and the runner must not silently ignore
the caller's explicit model input.

The suggestions come from the loaded model configs by scanning for model
ids whose provider list includes the resolved provider. If none exist,
the message says so plainly.

Bare full UUID only is deliberate. Prefix lookup stays out of scope
because the research found heavy prefix collisions in real data.

# 6. Composition with `session_capture`

When `--resume <id>` is used, the runner does not run the
`session_capture` parser and does not inject any `session_capture`
flags. Resume is not capture.

The exact decision:

- No capture parser runs during `repl --resume`.
- No `session_capture.flag`, `readback_args`, `json_flag`, or
  `last_message_flag` are injected.
- The invocation row gets the known `session_id` by calling the
  existing atomic DB writer directly, not through generic capture
  parsing.
- `invocations.session_capture_method` is set to the new persisted
  string `"resumed"`.

The resume path calls
`update_session_capture(id, Some(session_id), "resumed")` immediately
after provider lookup and model-pool validation succeed and before
spawn. That timing is intentional. The column records provenance of the
stored session id, not proof that the native CLI accepted it. This is
the right value tradeoff: under V10, a long-running interactive `repl`
must be inspectable while it is still running, so `trace <invocation>`
from another shell should show which session the runner is trying to
resume rather than hiding that intent behind `NULL` until finalize;
under V13, the user supplied an explicit composite target and the runner
should persist it immediately; under V11, the runner should record what
it explicitly did across the process boundary, namely "asked the child
to resume `<id>`," rather than pretending to know whether the child
accepted it. This matches the existing semantics of method strings like
`"forced_flag_verified"`, which mean "the runner verified it passed the
flag," not "the session was definitely valid." If the child later exits
with "No conversation found," the row still truthfully says "the runner
attempted an explicit resume of this session id"; success and stderr
carry the runtime outcome.

PR-F therefore keeps the pre-spawn write and instead extends the trace
read path to recognize `"resumed"` as an attempted resume target rather
than a confirmed session attach. This follows the same established
pattern already present for `"failed"` warnings in `trace/mod.rs`: the
renderer still resolves transcript path and turn counts, because the
target session does exist on disk and that context is useful under V10,
but it also pushes a warning explaining that child acceptance is not
confirmed by this row and that the caller should inspect `exit_code` and
recent errors for the outcome. JSON output stays additive with the
existing `capture_method` field; the text renderer may label the line as
`Resume target:` instead of `Session:` when the method is `"resumed"`.

For plain `repl <model>` with no `--resume`, the runner writes
`session_capture_method = "none"` on completion and leaves
`session_id = NULL`. This initiative does not invent a new
interactive-session capture strategy.

# 7. Schema additions

No new columns are required, but PR-F does require one new index.
Shortcut F1 is correct: all current `session_turns` indexes lead with
`provider_name`, so a bare `WHERE session_id = ?` lookup will full-scan
the table. That is not acceptable under V8.

PR-F therefore adds:

```sql
CREATE INDEX IF NOT EXISTS idx_session_turns_session_lookup
    ON session_turns (session_id, timestamp);
```

Add the index to `session_turns_index_sql()` for fresh bootstrap and to
the additive schema-ensure path for existing DBs. Keep the existing
provider-leading indexes; they still serve ingest and provider-scoped
trace queries. This is a small migration step, not a separate
optimization proposal.

# 8. README updates

- `README.md`: document `repl`, `interactive_args`,
  `[providers.resume]`, and the persisted `"resumed"` / `"none"` markers.
- `scripts/README.md`: clarify that `session_capture` remains the
  one-shot/fresh-session mechanism and is intentionally bypassed during
  `repl --resume`.

# 9. Anti-scope

- No mid-session quota rebalance after a REPL attaches to a provider.
- No OpenCode/GLM interactive or resume contract in this initiative.
- No fork semantics (`claude --fork-session`, `codex fork`).
- No multi-machine session migration or reconstruction if the
  provider's local store has lost the session.
- No model continuity validation on resume.
- No partial UUID or prefix matching.
- No prompt-bearing `repl --resume <id> "continue"` surface in v1.
- No fresh interactive session-id capture mechanism beyond what later
  session ingestion may learn from raw logs.
- No interactive diagnostics classification from inherited stderr.

# 10. Tradeoff justification recap

- `V1/V2/V3`: Claude vs Codex differences still live in
  `interactive_args` and `[providers.resume]`, not in command-name
  branching. The runner stays generic.
- `V8`: provider lookup is still lazy, but now it is also cheap on use
  because PR-F adds `idx_session_turns_session_lookup` for the exact
  query the feature needs.
- `V10`: missing resume hits, provider/model mismatches, and resumed
  provenance all surface explicitly. Resume selection also becomes
  visible at a TTY via `[resume] -> X`; nothing silently falls through
  to a fresh session or a different provider.
- `V10/V14`: this proposal explicitly revises the synthesis fallback
  contract for `interactive_args`. The synthesis said absent
  `interactive_args` should fall back to `args`, but the problem
  research showed current repo-owned `args` encode one-shot entry points
  like Claude `-p` and Codex `exec`. Reusing them would silently launch
  the wrong surface, so `repl` fails clearly instead.
- `V13`: a bare input UUID is acceptable because provider lookup is the
  feature, but once resolved, provider locality remains explicit and
  load-bearing. That is why a mismatched `<MODEL>` errors instead of
  being ignored.
- `V14`: `repl` is a new subcommand, not a flag bolted onto the old
  prompt path. That avoids ambiguous interaction with the existing
  positional parser.
- `V15`: the caller still controls verbose detail. The runner always
  emits a one-line resume selection summary, but multi-provider detail
  stays gated to non-TTY stderr or a future `--verbose`.
- `V16`: the config shape stays deliberately simple. I am not taking a
  `common_args` + delta mini-language until it buys real value rather
  than theoretical neatness.
- `V11`: parent-child relationships still cross the process boundary
  only via `OULIPOLY_PARENT_INVOCATION`, and resume correlation uses the
  explicit user-supplied session id, not timestamp inference.

# 11. Open questions and explicit followups

- PR-F must runtime-verify Codex composition end to end, not just
  syntactic parseability. The open question is whether top-level Codex
  options in `interactive_args` are actually honored when composed with
  the `resume` subcommand at runtime.
- Windows console control handling remains a real platform question.
  `Stdio::inherit()` is portable, but the exact interaction between
  `CTRL_C_EVENT`, parent survival, and guaranteed finalization needs
  Windows-specific integration coverage.
- Stranded `running` row reconciliation is a separate initiative. Per
  V8/V16, it does not belong in PR-E or PR-F. Per V10 and Init-01's
  `trace`, stranded rows are already observable; what is missing is an
  opinionated resolution pass, which is its own design question. Per
  V11, this proposal does not guess an outcome for a row it cannot
  actually classify.
- The known followup on config duplication remains explicit: if
  first-party config maintenance shows repeated `args` /
  `interactive_args` drift, replace the duplicated arrays with a shared
  prefix shape in a later proposal.
- The known Init-01 followup remains: Claude print-mode `--session-id`
  does not prove resumability. This proposal intentionally does not
  reinterpret or fix that behavior.
- Fresh interactive launches still rely on later session ingestion, not
  immediate capture, for cross-provider resume lookup.

# 12. PR decomposition

PR-E: `repl` subcommand + interactive launch

- In: `Subcommands::Repl`; `interactive_args`; interactive executor with
  TTY inheritance; stderr gating for `OULIPOLY_INVOCATION`; invocation
  lifecycle guard with explicit no-op-after-finalize behavior;
  interactive parent-env propagation; config-shape tests.
- Out: `--resume`; provider lookup; `[providers.resume]`; session lookup
  index; `"resumed"` persistence.
- Estimated lines: 320-430 Rust plus 70-110 tests/docs.
- Tests: clap parsing; provider-config round-trip for
  `interactive_args`; canonical Claude/Codex config-shape drift tests;
  interactive command assembly; TTY-safe stderr gating;
  finalize-on-success/failure/panic guard behavior; signal-handling
  integration on Unix.
- User value: launch balanced interactive Claude/Codex sessions without
  remembering startup flags.
- Dependencies: none beyond current main.

PR-F: cross-provider resume

- In: `--resume <session-id>` on `repl`; `ResumeStrategy`; ordered
  provider lookup from `session_turns`; model-pool validation and
  helpful mismatch errors; always-on `[resume] -> <provider>` logging;
  duplicate-detail logging gate; direct
  `update_session_capture(id, Some(session_id), "resumed")`;
  `session_capture_method = "resumed"`; trace rendering that treats
  `"resumed"` as an attempted resume target, still resolves transcript
  path and turn counts, and warns that child acceptance is unconfirmed;
  text trace labeling may render `Resume target:` for that case while
  JSON stays additive; full-UUID validation;
  `idx_session_turns_session_lookup` migration/bootstrap.
- Out: new interactive capture strategy for fresh sessions; prefix
  lookup; non-Claude/Codex resume expansion.
- Estimated lines: 290-390 Rust plus 80-120 tests/docs.
- Tests: TOML round-trip for `flag` and `subcommand`; validate
  failures; session lookup ordering; index presence; duplicate
  selection; one-line resume logging; not-found error; provider/model
  mismatch error with suggestions; resumed-session persistence; resume
  command assembly for Claude and Codex; runtime Codex resume
  verification; a finalized `repl --resume` row with
  `session_capture_method = "resumed"` and a non-zero exit code emits
  the attempted-resume warning in trace output.
- User value: a copied session UUID is enough to re-enter the right
  provider session without remembering which account owns it, and the
  runner's choice is visible.
- Dependencies: PR-E, because resume reuses the interactive executor and
  `repl` entrypoint.

# 13. Revision Log

For each medium-or-higher finding, this section records whether the
review identified a real value violation, a value-neutral preference, or
a suggestion that would itself conflict with the values. The scope risk
report had no medium-or-higher findings.

| Finding | Category | Original stance | Revised stance | Value cited | Scope impact |
|---|---|---|---|---|---|
| Shortcut F1 (load-bearing) | value-aligned correction | "If profiling later shows..." deferred the bare-`session_id` lookup cost. | Add `idx_session_turns_session_lookup` on `(session_id, timestamp)` in PR-F and include it in both bootstrap and additive ensure paths. | V8 | Adds one index migration/bootstrap step and a small schema test in PR-F. |
| Shortcut F2 (medium) | value-neutral preference | Separate `args` and `interactive_args` were presented as the final shape without acknowledging drift risk. | Keep the duplication deliberately, document why `common_args` + delta is not worth the extra shape yet, add canonical config-shape tests, and carry an explicit followup if drift becomes recurrent. | V1, V3, V16 | No new runtime surface beyond tests/docs; avoids adding a more complex arg mini-language in v1. |
| Shortcut F3 (low-medium) | value-aligned correction | Duplicate-resolution logging was fully gated on `!isatty(stderr)`, hiding the runner's choice from the users who benefit most from seeing it. | Always emit a brief `[resume] -> <provider>` to stderr; keep longer duplicate-detail logging gated to non-TTY stderr or a future `--verbose`. | V10, V15 | Adds one unconditional short stderr line on resume and preserves caller control over verbosity. |
| Audit F1 (medium) | value-aligned correction | Proposal said `<MODEL>` is required but did not define what happens if the resumed provider is outside that model's provider list. | Error with a helpful message: session belongs to provider `<X>`, which is not in model `<Y>`'s provider pool; suggest models that include `<X>`. | V10, V13 | Adds one validation branch and suggestion text in PR-F. |
| Audit F2 (medium) | value-aligned correction | Codex `interactive_args ++ resume` composition was plausible and syntactically legal, but the proposal did not require runtime verification. | Add explicit PR-F runtime verification that top-level Codex interactive options are honored when composed with `resume`, and record that ask in §11. | V10 | Adds one end-to-end test requirement in PR-F. |
| Audit F3 (medium) | value-aligned correction | RAII finalizer was described, but the happy-path/double-finalize interaction was not spelled out. | Require the guard to no-op after explicit finalization via a `finalized: bool` flag or equivalent defensive wrapper, and test that path in PR-E. | V10 | Adds a small lifecycle guard detail and one focused test case in PR-E. |
| Audit F1 (round 2) | synthesis revision | The synthesis said absent `interactive_args` should fall back to `args`, and the round-1 text changed that behavior without calling out the contract change. | Make the revision explicit: `repl` errors unless `interactive_args` is declared, because the problem research showed current `args` encode one-shot surfaces like Claude `-p` and Codex `exec`; silent fallback would launch the wrong surface. | V10, V14 | No new runtime scope; records the safer contract revision explicitly in §3 and §10. |
| Shortcut F2 (round 2) | value-aligned clarification | The signal-handling paragraph read too much like a solved cross-platform rule even though Windows remained open. | Mark the strategy explicitly as Unix behavior, and make Unix signal-handling integration tests part of PR-E scope while keeping the Windows question open in §11. | V10 | No scope expansion; tightens wording and verification expectations only. |
| Shortcut F3 (round 2) | value-conflicting suggestion | Add a startup/open-time reconciliation pass that scans the DB for stranded `running` rows and auto-finalizes them. | Defer that work explicitly: per V8/V16, startup-time reconciliation is a separate initiative; per V10 and Init-01 `trace`, stranded rows are already observable; per V11, auto-resolution would guess an outcome the runner does not know. | V8, V10, V11, V16 | No change to PR-E or PR-F; records stranded-row resolution as a separate future design question. |
| Shortcut F1 (round 3) | value-aligned correction | Add `mark_resumed_session()` as a dedicated DB writer for `session_id` / `session_capture_method` because resume provenance is semantically distinct from post-output capture. | Drop the second writer and call `update_session_capture(id, Some(session_id), "resumed")` directly at the resume lifecycle point; the `"resumed"` method string preserves the semantic distinction without creating a second API for the same column pair. | V5 | No new runtime scope; keeps one atomic writer for the column pair and avoids future drift between duplicate persistence paths. |
| Audit F1 (round 4) | value-aligned correction | Pre-spawn persistence of `update_session_capture(id, Some(session_id), "resumed")` risked being read by `trace` as confirmed session correlation, which would overstate what the runner actually knows after a failed resume attempt. | Keep the pre-spawn write because it is the correct V10/V13/V11 representation of runner intent, and extend `trace/mod.rs` to treat `"resumed"` as an attempted resume target: still show transcript path and turn counts, but emit a warning that acceptance is unconfirmed and the caller should inspect `exit_code` / recent errors. | V10 | Adds about 30 lines of Rust in `trace/mod.rs` plus one focused PR-F trace test; no data-model change. |
