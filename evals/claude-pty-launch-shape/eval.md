# Claude PTY Launch Shape Eval

eval-id: claude-pty-launch-shape

Lifecycle state: WRITE

## Unwanted Behavior

The Claude proxy interactive PTY launch path emits or accepts a final argv shape
that pairs `--tools` with an MCP replacement tool value such as
`mcp__age104p2__Task`. AGE-104 showed this shape initializes the MCP server and
can list tools, but the PTY session does not dispatch `tools/call`, so the
replacement tool never returns to Claude.

This eval covers the Claude proxy argv contract, including typed
`tool_restrictions.claude.allowed_tools` rendering and raw `interactive_args`
pass-through. The bad shape is any effective Claude argv containing
`--tools mcp__...` or `--tools=mcp__...` in interactive PTY replacement-tool
use.

## Positive Evidence

Fire when trace, fixture, or source-equivalent argv evidence shows a Claude
proxy PTY command with `--tools` paired to any value beginning with `mcp__`.
The finding should identify whether the evidence came from typed provider
policy, raw `interactive_args`, or a rendered argv fixture.

The AGE-104 Claude Code 2.1.143 baseline is the behavioral proof: M3-C1 and
M4-C1 fail under `--tools mcp__...`, while M3-C2/M4-C2 succeed under
`--allowedTools mcp__...` and M3-C3/M4-C3 succeed with no tool filter.

## Non-Fire Cases

Do not fire when the effective argv uses `--allowedTools mcp__...`, uses the
accepted `--allowed-tools mcp__...` allowed-tools family spelling, or omits the
tool filter entirely. Do not fire on print-mode evidence by itself; the guarded
behavior is the Claude proxy interactive PTY launch shape.

## Required Trace Fields

- eval id or invocation context for `claude-pty-launch-shape`
- evidence path for the provider fixture, source-equivalent argv, or trace row
- provider command and effective argv tokens
- argv source, such as typed Claude tool restrictions or `interactive_args`
- mode cell when replaying the AGE-104 matrix, such as `M3-C1`
- observed MCP dispatch booleans: `mcp_server_initialized`,
  `tools_call_reached_server`, and `tool_returned_to_claude`

## Finding Schema

Findings must preserve the minimum eval finding fields:

- `eval_id`
- `severity`
- `evidence_paths`
- `summary`
- `suggested_action`
- `confidence`

`severity` is `HIGH` when `--tools mcp__...` appears in the effective Claude
proxy PTY argv. `evidence_paths` should name the fixture, trace artifact, or
source-equivalent argv file that proves the launch shape.

## Suggested Action

Replace the bad PTY launch shape with `--allowedTools mcp__...` when a tool
filter is required, use the accepted `--allowed-tools` family only where that
spelling is deliberately supported, or remove the tool filter. Keep raw
`interactive_args` from injecting `--tools mcp__...`.

## Related Tickets

AGE-113 owns this eval. AGE-104 provides the predecessor proof. AGE-105,
AGE-107, and AGE-108 depend on the replacement tool actually dispatching before
completion, routing, and transcript timing evidence can be trusted. AGE-89 is
the parent orchestration hardening context.
