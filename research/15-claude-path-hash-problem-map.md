# WU-14-02 Existing-State Problem Map

## 1. Touched surface

- `src-tauri/src/migration/mod.rs:27` defines `MigrationError::SpawnCwdUnsupported { provider, cwd }`, the current error used when the spawn cwd shape is rejected.
- `src-tauri/src/migration/mod.rs:84` `migrate_chain_segment` is the supported migration writer. Its `resume_working_dir` parameter at `src-tauri/src/migration/mod.rs:89` is the cwd input that feeds the Claude project-dir encoder.
- `src-tauri/src/migration/mod.rs:161` is the only production caller of `claude_project_dir_for`; it derives `cwd_project_dir` from the target provider name and `resume_working_dir`.
- `src-tauri/src/migration/mod.rs:188` writes under `projects_dir.join(&cwd_project_dir)`, so the encoder output directly names the target Claude Code project directory.
- `src-tauri/src/migration/mod.rs:195` appends `<target_session_id>.jsonl` under the encoded target directory, and `src-tauri/src/migration/mod.rs:206` through `src-tauri/src/migration/mod.rs:214` write the temp file and rename it into place.
- `src-tauri/src/migration/mod.rs:256` defines `pub(crate) fn claude_project_dir_for(provider: &str, cwd: &Path) -> Result<String, MigrationError>`.
- `src-tauri/src/migration/mod.rs:257` rejects empty cwd and any path for which `Path::is_absolute()` is false on the current platform, returning `SpawnCwdUnsupported` at `src-tauri/src/migration/mod.rs:258`.
- `src-tauri/src/migration/mod.rs:264` is the current encoder body: `cwd.to_string_lossy().replace('/', "-")`.
- `src-tauri/src/migration/mod.rs:319` defines the inline test helper `claude_project_dir_name` with the same slash-only rule at `src-tauri/src/migration/mod.rs:320`.
- `src-tauri/src/migration/mod.rs:328` uses that inline helper to seed source JSONL fixtures under a Claude project directory.
- `src-tauri/src/migration/mod.rs:391` and `src-tauri/src/migration/mod.rs:432` exercise `migrate_chain_segment` from inline tests, passing source/spawn workspaces that currently contain only simple path components.
- `src-tauri/src/migration/mod.rs:407`, `src-tauri/src/migration/mod.rs:445`, and `src-tauri/src/migration/mod.rs:448` build expected paths with the inline slash-only helper.
- `src-tauri/src/migration/mod.rs:463` `claude_project_dir_for_encodes_absolute_unix_path` asserts `/home/nes/x -> -home-nes-x`; it does not include `_`, `.`, `:`, backslash, accented, CJK, or symlink cases.
- `src-tauri/src/migration/mod.rs:472` `claude_project_dir_for_rejects_relative_path` asserts that a relative path produces `SpawnCwdUnsupported`.
- `src-tauri/src/migration/mod.rs:485` `claude_project_dir_for_rejects_empty_path` asserts that an empty path produces `SpawnCwdUnsupported`.
- `DECISIONS.md:229` records D-010, the prior Windows Claude project-directory hashing deferral. It states the helper accepts an absolute Unix-style cwd and rejects other shapes at `DECISIONS.md:239`.
- `DECISIONS.md:259` records D-011, the prior symlink/canonicalization deferral. It states the migration helper does not canonicalize at `DECISIONS.md:266`.
- `risk/14-test-residuals.md:19` records the Windows Claude project directory hashing residual, and `risk/14-test-residuals.md:31` records the symlink and canonicalization residual.

The product Code Boundary for this WU is local to `src-tauri/src/migration/mod.rs`; `rg` found no other production caller of `claude_project_dir_for` beyond `src-tauri/src/migration/mod.rs:161`.

## 2. Already-risky / brittle behavior

