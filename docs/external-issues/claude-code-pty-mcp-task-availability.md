# Claude Code PTY MCP `tools/call` dispatch gap

## Status

Disposition: file local only (draft, do not submit).

Submitted to: not yet; gated behind explicit root authorization per AGE-115 proposal § Disposition.

Decision authority: AGE-115.

Last updated: 2026-05-16.

## Summary

In interactive PTY mode, Claude Code 2.1.141 through 2.1.143 connects the MCP server and dispatches `tools/list` successfully, but does NOT dispatch `tools/call` when launched with `--tools mcp__<server>__<name>,...`. The same Claude Code binary calls `tools/call` correctly under `--allowedTools mcp__<server>__<name>,...` and under no tool filter. Print mode (`claude -p` / `--print --output-format stream-json`) calls `tools/call` for all three flag shapes. The framing from the AGE-104 V4 verdict is a main-PTY tool-dispatch/tool-registry issue, NOT a config inheritance issue.

## Affected Claude Code Versions

Observed affected range: `2.1.141 through 2.1.143`.

This is the observed range as of AGE-104 V4 research on 2026-05-15; future versions may regress, fix, or change the behavior. See `docs/architecture/claude-proxy-mcp-launch-shape.md` § `Affected Claude Code Versions` for the maintainer-facing runbook.

## Expected vs Actual Behavior

Expected: `tools/call` dispatches in all four mode/flag combinations that complete `tools/list`, including `M3-C1`, `M3-C2`, and `M3-C3`.

Actual: `tools/call` is silently dropped, or `No such tool available` is returned, only for the `M3-C1` cell: interactive PTY plus `--tools mcp__<server>__<name>,...`. `M3-C2` with `--allowedTools mcp__<server>__<name>,...` and `M3-C3` with no tool filter both dispatch `tools/call`.

## Truth Table Summary

| Mode | C1 --tools | C2 --allowedTools | C3 no filter |
|---|---|---|---|
| M1 print -p | call | call | call |
| M2 print stream-json | call | call | call |
| M3 interactive PTY with hooks | no call | call | call |
| M4 interactive PTY without hooks | call | call | call |

## Minimal Reproducer

The canonical proof commands are the PR #90 harness paths `prototype-tests/age-104-pty-mcp-gap/p2-proof-tests.sh` and `prototype-tests/age-104-pty-mcp-gap/p2-truth-table-harness/run-all.sh` on https://github.com/nestharus/agent-runner/pull/90, branch `prototype-tests-age-104-pty-mcp-gap`, tip `b3347b7`.

The harness also uses `prototype-tests/age-104-pty-mcp-gap/p2-truth-table-harness/server.py` and asserts that `M3-C1` fails while `M3-C2` and `M3-C3` succeed. Dependencies are `python3` for `server.py`, `util-linux script` for PTY emulation because `expect` and `pexpect` were unavailable on the AGE-104 host, and the Claude Code binary at `/home/nes/.local/share/claude/versions/2.1.143`.

The harness contents are intentionally not inlined here; PR #90 is the source of truth.

## Local Workaround

Use `--allowedTools mcp__<server>__<name>,...` or omit the tool filter; do NOT use `--tools mcp__<server>__<name>,...` for interactive PTY MCP replacement launches.

See `docs/architecture/claude-proxy-mcp-launch-shape.md` as the maintainer-facing operational runbook.

## Suggested Fix Surface

Investigate the main-PTY tool-dispatch / tool-registry path inside Claude Code. The bug is consistent with `--tools` suppressing MCP-registered tool dispatch in PTY mode while leaving `tools/list` intact. Hooks, transport, naming, schema, and config-path inheritance are invalidated as causes per AGE-104 V1-V4 evidence.

## Invalidated Hypotheses

- TOOL-ALLOWLIST: supported and narrowed; `--tools` specifically suppresses MCP `tools/call` dispatch in PTY, while `--allowedTools` and no filter do not.
- TRANSPORT: invalidated; PTY can call MCP tools backed by the tested server transports.
- INPUT-MODE / TTY: invalidated as direct cause; true PTY is the boundary, but the load-bearing cause is the PTY `--tools` launch shape.
- NAMING: invalidated; `mcp__<server>__Task`, `mcp__<server>__AgentTask`, dispatch aliases, and unrelated names work in PTY under `--allowedTools`.
- CONFIG-INHERITANCE: invalidated; config paths differ, but loaded config sources show the same print-success and PTY-`--tools`-failure pattern.

## Evidence Pointers

- `dossier/answer.md`
- `dossier/evidence/p2-truth-table.md`
- `dossier/evidence/p2-proof-tests.sh`
- `dossier/evidence/v1-verdict.md`
- `dossier/evidence/v2-verdict.md`
- `dossier/evidence/v3-verdict.md`
- `dossier/evidence/v4-verdict.md`
- `dossier/evidence/v4-version-research.md`

These are internal planning artifacts NOT part of any upstream submission. The upstream submission would carry only the Summary, Truth Table, Minimal Reproducer link to PR #90, and Expected vs Actual sections.

## File-vs-Decline Rationale

AGE-104 evidence is sufficient to draft a clean upstream report, so a local record is more useful than declining to file. Public submission still requires explicit root authorization because publishing to `anthropics/claude-code` has HIGH blast radius and creates a durable upstream URL. The local draft preserves the option value of a clean later submission without forcing a public side effect during AGE-115. A future authorized Phase 6 step may run `gh api repos/anthropics/claude-code/issues` with this file's content as the body.

## Caveats

- Binary path matters: cite `/home/nes/.local/share/claude/versions/2.1.143`, not `/home/nes/.local/bin/claude`.
- Under `--tools` in PTY there are two observed failure shapes: Mode A returns `No such tool available`; Mode B lists the MCP tool but never sends `tools/call`.
- The P2 harness requires `--verbose` with `--print --output-format stream-json` on Claude Code 2.1.143.
- `expect` and `pexpect` were unavailable on the test host; the truth table relies on `util-linux script`.

## See Also

- `docs/architecture/claude-proxy-mcp-launch-shape.md`
- AGE-104 dossier path: `/home/nes/projects/agent-runner/planning/prototype-age-104-pty-mcp-gap/dossier/`
- PR #90: https://github.com/nestharus/agent-runner/pull/90
- AGE-115 ticket: https://linear.app/oulipoly/issue/AGE-115/file-or-explicitly-decline-upstream-claude-code-bug-report
- Sister tickets: AGE-101, AGE-102, AGE-103, AGE-113, AGE-114.
