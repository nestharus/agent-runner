# Function Classification Audit

## Inputs Read

- `worktree_path=/home/nes/projects/agent-runner/trunk`
- `repo_root=/home/nes/projects/agent-runner/trunk`
- `diff_path=/home/nes/projects/agent-runner/planning/code-quality-sweep/diffs/agent-runner_trunk_src-tauri_src_run_balancing_validator_rs.diff`
- `touched_surfaces_path=/home/nes/projects/agent-runner/planning/code-quality-sweep/touched/agent-runner_trunk_src-tauri_src_run_balancing_validator_rs.md`
- `planning_dir=/home/nes/projects/agent-runner/planning/code-quality-sweep/planning`
- `wu_id=cqs-agent-runner_trunk_src-tauri_src_run_balancing_validator_rs-r1`
- `output_path=/home/nes/projects/agent-runner/planning/code-quality-sweep/composite/agent-runner_trunk_src-tauri_src_run_balancing_validator_rs__funcclass__r1.md`

## References Read

- `/home/nes/ai/conventions/code-quality.md`
- `/home/nes/projects/agent-runner/planning/code-quality-sweep/diffs/agent-runner_trunk_src-tauri_src_run_balancing_validator_rs.diff`
- `/home/nes/projects/agent-runner/planning/code-quality-sweep/touched/agent-runner_trunk_src-tauri_src_run_balancing_validator_rs.md`
- `/home/nes/projects/agent-runner/trunk/src-tauri/src/run/balancing/validator.rs`

A1 preservation verified: the source contains the category list (`orchestration`, `filter`, `validator`, `predicate`, `mapper`, `accessor`, `formatter`, `parser`), the single-classification rule, the `Function categories per function` threshold row (`LOW = 1`, `MEDIUM = n/a`, `HIGH = >= 2`), and the `multi-classifier function` failure mode.

## Functions In Touched Files

### LOW coverage (per file)

| Path | LOW functions inspected | Test file excluded? |
|---|---:|---|
| `src-tauri/src/run/balancing/validator.rs` | 8 | No |

### HIGH functions (enumerated individually)

| Path | Function / symbol | Line span or diff hunk | Categories mixed | Evidence |
|---|---|---|---|---|

## Multi-Classifier Findings

| ID | Path | Function / symbol | Categories mixed | Evidence | Suggested split | Blocking or residual | Finding origin | Domain relation |
|---|---|---|---|---|---|---|---|---|

## Residual Ambiguity / Stop-Condition Notes

- Touched-file discovery: `diff_path` creates `src-tauri/src/run/balancing/validator.rs`, and `touched_surfaces_path` confirms that single touched surface.
- Test exclusion: no test file, `#[cfg(test)]` module, or `#[test]` function was present in the touched file; no function-like symbols were excluded as test code.
- Inventory classification: all eight admitted symbols classify as `validator` only. The five `expect_*_disposition` functions validate that a `TerminalSignalDisposition` matches the required variant set via `debug_assert!(matches!(...))`. The three `required_*` functions validate required `Option` presence and return the accepted inner value/reference or raise the supplied/static validation failure message via `expect(...)`. Returning the accepted value is part of the A1 `validator` category and does not add an `accessor` category.
- No unresolved function-boundary ambiguity remained after reading the full touched source file.

Verdict: LOW
