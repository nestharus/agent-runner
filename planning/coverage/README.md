# `planning/coverage/` — Test-audit spec index

This directory is the consumer-side schema for the
`~/ai/agents/test-audit-gate.md` operator contract (see § 4 "Discover
Candidate Specs"). Each `spec-*.md` file declares the behavior contract
for one product surface and lists every source file that surface owns.

## How a spec resolves a changed product file

The auditor runs, for each changed product path:

```
rg -l --fixed-strings "<changed path>" planning/coverage/spec-*.md
```

The path appears verbatim (backtick-wrapped for human readability) in
exactly one (or, by design, a deliberate union of) `## Source files`
sections. The auditor returns the spec(s) that match.

## Index

| Spec | Surface |
|------|---------|
| `spec-balancer.md` | Provider selection and routing matrix. |
| `spec-quota.md` | Quota refresh script, parse, cache. |
| `spec-recognizer.md` | Per-provider terminal-signal taxonomy. |
| `spec-usage.md` | `--usage` CLI flow (accessor, fetcher, renderer). |
| `spec-session-lifecycle.md` | Resume, chain segments, migration, lock, metadata. |
| `spec-executor.md` | Process supervision and provider CLI dispatch. |
| `spec-discovery.md` | Installed-provider discovery + REPL default. |
| `spec-diagnostics.md` | Diagnostics, trace, services, ports. |
| `spec-state-db.md` | SQLite store, schema, migrations, deployment. |
| `spec-config.md` | Provider/model/app config resolution. |
| `spec-setup.md` | First-run wizard, detection, sync. |
| `spec-agent-channels.md` | Sub-agent IPC (store + scratchpad + messenger). |
| `spec-tauri-client.md` | Top-level Tauri client wiring + CLIs. |
| `spec-result-envelope.md` | Result markers, failure identity, pre-invocation failures. |
| `spec-provider-client.md` | Provider artifact client, resolver, process substrate, launch stream. |

## Spec schema

Each spec contains, in order:

1. `## Source files` — verbatim repo-relative paths (load-bearing).
2. `## Preconditions`
3. `## Input → Expected output`
4. `## Edge cases`
5. `## Error conditions`
6. `## Boundaries` — what the surface explicitly does NOT do.
7. `## Declared test patterns` — cite-back to `crates/*/tests/`,
   `src-tauri/tests/`.
8. `## Cross-references` — sibling specs and `AGENTS.md` anchors.

## Anti-scope

- Frontend product code (`src/**/*.ts`, `src/**/*.tsx` excluding
  `*.test.*` and `__tests__/`) is intentionally not covered. Frontend
  coverage is a separate enumeration that AGE-165 does not undertake.
- Test files (`crates/*/tests/*.rs`, `src-tauri/tests/*.rs`) are NOT
  product files and are not spec-anchored — they are CITED in the
  `## Declared test patterns` section of the spec for the surface they
  exercise.

## Authoring a new spec

1. Run `find crates -name '*.rs' -not -path '*/tests/*'` and
   `find src-tauri/src -name '*.rs'` against the worktree.
2. Confirm the file is not already anchored in an existing spec.
3. Either extend an existing spec's `## Source files` section, or
   create a new `spec-<surface>.md` following the schema above.
4. Verify with:
   ```bash
   rg -l --fixed-strings "<new path>" planning/coverage/spec-*.md
   ```
   The output must be non-empty.

## Coverage workflow

The CI coverage baseline lives in `.github/workflows/coverage.yml` and
emits a `rust-coverage` artifact (lcov + cobertura + JSON summary) per
PR and per `main` push. The test-audit gate consumes those artifacts
via `gh api ... workflow_runs`.
