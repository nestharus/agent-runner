# AGE-104 prototype-test contract

Published proof tests for the AGE-104 sub-prototype (`prototype-age-104-pty-mcp-gap`). See [`MARKERS.md`](./MARKERS.md) for the marker convention, ticket mapping, and acceptance criterion.

## Quick start

```bash
./p2-proof-tests.sh
```

## Files

- `p2-proof-tests.sh` — canonical proof script. Runs M3-C1 (expected fail), M3-C2 (expected pass), M3-C3 (expected pass).
- `p2-truth-table-harness/` — full 4×3 sweep harness (4 modes × 3 tool-flag shapes). `run-all.sh` produces a truth-table markdown alongside it.
- `MARKERS.md` — `prototype-pending:` marker mapping per spawned implementation ticket.

## Dossier

The full AGE-104 dossier (answer, risk profile, challenges, spawned tickets, branch disposition, etc.) lives under `${repo_root}/../planning/prototype-age-104-pty-mcp-gap/dossier/` on the orchestrator host. The dossier is the load-bearing artifact; this branch publishes only the durable proof tests.

## Reviewer guidance

Per `~/ai/conventions/prototype-review.md`, this PR is a test-design + outcome-alignment review, not a production source-quality review. Reviewers should:

- Verify each proof-test cell maps to a real spawned implementation ticket (see `MARKERS.md`).
- Verify the `prototype-pending:` markers cite real Linear keys/URLs.
- Verify the run command exits 0 in the manifest's expected control pattern.
- Not request production-style refactors or CodeRabbit-style nits.

This PR is fail-expected (M3-C1 is a negative control). Production PR-review gates do not apply by default.
