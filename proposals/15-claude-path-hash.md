# WU-14-02 — Claude Path Hash Proposal

Phase: 3 proposal  
Work unit: `claude-path-hash`  
Intent: complete the Claude Code cwd-to-project-directory encoder used by
session migration.

## 1. Summary

This change will replace the current slash-only Claude project-directory
encoder in `src-tauri/src/migration/mod.rs` with the authoritative Claude Code
string rule from anthropics/claude-code#19972, canonicalize the resume cwd
before hashing when possible, and keep empty cwd rejection as the only
`SpawnCwdUnsupported` trigger. It reduces the current risk that migrated JSONL
is written under a directory Claude Code will not search for Windows-shape
paths, paths containing filtered characters such as `_`, `.`, accented latin,
or CJK, and symlinked workspaces.

## 2. Anti-Scope

Ticket anti-scope, carried forward:

- Do NOT change `MigrationError` variants in unrelated ways. Only the
  `SpawnCwdUnsupported` trigger condition is in scope.
- Do NOT introduce a runtime feature flag for "old encoder" per
  `~/ai/conventions/no-backwards-compatibility.md`.
- Do NOT canonicalize beyond symlink resolution: no macOS case-folding, no
  platform-specific normalization, and no custom cleanup beyond what
  `std::fs::canonicalize` naturally performs.
- Do NOT rewrite `MigratedSegment.target_jsonl_path`'s contract.
- Do NOT introduce platform-specific code with `#[cfg(target_os)]`; the encoder
  rule is platform-neutral string processing.
- Do NOT alter the contract or test logic of the WU-14-01 RC-1 reproduction
  (`src-tauri/tests/session_migration_rca/`). The test's assertions, fixtures,
  and assertion semantics stay unchanged; the only updates inside this
  directory are the two encoder mirrors that this WU is changing in production:
  `tests/session_migration_rca/mod.rs::claude_project_dir_name` gets a
  one-function rewrite to apply the same encoder rule the production code now
  applies, and the fake-Claude Bash `${PWD//\//-}` snippet at
  `tests/session_migration_rca/mod.rs:109-115` is rewritten to apply the same
  rule. The test must stay GREEN under these updates; AC-4 is unchanged.
- Do NOT remove the existing
  `claude_project_dir_for_encodes_absolute_unix_path` inline test; update it to
  assert the FULL rule by including filtered characters.

Additional anti-scope:

- Do NOT change migration policy, provider selection, balancer behavior, state
  DB schema, transcript locator behavior, executor argv composition, frontend
  files, Tauri config, Cargo manifests, or adapter scripts.
- Do NOT add Windows-specific drive-letter handling. `:` and `\` are handled by
  the same generic encoder rule as every other character.
- Do NOT rewrite already-migrated JSONL files in bulk.
- Do NOT update adjacent slash-only helpers outside the two scoped
  `tests/session_migration_rca/mod.rs` encoder mirrors. Out-of-scope helpers
  are `tests/fixtures/initiative_06.rs:886-888`,
  `tests/fixtures/initiative_06_import_replace.rs:995-997`,
  `tests/fixtures/initiative_06_export.rs:605-607`,
  `tests/initiative_05_migration.rs:636-638` and its assertions, and
  `tests/pr_f_resume_integration.rs:949-959`.

## 3. Design

Chosen design: **one platform-neutral encoder, canonicalize first when
possible, warn and fall back to literal path when canonicalization fails**.

Production call site:

- `src-tauri/src/migration/mod.rs:161` remains the only production caller that
  derives `cwd_project_dir` from `resume_working_dir`.
- The supported resume paths continue to feed this call from
  `run_repl` and `run_resume` in `src-tauri/src/main.rs` as mapped by the
  problem map: interactive migration at approximately
  `src-tauri/src/main.rs:1622`, and session-bound migration at approximately
  `src-tauri/src/main.rs:1847`.

Encoder pseudocode mirrors anthropics/claude-code#19972:

```text
fn claude_project_dir_for(provider, cwd, stderr) -> Result<String, MigrationError>:
    if cwd.as_os_str().is_empty():
        return Err(SpawnCwdUnsupported { provider, cwd })

    path_for_hash = match std::fs::canonicalize(cwd):
        Ok(resolved) => resolved
        Err(error) =>:
            writeln!(
                stderr,
                "Warning: Claude project-dir canonicalize failed for provider={provider} cwd={cwd}: {error}; falling back to literal cwd"
            )
            cwd.to_path_buf()

    input = path_for_hash.to_string_lossy()
    replaced = input.replace('/', '-').replace('\\', '-')
    encoded = ""
    for char in replaced.chars():
        if (char.is_ascii() and char.is_alphanumeric()) or char == '-':
            encoded.push(char)
        else:
            encoded.push('-')
    return Ok(encoded)
