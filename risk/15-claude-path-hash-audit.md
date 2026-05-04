# WU-14-02 Phase 4 R4 Audit Risk Gate

Audited proposal: `proposals/15-claude-path-hash.md`.

Note: the referenced NEEDS_INPUT answer path was not present in this
worktree during audit, but the R4 brief includes the root-approved Option A
scope expansion. This audit treats the two-locus
`src-tauri/tests/session_migration_rca/mod.rs` mirror update as approved.

## Checklist

1. **Anti-scope** — present.

   The proposal carries forward the ticket anti-scope items: no unrelated
   `MigrationError` changes, no runtime old-encoder flag, no over-
   canonicalization beyond `std::fs::canonicalize`, no
   `MigratedSegment.target_jsonl_path` rewrite, no platform-specific
   `#[cfg(target_os)]`, and no removal of
   `claude_project_dir_for_encodes_absolute_unix_path`
   (`proposals/15-claude-path-hash.md:21-44`).

   It encodes the approved Option A expansion as the only allowed touch inside
   `src-tauri/tests/session_migration_rca/`: the Rust
   `claude_project_dir_name` mirror and the fake-Claude Bash lookup snippet
   (`proposals/15-claude-path-hash.md:33-41`). The Bash locus is cited at
   `mod.rs:109-115` (`proposals/15-claude-path-hash.md:39-40`), and the helper
   is named directly as `tests/session_migration_rca/mod.rs::claude_project_dir_name`
   (`proposals/15-claude-path-hash.md:37-39`, `proposals/15-claude-path-hash.md:143-146`).
   The proposal also says the WU-14-01 test assertions, fixtures, and assertion
   semantics stay unchanged, with only those two encoder mirrors updated
   (`proposals/15-claude-path-hash.md:33-41`), which keeps
   `rc1_cwd_project_dir_mismatch.rs`'s test body intact under the approved
   expansion.

   Other adjacent slash-only helpers are explicitly out of scope:
   `tests/fixtures/initiative_06.rs:886-888`,
   `tests/fixtures/initiative_06_import_replace.rs:995-997`,
   `tests/fixtures/initiative_06_export.rs:605-607`,
   `tests/initiative_05_migration.rs:636-638`, and
   `tests/pr_f_resume_integration.rs:949-959`
   (`proposals/15-claude-path-hash.md:54-60`).

2. **Encoder algorithm** — present.

   The design includes pseudocode that replaces `/`, replaces `\`, then filters
   to ASCII alphanumeric plus `-`, replacing every other character with `-`
   (`proposals/15-claude-path-hash.md:77-102`).

3. **Canonicalization step** — present.

   The proposal commits to `std::fs::canonicalize(cwd)` before hashing and
   states that success hashes the resolved absolute path
   (`proposals/15-claude-path-hash.md:84-91`,
   `proposals/15-claude-path-hash.md:121-126`).

4. **`canonicalize` failure fallback** — present.

   On canonicalize failure, the proposal hashes the literal cwd and emits a
   warning (`proposals/15-claude-path-hash.md:86-91`,
   `proposals/15-claude-path-hash.md:121-139`). It names the warning channel as
   the existing `stderr: &mut dyn Write` stream
   (`proposals/15-claude-path-hash.md:127-130`) and gives the warning shape
   (`proposals/15-claude-path-hash.md:131-135`).

5. **`MigrationError::SpawnCwdUnsupported` posture** — present.

   The proposal explicitly keeps the variant, narrows it to empty cwd only, and
   removes the current `cwd.is_absolute()` rejection
   (`proposals/15-claude-path-hash.md:108-119`).

6. **String input shape** — present.

   The proposal commits to `path_for_hash.to_string_lossy()` in pseudocode and
   restates that implementation should use `path.to_string_lossy()`
   (`proposals/15-claude-path-hash.md:93-106`).

7. **Supported-surface track** — present.

   The supported-surface section covers deployment mode, customer cohort,
   adjacent user-reachable paths, blast-radius notes, migration path, rollback
   path, and observability (`proposals/15-claude-path-hash.md:178-235`).

8. **Assumption register** — present.

   The proposal has at least four assumptions, each with evidence and an
   invalidator: A1 through A4 (`proposals/15-claude-path-hash.md:237-267`).

9. **Test-intent track** — present.

   The proposal includes entries for RC-1, RC-2, and RC-3
   (`proposals/15-claude-path-hash.md:271-322`), plus AC-4 inline regression
   coverage (`proposals/15-claude-path-hash.md:353-365`) and AC-5 cross-RCA
   stability (`proposals/15-claude-path-hash.md:367-381`).

10. **Net-value statement** — present.

    The proposal qualitatively states the risk reduction, why the change is
    localized, why legacy alphanumeric Unix paths remain stable, and why the
    two-locus test mirror expansion does not change the value case
    (`proposals/15-claude-path-hash.md:423-444`).

11. **Test residuals plan** — present.

    The proposal says `risk/15-test-residuals.md` is needed if Phase 6b treats
    lack of a live Windows Claude Code probe as a residual, and expects that
    residual to be recorded (`proposals/15-claude-path-hash.md:446-453`).

12. **AC mapping** — present.

    The proposal maps AC-1 through AC-8 to concrete design elements
    (`proposals/15-claude-path-hash.md:157-176`).

13. **Reproduction harness preservation** — present.

    The proposal states the three WU-14-02 RCA harnesses must flip RED -> GREEN
    and remain in place as regression tests (`proposals/15-claude-path-hash.md:324-332`).
    It also states the WU-14-01 `session_migration_rca/` harness remains GREEN
    with its cwd-mismatch contract unchanged after the two scoped encoder
    mirrors are updated (`proposals/15-claude-path-hash.md:333-335`).

14. **Inline test update plan** — present.

    The proposal names the existing inline
    `claude_project_dir_for_encodes_absolute_unix_path` test and says it will be
    updated, not removed, to assert the full rule
    (`proposals/15-claude-path-hash.md:42-44`,
    `proposals/15-claude-path-hash.md:165-166`,
    `proposals/15-claude-path-hash.md:353-363`).

## Verdict

All 14 checklist items are present. The approved Option A scope expansion is
encoded narrowly enough for Phase 4 risk purposes.

Verdict: LOW
