# Multi-concern review — 06-pause-handshake

## Verdict

**`SINGLE_CONCERN`** — the diff is cohesive: one v1 feature (session
pause/resume handshake = file-backed lock primitive + its only
consumer, the two `agents session` subcommands), plus tests and
workflow artifacts strictly scoped to that feature. Decomposition
would produce dead-code or untested-primitive PRs without reducing
review surface.

## Inputs consulted

- Spec: `research/06-pause-handshake-contract.md`.
- Approved proposal: `proposals/06-pause-handshake.md` (Rev 4).
- Existing-state framing: `research/06-pause-handshake-problem-map.md`.
- Audit history: `risk/06-pause-handshake-audit-history.md`.
- Phase 6 process-tree audit (PASS-WITH-ADVISORY): `risk/06-pause-handshake-process-tree-audit.md`.
- Actual diff: `git diff main..HEAD` (18 files, +3811).

## Diff inventory

Production code (3 files):

- `src-tauri/src/session_lock/mod.rs:1-374` — new lock primitive
  (`SessionLock`, `Lease`, `ReleaseReceipt`, `LockError`,
  sentinel-flock `with_flock`, atomic-rename `acquire`/`release`,
  `token_hash`, CSPRNG token).
- `src-tauri/src/main.rs:8,18-19,159-163,177-191,360-368,800-996` —
  purely additive: new `Session` variant on `Subcommands`, new
  `SessionSubcommands` enum, two TTL consts, dispatch arm,
  `run_pause_handshake` / `run_resume_handshake`,
  `default_lock_dir`, `emit_resume_resolution_error`,
  `emit_lock_error`, `emit_json_error`. No edits to existing
  function bodies.
- `src-tauri/src/lib.rs:8` — single line `pub mod session_lock;`.

Build / dependency (2 files):

- `src-tauri/Cargo.toml:25-27` — `nix`, `getrandom`, `sha2` deps.
  Each is consumed by `session_lock/mod.rs` (flock, CSPRNG, hashing).
- `src-tauri/Cargo.lock` — regenerated for the three deps.

Tests + fixtures (3 files):

- `src-tauri/tests/initiative_06_pause_handshake.rs:1-357` — T1-T11
  plus `T-release-after-expiry-no-marker`, all driving the binary
  via `Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"))`.
- `src-tauri/tests/fixtures/initiative_06_pause_handshake.rs:1-371`
  — temp `XDG_*`, model/provider seed, `pause_command`/`resume_command`
  spawners, expired-lock fixture, JSON readers.
- `src-tauri/tests/fixtures/mod.rs:1-3` — module declaration.

Workflow / audit artifacts (10 files): `proposals/`, `research/`,
`risk/` markdown only — Phase 2.5 → Phase 6 outputs. No production
behavior implications.

## Concerns analysis

### Candidate concerns

| # | Candidate | Files | Independent? |
|---|---|---|---|
| C1 | `session_lock` primitive (sentinel flock + atomic rename + token hashing) | `src-tauri/src/session_lock/mod.rs`, `src-tauri/Cargo.{toml,lock}`, `src-tauri/src/lib.rs` | No — the primitive's only v1 consumer is C2; without C2 it is dead code. |
| C2 | `agents session pause-handshake` / `resume-handshake` CLI surface | `src-tauri/src/main.rs` (Subcommands variant + two run_* + helpers) | No — directly imports `LockError`/`SessionLock` from C1; cannot land first. |
| C3 | T1-T11 + `T-release-after-expiry-no-marker` and CLI fixtures | `src-tauri/tests/...` | No — every test drives the binary via Cargo's `CARGO_BIN_EXE_*` and validates the JSON contract; both C1 and C2 are required to compile and pass. |
| C4 | Workflow audit-trail artifacts | `proposals/`, `research/`, `risk/` | Conventional Phase 2.5–6 outputs that ship with the PR; not an independent shippable unit. |

### Why no split is warranted

1. **C1 → C2 dependency is total.** The proposal §1 explicitly
   ships "the lock primitive only" with the CLI as its sole v1
   surface. Splitting C1 from C2 would land a module that the
   sibling-PR observers (import-replace, migration, repl/resume,
   balanced one-shot) are deferred to per §13 — it would be
   completely unreferenced product code at merge.

2. **All tests are end-to-end CLI tests.** The contract §8 mandates
   `std::process::Command` against the binary; T1-T11 honor that.
   A "C1-only" PR would have no executable validation of the
   primitive — Test Audit would flag the missing firstness
   coverage. Tests depend on both halves landing together.

3. **No drive-by edits.** `main.rs` adds new variants/functions
   only; existing dispatch arms, `run_*` functions, and helpers
   are untouched. `lib.rs` adds one `pub mod` line. There is no
   mixed refactor + behavior change to disentangle.

4. **Dependency adds are scoped.** `nix`, `getrandom`, `sha2` each
   have a single call site in `session_lock/mod.rs`
   (`flock`/`FlockArg`, `getrandom::getrandom`, `Sha256`). They
   are not speculative additions; they belong with C1.

5. **Audit artifacts are workflow context, not concerns.** They
   are the evidence stack the gates require (Phase 2.5 problem
   map, Phase 3 proposal Rev 4, Phase 4 risk reports, Phase 5
   hookpoints, Phase 6 contract + process-tree audit). Multi-
   concern doctrine targets independently shippable behavior, not
   the audit trail that justifies it.

### Operating-rule checks

- "A PR that touches N independent concerns should be split" —
  C1/C2/C3 are not independent; they are a single feature slice.
- "Additive changes go before behavioral changes" — the diff is
  purely additive against `main`; no behavior was removed or
  swapped, so the additive-first ordering is already satisfied
  in a single PR.
- "A large deletion stands alone" — no deletions in this diff.
- "Shared refactors only belong with behavior changes when
  strictly required for that behavior" — no refactor: every new
  symbol is consumed by this feature.
- "Cannot decompose further without creating more churn than
  clarity" — splitting C1 from C2 would produce one PR with
  unreferenced code and zero tests, then a second PR that
  actually exercises it. That is more churn, not less.

## Findings

None blocking, none ordinary.

The Phase 6 process-tree audit (`risk/06-pause-handshake-process-tree-audit.md`)
records one advisory (A1: Step 6c fixture-capture fix-pass commit
`7a4e3e7` was an infrastructure-only correction). That is a
firstness/process observation and does not affect the
multi-concern judgment; surfacing it is the process-tree gate's
job, not this gate's.

## Decomposition recommendation

None. The PR is cohesive and ready to advance.