```

The implementation should use `path.to_string_lossy()` for the string input
shape. That commits to the ticket's Phase 2.5 note: non-UTF8 replacement bytes
are acceptable because they already become `-` under the filter.

`MigrationError::SpawnCwdUnsupported` posture:

- Keep the variant.
- Narrow its trigger to empty cwd only.
- Remove the current `cwd.is_absolute()` rejection from the encoder. This
  satisfies AC-2 because Windows-shaped paths such as
  `C:\Users\foo.bar\work_tree\漢字` are string-encoded instead of rejected on
  Unix.
- Justification: empty cwd remains a malformed migration input worth surfacing
  as a typed error. Removing the variant would force either an unrelated error
  variant or silent handling of an invalid empty path. Keeping it with a narrow
  trigger is the smallest contract change.

Canonicalization fallback contract:

- The encoder attempts `std::fs::canonicalize(cwd)` before applying the string
  rule.
- On success, it hashes the resolved absolute path. This resolves symlink
  components and matches the real-Claude symlink probe in the RCA.
- On failure, it hashes the literal `cwd` and emits a warning.
- Warning channel: the existing `stderr: &mut dyn Write` used by
  `migrate_chain_segment`, not a new logging dependency. This keeps warnings
  on the same stream as `[migrate]` and makes them test-capturable.
- Warning shape:

```text
Warning: Claude project-dir canonicalize failed for provider=<provider> cwd=<cwd>: <error>; falling back to literal cwd
```

The warning is part of the production fallback contract. It avoids silently
mis-encoding when paths do not exist, permission is denied, or the platform
cannot canonicalize a Windows-shaped path on a Unix host.

Fixture encoder mirror update:

- `src-tauri/tests/session_migration_rca/mod.rs::claude_project_dir_name`
  mirrors the production encoder rule: replace `/` and `\` with `-`, then
  filter to ASCII alphanumeric plus `-`, replacing every other character with
  `-`.
- The fake-Claude Bash snippet at `src-tauri/tests/session_migration_rca/mod.rs:109-115`
  should derive its `project=...` lookup through a small per-test reusable Bash
  helper string, for example a function that runs
  `printf '%s' "$1" | sed -e 's#[/\\]#-#g' -e 's/[^A-Za-z0-9-]/-/g'`, then
  calls that helper with `$PWD`. This keeps shell duplication limited to the
  fixture's lookup mirror instead of scattering inline substitutions.
- No symlink canonicalization is added to the fake-Claude Bash. The WU-14-01
  RC-1 test does not exercise symlinks; this helper update is
  character-filter-only.

AC mapping:

- AC-1 / RC-1: the filter rule replaces every non-ASCII-alphanumeric,
  non-`-` character with `-`.
- AC-2 / RC-2: the `is_absolute()` rejection is lifted; only empty cwd returns
  `SpawnCwdUnsupported`.
- AC-3 / RC-3: canonicalize before hashing; warn and literal-fallback on
  canonicalize failure.
- AC-4: inline migration tests are updated, including the existing
  `claude_project_dir_for_encodes_absolute_unix_path`, to assert the full rule.
- AC-5: prior RCA harnesses stay green. The WU-14-01
  `session_migration_rca/` harness stays green after the two scoped encoder
  mirrors in `mod.rs` are updated; the other named prior harnesses stay green
  with no fixture edits.
- AC-6: backend and frontend gates run; no frontend changes are expected.
- AC-7: `DECISIONS.md` D-010 and D-011 are marked resolved, and the Phase 2.5
  human-gate skip entry is appended in Phase 6c.
- AC-8: `risk/14-test-residuals.md` is updated in Phase 6c to mark the Windows
  hashing and symlink/canonicalization residuals resolved by the new harnesses
  and implementation.

## 4. Supported-Surface Track

Deployment mode: next release.

Customer cohort: local developer users of the desktop/control-plane app and
CLI runner who resume Claude Code sessions across configured Claude providers,
especially quota-triggered or manual migration between accounts.

Adjacent public and user-reachable paths:

- Session-bound resume with prompt/file/stdin enters `run_resume` in
  `src-tauri/src/main.rs`, reaches migration when policy selects another
  provider, and then launches the child provider with the same session id.
- Interactive resume enters `run_repl`, reaches the same migration writer, and
  launches Claude Code from the supplied or effective cwd.
- In both paths, `migrate_chain_segment` derives the target Claude project
  directory at `src-tauri/src/migration/mod.rs:161` and writes
  `<target_session_id>.jsonl` under `projects_dir.join(cwd_project_dir)`.

Blast-radius notes for unchanged adjacent paths:

- Migration policy and provider selection remain unchanged.
- Source transcript discovery, compaction slicing, temp-write-plus-rename,
  conflict detection, and session-chain DB mutation remain unchanged.
- Executor resume argv remains unchanged: it still passes provider resume args
  plus session id, not a migrated file path.
- `MigratedSegment.target_jsonl_path` remains the path actually written.
- `decode_claude_project_dir_candidates` and metadata derivation are not in the
  migration writer path and are left unchanged. Their Unix-oriented inversion
  may remain incomplete for metadata, but it does not determine where migrated
  JSONL is written.
- Existing alphanumeric Unix paths remain byte-for-byte stable. For example,
  `/home/nes/x` becomes `-home-nes-x` under both the old
  `replace('/', "-")` rule and the new rule because every non-separator
  character is ASCII alphanumeric and therefore survives the filter.

Migration path:

- Code-only migration; no SQLite schema migration and no config migration.
- Update the encoder and its direct callers/tests in one release.
- Existing migrated JSONLs are not rewritten. Future migrations write to the
  corrected project directory.

Rollback path:

- Revert the commit.
- No DB rollback or file cleanup is required.
- Rollback restores the old placement risk for future migrations but does not
  affect app startup or existing local transcript files.

Observability:

- The existing `[migrate] <source> -> <target> reason=<reason>` line continues
  to confirm migration ran.
- Canonicalize fallback emits the warning specified in the design section to
  the migration stderr stream.
- Tests observe exact `MigratedSegment.target_jsonl_path` values and file
  existence.

## 5. Assumption Register

A1. anthropics/claude-code#19972 accurately describes Claude Code's encoder.

- Evidence: community reverse-engineering quoted in
  `research/15-claude-path-hash-rca.md`, plus collision examples cited there.
- Invalidator: a real-Claude probe showing a different encoding rule.

A2. `std::fs::canonicalize` matches Claude Code's symlink resolution closely
enough for migrated resume lookup.

- Evidence: the real-Claude symlink probe in the RCA found the session only
  when JSONL was placed under the resolved symlink target's encoded directory.
- Invalidator: a probe showing Claude resolves only the leaf component, skips
  some intermediate symlinks, or uses a different canonicalization algorithm.

A3. `path.to_string_lossy()` is acceptable as the encoder input shape.

- Evidence: ticket Notes for Phase 2.5+ state that non-UTF8 bytes already
  encode to `-` under the filter.
- Invalidator: a non-UTF8 path where Claude preserves structure that lossy
  conversion would erase; none is known.

A4. The warning emitted on canonicalize failure is sufficient observability for
the fallback.

- Evidence: the migration module already writes operational output to its
  stderr writer, and adjacent main paths use stderr warnings for recoverable
  runtime issues.
- Invalidator: a supported caller invokes the encoder without a stderr path or
  users need structured IPC/metrics for canonicalize fallback.

## 6. Test-Intent Track

RC-1 non-alnum filtering:

- Risk type: change risk.
- Intended behavior / acceptance condition: paths containing `.`, `_`, CJK,
  accented latin, or other filtered characters write the migrated JSONL under
  the project directory produced by the full Claude encoder.
- Selected level: particular-integration for the existing RCA harness, plus
  unit for the inline encoder test.
- Fixture source / application point:
  `src-tauri/tests/claude_path_hash_rca/rc1_non_alnum_encoding.rs` and
  `ClaudePathHashFixture::path_with_non_alnum`; inline test updates in
  `src-tauri/src/migration/mod.rs`.
- Expected observable signal: `migrated.target_jsonl_path` equals
  `expected_claude_code_project_dir(resume_workspace)` and the JSONL exists.
- Residual risk not verified: A1 can still be invalidated by a future
  real-Claude encoder probe.

RC-2 Windows-shaped paths and `SpawnCwdUnsupported` posture:

- Risk type: change risk.
- Intended behavior / acceptance condition: a Windows-shaped path with
  backslashes, drive punctuation, `.`, `_`, and CJK is accepted and encoded as
  `C--Users-foo-bar-work-tree---`; only empty cwd produces
  `SpawnCwdUnsupported`.
- Selected level: particular-integration for the existing RCA harness, plus
  unit coverage for empty cwd rejection and Windows-shaped literal fallback.
- Fixture source / application point:
  `src-tauri/tests/claude_path_hash_rca/rc2_windows_backslash_encoding.rs`,
  `windows_shape_path()`, and inline migration tests.
- Expected observable signal: migration returns `Ok`, target path equals the
  authoritative fixture encoder output, and empty cwd still returns the typed
  error.
- Residual risk not verified: no live Windows Claude Code probe is run in this
  WU; A1's Windows interpretation is invalidated only by a future real-Claude
  probe. This should be recorded in `risk/15-test-residuals.md`.

RC-3 symlink canonicalization:

- Risk type: change risk.
- Intended behavior / acceptance condition: symlinked workspaces hash the
  resolved target path, not the literal symlink path.
- Selected level: particular-integration for the existing Unix-only RCA
  harness, plus unit or helper-level coverage if Phase 6b needs a focused
  canonicalize fallback fixture.
- Fixture source / application point:
  `src-tauri/tests/claude_path_hash_rca/rc3_symlink_canonicalization.rs` and
  `symlinked_workspace(base)` under `#[cfg(unix)]`.
