# Claude Code Project-Dir Hash RCA

## Symptom

The initial trigger is the WU-11-01 resume failure captured in
`research/14-session-migration-rca.md`: Oulipoly emitted a successful
`[migrate] claude3 -> claude reason=quota_threshold` line, then the child
Claude Code process still reported:

```text
No conversation found with session ID: 1bc948a0-2c57-4261-b703-7e4c27ecff00
```

WU-14-01 partially fixed that failure by changing migration to write under
the spawn cwd's Claude project directory instead of the source transcript's
project directory. The partial fix left the cwd-to-project-dir encoder itself
too narrow. It still only replaces `/` with `-`, so migration can write a
JSONL under a project directory name that Claude Code will not search when
the cwd contains Windows separators, special characters, or symlink
components.

## Authoritative evidence

The in-repo evidence baseline for this work unit is this RCA file. It records
the root-provided external encoder evidence and the root-provided real-Claude
symlink probe.

The Claude Code community reverse-engineered the actual project-dir encoder
in anthropics/claude-code#19972:

```python
def encode_path(path):
    result = path.replace('/', '-').replace('\\', '-')
    # Keep only ASCII alphanumeric characters
    result = ''.join([c if (c.isascii() and c.isalnum()) or c == '-' else '-' for c in result])
    return result
```

The issue evidence was reverse-engineered by user `japsoon` on 2026-01-21
and corroborated by `sindrijo` on 2026-02-12 with collision examples.

The root also verified symlink behavior with real Claude Code at
`/tmp/symtest$$/`:

```text
Probe A: jsonl at LITERAL symlink-encoded path, cwd=symlink ->
  "No conversation found"

Probe B: jsonl at RESOLVED symlink-encoded path, cwd=symlink ->
  "Error: No deferred tool marker found in the resumed session"
```

The changed error in Probe B means Claude found the session JSONL only when
it was placed under the resolved symlink target's encoded project directory.
Claude Code therefore canonicalizes symlinks before hashing for `--resume`
lookup.

## Root Cause(s)

### RC-1 — Encoder preserves non-alnum characters that Claude replaces

`src-tauri/src/migration/mod.rs:256` implements:

```rust
Ok(cwd.to_string_lossy().replace('/', "-"))
```

That preserves `_`, `.`, `:`, accented characters, and CJK characters. The
Claude Code encoder replaces every character except ASCII alphanumeric and
`-` with `-`. The existing inline unit test at
`src-tauri/src/migration/mod.rs:463` only covers `/home/nes/x`, so it does
not exercise any character that distinguishes the repo encoder from Claude's
encoder.

### RC-2 — Encoder does not handle Windows-shaped paths or backslashes

The same helper at `src-tauri/src/migration/mod.rs:256` rejects paths unless
`Path::is_absolute()` is true on the current platform and then only replaces
forward slashes. On Unix, a Windows-shaped `PathBuf::from(r"C:\Users\...")`
falls into `MigrationError::SpawnCwdUnsupported`; if accepted without a
complete encoder, backslashes would also survive instead of becoming `-`.

D-010 intentionally deferred Windows hashing because the project lacked an
authoritative rule. The encoder evidence above resolves that deferral.

### RC-3 — Encoder hashes the literal symlink path, but Claude hashes the resolved path

`src-tauri/src/migration/mod.rs:256` encodes the `cwd` string directly. It
does not resolve symlink components first. The root's real-Claude probe shows
that `claude --resume` canonicalizes symlinks before hashing, so migration
currently writes under the literal symlink path's project directory while
Claude searches under the resolved path's project directory.

D-011 intentionally deferred symlink canonicalization because real-Claude
behavior was unknown. The root probe resolves that deferral for symlinked
workspaces.

## Files Involved

- `src-tauri/src/migration/mod.rs`: `claude_project_dir_for` at line 256 and
  its narrow inline coverage at line 463.
- `src-tauri/tests/claude_path_hash_rca.rs`: integration-test entry point.
- `src-tauri/tests/claude_path_hash_rca/mod.rs`: shared migration fixture and
  authoritative test encoder.
- `src-tauri/tests/claude_path_hash_rca/rc1_non_alnum_encoding.rs`: RC-1
  reproduction harness.
- `src-tauri/tests/claude_path_hash_rca/rc2_windows_backslash_encoding.rs`:
  RC-2 reproduction harness.
- `src-tauri/tests/claude_path_hash_rca/rc3_symlink_canonicalization.rs`:
  RC-3 reproduction harness.
- `DECISIONS.md`: D-010 and D-011 record the prior Windows and symlink
  deferrals.
- `risk/14-test-residuals.md`: prior residuals for Windows Claude project
  directory hashing and symlink/canonicalization behavior.

## Reproduction

### RC-1 Harness

Harness:

```text
src-tauri/tests/claude_path_hash_rca.rs
src-tauri/tests/claude_path_hash_rca/mod.rs
src-tauri/tests/claude_path_hash_rca/rc1_non_alnum_encoding.rs
```

Red-run command:

```bash
cd src-tauri && cargo test --test claude_path_hash_rca rc1_project_dir_encoder_replaces_all_non_alnum_except_dash
```

Verbatim failure output:

