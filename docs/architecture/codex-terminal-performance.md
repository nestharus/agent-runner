# Codex interactive terminal performance

`agents --new` and `agents repl` run native clients through the shared PTY
broker. The headless `agent-runner-codex` adapter does not render that screen.
Codex normally leaves wheel scrolling to the surrounding terminal, so the
broker renders its retained history and the top-right `SCROLLBACK` badge.
A mouse-aware client such as OpenCode handles wheel events inside its own UI.

## Measured redraw bottleneck

The broker previously passed an unbuffered `File` to the Crossterm backend.
Ratatui emits individual cell commands, resulting in thousands of writes for a
large screen change. A deterministic 160-column, 50-row repaint produced 8,259
writes for 8,354 bytes. The buffered backend produces identical bytes in one
frame flush, with at most two underlying writes in the regression test.

A 30-frame PTY benchmark with an independent reader measured:

| Backend | Median frame | 95th percentile |
| --- | ---: | ---: |
| Unbuffered | 20.945 ms | 22.522 ms |
| 64 KiB buffer | 0.744 ms | 0.840 ms |

These timings measure backend/kernel overhead, excluding terminal-emulator
rendering. They do not imply a 28-fold improvement in total application latency.
The production backend now buffers writes and retains Ratatui's flush on every
completed frame. Terminal-mode restoration and clipboard operations keep their
existing ordering on the render thread.

## Terminal query compatibility

The default vt100 parser silently discarded cursor-position queries. An
isolated child waiting for `CSI 6 n` timed out after 605 ms with the overlay,
while raw passthrough returned a reply in 0.2 ms. Codex's cursor synchronization
can query this position; missing replies are a compatibility failure even
though a fresh, empty composer did not reproduce a multi-second typing delay.

The broker now answers cursor position, terminal status and basic primary
device attributes from the emulated child screen. Cursor reports use physical
screen bounds, including the right-margin pending-wrap case. Replies share the
existing child-input FIFO so partial writes cannot splice them into a paste or
keyboard escape sequence. Response queues are bounded under backpressure, and
consumed prefixes are reclaimed even when a busy queue never empties.

## Optional profiling

Run an interactive session with an unused absolute output path:

```sh
OULIPOLY_TUI_PROFILE=/tmp/codex-tui-profile-1.json agents --new
```

After a normal session shutdown, the broker writes one private JSON summary.
It never replaces an existing file. Profiling is disabled by default and
records no terminal text, prompts, commands or credentials.

The summary contains counts, total/max nanoseconds and histogram buckets for:

- `render.snapshot`: copying the virtual screen into a render snapshot;
- `render.draw`: drawing and flushing a frame to the terminal;
- `input.forward` and `input.write`: routing input and writing the child FIFO;
- `pty.parse`: reading/parsing available child-output bursts;
- `relay.iteration` and `relay.poll`: total iteration time and idle poll time.

`render.frames` and `render.snapshots` count render activity. Large poll timings
alone indicate idle waiting. Large draw timings identify rendering/output
backpressure; large snapshot or parse timings identify processing overhead.
The file is written when the final profiler owner drops, so forced process
termination may leave no summary.

## Validation

Runtime library tests cover exact buffered output, completed-frame flushing,
query boundaries, pending wrap, FIFO ordering, bounded response backlog,
partial-write storage reclamation and profiler isolation. The outer-PTY relay
test verifies that a child receives its cursor reply followed by user input.
All 663 runtime library tests passed for this change.

The optimized runner returned the isolated child's correct cursor position in
0.67 ms. A Codex 0.153.4 UI fixture with 3,000 synthetic rows, served entirely
from a loopback Responses stub without credentials, produced these aggregate
profile timings at 120 columns:

| Stage | Mean | Maximum |
| --- | ---: | ---: |
| Input routing | 0.017 ms | 0.091 ms |
| PTY output parsing | 0.346 ms | 5.354 ms |
| Screen snapshot | 0.362 ms | 1.747 ms |
| Frame draw/flush | 0.259 ms | 0.793 ms |

This fixture validates the installed-binary path independently of model speed.
It cannot measure the operator's terminal-emulator frontend. Existing running
sessions keep their original executable and need a restart after installation.