- Expected observable signal: `migrated.target_jsonl_path` equals the resolved
  target's encoded path, differs from the literal link path, and exists.
- Residual risk not verified: broad case normalization, `..` behavior beyond
  `canonicalize`, and live Claude behavior on non-Unix platforms are not
  verified.

WU-14-02 RCA harness preservation:

- The three WU-14-02 RCA harnesses must flip RED -> GREEN under the post-fix
  encoder:
  `src-tauri/tests/claude_path_hash_rca/rc1_non_alnum_encoding.rs`,
  `src-tauri/tests/claude_path_hash_rca/rc2_windows_backslash_encoding.rs`, and
  `src-tauri/tests/claude_path_hash_rca/rc3_symlink_canonicalization.rs`.
- These harnesses remain in place as regression tests; they are not deleted,
  weakened, or replaced.
- The WU-14-01 RCA harness at `src-tauri/tests/session_migration_rca/` remains
  GREEN with its cwd-mismatch contract unchanged after the two scoped encoder
  mirrors are updated.

AC-3 canonicalize failure fallback:

- Risk type: verification risk.
- Intended behavior / acceptance condition: when canonicalize fails, migration
  hashes the literal path and emits the documented warning to the migration
  stderr writer.
- Selected level: unit for encoder/fallback behavior.
- Fixture source / application point: inline migration test using a
  non-existing path or Windows-shaped path on Unix and a captured `Vec<u8>`
  stderr sink.