```text
   Compiling oulipoly-agent-runner v0.1.0 (/home/nes/projects/agent-runner/worktrees/rca-claude-path-hash/src-tauri)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 5.39s
     Running tests/claude_path_hash_rca.rs (target/debug/deps/claude_path_hash_rca-f0f5c5568a4c68ae)

running 1 test
test claude_path_hash_rca::rc1_non_alnum_encoding::rc1_project_dir_encoder_replaces_all_non_alnum_except_dash ... FAILED

failures:

---- claude_path_hash_rca::rc1_non_alnum_encoding::rc1_project_dir_encoder_replaces_all_non_alnum_except_dash stdout ----

thread 'claude_path_hash_rca::rc1_non_alnum_encoding::rc1_project_dir_encoder_replaces_all_non_alnum_except_dash' (53908) panicked at tests/claude_path_hash_rca/rc1_non_alnum_encoding.rs:17:5:
assertion `left == right` failed: Claude Code project-dir encoding must replace '.', '_', and CJK characters with '-'
  left: "/tmp/.tmpvcQRta/claude-target/projects/-tmp-.tmpvcQRta-work_trees-tmp.UfwcMhrgHV-漢字_model/0de9435c-3727-49fd-998c-cd0ea2c177f7.jsonl"
 right: "/tmp/.tmpvcQRta/claude-target/projects/-tmp--tmpvcQRta-work-trees-tmp-UfwcMhrgHV----model/0de9435c-3727-49fd-998c-cd0ea2c177f7.jsonl"
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    claude_path_hash_rca::rc1_non_alnum_encoding::rc1_project_dir_encoder_replaces_all_non_alnum_except_dash

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2 filtered out; finished in 0.01s

error: test failed, to rerun pass `--test claude_path_hash_rca`
```

### RC-2 Harness

Harness:

```text
src-tauri/tests/claude_path_hash_rca.rs
src-tauri/tests/claude_path_hash_rca/mod.rs
src-tauri/tests/claude_path_hash_rca/rc2_windows_backslash_encoding.rs
```

Red-run command:

```bash
cd src-tauri && cargo test --test claude_path_hash_rca rc2_windows_shape_path_uses_backslash_and_non_alnum_encoding_rule
```

Verbatim failure output:

```text
   Compiling oulipoly-agent-runner v0.1.0 (/home/nes/projects/agent-runner/worktrees/rca-claude-path-hash/src-tauri)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 5.22s
     Running tests/claude_path_hash_rca.rs (target/debug/deps/claude_path_hash_rca-f0f5c5568a4c68ae)

running 1 test
test claude_path_hash_rca::rc2_windows_backslash_encoding::rc2_windows_shape_path_uses_backslash_and_non_alnum_encoding_rule ... FAILED

failures:

---- claude_path_hash_rca::rc2_windows_backslash_encoding::rc2_windows_shape_path_uses_backslash_and_non_alnum_encoding_rule stdout ----

thread 'claude_path_hash_rca::rc2_windows_backslash_encoding::rc2_windows_shape_path_uses_backslash_and_non_alnum_encoding_rule' (54548) panicked at tests/claude_path_hash_rca/rc2_windows_backslash_encoding.rs:13:10:
post-fix migration should encode Windows-shaped paths via the Claude Code rule: SpawnCwdUnsupported { provider: "claude-target", cwd: "C:\\Users\\foo.bar\\work_tree\\漢字" }
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    claude_path_hash_rca::rc2_windows_backslash_encoding::rc2_windows_shape_path_uses_backslash_and_non_alnum_encoding_rule

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2 filtered out; finished in 0.00s

error: test failed, to rerun pass `--test claude_path_hash_rca`
```

### RC-3 Harness

Harness:

```text
src-tauri/tests/claude_path_hash_rca.rs
src-tauri/tests/claude_path_hash_rca/mod.rs
src-tauri/tests/claude_path_hash_rca/rc3_symlink_canonicalization.rs
```

Red-run command:

```bash
cd src-tauri && cargo test --test claude_path_hash_rca rc3_symlinked_workspace_hashes_resolved_path_not_literal_link_path
```

Verbatim failure output:

```text
   Compiling oulipoly-agent-runner v0.1.0 (/home/nes/projects/agent-runner/worktrees/rca-claude-path-hash/src-tauri)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 5.54s
     Running tests/claude_path_hash_rca.rs (target/debug/deps/claude_path_hash_rca-f0f5c5568a4c68ae)

running 1 test
test claude_path_hash_rca::rc3_symlink_canonicalization::rc3_symlinked_workspace_hashes_resolved_path_not_literal_link_path ... FAILED

failures:

---- claude_path_hash_rca::rc3_symlink_canonicalization::rc3_symlinked_workspace_hashes_resolved_path_not_literal_link_path stdout ----

thread 'claude_path_hash_rca::rc3_symlink_canonicalization::rc3_symlinked_workspace_hashes_resolved_path_not_literal_link_path' (55223) panicked at tests/claude_path_hash_rca/rc3_symlink_canonicalization.rs:24:5:
assertion `left == right` failed: Claude Code hashes the resolved cwd path for symlinked workspaces
  left: "/tmp/.tmpKl8OZJ/claude-target/projects/-tmp-.tmpKl8OZJ-linked-workspace/0de9435c-3727-49fd-998c-cd0ea2c177f7.jsonl"
 right: "/tmp/.tmpKl8OZJ/claude-target/projects/-tmp--tmpKl8OZJ-real-workspace/0de9435c-3727-49fd-998c-cd0ea2c177f7.jsonl"
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    claude_path_hash_rca::rc3_symlink_canonicalization::rc3_symlinked_workspace_hashes_resolved_path_not_literal_link_path

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2 filtered out; finished in 0.01s

error: test failed, to rerun pass `--test claude_path_hash_rca`
```

## Open Questions

- I did not rerun a live Windows Claude Code probe. The Windows-shaped harness
  uses the authoritative encoder from anthropics/claude-code#19972 and
  validates the resulting project directory deterministically on this Unix
  checkout.
- I did not test case normalization, `..` cleanup, or broader path
  normalization. The evidence gathered for this RCA is specifically about
  backslash handling, non-alnum encoding, and symlink resolution before
  hashing.
