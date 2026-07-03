# Function Classification Audit

## Inputs Read

- `worktree_path=/home/nes/projects/agent-runner/trunk`
- `repo_root=/home/nes/projects/agent-runner/trunk`
- `diff_path=/home/nes/projects/agent-runner/planning/code-quality-sweep/diffs/agent-runner_trunk_src-tauri_src_run_balancing_mapper_rs.diff`
- `touched_surfaces_path=/home/nes/projects/agent-runner/planning/code-quality-sweep/touched/agent-runner_trunk_src-tauri_src_run_balancing_mapper_rs.md`
- `planning_dir=/home/nes/projects/agent-runner/planning/code-quality-sweep/planning`
- `wu_id=cqs-agent-runner_trunk_src-tauri_src_run_balancing_mapper_rs-r2`
- `output_path=/home/nes/projects/agent-runner/planning/code-quality-sweep/composite/agent-runner_trunk_src-tauri_src_run_balancing_mapper_rs__funcclass__r2.md`

## References Read

- `/home/nes/ai/conventions/code-quality.md`
- `src-tauri/src/run/balancing/mapper.rs`

A1 preservation verified from `/home/nes/ai/conventions/code-quality.md`: single-classification rule, A1 category list (`orchestration`, `filter`, `validator`, `predicate`, `mapper`, `accessor`, `formatter`, `parser`), `Function categories per function` threshold row (`LOW = 1`, `MEDIUM = n/a`, `HIGH = >= 2`), and `multi-classifier function` failure mode are present and non-contradictory.

## Functions In Touched Files

### LOW coverage (per file)

| Path | LOW functions inspected | Test file excluded? |
|---|---:|---|
| `src-tauri/src/run/balancing/mapper.rs` | 0 | No |

### HIGH functions (enumerated individually)

| Path | Function / symbol | Line span or diff hunk | Categories mixed | Evidence |
|---|---|---|---|---|
| _None_ | _n/a_ | _n/a_ | _n/a_ | _No executable function-like symbols with bodies are present in the touched file._ |

## Multi-Classifier Findings

| ID | Path | Function / symbol | Categories mixed | Evidence | Suggested split | Blocking or residual | Finding origin | Domain relation |
|---|---|---|---|---|---|---|---|---|
| _None_ | _n/a_ | _n/a_ | _n/a_ | _No multi-classifier functions found._ | _n/a_ | _n/a_ | _n/a_ | _n/a_ |

## Residual Ambiguity / Stop-Condition Notes

- `src-tauri/src/run/balancing/mapper.rs` is a Rust facade containing only module declarations and `pub(super) use` re-exports; it contains no actual executable function-like symbols with inspectable bodies for A5 scoring.
- No test files or test-guarded function-like symbols were present in the touched file.
- No unresolved function-boundary ambiguity remains from the supplied diff and whole-file source evidence.

Verdict: LOW
