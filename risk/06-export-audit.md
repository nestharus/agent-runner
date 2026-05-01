# 06-export - Phase 4 Audit Risk Report (Rev 2 / Round 2)

**Verdict: LOW**

R1-F01 is closed. Rev 2 makes the previously conditional `STATE_DIR`
mkdir behavior a proposal-level contract, so the side-effect surface is
reviewable before Phase 5. No new audit findings or regressions were found
in the Rev 2 delta.

## Round 2 scope

Inputs reviewed:

- `proposals/06-export.md` Rev 2.
- Prior Rev 1 report at `risk/06-export-audit.md`.
- `risk/06-export-audit-history.md`.
- `research/06-export-problem-map.md`.
- `/home/nes/projects/agent-runner/worktrees/06-locate/initiatives/06-session-override-contract.md`.
- `/home/nes/projects/agent-runner/worktrees/06-locate/proposals/06-locate.md` for the matching locate-side `STATE_DIR` clause.

Round 2 was limited to closure of R1-F01, fresh assessment of the Rev 2
§8 change, and regression review against the Rev 1 audit surface.

## Prior finding closure

### R1-F01 (MEDIUM) - CLOSED

Rev 1 found that export inherited `locate_session_metadata(...)` while
leaving today's `locate_transcript` adapter `STATE_DIR` mkdir behavior to
a future Phase 5 hookpoint decision. That made the no-side-effect contract
conditional rather than reviewable.

Rev 2 resolves that gap:

- The revision log explicitly identifies the §8 `STATE_DIR` mkdir clause
  as the R1-F01 closure item (`proposals/06-export.md:35-39`).
- §4 still requires the 06-schema-probe read-only `StateDb` open variant
  and forbids use of today's mutating `StateDb::open_default()` unless
  Phase 4 revises the dependency (`proposals/06-export.md:183-186`).
- §8 now states that export may run the configured transcript locator only
  through `locate_session_metadata`, depends on the read-only state open,
  and may create only the locator adapter `state_dir` directory when
  `locate_transcript` is invoked (`proposals/06-export.md:339-350`).
- The clause matches 06-locate's accepted contract: locate may create the
  locator adapter `state_dir` directory, and writes no file inside it
  (`/home/nes/projects/agent-runner/worktrees/06-locate/proposals/06-locate.md:232-245`).

This closes the audit issue because the implementation is no longer asked
to discover the side-effect policy later. The accepted contract is explicit:
read-only DB open is mandatory; the configured locator path may perform
the existing directory creation; export writes no canonical records on
error and writes no files inside the adapter state directory.

## Fresh Rev 2 assessment

The Rev 2 change does not weaken the command/output contract. Export still
has one CLI surface, one v1 format, compact JSONL success stdout, JSON
stderr errors, and no partial canonical stdout on malformed transcripts
(`proposals/06-export.md:73-107`, `proposals/06-export.md:215-217`).

The side-effect contract is now internally reviewable. §7 forbids provider
spawn, DB writes, transcript writes, temp files, adapter cursor writes,
scans, turn scripts, migrations, and config writes
(`proposals/06-export.md:312-327`). §8 narrows the only accepted filesystem
exception to the existing locator adapter directory creation and forbids
files inside that directory (`proposals/06-export.md:328-353`).

The cross-feature dependency remains aligned with Initiative 06: export is
third in sequence, depends on locate and schema-probe, and consumes the
read-only `StateDb` variant that schema-probe is assigned to provide
(`/home/nes/projects/agent-runner/worktrees/06-locate/initiatives/06-session-override-contract.md:41-50`,
`/home/nes/projects/agent-runner/worktrees/06-locate/initiatives/06-session-override-contract.md:118-122`).

The test-intent track remains sufficient for audit gate purposes. The
read-only row snapshots DB rows, transcript mtimes, adapter state, and temp
dirs (`proposals/06-export.md:370`). In Phase 6, that fixture should either
pre-create the allowed locator `state_dir` or assert the exact permitted
delta: directory creation only, no files written. That is test-shaping work,
not a new Phase 4 finding, because §8 now pins the behavior being tested.

## Regression review

No regression found against the Rev 1 checklist:

| Area | Round 2 status | Evidence |
| --- | --- | --- |
| CLI and format surface | Unchanged / acceptable | `session export <session-id> [--format canonical-jsonl]`; only `canonical-jsonl`; invalid formats exit `2` (`proposals/06-export.md:73-107`). |
| Canonical schema and source preimage | Unchanged / acceptable | Required record fields and source line/byte/hash metadata remain defined (`proposals/06-export.md:109-174`). |
| Resolution flow | Improved | Read-only state open remains mandatory; locator side-effect exception is now explicit (`proposals/06-export.md:176-222`, `proposals/06-export.md:339-350`). |
| Exit codes | Unchanged / acceptable | Exit codes `0`, `1`, `2`, `10`, `11`, `12`, and `15` remain mapped (`proposals/06-export.md:224-239`). |
| Reusable API | Unchanged / acceptable | `CanonicalRecord`, `RecordSource`, `ExportError`, and `read_canonical_transcript` remain defined (`proposals/06-export.md:241-310`). |
| Anti-scope | Unchanged / acceptable | No DB writes, no `session_turns` reconstruction, no provider spawn, no scans, no migrations, no GUI surface (`proposals/06-export.md:312-327`). |
| Test-intent and residuals | Unchanged / acceptable | Parser drift, compaction, ordering, no partial stdout, and read-only behavior are covered; parser drift remains an accepted residual (`proposals/06-export.md:355-377`, `proposals/06-export.md:427-447`). |
| Cross-feature constraints | Unchanged / acceptable | Error namespace, resolver reuse, read-only state dependency, and anti-scope remain listed (`proposals/06-export.md:448-461`). |

## Findings

None.

## Accepted residuals

- Parser drift remains the main implementation residual for private Claude
  Code and Codex JSONL formats (`proposals/06-export.md:431-433`).
- Codex compaction remains unsupported in v1 unless Phase 5 proves a stable
  raw marker and the proposal is revised (`proposals/06-export.md:434-436`).
- Timestamp regression remains fail-closed, which may reject real provider
  transcripts with benign clock skew (`proposals/06-export.md:437-438`).
- Whole-transcript buffering remains the chosen tradeoff for no partial
  stdout (`proposals/06-export.md:439-440`).

## Determination

Audit risk is `LOW`. R1-F01 is closed, Rev 2 introduces no blocking audit
issue, and the proposal can proceed to the next Phase 4 gate/exit decision
from the audit perspective.