- Expected observable signal: encoded output matches the literal-path full
  encoder, and stderr contains
  `Warning: Claude project-dir canonicalize failed for provider=...`.
- Residual risk not verified: permission-denied canonicalize failures may be
  represented by the same contract but need not get a separate fixture.

AC-4 inline regression coverage:

- Risk type: verification risk.
- Intended behavior / acceptance condition: existing inline migration tests
  stay green, and `claude_project_dir_for_encodes_absolute_unix_path` is not
  removed; it is strengthened to assert the full rule.
- Selected level: unit.
- Fixture source / application point: `src-tauri/src/migration/mod.rs` inline
  tests.
- Expected observable signal: inline unit tests pass and include filtered
  characters in the Unix fixture.
- Residual risk not verified: inline tests do not prove the end-to-end child
  Claude lookup; the RCA integration harnesses cover placement.

AC-5 cross-RCA stability:

- Risk type: verification risk.
- Intended behavior / acceptance condition: prior reproduction harnesses stay
  green. The WU-14-01 `session_migration_rca/` test stays green AFTER its
  encoder mirrors (`mod.rs`'s `claude_project_dir_name` and the fake-Claude
  `${PWD//\//-}` snippet) are updated to the new rule. The other prior
  reproduction harnesses (`tests/routing_fanout_rca/`,
  `tests/empty_bodies_ref_rca/`, `tests/release_yml_contract.rs`,
  `tests/session_lock_cross_platform.rs`) stay green with no fixture edits.
