# Claude Proxy MCP Launch Shape

## Rule

For Claude Code interactive PTY MCP replacement launches, use `--allowedTools mcp__<server>__<tool>,...` or omit any tool filter. Do not use `--tools mcp__<server>__<tool>,...` — in interactive PTY mode the `--tools` flag suppresses MCP `tools/call` dispatch even though the MCP server initializes and `tools/list` succeeds.

Print mode is unaffected: in `claude -p` (and `claude --print --output-format stream-json`) print mode, all three flag shapes — `--tools mcp__<server>__<tool>,...`, `--allowedTools mcp__<server>__<tool>,...`, and no filter — successfully reach MCP `tools/call`. The rule applies only to interactive PTY MCP replacement launches.

## Affected Claude Code Versions

`2.1.141 through 2.1.143`.

This range is observed as of AGE-104 V4 research (`2026-05-15`). Future Claude Code versions may regress, fix, or change the behavior; rerun the proof harness on each version bump (see `## Version-Bump Runbook`) before assuming the rule still applies as written.

## Proof Command

The canonical reproducer is the AGE-104 prototype harness at:

- `prototype-tests/age-104-pty-mcp-gap/p2-proof-tests.sh`
- `prototype-tests/age-104-pty-mcp-gap/p2-truth-table-harness/run-all.sh`
- `prototype-tests/age-104-pty-mcp-gap/p2-truth-table-harness/server.py`

These files live on PR #90 (<https://github.com/nestharus/agent-runner/pull/90>), branch `prototype-tests-age-104-pty-mcp-gap`, tip `b3347b7` — not on this branch. Do not duplicate the harness here; clone the prototype branch and run `p2-proof-tests.sh` directly.

The prototype MCP server `p2-truth-table-harness/server.py` distinguishes `tools/list` availability from actual `tools/call` dispatch: under PTY `--tools` the server logs that Claude Code requested `tools/list` (so the tool appears available to the model) but never dispatches `tools/call`, while under PTY `--allowedTools` and PTY no-filter the server logs both `tools/list` and a real `tools/call` dispatch. This `tools/list`-vs-`tools/call` distinction is the proof signal — printing the tool name in the listing is not equivalent to invoking it.

The harness contents are intentionally not inlined here; PR #90 is the source of truth.

## Expected Control Pattern

Interactive PTY rows (M3) — the load-bearing rows for this rule:

| Control row | Flag shape | Outcome |
|---|---|---|
| M3-C1 | `--tools mcp__<server>__<tool>,...` | FAILS — Claude Code never sends `tools/call` to the MCP server |
| M3-C2 | `--allowedTools mcp__<server>__<tool>,...` | SUCCEEDS — `tools/call` reaches the MCP server and returns a tool result |
| M3-C3 | no tool filter | SUCCEEDS — `tools/call` reaches the MCP server and returns a tool result |

Print rows (M1 = `claude -p`, M2 = `claude --print --output-format stream-json`): print mode succeeds for all three shapes — M1 and M2 succeed under C1, C2, and C3 alike. The interactive PTY mode is the only mode where the launch-shape rule changes behavior.

## Bounded Spelling Ambiguity

Two spellings of the allow-list flag exist in the code paths that touch Claude Code:

- `--allowedTools` (camelCase) — the spelling AGE-104 proved succeeds under interactive PTY MCP replacement (rows M3-C2).
- `--allowed-tools` (kebab-case) — the spelling currently emitted by the Rust executor through `crates/oulipoly-runtime/src/executor/cli/policy/orchestration.rs::apply_provider_policy` and `crates/oulipoly-runtime/src/executor/provider_specific/policy/claude.rs` (which also emits `--disallowed-tools`).

PTY MCP equivalence between `--allowedTools` and `--allowed-tools` is **not in evidence**. AGE-104 only proved camelCase `--allowedTools` for the M3-C2 positive control; kebab-case `--allowed-tools` was not exercised under interactive PTY MCP replacement on Claude Code 2.1.141–2.1.143. This runbook does not claim equivalence. Do not assume `--allowed-tools` works for interactive PTY MCP replacement until a focused harness run proves or disproves it; this is recorded as Assumption A5 in `planning/age-114-claude-launch-shape-doc/proposals/age-114-AGE-114.md`.

## Binary-Path Caveat

For reproduction, cite the version-pinned binary at `/home/nes/.local/share/claude/versions/2.1.143`. The AGE-104 truth table and proof harness used this exact path.

Do not cite `/home/nes/.local/bin/claude` for reproduction. That symlink (or wrapper) may resolve to a different installed version on a different host or after an upgrade, which silently changes the binary under test and invalidates the proof signal. Always pin to the explicit version directory.

## Upstream Issue Status

No exact upstream Claude Code issue was found as of AGE-104 V4 research (`2026-05-15`). AGE-115 tracks the file-or-decline decision for opening one upstream.

## Version-Bump Runbook

When a new Claude Code release lands (or an existing installed version changes), maintainers should:

1. Clone PR #90 (`prototype-tests-age-104-pty-mcp-gap`, tip `b3347b7`) and rerun `prototype-tests/age-104-pty-mcp-gap/p2-proof-tests.sh` against the new Claude Code binary, pinned by version directory (see `## Binary-Path Caveat`).
2. If the M3 control pattern still holds (M3-C1 fails, M3-C2 succeeds, M3-C3 succeeds), update the version range in `## Affected Claude Code Versions` to extend through the new version and record the rerun date.
3. If the control pattern changes — for example M3-C1 starts succeeding, or M3-C2 starts failing — file an updated AGE ticket capturing the new behavior, update the `## Rule` section accordingly, and amend `## Affected Claude Code Versions` to mark the boundary version where the behavior changed. Do not silently widen the rule.
4. If the bounded spelling ambiguity is resolved by an explicit `--allowed-tools` PTY MCP run, update `## Bounded Spelling Ambiguity` and Assumption A5 in the proposal.

## See Also

- `docs/architecture/provider-accounts-redesign.md` § `Example: Claude CLI Integration` — provider integration entrypoint that routes here before changing Claude PTY launch flags.
- `docs/architecture/provider-accounts-redesign.md` § `Version-Aware Integration` — version-aware provider rollout path that links to `## Version-Bump Runbook` and `## Affected Claude Code Versions`.
- `README.md` § `Interactive REPL` — operator-facing pointer for `interactive_args` editors.
- `README.md` § `providers.toml` — operator-facing pointer for `--tools` / `--allowedTools` editors of Claude provider entries.
- `AGENTS.md` § `Model Command Syntax` — repo-local agent pointer for agents constructing or modifying Claude launch argv.
- AGE-104 dossier directory: `/home/nes/projects/agent-runner/planning/prototype-age-104-pty-mcp-gap/dossier/` — full evidence including truth table, per-vector verdicts, and harness sources.
- PR #90 — <https://github.com/nestharus/agent-runner/pull/90> — canonical prototype harness.
- Production touchpoint: `crates/oulipoly-runtime/src/executor/cli/policy/orchestration.rs` (`apply_provider_policy`) delegates Claude argv emission to `crates/oulipoly-runtime/src/executor/provider_specific/policy/claude.rs` — current Rust executor path that emits `--allowed-tools` / `--disallowed-tools` (see `## Bounded Spelling Ambiguity`).
- Sister tickets: AGE-101, AGE-102, AGE-103, AGE-113, AGE-115.
