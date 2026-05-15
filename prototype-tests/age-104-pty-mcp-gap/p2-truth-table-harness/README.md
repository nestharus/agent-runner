# AGE-104 P2 Truth-Table Harness

This harness reproduces the Claude Code 2.1.143 PTY-vs-print MCP tool gap for
`--tools mcp__age104p2__Task,mcp__age104p2__Echo`, plus the two PTY workarounds:
`--allowedTools ...` and no tool filter.

It is self-contained and uses only Bash, Python stdlib, Claude Code at
`/home/nes/.local/share/claude/versions/2.1.143`, and util-linux `script`.

## Files

- `server.py`: stdio MCP server exposing `Echo` and `Task`. `Task` runs
  `/bin/echo TASK_OK:<message>`.
- `mcp.json`: Claude MCP config for server name `age104p2`.
- `settings.json`: hook-enabled settings used for M3.
- `prompts/call-task.md`: fixed prompt asking for `mcp__age104p2__Task`.
- `run-mode.sh`: runs one cell, for example `./run-mode.sh M3 C1`.
- `run-all.sh`: runs all 12 cells and writes `../p2-truth-table.md`.

## Modes

- `M1`: `claude -p` raw print mode.
- `M2`: `claude --print --output-format stream-json`.
- `M3`: interactive PTY via `script -qfec`, with hooks.
- `M4`: interactive PTY via `script -qfec`, without hooks.

## Columns

- `C1`: `--tools mcp__age104p2__Task,mcp__age104p2__Echo`.
- `C2`: `--allowedTools mcp__age104p2__Task,mcp__age104p2__Echo`.
- `C3`: no tool filter.

## Reproduce

From this directory:

```bash
./run-all.sh
```

The expected pattern is:

- M1 and M2: all columns call `Task` and return `TASK_OK:AGE104_P2_SENTINEL`.
- M3 and M4: C1 initializes/lists the MCP server but does not dispatch
  `tools/call`; C2 and C3 dispatch and return `TASK_OK:AGE104_P2_SENTINEL`.

The focused proof controls live at `../p2-proof-tests.sh`:

```bash
../p2-proof-tests.sh
```
