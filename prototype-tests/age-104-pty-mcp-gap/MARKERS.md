# AGE-104 prototype-pending markers

These proof tests were published by the AGE-104 sub-prototype (`prototype-age-104-pty-mcp-gap`) — see the prototype dossier (referenced by the prototype-test PR description) for the full answer and the test-publication manifest.

Per `~/ai/conventions/prototype-pending-tests.md`, every published proof test carries a `prototype-pending:` marker that names a real implementation ticket. Reviewers should treat these markers as **implementation handoff debt**, not generic skip permission. The marker reason format is:

```
prototype-pending: implementation pending in <ticket-key-or-url>; remove marker and make this test pass
```

## Acceptance criterion (per spawned implementation ticket)

Remove the `prototype-pending:` markers in the listed test files, make these tests pass against production code, and preserve the original assertions unless the manifest, spawned ticket payload, or Phase 6 Step 6b output index records a strictly stronger equivalent supersession.

## Ticket mapping (test cell → spawned implementation ticket)

| Proof-test cell or scope | Spawned ticket (AGE-104 dossier ID → Linear key) |
|---|---|
| `p2-proof-tests.sh` M3-C1 negative control (`--tools mcp__...` fails in PTY) | ST-01 → [AGE-101](https://linear.app/oulipoly/issue/AGE-101) |
| `p2-proof-tests.sh` M3-C2 positive control (`--allowedTools` works in PTY) | ST-01 → [AGE-101](https://linear.app/oulipoly/issue/AGE-101), ST-04 → [AGE-103](https://linear.app/oulipoly/issue/AGE-103) |
| `p2-proof-tests.sh` M3-C3 positive control (no filter works in PTY) | ST-01 → [AGE-101](https://linear.app/oulipoly/issue/AGE-101), ST-06 → [AGE-113](https://linear.app/oulipoly/issue/AGE-113) |
| All M3/M4 cells (PTY-with-hooks + PTY-without-hooks) | ST-05 → [AGE-112](https://linear.app/oulipoly/issue/AGE-112) |
| `p2-truth-table-harness/run-all.sh` full 4×3 sweep | ST-02 → [AGE-90](https://linear.app/oulipoly/issue/AGE-90) (rollout), ST-03 → [AGE-102](https://linear.app/oulipoly/issue/AGE-102) (MCP replacement smoke), ST-07 → [AGE-114](https://linear.app/oulipoly/issue/AGE-114) (runbook/version-bump docs), ST-08 → [AGE-115](https://linear.app/oulipoly/issue/AGE-115) (upstream bug report decision) |

## Marker convention boundary

These are bash + Python harness scripts. They are not Pytest, Playwright, or cargo tests, so they do not use `@pytest.mark.xfail`, `test.fixme`, or `#[ignore]` primitives. The marker is a **comment-block sentinel** at the top of each published file:

```bash
# prototype-pending: implementation pending in <linear-url>; remove marker and make this test pass
```

A `grep -RIn "^# prototype-pending:" prototype-tests/age-104-pty-mcp-gap/` enumerates every traceable marker. Implementation tickets must remove these markers when the underlying behavior lands in production code.

## Run the proof

From this directory:

```bash
./p2-proof-tests.sh
```

Expected:

- M3-C1 (PTY w/ hooks + `--tools`) fails (`tools/call` never reaches MCP server).
- M3-C2 (PTY w/ hooks + `--allowedTools`) succeeds.
- M3-C3 (PTY w/ hooks + no filter) succeeds.
- Exit code 0 if all three assertions match expectations.

The full 4×3 sweep is `./p2-truth-table-harness/run-all.sh` (writes `p2-truth-table.md` next to it).

## Caveats

- Run on Linux with `script` (util-linux), Python 3.12+, and Claude Code 2.1.143 installed at `/home/nes/.local/share/claude/versions/2.1.143`. The harness intentionally bypasses the `/home/nes/.local/bin/claude` symlink because the symlink target can drift.
- Each PTY cell uses an outer `timeout` wall-clock cap as a SAFETY net only. Completion is sentinel-driven (server-log + visible-transcript inspection), not idle-timeout.
- Print-mode cells (M1/M2) work under all three flag shapes; PTY-mode cells (M3/M4) fail only for C1 `--tools` and succeed for C2 `--allowedTools` + C3 no filter. Hooks are NOT load-bearing (M3 ≡ M4).
- Reproduces across Claude Code 2.1.141, 2.1.142, 2.1.143 (per dossier V4 version sweep).
