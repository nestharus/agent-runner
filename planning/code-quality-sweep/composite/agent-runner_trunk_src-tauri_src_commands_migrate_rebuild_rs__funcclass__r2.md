# Function Classification Audit

## Inputs Read

- `worktree_path=/home/nes/projects/agent-runner/trunk`
- `repo_root=/home/nes/projects/agent-runner/trunk`
- `diff_path=/home/nes/projects/agent-runner/planning/code-quality-sweep/diffs/agent-runner_trunk_src-tauri_src_commands_migrate_rebuild_rs.diff`
- `touched_surfaces_path=/home/nes/projects/agent-runner/planning/code-quality-sweep/touched/agent-runner_trunk_src-tauri_src_commands_migrate_rebuild_rs.md`
- `output_path=/home/nes/projects/agent-runner/planning/code-quality-sweep/composite/agent-runner_trunk_src-tauri_src_commands_migrate_rebuild_rs__funcclass__r2.md`
- `wu_id=cqs-agent-runner_trunk_src-tauri_src_commands_migrate_rebuild_rs-r2`

## References Read

- `/home/nes/ai/conventions/code-quality.md`
- `/home/nes/projects/agent-runner/planning/code-quality-sweep/diffs/agent-runner_trunk_src-tauri_src_commands_migrate_rebuild_rs.diff`
- `/home/nes/projects/agent-runner/planning/code-quality-sweep/touched/agent-runner_trunk_src-tauri_src_commands_migrate_rebuild_rs.md`
- `/home/nes/projects/agent-runner/trunk/src-tauri/src/commands/migrate/rebuild.rs`
- A1 preservation verified: category list, single-classification rule, `Function categories per function` threshold row, and `multi-classifier function` failure mode are present and non-contradictory in `code-quality.md`.

## Functions In Touched Files

### LOW coverage (per file)

| Path | LOW functions inspected | Test file excluded? |
|---|---:|---|
| `src-tauri/src/commands/migrate/rebuild.rs` | 5 | No |

### HIGH functions (enumerated individually)

| Path | Function / symbol | Line span or diff hunk | Categories mixed | Evidence |
|---|---|---|---|---|
| `src-tauri/src/commands/migrate/rebuild.rs` | `prepare_migrate_backup_root` | Lines 31-36; diff hunk `@@ -0,0 +1,68 @@` | `orchestration`, `mapper` | Orchestration: sequences `state_db_parent_dir(db_path)?`, `create_backup_root_dir(&backup_root)?`, and `Ok(backup_root)`. Mapper: inline `let backup_root = data_dir.join("state-backups");` transforms the state DB parent directory into the backup-root path using a domain literal. |

## Multi-Classifier Findings

| ID | Path | Function / symbol | Categories mixed | Evidence | Suggested split | Blocking or residual | Finding origin | Domain relation |
|---|---|---|---|---|---|---|---|---|
| FC-001 | `src-tauri/src/commands/migrate/rebuild.rs` | `prepare_migrate_backup_root` | `orchestration`, `mapper` | Orchestration: helper sequencing and error propagation through `state_db_parent_dir(db_path)?`, `create_backup_root_dir(&backup_root)?`, and `Ok(backup_root)`. Mapper: inline backup-root path construction in `data_dir.join("state-backups")`. Failure mode: `multi-classifier function`. | Split direction: separate backup-root path construction from backup-root preparation, so the current preparation function only orchestrates already-named operations while a mapper-owned boundary derives the backup-root path. Convergence proof: current blocking finding `FC-001`; this strictly reduces the blocking set by removing inline path mapping from the orchestration function; any introduced helper is handled under the audit overlay as a newly scored function that must classify as mapper only. | blocking | changed_function | same_domain |

### Finding Details

```yaml
id: FC-001
path: src-tauri/src/commands/migrate/rebuild.rs
function: prepare_migrate_backup_root
line_span_or_diff_hunk: Lines 31-36; diff hunk @@ -0,0 +1,68 @@
categories_mixed:
  - orchestration
  - mapper
evidence: >-
  Orchestration is present because the function sequences state_db_parent_dir(db_path)?,
  create_backup_root_dir(&backup_root)?, and Ok(backup_root). Mapper work is present
  because the function performs inline path transformation with data_dir.join("state-backups"),
  deriving the backup-root path from the state DB parent directory.
failure_mode: multi-classifier function
blocking_or_residual: blocking
finding_origin: changed_function
domain_relation: same_domain
suggested_split:
  direction: >-
    Separate backup-root path construction from backup-root preparation, keeping the
    preparation function as orchestration over named operations and moving path derivation
    behind a mapper-owned boundary.
  convergence_proof:
    current_blocking_finding: FC-001 on prepare_migrate_backup_root
    why_split_reduces_blocking_set: >-
      The split removes the inline mapper operation from the current orchestration body,
      leaving no mixed body evidence in that function.
    helper_overlay_handling: >-
      Any introduced path-derivation helper remains in the touched-file inventory and must
      independently classify as mapper only under the same A1 overlay.
```

## Residual Ambiguity / Stop-Condition Notes

- Touched-file discovery: the diff creates `src-tauri/src/commands/migrate/rebuild.rs`, and the touched-surfaces file corroborates that same path. Per the ad-hoc instruction, the whole file was treated as touched.
- Test exclusion: no test file, `#[cfg(test)]` module, or `#[test]` function was present in the touched file; no test-code symbols were excluded.
- Inventory exclusion: `MigrateRebuildPlan` is a struct declaration, not an executable function-like symbol with a body, so it was not scored.
- LOW coverage details: `migrate_rebuild_plan`, `migrate_rebuild_plan_from_paths`, and `execute_migrate_rebuild` classify as `orchestration` only under pure helper-dispatch recognition; `migrate_rebuild_plan_value` and `db_sidecar_paths` classify as `mapper` only.
- No unresolved function-boundary ambiguity materially affects the verdict.

Verdict: HIGH
