# Audit Risk Assessment: proposals/02-interactive-resume.md

## Verdict: LOW

The proposal's high-risk factual claims hold up against the current Claude/Codex CLIs, the current Rust/Tauri code, and spot-checked citations; the remaining issues are low-risk implementation cautions, not reasoning failures.

## Findings (severity ≥ medium)

No medium-or-higher findings.

The highest-risk claims verified cleanly:

- Claude exposes `-r, --resume [value]`, and the help text treats the value as a session ID or optional search term, so a bare UUID input is valid.
- Codex uses `resume` / `exec resume` subcommands, not a top-level `--resume` flag.
- `session_turns` already stores `provider_name`, `session_id`, and `timestamp`, and the current schema includes `(provider_name, session_id, timestamp)` indexes.
- The current `session_turns` indexes do not support cheap bare `WHERE session_id = ?` lookup; on the live DB, `EXPLAIN QUERY PLAN` showed a scan, so the proposal's new `session_id`-leading index is justified.
- On Unix, terminal-generated `SIGINT` goes to the foreground process group; with the default inherited process group, the child receives Ctrl-C directly, so the proposal is correct that the parent does not need to forward terminal-generated `SIGINT`.
- `std::io::IsTerminal` is stable since Rust 1.70.0, the import path is correct, and the repo already uses it for stdin TTY detection.
- The current DB and trace code do not enforce a closed enum for `session_capture_method`; the proposal's explicit trace handling for `"resumed"` is sufficient to keep the design coherent.
- `finalize_invocation()` already errors on double-finalize, so the proposal's RAII guard requirement to no-op after explicit finalize is the right composition with the existing lifecycle.
- Adding `Subcommands::Repl` matches the current parser architecture. Residual low risk: `repl` becomes a reserved first token, just as `trace` already is.

## Synthesis adherence check

- Section 4 / V13 composite identifiers: honored.
- Section 4 / V8 lazy on use: honored-with-caveat. The synthesis assumed the existing provider-led index was enough; the proposal correctly revises this and adds a `session_id`-leading index after verifying the current query would scan.
- Section 4 / V10 failures observable: honored.
- Section 4 / V1/V2/V3 declarative, not procedural: honored.
- Section 4 / V14 no compat shims: honored-with-caveat. `repl` fits the existing `Subcommands` shape, but it also reserves another first-token keyword.
- Section 4 / V15 surface choice belongs to the caller: honored.
- Section 4 / V11 explicit propagation, not inference: honored.
- Section 6 / `interactive_args` shape: honored-with-caveat. The synthesis fallback-to-`args` contract is explicitly revised, not silently dropped, and the revision is justified by current one-shot provider args such as Claude `-p` and Codex `exec`.
- Section 6 / `[providers.resume]` shape: honored.
- Section 6 / `repl` gets an invocation row: honored.
- Section 6 / stderr gating: honored.
- Section 6 / provider-not-found path: honored.
- Section 6 / cleanup on subprocess crash: honored.
- Section 6 / PR-C followup remains deferred: honored.
- Section 6 / Codex required-flags open question: honored. The proposal keeps runtime verification explicitly open instead of pretending the syntax question is already settled.
- Section 6 / Windows TTY handoff open question: honored. The proposal keeps it open and scopes its concrete claims to Unix.
- Section 6 / PR-C composition question: honored. The proposal resolves it explicitly by bypassing capture on resume and teaching trace to treat `"resumed"` as attempted-resume provenance.

## Recommended revisions (if any)

- Keep the proposal's explicit `trace` handling for `"resumed"` as a must-implement part of PR-F, not an optional follow-on.
- Add one clap regression test documenting that `repl` is now a reserved first token, so the compatibility tradeoff is explicit.
- Add one schema/query-plan test for the new `idx_session_turns_session_lookup` path, since that index is now load-bearing for V8.
