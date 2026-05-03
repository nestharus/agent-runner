# Session Migration RCA

## Symptom

Observed in the WU-11-01 orchestrator resume path:

```text
[resume] -> claude3
[migrate] claude3 -> claude reason=quota_threshold
OULIPOLY_INVOCATION={"source":"claude","id":"19b6c8d7-..."}
[diagnostics] Failed to diagnose: Empty command
No conversation found with session ID: 1bc948a0-2c57-4261-b703-7e4c27ecff00
```

The contradiction is real: `[migrate]` is emitted after `migrate_chain_segment` writes the target JSONL, but the child Claude Code process still rejects `--resume <session_id>`.

Independent local verification on this checkout:

```text
$ find ~/.claude3/projects -path '*/1bc948a0-2c57-4261-b703-7e4c27ecff00.jsonl' -print
/home/nes/.claude3/projects/-home-nes-projects-server-manager-worktrees-init-142i-cleanup/1bc948a0-2c57-4261-b703-7e4c27ecff00.jsonl
```

Running Claude Code from this worktree, where the matching cwd-derived project directory is different, produced the same child error:

```text
$ env CLAUDE_CONFIG_DIR="$HOME/.claude3" claude --resume 1bc948a0-2c57-4261-b703-7e4c27ecff00 -p ''
No conversation found with session ID: 1bc948a0-2c57-4261-b703-7e4c27ecff00
```

Copying the same JSONL into a temp Claude config under this worktree's cwd-derived project directory changed the error, proving Claude found the JSONL at that location:

```text
$ env CLAUDE_CONFIG_DIR="$PWD/.tmp/claude-config.4JfZL5" claude --resume 1bc948a0-2c57-4261-b703-7e4c27ecff00 -p ''
Error: No deferred tool marker found in the resumed session. Either the session was not deferred, the marker is stale (tool already ran), or it exceeds the tail-scan window. Provide a prompt to continue the conversation.
```

## Root Cause(s)

### RC-1 — Migration writes under the source workspace project directory, but Claude Code resumes from the child process cwd project directory

`migrate_chain_segment` derives the target directory from the source transcript parent directory:

- `src-tauri/src/migration/mod.rs:155-161` reads `cwd_hash` from `source_path.parent().file_name()`.
- `src-tauri/src/migration/mod.rs:188-195` writes `<target_projects_dir>/<source_project_dir>/<session_id>.jsonl`.

Claude Code, when invoked as `claude --resume <session_id>`, looks under the project directory derived from the process cwd. In the observed case, the source file lived under:

```text
~/.claude3/projects/-home-nes-projects-server-manager-worktrees-init-142i-cleanup/1bc948a0-2c57-4261-b703-7e4c27ecff00.jsonl
```

The resume attempt ran from:

```text
/home/nes/projects/agent-runner/worktrees/rca-session-migration
```

which maps to a different Claude project directory:

```text
-home-nes-projects-agent-runner-worktrees-rca-session-migration
```

The executor also does not consume the migrated target path:

- `src-tauri/src/main.rs:1839-1848` receives `MigratedSegment` and keeps only provider/session identity.
- `src-tauri/src/main.rs:1897-1900` passes `target_jsonl_path: None`.
- `src-tauri/src/executor/cli.rs:282-289` accepts `target_jsonl_path` but ignores it.
- `src-tauri/src/executor/cli.rs:292-297` composes only provider args plus `--resume <session_id>`.

The target JSONL can therefore exist exactly where migration wrote it and still be invisible to the target child process.

## Files Involved

- `src-tauri/src/migration/mod.rs` at current pre-fix commit `754ebb8`: migration copies source bytes and derives target dir from source transcript parent.
- `src-tauri/src/main.rs` at `754ebb8`: both resume paths update `resolved` after migration but do not preserve the migrated JSONL path for spawn.
- `src-tauri/src/executor/cli.rs` at `754ebb8`: resume argv composition ignores `ResumePayload.target_jsonl_path` and sends only the configured resume flag/subcommand plus session id.
- `scripts/claude-code-locate-transcript` at `754ebb8`: confirms the runner's locator can find transcripts by filename across a projects tree, which differs from Claude Code's own cwd-scoped resume lookup.
- Migration file-write logic was introduced with `91403a0 feat(05): session migration with chain identity and Claude Code support`; `git log -p -S 'std::fs::write(&tmp, slice)' -- src-tauri/src/migration/mod.rs` shows the copy was present from the initial migration implementation, so H5 is refuted for this branch.

## Reproduction

### RC-1 Harness

Harness:

```text
src-tauri/tests/session_migration_rca.rs
src-tauri/tests/session_migration_rca/mod.rs
src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs
```

Red-run command:

```bash
cd src-tauri && cargo test --test session_migration_rca rc1_migrated_transcript_must_be_honorable_from_resume_working_dir 2>&1 \
  | tee ../.tmp/rc1-red-run.log
```

Verbatim failure output:

```text
running 1 test
test session_migration_rca::rc1_cwd_project_dir_mismatch::rc1_migrated_transcript_must_be_honorable_from_resume_working_dir ... FAILED

failures:

---- session_migration_rca::rc1_cwd_project_dir_mismatch::rc1_migrated_transcript_must_be_honorable_from_resume_working_dir stdout ----

thread 'session_migration_rca::rc1_cwd_project_dir_mismatch::rc1_migrated_transcript_must_be_honorable_from_resume_working_dir' (21572) panicked at tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs:62:5:
assertion `left == right` failed: post-fix migration should make target provider resume honorable; stderr was: No conversation found with session ID: 1bc948a0-2c57-4261-b703-7e4c27ecff00

  left: 1
 right: 0
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    session_migration_rca::rc1_cwd_project_dir_mismatch::rc1_migrated_transcript_must_be_honorable_from_resume_working_dir

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

error: test failed, to rerun pass `--test session_migration_rca`
```

## Open Questions

- H2, provider-specific metadata in the JSONL body: refuted for the observed mechanism. The sampled real JSONL has `sessionId` and `cwd` fields, but copying it into the cwd-derived target project directory changed Claude's error away from "No conversation found", so body metadata did not prevent discovery.
- H3, target `projects_dir` versus child config directory: not confirmed as the observed failure. The live `claude3` wrapper sets `CLAUDE_CONFIG_DIR="$HOME/.claude3"` and plain `claude` uses the default `~/.claude`; the observed session file existed in those stores. A separate config-consistency issue remains possible if a provider's `session_storage.projects_dir` does not match the command's Claude config directory.
- H4, target resume argv composition: refuted by code inspection. `compose_resume_provider_args` appends the target provider's configured resume flag/subcommand and the unchanged session id.
- H5, stale pre-content-transfer `[migrate]` line: refuted. The file write existed in the initial migration commit `91403a0`, and this branch is `754ebb8`.
- H6, target `session_turns` ingestion: not the immediate child failure. Migration closes the source chain segment and opens the target segment, so `resolve_resume` can route the chain to the target provider even without target `session_turns`; missing target turns may affect later previews or compaction-boundary behavior but does not explain Claude Code's own "No conversation found" stderr.
