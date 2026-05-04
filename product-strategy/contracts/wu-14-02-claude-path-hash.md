# Contract — WU-14-02 claude-path-hash

Owner: implementation-pipeline-orchestrator (Phase 6a; orchestrator-authored)

Source artifacts:

- `proposals/15-claude-path-hash.md` (Phase 4 R4 LOW on all four gates)
- `research/15-claude-path-hash-problem-map.md` (Phase 2.5 R2)
- `research/15-claude-path-hash-hookpoints.md` (Phase 5 R2; PHASE-5-RESOLVED)
- `research/15-claude-path-hash-rca.md` (Phase 0 RCA, inherited from `rca/claude-path-hash`)
- WU-14-02 ticket on `tickets/phase-14`
- NEEDS_INPUT answer: `tmp/scratch/wu-14-02/questions/phase-3-r3-ticket-scope-contradiction.answer.md`
  (Option A — narrow two-locus Anti-scope expansion approved)

Inputs to Step 6b (test writer) and Step 6c (code writer).

This contract is the orchestrator's interface between the test
agent (Step 6b) and the code agent (Step 6c). The test agent does
NOT see the code agent's output. The code agent reads this
contract, the proposal, the hookpoints, the problem map, the RCA,
and the Step 6b output index — and only then writes product code.

---

## 1. Acceptance criteria (from ticket)

- **AC-1 (RC-1)** — `claude_path_hash_rca::rc1_non_alnum_encoding::rc1_project_dir_encoder_replaces_all_non_alnum_except_dash`
  passes on the post-fix branch. The encoder replaces every non-`-`,
  non-ASCII-alphanumeric character with `-`.
