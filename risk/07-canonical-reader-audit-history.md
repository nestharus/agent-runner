# 07 Canonical Reader CodeRabbit Audit History

## Pre-Pass Sanity

- Branch: `07-canonical-reader-rca`
- Base: `main`
- Worktree: `/home/nes/projects/agent-runner/worktrees/07-canonical-reader-rca`
- `main` refresh: PASS — `git fetch origin main && git update-ref refs/heads/main refs/remotes/origin/main`
- Diff base: PASS — `git log --oneline main..HEAD` showed the five expected branch commits.
- Worktree before pass 1: PASS — clean except for generated CodeRabbit pass output after the pass began.
- Baseline tests: PASS — `cargo test --manifest-path src-tauri/Cargo.toml`

## Pass 1

- Source: `CODERABBIT_pass1.md`
- Findings: 15
- Real applied: 11
- Skipped: 4
- Determination: continue after amend
- Tests: PASS — `cargo test --manifest-path src-tauri/Cargo.toml`

Applied finding IDs:
- `R1-F03`: Added `CODERABBIT_*.md` to `.gitignore` so operator pass logs do not pollute future reviews. The file remains in the working tree as required loop output, but is not part of the committed branch diff.
- `R1-F04`: Added `text` language tag to the RCA failing import-replace test output fence.
- `R1-F05`: Added `text` language tag to the RCA invariant fence.
- `R1-F06`: Added `text` language tag to the RCA failing canonical-reader test output fence.
- `R1-F08`: Added early renderability validation for canonical input chunks without `text`, preventing lossy empty-text rendering before staging/journal mutation; added `t_non_text_chunk_without_text_rejects_without_mutation`.
- `R1-F09`: Corrected stale scope-gate prose for AIR-SCOPE-F03; the duplicate metadata bridge helper no longer exists.
- `R1-F11`: Fixed `re-introducable` to `reintroducible`.
- `R1-F12`: Added a top-level heading to `risk/07-canonical-reader-audit.md`.
- `R1-F13`: Added `text` language tag to shortcut-gate build/test evidence.
- `R1-F14`: Added `text` language tag to scope-gate diff-stat evidence.
- `R1-F15`: Mapped export malformed-transcript line sentinel `0` to `None` in the replace error bridge.

Skipped finding IDs:
- `R1-F01`: Operator-artifact markdown nit on `CODERABBIT_pass1.md`; skipped because pass logs are local output artifacts, not project documentation.
- `R1-F02`: Operator-artifact absolute path concern on `CODERABBIT_pass1.md`; skipped because pass logs are ignored and not committed.
- `R1-F07`: Test helper reuse nit; skipped because the helper is a test-local legacy oracle and making production SHA-256 helpers visible for it would expand API surface without changing behavior.
- `R1-F10`: Stale/false-positive code finding; `SessionMetadata::to_export_metadata` is absent from `session_replace/internal/mod.rs`. The stale risk prose was corrected under `R1-F09`.

Watch signals for pass 2:
- If CodeRabbit still scans ignored `CODERABBIT_*.md`, classify those comments as operator-artifact churn.
- Further requests to expose production internals only for test helper reuse should remain nit-level absent a concrete behavior defect.

## Pass 2

- Source: `CODERABBIT_pass2.md`
- Findings: 1
- Real applied: 0
- Skipped: 1
- Determination: converge (`ALL_CHURN`)
- Tests: not rerun; no fixes were applied in pass 2.

Skipped finding IDs:
- `R2-F01`: Nitpick asking to move CodeRabbit scratch files under `tmp/`. Skipped because the CodeRabbit operator contract explicitly names root-level `CODERABBIT_pass<N>.md` files as pass outputs, and pass 1 already made those files ignored so they do not enter the committed branch diff.

Convergence determination:
- Pass 2 contained no code, test, contract, or risk-gate finding.
- The only remaining item was an operator-artifact organization preference.
- Value dropped to zero after pass 2; stop reason is `ALL_CHURN`.

Final disposition:
- `CONVERGED:ALL_CHURN`
- Final amended commit: the branch `HEAD` after this audit-history entry is amended. The concrete short SHA is recorded in local `CODERABBIT_summary.md` after the final amend.