- The encoder only replaces forward slashes. `src-tauri/src/migration/mod.rs:264` preserves `_`, `.`, `:`, backslashes, accented characters, CJK characters, and any other non-ASCII-alphanumeric character that Claude Code's authoritative rule filters to `-`.
- The authoritative rule quoted in the RCA first replaces `/` and `\` with `-`, then keeps only ASCII alphanumeric characters and `-`; all other characters become `-` (`research/15-claude-path-hash-rca.md:31` through `research/15-claude-path-hash-rca.md:37`).
- `src-tauri/src/migration/mod.rs:257` rejects non-empty paths that are not absolute according to the host platform. On Unix, a Windows-shaped `PathBuf::from(r"C:\Users\foo.bar\work_tree\漢字")` is not absolute, so it falls into `MigrationError::SpawnCwdUnsupported` instead of being encoded.
- `MigrationError::SpawnCwdUnsupported` is broad in current behavior: it covers both empty cwd and non-Unix-shaped cwd through the single condition at `src-tauri/src/migration/mod.rs:257`.
- There is no canonicalization before hashing. `claude_project_dir_for` consumes the literal `cwd` string at `src-tauri/src/migration/mod.rs:264`; no `canonicalize` call appears between the migration entry point's `resume_working_dir` parameter at `src-tauri/src/migration/mod.rs:89` and the encoder call at `src-tauri/src/migration/mod.rs:161`.
- `effective_spawn_cwd` absolutizes relative paths by joining them with `std::env::current_dir()` at `src-tauri/src/main.rs:1050` through `src-tauri/src/main.rs:1058`, but it does not canonicalize symlinks.
- `build_command` passes the requested `working_dir` into `cmd.current_dir(dir)` at `src-tauri/src/executor/cli.rs:346`, so the child process uses the caller-supplied cwd shape for its own runtime environment.
- The inline encoder test at `src-tauri/src/migration/mod.rs:463` only checks a simple absolute Unix path. The rejection tests at `src-tauri/src/migration/mod.rs:472` and `src-tauri/src/migration/mod.rs:485` check errors, not filtered-character encoding.

## 3. Adjacent surfaces inside the blast radius

- Production caller enumeration: `rg` found `claude_project_dir_for` at `src-tauri/src/migration/mod.rs:161`, its definition at `src-tauri/src/migration/mod.rs:256`, and inline tests at `src-tauri/src/migration/mod.rs:465`, `src-tauri/src/migration/mod.rs:473`, and `src-tauri/src/migration/mod.rs:486`.
- The WU-14-02 RCA harness fixture has an independent authoritative test encoder in `src-tauri/tests/claude_path_hash_rca/mod.rs:129`. It replaces both slash directions at `src-tauri/tests/claude_path_hash_rca/mod.rs:131` and filters non-ASCII-alphanumeric characters at `src-tauri/tests/claude_path_hash_rca/mod.rs:132` through `src-tauri/tests/claude_path_hash_rca/mod.rs:140`.
- `src-tauri/tests/session_migration_rca/mod.rs:57` seeds source JSONL under a test project directory, and `src-tauri/tests/session_migration_rca/mod.rs:129` defines `claude_project_dir_name` with `path.to_string_lossy().replace('/', "-")` at `src-tauri/tests/session_migration_rca/mod.rs:130`.
- The fake Claude in `src-tauri/tests/session_migration_rca/mod.rs:109` also models project lookup with Bash `${PWD//\//-}`, so it uses the same slash-only behavior for that older RCA surface.
- `src-tauri/tests/fixtures/initiative_06.rs:336` stages transcripts under `claude_project_dir_name(encoded_source_path)`, with the helper at `src-tauri/tests/fixtures/initiative_06.rs:886` through `src-tauri/tests/fixtures/initiative_06.rs:888` using `trim_start_matches('/').replace('/', "-")`.
- `src-tauri/tests/fixtures/initiative_06_import_replace.rs:260` stages JSONL under `claude_project_dir_name(workspace_root)`, with the helper at `src-tauri/tests/fixtures/initiative_06_import_replace.rs:995` through `src-tauri/tests/fixtures/initiative_06_import_replace.rs:997` using the same slash-only rule.
- `src-tauri/tests/fixtures/initiative_06_export.rs:282` stages JSONL under `claude_project_dir_name(workspace_root)`, with the helper at `src-tauri/tests/fixtures/initiative_06_export.rs:605` through `src-tauri/tests/fixtures/initiative_06_export.rs:607` using the same slash-only rule.
- `src-tauri/tests/initiative_05_migration.rs:636` defines another slash-only test helper at `src-tauri/tests/initiative_05_migration.rs:637`, and migration assertions use it at `src-tauri/tests/initiative_05_migration.rs:680`, `src-tauri/tests/initiative_05_migration.rs:846`, and nearby stale-target setup paths.
- `src-tauri/tests/pr_f_resume_integration.rs:951` computes `expected_target_dir` with `fixture.dir.path().to_string_lossy().replace('/', "-")` for the run-repl migration integration path.
- `decode_claude_project_dir_candidates` is the adjacent production inversion helper. `derive_claude_workspace_root` extracts the transcript directory name at `src-tauri/src/session_metadata/mod.rs:294` through `src-tauri/src/session_metadata/mod.rs:300`, then calls `decode_claude_project_dir_candidates(encoded)` at `src-tauri/src/session_metadata/mod.rs:301`.
- `decode_claude_project_dir_candidates` requires an encoded name that starts with `-` at `src-tauri/src/session_metadata/mod.rs:338` through `src-tauri/src/session_metadata/mod.rs:340`, maps bare `-` to `/` at `src-tauri/src/session_metadata/mod.rs:342` through `src-tauri/src/session_metadata/mod.rs:343`, and recursively constructs Unix-rooted candidates beginning from `PathBuf::from("/")` at `src-tauri/src/session_metadata/mod.rs:363` through `src-tauri/src/session_metadata/mod.rs:368`.
- The decoder is an inversion helper for metadata, not the migration writer. It does not participate in the encoder output that reaches `projects_dir.join(&cwd_project_dir)` at `src-tauri/src/migration/mod.rs:188`; its current Unix-rooted decomposition may become less complete as an inverse once encoded names can contain filtered non-separator characters, backslashes, and drive punctuation.

### 3.1 Load-bearing `session_migration_rca/` encoder mirrors

- `src-tauri/tests/session_migration_rca/` is a load-bearing WU-14-02 test dependency, not a passive blast-radius watchpoint. Its test helper `claude_project_dir_name` is slash-only at `src-tauri/tests/session_migration_rca/mod.rs:129` through `src-tauri/tests/session_migration_rca/mod.rs:130`, and its fake Claude script derives lookup paths with the same slash-only Bash substitution `${PWD//\//-}` at `src-tauri/tests/session_migration_rca/mod.rs:109` through `src-tauri/tests/session_migration_rca/mod.rs:115`. Both are encoder mirrors for the same project-directory rule that the production encoder currently implements as `cwd.to_string_lossy().replace('/', "-")` at `src-tauri/src/migration/mod.rs:264`.
- The RC-1 cwd-mismatch test depends on those mirrors for the migrated target path. It builds `resume_project_target` with `claude_project_dir_name(&fixture.resume_workspace)` at `src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs:37` through `src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs:40`, then asserts `migrated.target_jsonl_path` equals that value at `src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs:41`. The same test then launches the fake target Claude from `fixture.resume_workspace` at `src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs:47` through `src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs:61`, so the fake provider lookup and the assertion are both coupled to the encoded cwd result.
- `MigrationFixture::new` creates both workspaces under a `tempfile::tempdir()` root at `src-tauri/tests/session_migration_rca/mod.rs:23` through `src-tauri/tests/session_migration_rca/mod.rs:39`, with the tempdir created at `src-tauri/tests/session_migration_rca/mod.rs:25` and the `resume_workspace` path derived from that root at `src-tauri/tests/session_migration_rca/mod.rs:29`. The RCA red-run evidence shows these tempdir-derived roots contain `.` and therefore diverge under the full Claude Code rule: the old slash-only path contains `-tmp-.tmpvcQRta-...` while the full-rule expected path contains `-tmp--tmpvcQRta-...` at `research/15-claude-path-hash-rca.md:148` through `research/15-claude-path-hash-rca.md:151`.
- The conflict is not with the RC-1 contract. The contract remains that migration writes under the resume cwd's Claude project directory and not the source cwd's project directory, as asserted at `src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs:33` through `src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs:45`. The conflict is that the test helper and fake provider are stale encoder mirrors once the production encoder changes. Carrying this forward expands the WU-14-02 Code Boundary by exactly two test loci: `src-tauri/tests/session_migration_rca/mod.rs::claude_project_dir_name` at `src-tauri/tests/session_migration_rca/mod.rs:129` through `src-tauri/tests/session_migration_rca/mod.rs:130`, and the fake-Claude `${PWD//\//-}` snippet in the same file at `src-tauri/tests/session_migration_rca/mod.rs:109` through `src-tauri/tests/session_migration_rca/mod.rs:115`. The proposal-owned resolution is a one-function rewrite of that helper plus a fake-Claude rewrite that applies the same encoder rule, either with a Bash filter or by sourcing a small helper.
- Other adjacent slash-only helpers remain out of WU-14-02 scope. Phase 5 conflict research enumerated `src-tauri/tests/fixtures/initiative_06.rs:886` through `src-tauri/tests/fixtures/initiative_06.rs:888`, `src-tauri/tests/fixtures/initiative_06_import_replace.rs:995` through `src-tauri/tests/fixtures/initiative_06_import_replace.rs:997`, `src-tauri/tests/fixtures/initiative_06_export.rs:605` through `src-tauri/tests/fixtures/initiative_06_export.rs:607`, `src-tauri/tests/initiative_05_migration.rs:636` through `src-tauri/tests/initiative_05_migration.rs:638`, and `src-tauri/tests/pr_f_resume_integration.rs:949` through `src-tauri/tests/pr_f_resume_integration.rs:959` as adjacent slash-only surfaces (`research/15-claude-path-hash-hookpoints.md:38` through `research/15-claude-path-hash-hookpoints.md:46`). They are anti-scope for this revision because they do not participate in post-fix migration writer assertions where the new rule changes the expected encoded output for the returned `session_migration_rca/` conflict.
- The orchestrator brief item that the integration tests under `tests/session_migration_rca/` validate the WU-14-01 cwd-mismatch contract and stay green on the post-fix encoder is correct in spirit: the cwd-mismatch contract should remain green. The literal reading that the harness stays green unchanged is not achievable because the brief did not account for tempdir paths containing `.` (`src-tauri/tests/session_migration_rca/mod.rs:25`, `research/15-claude-path-hash-rca.md:148` through `research/15-claude-path-hash-rca.md:151`) and did not account for the slash-only helper and fake-Claude mirror at `src-tauri/tests/session_migration_rca/mod.rs:109` through `src-tauri/tests/session_migration_rca/mod.rs:130`.

## 4. Supported / user-reachable paths through the touched surface

- Top-level `--resume <id>` dispatches at `src-tauri/src/main.rs:432`. With a prompt, file, or stdin prompt it calls `run_resume` at `src-tauri/src/main.rs:459`; without prompt input it calls `run_repl` at `src-tauri/src/main.rs:469`.
- Interactive resume path: `run_repl` resolves the active session at `src-tauri/src/main.rs:1548` through `src-tauri/src/main.rs:1565`, builds the migration pool at `src-tauri/src/main.rs:1614`, and asks `decide_migration` at `src-tauri/src/main.rs:1615` through `src-tauri/src/main.rs:1618`.
- When interactive migration is selected, `run_repl` derives `effective_spawn_cwd` at `src-tauri/src/main.rs:1620` and passes it into `migrate_chain_segment` at `src-tauri/src/main.rs:1622` through `src-tauri/src/main.rs:1630`.
- Session-bound resume path: `run_resume` resolves the session at `src-tauri/src/main.rs:1806` through `src-tauri/src/main.rs:1827`, builds the migration pool at `src-tauri/src/main.rs:1839`, and asks `decide_migration` at `src-tauri/src/main.rs:1840` through `src-tauri/src/main.rs:1843`.
- When session-bound migration is selected, `run_resume` derives `effective_spawn_cwd` at `src-tauri/src/main.rs:1845` and passes it into `migrate_chain_segment` at `src-tauri/src/main.rs:1847` through `src-tauri/src/main.rs:1855`.
- Inside migration, `migrate_chain_segment` derives `cwd_project_dir` with `claude_project_dir_for` at `src-tauri/src/migration/mod.rs:161`, then creates `projects_dir.join(&cwd_project_dir)` at `src-tauri/src/migration/mod.rs:188` through `src-tauri/src/migration/mod.rs:194`.
- The target JSONL path is `target_dir.join(format!("{target_session_id}.jsonl"))` at `src-tauri/src/migration/mod.rs:195`, written through the temp/rename sequence at `src-tauri/src/migration/mod.rs:206` through `src-tauri/src/migration/mod.rs:214`, and returned as `MigratedSegment.target_jsonl_path` at `src-tauri/src/migration/mod.rs:244` through `src-tauri/src/migration/mod.rs:253`.
- After successful migration, both supported paths mutate only provider/session identity: interactive at `src-tauri/src/main.rs:1632` through `src-tauri/src/main.rs:1638`, session-bound at `src-tauri/src/main.rs:1857` through `src-tauri/src/main.rs:1866`.
- Interactive child launch builds `ResumePayload { session_id, strategy }` at `src-tauri/src/main.rs:1710` through `src-tauri/src/main.rs:1719`, then calls `execute_interactive` with the original `working_dir` at `src-tauri/src/main.rs:1721` through `src-tauri/src/main.rs:1726`.
- Session-bound child launch calls `execute_resume` with the original `working_dir` and `ResumePayload { session_id: &resolved.active_session_id, strategy }` at `src-tauri/src/main.rs:1908` through `src-tauri/src/main.rs:1919`.
- `ResumePayload` carries only `session_id` and `strategy` at `src-tauri/src/executor/cli.rs:276` through `src-tauri/src/executor/cli.rs:279`; it has no migrated path field in current HEAD.
- `compose_resume_provider_args` appends the provider's resume flag or subcommand and the session id at `src-tauri/src/executor/cli.rs:290` through `src-tauri/src/executor/cli.rs:295`; the flag/subcommand mechanics are implemented at `src-tauri/src/executor/cli.rs:298` through `src-tauri/src/executor/cli.rs:325`.
- `execute_resume` composes `claude --resume <session_id>` style arguments at `src-tauri/src/executor/cli.rs:460` through `src-tauri/src/executor/cli.rs:472`. `execute_interactive` composes the same resume payload for REPL launches at `src-tauri/src/executor/cli.rs:577` through `src-tauri/src/executor/cli.rs:582`.
- `build_command` sets the child cwd with `cmd.current_dir(dir)` at `src-tauri/src/executor/cli.rs:346` through `src-tauri/src/executor/cli.rs:348`. That is the cwd Claude Code uses for its own `--resume` lookup.
- The broader migrated resume flow is mapped in `research/14-problem-map.md`; the encoder-specific path is the handoff from `effective_spawn_cwd` to `claude_project_dir_for`, then to `projects_dir.join(<encoded_cwd>)`, and finally to the child `claude --resume <session_id>` running in that cwd.

## 5. Cross-platform considerations

- Current Windows-shaped path behavior is determined by host `Path::is_absolute()`. On Unix, `PathBuf::from(r"C:\Users\foo.bar\work_tree\漢字")` is not absolute and is rejected by `src-tauri/src/migration/mod.rs:257` through `src-tauri/src/migration/mod.rs:262`; the RC-2 harness encodes this as a failure at `src-tauri/tests/claude_path_hash_rca/rc2_windows_backslash_encoding.rs:11` through `src-tauri/tests/claude_path_hash_rca/rc2_windows_backslash_encoding.rs:13`.
- The RCA fixture's expected Windows-shape path is `C:\Users\foo.bar\work_tree\漢字` at `src-tauri/tests/claude_path_hash_rca/mod.rs:125` through `src-tauri/tests/claude_path_hash_rca/mod.rs:127`, and its expected post-rule string is asserted as `C--Users-foo-bar-work-tree---` at `src-tauri/tests/claude_path_hash_rca/rc2_windows_backslash_encoding.rs:15` through `src-tauri/tests/claude_path_hash_rca/rc2_windows_backslash_encoding.rs:18`.
- The authoritative encoder rule in the RCA is platform-neutral string processing: replace `/` and `\`, then filter non-ASCII-alphanumeric characters except `-` (`research/15-claude-path-hash-rca.md:31` through `research/15-claude-path-hash-rca.md:37`).
- D-010 explicitly says the current helper accepts an absolute Unix-style cwd and rejects other shapes via `SpawnCwdUnsupported` at `DECISIONS.md:239` through `DECISIONS.md:244`.
- D-011 explicitly says the current main call sites absolutize relative paths but do not canonicalize symlinks, and the migration helper does not canonicalize either, at `DECISIONS.md:266` through `DECISIONS.md:269`.
- The Rust standard library documents `std::fs::canonicalize` as returning a canonical absolute form with intermediate components normalized and symlinks resolved; its platform-specific behavior currently maps to `realpath` on Unix and `CreateFile` plus `GetFinalPathNameByHandle` on Windows. Source: Rust stable std docs, `std::fs::canonicalize` (`https://doc.rust-lang.org/stable/std/fs/fn.canonicalize.html`).
- The current code has no `#[cfg(target_os)]` branch in the encoder path: `claude_project_dir_for` is one function at `src-tauri/src/migration/mod.rs:256` through `src-tauri/src/migration/mod.rs:265`, and the RCA harness keeps RC-3 Unix-only only because it creates a Unix symlink at `src-tauri/tests/claude_path_hash_rca/mod.rs:143` through `src-tauri/tests/claude_path_hash_rca/mod.rs:155`.
- The ticket anti-scope says platform-specific code should not be introduced unless absolutely required; in the existing state, the only platform-specific behavior in the touched encoder is the implicit `Path::is_absolute()` gate at `src-tauri/src/migration/mod.rs:257`.

## 6. Pre-fix evidence

- RC-1 harness files are listed in the RCA at `research/15-claude-path-hash-rca.md:120` through `research/15-claude-path-hash-rca.md:126`. Current worktree entry points are `src-tauri/tests/claude_path_hash_rca.rs:1`, `src-tauri/tests/claude_path_hash_rca/mod.rs:12`, and `src-tauri/tests/claude_path_hash_rca/rc1_non_alnum_encoding.rs:8`.
- RC-1 red-run command from the RCA:

```bash
cd src-tauri && cargo test --test claude_path_hash_rca rc1_project_dir_encoder_replaces_all_non_alnum_except_dash
```

- RC-1 one-line failure summary, verbatim from `research/15-claude-path-hash-rca.md:148` through `research/15-claude-path-hash-rca.md:149`:

```text
assertion `left == right` failed: Claude Code project-dir encoding must replace '.', '_', and CJK characters with '-'
```
- RC-2 harness files are listed in the RCA at `research/15-claude-path-hash-rca.md:165` through `research/15-claude-path-hash-rca.md:171`. Current worktree entry points are `src-tauri/tests/claude_path_hash_rca.rs:1`, `src-tauri/tests/claude_path_hash_rca/mod.rs:13`, and `src-tauri/tests/claude_path_hash_rca/rc2_windows_backslash_encoding.rs:7`.
- RC-2 red-run command from the RCA:

```bash
cd src-tauri && cargo test --test claude_path_hash_rca rc2_windows_shape_path_uses_backslash_and_non_alnum_encoding_rule
```

- RC-2 one-line failure summary, verbatim from `research/15-claude-path-hash-rca.md:193` through `research/15-claude-path-hash-rca.md:194`:

```text
post-fix migration should encode Windows-shaped paths via the Claude Code rule: SpawnCwdUnsupported { provider: "claude-target", cwd: "C:\\Users\\foo.bar\\work_tree\\漢字" }
```
- RC-3 harness files are listed in the RCA at `research/15-claude-path-hash-rca.md:208` through `research/15-claude-path-hash-rca.md:214`. Current worktree entry points are `src-tauri/tests/claude_path_hash_rca.rs:1`, `src-tauri/tests/claude_path_hash_rca/mod.rs:15` through `src-tauri/tests/claude_path_hash_rca/mod.rs:16`, and `src-tauri/tests/claude_path_hash_rca/rc3_symlink_canonicalization.rs:8`.
- RC-3 red-run command from the RCA:

```bash
cd src-tauri && cargo test --test claude_path_hash_rca rc3_symlinked_workspace_hashes_resolved_path_not_literal_link_path
```

- RC-3 one-line failure summary, verbatim from `research/15-claude-path-hash-rca.md:236` through `research/15-claude-path-hash-rca.md:237`:

```text
assertion `left == right` failed: Claude Code hashes the resolved cwd path for symlinked workspaces
```
- The three RCA harnesses are already present in current HEAD and encode the pre-fix red evidence against the current slash-only product encoder: RC-1 asserts migrated path equality at `src-tauri/tests/claude_path_hash_rca/rc1_non_alnum_encoding.rs:17`, RC-2 expects migration to accept the Windows-shaped path at `src-tauri/tests/claude_path_hash_rca/rc2_windows_backslash_encoding.rs:11`, and RC-3 expects the resolved symlink target path at `src-tauri/tests/claude_path_hash_rca/rc3_symlink_canonicalization.rs:24`.

## 7. Open questions / unknowns

- No new value, scope, or trade-off question surfaced while mapping the current state; no `NEEDS_INPUT` artifact was emitted.
- The current repository does not contain a production Windows-specific Claude path-hash encoder. The authoritative rule is the RCA-quoted anthropics/claude-code#19972 evidence at `research/15-claude-path-hash-rca.md:31` through `research/15-claude-path-hash-rca.md:37`.
- The current repository does not contain a live Windows Claude Code probe. The RCA states that the Windows-shaped harness validates the authoritative encoder deterministically on the Unix checkout at `research/15-claude-path-hash-rca.md:253` through `research/15-claude-path-hash-rca.md:256`.
- The current repository does not test case normalization, `..` cleanup, or broader path normalization in this RCA; the RCA bounds evidence to backslash handling, non-alnum encoding, and symlink resolution before hashing at `research/15-claude-path-hash-rca.md:257` through `research/15-claude-path-hash-rca.md:260`.