- **AC-2 (RC-2)** — `claude_path_hash_rca::rc2_windows_backslash_encoding::rc2_windows_shape_path_uses_backslash_and_non_alnum_encoding_rule`
  passes. Windows-shape paths (with `\` and drive letters) are
  encoded via the same rule. The `MigrationError::SpawnCwdUnsupported`
  rejection of non-Unix shapes is lifted; only empty cwd is still
  rejected.
- **AC-3 (RC-3)** — `claude_path_hash_rca::rc3_symlink_canonicalization::rc3_symlinked_workspace_hashes_resolved_path_not_literal_link_path`
  passes. The encoder canonicalizes (resolves symlinks) before
  applying the encoding rule. On `canonicalize` failure, the
  encoder falls back to the literal cwd and emits the documented
  warning to the migration `stderr` writer.
- **AC-4** — Existing migration tests stay GREEN: the inline
  `migration::tests` (`claude_project_dir_for_*` and any others),
  plus all of `tests/session_migration_rca/` after the approved
  two-locus encoder-mirror update.
- **AC-5** — Other prior reproduction harnesses stay GREEN with no
  fixture edits: `tests/routing_fanout_rca/`,
  `tests/empty_bodies_ref_rca/`, `tests/release_yml_contract.rs`,
  `tests/session_lock_cross_platform.rs`.
- **AC-6** — Backend gates green:
  `cd src-tauri && cargo fmt --check && cargo clippy -- -D warnings && cargo test --no-fail-fast`.
  Frontend gates green (no `src/` changes expected):
  `bun run check && bunx tsc --noEmit && bun run test`.
- **AC-7** — `DECISIONS.md` updates (Phase 6c, before commit):
  - D-010 marked **resolved** by this WU; entry updated with a
    "Resolved by WU-14-02 / PR #N — date" line. The entry is updated
    in place, not rewritten or deleted.
  - D-011 marked **resolved** by this WU; same treatment.
  - Append a new `D-NN — WU-14-02 Phase 2.5 human-gate skip` per the
    pre-approval policy stated in the orchestrator brief.
  - Append a new `D-NN — WU-14-02 narrow two-locus Anti-scope amendment`
    naming the two loci, citing the question artifact, and noting the
    WU-14-01 RC-1 test contract is preserved.
- **AC-8** — `risk/14-test-residuals.md` updated:
  - "Windows Claude project directory hashing" entry marked
    **resolved**, with pointer to RC-2 harness and PR.
  - "Symlink and canonicalization behavior" entry marked
    **resolved**, with pointer to RC-3 harness and PR.

## 2. Code surfaces (in-scope)

- `src-tauri/src/migration/mod.rs`:
  - **Rewrite `claude_project_dir_for`** at the existing definition
    site (line 256+). Signature changes from
    `pub(crate) fn claude_project_dir_for(provider: &str, cwd: &Path) -> Result<String, MigrationError>`
    to
    `pub(crate) fn claude_project_dir_for(provider: &str, cwd: &Path, stderr: &mut dyn Write) -> Result<String, MigrationError>`.
    The new body implements the encoder algorithm in §4 below.
  - **Update production caller** at line 161 to pass the existing
    `stderr` parameter through:
    `claude_project_dir_for(&target.name, resume_working_dir, stderr)?`.
  - **Update inline tests** at line 463+:
    - `claude_project_dir_for_encodes_absolute_unix_path` —
      strengthen fixture path to include `_` and `.` (per the
      ticket's explicit instruction); assert the FULL rule output;
      pass a local `Vec<u8>` stderr sink.
    - `claude_project_dir_for_rejects_relative_path` — REPLACE with a
      new test that asserts a relative path is **accepted** under the
      new contract (the `is_absolute()` rejection is lifted) and that
      its canonicalized-or-literal-fallback output matches the new
      rule.
    - `claude_project_dir_for_rejects_empty_path` — keep; update its
      direct call to pass the new stderr sink parameter.
  - **Update `MigrationError::SpawnCwdUnsupported` trigger**: remove
    the `cwd.is_absolute()` check; only `cwd.as_os_str().is_empty()`
    triggers the variant.
  - **Add `use std::io::Write;`** if not already imported (the file
    already uses `writeln!`; verify the import).
- `src-tauri/tests/session_migration_rca/mod.rs` (approved
  two-locus update only; no other edits):
  - **Locus 1**: `claude_project_dir_name` at lines 129-130. Rewrite
    the function body to apply the SAME encoder rule as the
    production code (replace `/` and `\` with `-`, then filter to
    ASCII alnum + `-`, replacing every other character with `-`).
    The function signature and call sites stay unchanged.
  - **Locus 2**: the `fake_claude` Bash heredoc at lines 109-115.
    Rewrite the `project="${PWD//\//-}"` snippet so it applies the
    full rule. Use the helper-string approach from
    `proposals/15-claude-path-hash.md` §3:
    ```bash
    project=$(printf '%s' "$PWD" | sed -e 's#[/\\]#-#g' -e 's/[^A-Za-z0-9-]/-/g')
    ```
    Or an equivalent that produces byte-identical output to the new
    encoder for any `$PWD` with `tempfile::tempdir()` shape on Linux
    and macOS. The fake-Claude Bash does NOT canonicalize symlinks;
    the WU-14-01 RC-1 test does not exercise symlinks.

### Out of scope (per ticket Anti-scope + R3 amendment)

- Anywhere else in `tests/session_migration_rca/`, especially
  `rc1_cwd_project_dir_mismatch.rs` (the test body stays intact).
- Other adjacent slash-only test helpers:
  `tests/fixtures/initiative_06.rs:886-888`,
  `tests/fixtures/initiative_06_import_replace.rs:995-997`,
  `tests/fixtures/initiative_06_export.rs:605-607`,
  `tests/initiative_05_migration.rs:636-638`,
  `tests/pr_f_resume_integration.rs:949-959`.
- All `src/` (frontend), all other `src-tauri/src/` modules,
  Cargo manifests, Tauri config, `scripts/*`.
- `decode_claude_project_dir_candidates` at
  `src-tauri/src/session_metadata/mod.rs:338` (inversion helper;
  not in the WU-14-02 forward path).
- `MigratedSegment.target_jsonl_path` contract.
- Platform-specific `#[cfg(target_os)]` code in the encoder.

## 3. Test surfaces (in-scope)

- `src-tauri/tests/claude_path_hash_rca/rc1_non_alnum_encoding.rs` —
  must flip RED → GREEN, fixture and test body stay in place.
- `src-tauri/tests/claude_path_hash_rca/rc2_windows_backslash_encoding.rs`
  — must flip RED → GREEN, fixture and test body stay in place.
- `src-tauri/tests/claude_path_hash_rca/rc3_symlink_canonicalization.rs`
  — must flip RED → GREEN, fixture and test body stay in place.
- `src-tauri/tests/claude_path_hash_rca/mod.rs` — fixture builder;
  may extend with new fixture helpers IF the inline tests need them,
  but the existing fixtures must stay.
- `src-tauri/src/migration/mod.rs` inline tests at line 463+:
  - rewrite as described in §2 above
  - add a new inline test for the **canonicalize-failure fallback
    contract**: provide a `cwd` whose canonicalize fails (e.g., a
    non-existent path), pass a captured stderr sink, assert the
    encoder returns the literal-path full-rule output AND the stderr
    contains the documented warning text.
- New inline test for the **`SpawnCwdUnsupported` empty-cwd-only
  contract**: assert that an empty `&Path` returns
  `SpawnCwdUnsupported`, but a relative non-empty path does NOT.

## 4. Encoder algorithm (authoritative)

```rust
fn claude_project_dir_for(
    provider: &str,
    cwd: &Path,
    stderr: &mut dyn Write,
) -> Result<String, MigrationError> {
    if cwd.as_os_str().is_empty() {
        return Err(MigrationError::SpawnCwdUnsupported {
            provider: provider.to_string(),
            cwd: cwd.display().to_string(),
        });
    }

    let path_for_hash: PathBuf = match std::fs::canonicalize(cwd) {
        Ok(resolved) => resolved,
        Err(error) => {
            let _ = writeln!(
                stderr,
                "Warning: Claude project-dir canonicalize failed for provider={provider} cwd={} error={}; falling back to literal cwd",
                cwd.display(),
                error,
            );
            cwd.to_path_buf()
        }
    };

    let input = path_for_hash.to_string_lossy();
    let mut encoded = String::with_capacity(input.len());
    for ch in input.chars() {
        let mapped = match ch {
            '/' | '\\' => '-',
            c if (c.is_ascii() && c.is_alphanumeric()) || c == '-' => c,
            _ => '-',
        };
        encoded.push(mapped);
    }
    Ok(encoded)
}
```

Key invariants Step 6b tests must encode:

1. **Filter rule** — for any input `s: &str`, the output preserves
   only ASCII alphanumeric chars and `-`; all others (including `/`,
   `\`, `_`, `.`, `:`, accented latin, CJK, NUL) become `-`.
2. **Canonicalization** — when `cwd` exists on disk and contains a
   symlink, the encoder hashes the resolved target's path (per RC-3).
3. **Canonicalize-failure fallback** — when `std::fs::canonicalize`
   fails (path doesn't exist, permission denied, Windows-shape on a
   Unix host), the encoder hashes the literal cwd's path AND emits
   the warning to stderr containing the literal substrings
   `Warning:`, `provider=<provider>`, `cwd=<cwd display>`, and
   `falling back to literal cwd`.
4. **Empty-cwd rejection** — `cwd.as_os_str().is_empty()` returns
   `MigrationError::SpawnCwdUnsupported`. No other input shape
   (relative, absolute, Windows-shape, non-existent) returns this
   error.
5. **`to_string_lossy()` input shape** — non-UTF8 bytes in `cwd`
   produce `U+FFFD` replacement char in the input which then maps to
   `-` under the filter.
6. **Production caller** — `migrate_chain_segment` passes the
   already-existing `stderr` parameter through to
   `claude_project_dir_for`; no new stderr surface is introduced.

## 5. Step 6b output index requirements

The Step 6b output index at `tmp/scratch/wu-14-02/phase6/step6b-output-index.md`
must list, per the Phase 6b output-index spec:

- approved proposal path
- contract path (this file)
- approved problem-map path
- supported-surface risk path
- hookpoint research path
- Step 6b prompt path + log path
- one row per AC / RC / risk-level / fixture mapping with the
  emitted test file path and test name

Test residuals are recorded at `risk/15-test-residuals.md` if the
test set leaves a named risk unverified. The proposal flags the
"no live Windows Claude probe was run" residual; Step 6b decides
whether to author the residuals file.

## 6. Step 6c gates

After Step 6c writes product code, the following must all pass
before Phase 7:

- `cd src-tauri && cargo fmt --check`
- `cd src-tauri && cargo clippy -- -D warnings`
- `cd src-tauri && cargo test --no-fail-fast`
- `bun run check`
- `bunx tsc --noEmit`
- `bun run test`

The three RCA harnesses (`rc1`, `rc2`, `rc3`) must be GREEN. The
WU-14-01 RCA harness (`session_migration_rca::rc1_*`) must be GREEN
under the approved two-locus update.

## 7. Implementation guidance for Step 6c

- Keep the encoder body small (≤30 lines).
- Do NOT inline the encoder rule at the call site; keep it in
  `claude_project_dir_for`.
- Do NOT introduce a new logger dependency. The `stderr: &mut dyn Write`
  channel is what `migrate_chain_segment` already uses for `[migrate]`.
- Do NOT add `#[cfg(target_os)]` branches. The encoder is platform-
  neutral.
- Do NOT touch `MigratedSegment.target_jsonl_path`'s contract; the
  field returns the path actually written, which is now derived
  from the new encoder via the existing
  `projects_dir.join(&cwd_project_dir)` call at line 188.
- DECISIONS.md and risk/14-test-residuals.md updates (AC-7, AC-8)
  are part of Step 6c, NOT Step 6b.
- The Step 6b output index path
  (`tmp/scratch/wu-14-02/phase6/step6b-output-index.md`) MUST be
  echoed in Step 6c's log output before Step 6c modifies any
  product code, per the orchestrator's process-tree-audit
  evidence requirements.
