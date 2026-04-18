# PR-C Justification Review

**Verdict: MOSTLY TIGHT, with two principled contract deviations worth flagging**

The bulk of the diff maps directly to `tmp/01-pr-c-contract.md`. Two
items diverge from the literal contract — both deliberately, both
appealing to `VALUES.md`. I think both are correct calls but they
are deviations the author should be on record about.

## Hunk-by-hunk classification

### `src-tauri/src/config/model.rs` — `SessionCapture` + `validate()`
**In scope (struct, enum, parse, round-trip).** All four `kind`
variants required by contract §"Test contract" item 1; serde-tagged
enum pattern matches §"Implementation pattern requirements". TOML
round-trip helpers (`append_session_capture_toml`, etc.) are the
unavoidable cost of an existing hand-rolled `to_toml`.

**Adjacent (justified by V10): `SessionCapture::validate()`** —
contract §"Test contract" item 1 only requires "Reject unknown kind",
which serde's tagged enum already covers. `validate()` adds rejection
of *incomplete* configs (FFV without `flag`, SJE without
`json_flag`/`last_message_flag`/`event_type`/`event_id_path`). The
contract does not list this method, so strictly it is creep, but the
inline rationale is sound: without it, a malformed TOML would silently
degrade to `Failed("session_capture.flag is required")` at every
invocation rather than fail loudly at config-load time. V10 ("Failures
are observable, never silent") supports loud-at-load over
silent-at-runtime. Keep, but note the deviation.

### `src-tauri/src/config/sessions.rs` — `transcript_locator` field
**In scope.** Exactly the optional field defined in contract
§"TranscriptLocator contract", with a parse test for absence (maps
to `transcript_state = "no_locator"` per contract §"Trace integration"
step 3). Tight.

### `src-tauri/src/config/mod.rs` — re-exports
**In scope.** Mechanical export of new `SessionCapture` /
`SessionCaptureKind` types.

### `src-tauri/src/state/db.rs` — schema columns + index + `update_session_capture`
**In scope (columns, index, migration paths).** The two new columns,
the partial `idx_invocations_provider_session` index, the
`map_invocation_row` widening, and the migration ALTER TABLE branch
all match contract §"Schema additions" verbatim.

**Contract deviation (justified by V10):
`update_session_capture(_, None, "none")` is no longer a no-op.** The
contract literally reads:

> No-op (returns Ok) for None/None.

The implementation instead always writes both columns. The author's
inline comment cites V10 to argue that an explicit `"none"` on a
completed row is "a positive signal distinct from NULL (the row was
never finalized)."

Two observations on this:

1. The V10 framing is reasonable — explicit `"none"` is more
   self-describing than NULL — but the contract author already
   considered this and chose no-op. The implementer overrode that
   choice unilaterally. That should have been a contract amendment,
   not a code-only override.
2. The justification is partially undercut by the rest of the code:
   `trace/mod.rs:build_trace_session` uses
   `record.session_capture_method.clone()` and treats NULL identically
   to `Some("none")` — neither produces a warning, both fall through
   to the same `Unresolved` branch. So the V10 distinction the comment
   defends does not yet materialize in observable behavior.

