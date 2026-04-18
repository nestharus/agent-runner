# PR-B Multi-Concern Review

**Verdict: SINGLE-CONCERN. Do not split.**

## Scope of the diff

On top of PR-A, PR-B adds 1,294 lines across six files. The internal
structure is:

1. `StateDb::list_invocation_children` — one new query method on the
   existing `state::db` module (~25 lines + 3 unit tests).
2. `src-tauri/src/trace/mod.rs` — the entire trace module: `TraceOptions`,
   `trace_invocation`, ASCII renderer, `serde`-serialized JSON report,
   cycle/depth-limit guards, inline-transcript placeholder plumbing
   (~720 lines including inline tests).
3. `main.rs` CLI refactor — `Cli` gains `command: Option<Subcommands>`,
   a new `Subcommands::Trace` variant, a `run_trace_command` dispatcher,
   and parse-level tests; `args_conflicts_with_subcommands = true`
   preserves the bare `agents -m model "prompt"` flow.

## Could any piece ship independently?

**The DB method alone.** `list_invocation_children` has no caller outside
the trace module. Shipping it as a standalone PR produces dead code at
merge time and would violate V16's "deliver visible user value (or be a
strict prerequisite for one that does)" only in the narrowest sense — it
is a prerequisite, but one trivial enough that the boundary adds review
overhead without reducing review difficulty.

**The CLI Subcommand refactor alone.** In principle, `Cli` could be
restructured to accept an empty or stub `Subcommands` enum in one PR,
then `Trace` added on top. But an empty-variant enum has no user-facing
behavior change, and clap with a zero-variant subcommand is awkward. A
"refactor scaffold" PR with no variants delivers nothing observable at
merge — the existing `agents -m model "prompt"` flow looks identical
before and after. This fails V16's user-value bar.

**The trace module alone.** Not possible without the CLI refactor — the
module has no entry point. And without `list_invocation_children`, the
tree walker cannot load children; the walker is its only consumer.

## Mutual load-bearing

The three pieces fit V16's explicit carve-out: "Bundle only when
splitting introduces real coupling pain." Here the coupling is not
incidental — it is the minimum viable surface for one user-observable
feature (`agents trace <uuid>`). The DB method exists to serve the tree
walk; the tree walk exists to serve the subcommand; the subcommand
exists to serve the user.

The proposal's §12 already treats PR-B as a single atomic unit (300–420
Rust + 40–70 tests/docs). The actual diff (~1,294 lines) runs hotter
than estimated, driven mostly by the trace module's ASCII renderer,
JSON shape, and inline tests — but that volume sits inside one concern,
not across several.

## Scope-creep check

The `TraceSession` struct carries `capture_method`, `transcript_path`,
`turn_count`, `assistant_turn_count`, and `sidechain_turn_count`. These
are all null/unresolved in PR-B and only populate once PR-C/PR-D land.
That could look like scope creep, but the contract at
`tmp/01-pr-b-contract.md` §"JSON output" defines those fields
explicitly as part of PR-B's output shape, precisely so the JSON
schema is stable when PR-C/PR-D ship data for them. Pre-declaring the
shape in PR-B avoids schema churn — a value-aligned choice, not a
concern bundle.

The `--transcript` and `--inline-transcript` flags are similarly
accepted in PR-B with placeholder behavior. Deferring the flags to
PR-C would force a CLI-contract change mid-sequence; accepting them
with documented placeholder output is cleaner and matches the
contract.

## Recommendation

Ship PR-B as-is. The three internal pieces are the minimum that
delivers "user can run `agents trace <uuid>`" at merge time, which is
the unit of user value promised by proposal §12 PR-B. No meaningful
split exists that respects V16's user-value bar.