- Selected level: particular-integration.
- Fixture source / application point: see above.
- Expected observable signal: the named harnesses pass after implementation.
- Residual risk not verified: full emergent interactions are covered by the
  aggregate cargo test gate, not by each focused harness.

AC-6 cargo and frontend gates:

- Risk type: verification risk.
- Intended behavior / acceptance condition: backend formatting, lint, and tests
  pass; frontend gates pass despite no `src/` changes.
- Selected level: repository gate.
- Fixture source / application point:
  `cargo fmt --check`, `cargo clippy -- -D warnings`,
  `cargo test --no-fail-fast`, and frontend checks available in the repo
  (`bunx tsc --noEmit`, `bun run test`, and build/check scripts used by CI).
- Expected observable signal: all commands exit zero on supported CI platforms.
- Residual risk not verified: live Claude Code runtime compatibility remains
  bounded by A1 and A2.

AC-7 decisions update:

- Risk type: verification risk.
- Intended behavior / acceptance condition: D-010 and D-011 are marked resolved
  rather than deleted or rewritten, and the Phase 2.5 human-gate skip decision
  is appended.
- Selected level: review/documentation.
- Fixture source / application point: `DECISIONS.md`.
- Expected observable signal: Phase 6c diff includes dated resolution notes
  for D-010 and D-011 and a new D-NN entry.
- Residual risk not verified: no automated doc checker is required; review
  verifies this.

AC-8 prior residual cleanup:

- Risk type: verification risk.
- Intended behavior / acceptance condition: `risk/14-test-residuals.md` marks
  "Windows Claude project directory hashing" and "Symlink and canonicalization
  behavior" resolved with pointers to the harnesses and implementation PR.
- Selected level: review/documentation.
- Fixture source / application point: `risk/14-test-residuals.md`.
- Expected observable signal: Phase 6c diff updates those entries without
  erasing the historical residual context.
- Residual risk not verified: the new no-live-Windows-probe residual moves to
  `risk/15-test-residuals.md` if Phase 6b records it.

## 7. Net-Value Statement

The proposal reduces a concrete current-state risk: migrated JSONL can
currently land at a Claude-unfindable project directory for Windows-shape
paths, paths containing filtered characters such as `_`, `.`, accented latin,
or CJK, and symlinked workspaces. The change is deterministic and localized to
the migration encoder path.

The reduction outweighs the added blast radius. The encoder output for legacy
alphanumeric Unix paths is unchanged because the old `replace('/', "-")` rule
already produced strings that satisfy the new filter. Example verification:
`/home/nes/x` becomes `-home-nes-x` before and after, since `home`, `nes`, and
`x` are ASCII alphanumeric and the only replaced characters are `/`.

The two-locus Code Boundary expansion in
`src-tauri/tests/session_migration_rca/mod.rs` is mechanical: both edits mirror
the production encoder rule, preserve the WU-14-01 test contract, and do not
change this WU's net-value case.

Migration burden is next-release deploy. Rollback burden is revert one commit.
No schema migration, staged rollout, feature flag, compatibility shim, or
cleanup job is required.

## 8. Test Residuals

`risk/15-test-residuals.md` is needed if Phase 6b treats the lack of a live
Windows Claude Code probe as a residual. This proposal expects that residual to
be recorded because RC-2 is verified by the authoritative string rule and
deterministic in-repo fixtures, not by invoking a real Windows Claude Code
binary. The residual should state that A1's Windows interpretation is
invalidated only by a future real-Claude probe showing a different rule.

Updating `risk/14-test-residuals.md` is handled in Phase 6c per AC-8. That
update should mark the prior WU-14-01 Windows hashing and
symlink/canonicalization residuals resolved by WU-14-02 while preserving the
historical context.