Net: the change is harmless and arguably cleaner, but the spec-vs-code
divergence should be reconciled (either amend the contract to "always
writes" or restore no-op semantics). Flag, do not block.

The four DB tests are well-justified — including the explicit
`update_session_capture_none_none_persists_none_marker` test that
encodes the divergent behavior.

### `src-tauri/src/executor/mod.rs` — `SessionCaptureResult` / `Method`
**In scope.** The widened `ExecutionResult`, `SessionCaptureResult`
struct, and `SessionCaptureMethod` enum are exactly the shape in
contract §"Executor dispatch contract" step 3. `db_value()` helper is
a small mechanical mapping to the DB enum — keeps the executor type
the source of truth.

### `src-tauri/src/executor/cli.rs` — capture-aware dispatch
**In scope.** `build_capture_plan` / `finalize_capture` /
`maybe_restore_plain_stdout` realize the three-phase dispatch
(pre-spawn arg injection, post-spawn parse, plain-text restoration)
specified in contract §"Executor dispatch contract". `CapturePlan`'s
three variants mirror `SessionCaptureKind`. Generic dispatch — no
`match cli_name`, V1/V3 honored.

`temp_file: Option<PathBuf>` widened to `temp_files: Vec<PathBuf>` so
the SJE tmpfile and the existing large-prompt tmpfile can both be
cleaned up. Necessary consequence of the SJE path; not creep.

Five new fixture-script tests cover contract §"Test contract" item 3
(`None`, FFV happy / mismatch, SJE happy / no-event). The FFV happy
test additionally proves argv injection (the `--session-id` value
matches the captured id, and `readback_args` are present) — that's
sharper than the contract requires but is the right shape for the
"forced" half of "forced flag verified."

### `src-tauri/src/main.rs` — sessions.toml load + capture wiring
**In scope.** Contract §"Lifecycle integration" steps 7–8 (call the
capture-aware path, persist via `update_session_capture`) and §"Trace
integration" (load `sessions.toml` once, pass through). The
`[session-capture]` stderr line on `Failed(_)` is V9/V10 compliant
(metadata on stderr, failures observable).

`trace_invocation_with_sessions` rather than overloading
`trace_invocation` — see trace section below.

### `src-tauri/src/sessions/mod.rs` — `locate_transcript` + `run_session_script`
**In scope.** Generalizes the existing `run_turn_script` per contract
§"Locator script invocation" ("reuses the existing `run_turn_script`
infrastructure … generalize it if needed; do NOT duplicate"). The
old `run_turn_script` becomes a one-line wrapper around
`run_session_script`. Single-line stdout enforcement, non-zero exit
mapping to `Err`, and the four locator tests all match the contract.

`capitalize_script_kind` helper is a small one-off to keep error
messages reading naturally ("Turn script timed out" vs "turn script
timed out"). Borderline — could be inlined — but trivial. Keep.

### `src-tauri/src/trace/mod.rs` — locator wiring
**In scope.** `build_trace_session` realizes contract §"Trace
integration" steps 1–5 exactly: NULL session → `unresolved`,
session-but-no-locator → `no_locator`, locator + extant file →
`available`, locator + missing file → `missing`, locator-error →
`missing` + warning. The "session capture failed" warning when
`session_capture_method = "failed"` is V10-justified.

**Note: `trace_invocation_with_sessions` as a sibling, not a
replacement.** The contract says (§"Trace integration"):

> `TraceOptions` (or the trace_invocation entry point's signature)
> gains an optional `&SessionsConfig` parameter.

The implementation chose to add a new function rather than widen the
existing one, leaving `trace_invocation` as a thin
`(.., None)`-passing wrapper. This violates V14 ("No backwards-compat
shims for internal code") in spirit — there is no external caller
that needs the old signature; the only call site outside tests is
`main.rs`, which was updated anyway. The cleaner shape per V14 would
be to delete `trace_invocation` and have `main.rs` and tests call the
single widened entry point. Flag, do not block.

### `scripts/claude-code-locate-transcript` and `scripts/codex-locate-transcript`
**In scope (filename match path).** Contract §"Test contract" item 7
specifies filename-based lookup for both adapters. The Python-via-bash
heredoc pattern matches `scripts/claude-code-turns` and
`scripts/codex-turns` per contract §"Implementation pattern
requirements".

**Adjacent (mildly creep): content-based fallback in both scripts.**
The contract test only requires walking
`~/.claude/projects/**/<session_id>.jsonl` and
`~/.codex/sessions/**/rollout-*-<session_id>.jsonl`. The scripts add
a second pass that re-walks every `*.jsonl` (Claude) /
`rollout-*.jsonl` (Codex) and parses each line as JSON, looking for
`sessionId` / `session_meta.payload.id`. The inline comments justify
this as defense against future Claude/Codex versions changing
filename conventions while keeping the id internally.

Two concerns:

1. The fallback only runs when the filename match misses, so the
   common-case cost is zero — that's fine.
2. But it doubles the worst-case cost when a session genuinely doesn't
   exist (two full tree walks instead of one), and it adds line-by-line
   JSON parsing of every transcript. On a real Claude tree of "thousands
   of session files" (the comment's own framing), the fallback's miss
   path could be visibly slow.

The defense-against-future-drift argument is reasonable but
forward-looking; V8 ("lazy on use, not eager") and the contract's
test minimalism suggest the simpler shape would be filename-only,
with the fallback added when an actual schema change makes it
necessary. Mild scope creep, justified inline. Keep but note.

The path-injection comments (rgblob with literal `*.jsonl` then
filtering, rather than passing `session_id` into a glob pattern) are
a real correctness concern, not creep — `SESSION_ID` arrives via env
from the locator runner, but the defensive shape is right.

### `src-tauri/tests/pr_c_locator_scripts.rs`
**In scope.** Four integration tests covering both scripts' filename
path and Codex's content-fallback path. The Claude content-fallback
path is exercised only indirectly through the first test (which writes
content matching `sessionId` but uses an unrelated filename — though
actually it uses `session.jsonl` as filename, which doesn't match
`<session_id>.jsonl`, so it does exercise the fallback). Coverage of
the fallback path justifies its inclusion (per the previous section's
caveat).

## Things explicitly checked and clean

- No `match cli_name { "claude" => ..., "codex" => ... }` anywhere
  (V1, V3 respected).
- No hardcoded `~/.claude/projects/` paths in runner code (V1, V2).
- Locator runs lazily at trace time, never at invocation time (V8).
- No new Cargo dependencies (`serde_json` and `uuid` were already in
  the workspace).
- Executor public API only widened (existing `ExecutionResult`
  callers updated in lockstep), no parallel "v2" entry point (V14
  respected on the executor surface — contrast the trace deviation
  above).
- Stderr (`[session-capture]`) for diagnostics, stdout preserved for
  the model response, including the SJE tmpfile-restore path that
  keeps stdout binary-safe (V9).
- `SessionCaptureMethod::Failed(reason)` carries the failure reason in
  memory and stderr, but `db_value()` collapses to `"failed"` in the
  column — matches contract §"Schema additions" comment ("the reason
  is logged to stderr at execution time and surfaced in trace
  warnings; not stored in this column to keep it a fixed enum").
- PR-D scope (`session_turns.parent_turn_id`, `is_sidechain`,
  `claude-code-turns` widening) is untouched.

## Summary

PR-C is largely tight. Three notes for the author:

1. **`update_session_capture(_, None, "none")` writes "none" instead
   of being a no-op.** Contract says no-op, code says always-write,
   inline V10 justification is reasonable but not yet load-bearing in
   trace. Recommend amending the contract or restoring the no-op.
2. **`trace_invocation_with_sessions` as a sibling.** V14 prefers
   widening the existing function and updating call sites; the
   sibling/wrapper shape adds a permanently-dead default-`None` path.
3. **Content-fallback in locator scripts** is forward-defensive
   beyond the contract. Inline comments justify it; reasonable to
   keep but flagged for future trimming if it ever shows up in
   profiles.

`SessionCapture::validate()` is also a contract addition, but the V10
case for catching malformed configs at load time is strong enough that
I would not push back on it.

Verdict: **MOSTLY TIGHT** — no blocking creep, two intentional
contract deviations and one defensive script path that the author
should be on record about.
