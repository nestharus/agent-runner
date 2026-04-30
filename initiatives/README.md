# Agent Runner — Initiatives

Task packages tracking active and queued initiatives in this repo. Mirrors
the pattern used in `~/ai/initiatives/` for the workflow library itself.

Each file is a self-contained package:

- **Problem** — why this initiative exists (verbatim user framing where
  preserved; reconstructed from artifacts otherwise).
- **Scope** — what is in / what is out.
- **Dependencies** — what must land first.
- **Status** — one of `queued`, `researching`, `synthesizing`,
  `awaiting-decision`, `proposing`, `implementing`, `landed`, `superseded`.
- **Artifacts** — pointers to `research/`, `proposals/`, `review/`,
  `risk/` files and merged PRs.
- **Log** — dated entries of state changes.

Order is stable: numbered prefixes do not get reused. A dropped initiative
gets a `.DROPPED` suffix.

## Index

| # | Initiative | Status | Depends on |
|---|------------|--------|-----------|
| 01 | Trace inspection — invocation tree, session correlation, transcript locator | landed | — |
| 02 | Interactive resume — non-interactive resume-with-answer, session_id ingest, top-level `--resume` unification | landed | 01 |
| 03 | Load balancing tiers — per-window scoring, bootstrap cascade, risk classes (gating later removed by 04) | landed (3 PRs) | 02 |
| 04 | Reactive routing — replace 03's threshold/risk-class gating with per-account `exhausted_at` flag set on quota failure, cleared on successful refresh | landed (PRs #12, #13) | 03 |
| 05 | Session migration — chain identity decoupled from session_id; best-on-resume migration; resume without `-m`; compaction-aware target build | landed (commits `15c121a`, `a344bd0`, `91403a0`, `5f17d95`, `21c67f7`) | 04 |

## Backfill note

Initiatives 01–04 are backfilled into this template after the fact. Their
verbatim user framing was not preserved; the Problem sections are
reconstructed from the existing `research/<NN>-*-problem.md` artifacts.
Initiative 05 is the first to follow the template prospectively, with the
user framing captured verbatim.

## References

- Workflow library initiatives: `~/ai/initiatives/`
- Initiative pattern reference: `~/ai/initiatives/01-risk-and-value-axes.md`
- Audit-history convention: `~/ai/conventions/audit-history.md`
- Roadmap workflow: `~/ai/workflows/roadmap.md`
- Implementation pipeline: `~/ai/workflows/implementation-pipeline.md`
