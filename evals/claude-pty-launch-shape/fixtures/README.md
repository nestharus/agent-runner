# AGE-113 Claude PTY Launch-Shape Fixtures

These fixtures are the eval-surface successor for the AGE-104 P2 proof
materials. The default tests use static argv-shape fixtures and the
machine-checkable truth-table baseline so they run without a live Claude
binary. Step 6c must connect `eval.sh` to these fixtures for dry-run reporting
and optional live replay.

Inherited source mapping:

- `prototype-tests/age-104-pty-mcp-gap/p2-proof-tests.sh` is superseded by
  `eval.sh --dry-run --json --mode M3-C1`, `M3-C2`, and `M3-C3`, with optional
  live replay using these fixtures.
- `prototype-tests/age-104-pty-mcp-gap/p2-truth-table-harness/run-all.sh` is
  superseded by `truth-table-baseline.json` plus `eval.sh --dry-run --json
  --matrix`, with optional live replay for all 12 matrix cells.
- `prototype-tests/age-104-pty-mcp-gap/p2-truth-table-harness/server.py` is
  superseded by `server.py`, using eval-local environment variables instead of
  prototype absolute paths.

`good-allowedtools-kebab.json` is accepted by the source guard as an
allowed-tools-family shape. Live PTY equivalence for kebab-case remains a
separate residual unless Step 6c records direct Claude Code 2.1.143 evidence.
