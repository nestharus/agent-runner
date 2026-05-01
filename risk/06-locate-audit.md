# 06-locate — Phase 4 Audit Risk Report (Rev 2)

**Verdict: LOW**

Rev 2 closes all R1 findings at their original severities and does not introduce a new MEDIUM+ audit risk. The two HIGH findings are closed by adding D5 test coverage and replacing speculative Codex workspace-root success with an explicit v1 fail-closed contract. The MEDIUM config-load and locator-`STATE_DIR` side-effect gaps are closed. Fresh Rev 2 checks found the new change log, A4 rephrase, D5/D7 tests, Codex fail-closed branch, mkdir clause, migration-path wording, and residuals coherent enough for Phase 5/6. No oscillation is classified.

## R1 finding closure

| ID | Severity | Status | Evidence |
| --- | --- | --- | --- |
| R1-F01 | HIGH | closed | §9.1 now has a D5 row with risk, intended behavior, level, fixture source/application point, assumption link, observable signal, and residual risk (`proposals/06-locate.md:240-248`). |
| R1-F02 | HIGH | closed | A4 now states Codex is deferred/fail-closed (`proposals/06-locate.md:49`); §4 step 8 requires all Codex sessions to exit `12 unsupported-storage` in v1 (`proposals/06-locate.md:131-134`); §9.1 and §12 pin Codex as unsupported pending Phase 5 evidence (`proposals/06-locate.md:252`, `proposals/06-locate.md:303`). |
| R1-F03 | MEDIUM | closed | §8 explicitly permits only locator adapter `state_dir` directory creation, cites `locate_transcript`, ties it to existing trace behavior, and says locate writes no file inside it (`proposals/06-locate.md:232`). |
| R1-F04 | MEDIUM | closed | §4 step 3 now commits to `ProvidersConfig::load(...).unwrap_or_default()` and `SessionsConfig::load(...).unwrap_or_default()`, matching resume-adjacent code (`proposals/06-locate.md:126`; `src-tauri/src/main.rs:1079-1084`). |
| R1-F05 | advisory | closed | §4 step 8 adds a Claude path-hash inversion rule and rejects multiple existing decompositions as exit `12`; §9.1 adds an ambiguity row for zero/one/multiple decompositions (`proposals/06-locate.md:132`, `proposals/06-locate.md:252-253`). |
| R1-F06 | advisory | closed | §11.1 no longer says users can repair partial DBs with `migrate-db`; §12 records that `backfill_session_chains` skips when any chain row exists and locate maps remaining partials to exit `10` (`proposals/06-locate.md:289`, `proposals/06-locate.md:305`). |
| R1-F07 | cosmetic | closed | §12 records the future 06-pause-handshake lock condition for `mutable: false` and warns consumers not to infer cross-process write safety (`proposals/06-locate.md:306`). |
| R1-F08 | cosmetic | closed | §1 and §6 commit the module path to `src-tauri/src/session_metadata/` (`proposals/06-locate.md:15-16`, `proposals/06-locate.md:153-159`). |
| R1-F09 | cosmetic | closed | §10 requires README framing of `mutable` as a read-time eligibility hint, not a safety lock or mutation permission (`proposals/06-locate.md:267`). |

## Rev 2 fresh findings

### R2-F01. Rev 2 change-log truthfulness — RESOLVED

The Rev 2 changes block accurately points to the material edits. Spot checks:

- The D5 bullet says §9.1 added a D5 test row (`proposals/06-locate.md:30`), and the table now includes "D5 default DB only" (`proposals/06-locate.md:248`).
- The Codex bullet says A4/§4/§9.1/§12 fail-close Codex and drop the `payload.cwd` commitment (`proposals/06-locate.md:31`); A4 says Codex is deferred and v1 fail-closes (`proposals/06-locate.md:49`), §4 step 8 says all Codex sessions exit `12` in v1 (`proposals/06-locate.md:133`), and §12 records the residual (`proposals/06-locate.md:303`).
- The migration overpromise bullet says §11.1/§12 removed the `migrate-db` repair claim (`proposals/06-locate.md:35`); §11.1 now says partial DBs remain partial and repair is outside scope (`proposals/06-locate.md:289`).

### R2-F02. A4 invalidator rephrase — RESOLVED

A4 is falsifiable in its Rev 2 form. It can be invalidated by representative Claude path hashes failing to yield one unambiguous existing local workspace root, by Phase 5 proving a stable Codex root field that risk gates require in v1, or by the harness requiring roots for providers with no path/config provenance (`proposals/06-locate.md:49`). That is concrete enough to drive hookpoint sampling and Phase 6 fail-closed behavior.

### R2-F03. D5 test row completeness — RESOLVED

The new D5 row carries every required field in the §9.1 schema: change risk, intended behavior, level, fixture source/application point, assumption link, observable signal, and residual risk (`proposals/06-locate.md:240-248`). Its observable signal pins both clap rejection of `--state-db` and absence of alternate DB-path acceptance; GUI DB support remains explicitly out of scope in §4 and §7 (`proposals/06-locate.md:125`, `proposals/06-locate.md:217`).

### R2-F04. Codex fail-closed branch — RESOLVED

The new D7 branch is complete enough to keep Phase 6 from accidentally implementing Codex success. It says all Codex sessions exit `12 unsupported-storage` for `workspace_root` in v1, Phase 5 must sample real rollout JSONL before any future derivation, and the proposal does not commit to `session_meta.payload.cwd` or any other Codex field today (`proposals/06-locate.md:133`). The test row requires a Codex provider fixture with a located JSONL but unsupported root derivation to exit `12` (`proposals/06-locate.md:252`).

This resolves the R1 speculative-source issue. The current Codex locator only checks filename suffix and `session_meta.payload.id`; it does not inspect `payload.cwd` or `payload.workspace_root` (`scripts/codex-locate-transcript:21-31`, `scripts/codex-locate-transcript:45-60`).

### R2-F05. Claude path-hash tiebreaker — RESOLVED

The path-hash prose is implementable without pseudocode because the failure rule dominates the ordering rule: generate decoded candidate decompositions, accept only when exactly one existing path is produced, and return exit `12 unsupported-storage` when zero or multiple existing decompositions are found (`proposals/06-locate.md:132`, `proposals/06-locate.md:252-253`). The phrase "pick the first interpretation" could tempt early-return implementation, but §9.1's explicit multiple-existing fixture closes that risk by requiring implementers to detect multiplicity before success.

The source citation is also properly bounded: migration treats the source transcript parent directory name as `cwd_hash` for Claude target storage, but does not itself prove a reverse workspace decoder (`src-tauri/src/migration/mod.rs:155-188`). Rev 2 treats this as a Claude-only assumption with a fail-closed invalidator, not as universal proof (`proposals/06-locate.md:49`).

### R2-F06. Locator `STATE_DIR` mkdir clause — RESOLVED

The §8 clause is restrictive enough for the command contract. It permits the specific `state_dir` directory creation performed by `locate_transcript`, ties the behavior to existing `trace --json` locator execution, and forbids locate from writing files inside that directory (`proposals/06-locate.md:232`). The current code does create the directory before running the locator (`src-tauri/src/sessions/mod.rs:183-185`). Other §8 bullets still forbid DB row writes, adapter cursor files, transcript rewrites, provider commands, quota scripts, diagnostics, telemetry, and durable cache state (`proposals/06-locate.md:223-230`).

### R2-F07. Migration-path overpromise removal — RESOLVED

The supported-surface migration paragraph now removes the prior overpromise. It says no user state one-shot is required for read-only locate, partial DBs remain partial, locate returns exit `10` for segmentless sessions, and repair is outside this PR (`proposals/06-locate.md:289`). §12 backs that with the concrete `backfill_session_chains` skip condition (`proposals/06-locate.md:305`), which matches current code: `backfill_session_chains` returns a skipped report when any chain row exists (`src-tauri/src/state/db.rs:2256-2271`).

### R2-F08. A-K checklist re-walk — RESOLVED

The Rev 1 A-K checklist remains satisfied after Rev 2:

- A: success JSON and exit codes still cover harness-required fields and codes (`proposals/06-locate.md:97-108`, `proposals/06-locate.md:142-151`; `01-session-locate.md:20-46`).
- B/C: test-intent and assumption register now cover D1-D7 and all assumptions have invalidators (`proposals/06-locate.md:44-54`, `proposals/06-locate.md:240-256`).
- D/E: cross-feature constraints and anti-scope remain aligned with Initiative 06 (`proposals/06-locate.md:208-219`, `proposals/06-locate.md:308-321`; `initiatives/06-session-override-contract.md:108-122`).
- F/G: cited source surfaces for resolver, config load, locator, storage vocabulary, README ranges, and migration precedent were spot-checked below.
- H/I: residuals and partial-DB migration behavior are explicit (`proposals/06-locate.md:295-306`).
- J/K: no deferred runtime stubs or backwards-compatibility shim are introduced; the command is additive and keeps existing resume/trace/migrate behavior unchanged (`proposals/06-locate.md:20-26`, `proposals/06-locate.md:153-206`).

### R2-F09. Exit and JSON contract regression check — RESOLVED

Rev 2 did not weaken the harness-facing output contract while closing Codex.
Success still requires a compact one-line JSON object and all required fields
(`proposals/06-locate.md:95-108`), while incomplete transcript or workspace
location remains exit `12` with no partial success JSON (`proposals/06-locate.md:120`,
`proposals/06-locate.md:149`). The harness requires stable stdout JSON fields
and stable failure codes (`01-session-locate.md:20-46`). Codex fail-closed
therefore narrows success eligibility without changing the public schema.

### R2-F10. Residual register completeness — RESOLVED

The new §12 residuals cover the Rev 2 closure areas instead of hiding them:
physical read-only DB open remains assigned to 06-schema-probe, GUI DB divergence
remains out of scope, workspace derivation may reject otherwise valid sessions,
Codex root derivation is deferred, partial-chain repair is outside v1, and
future pause-handshake locks are named as a sixth mutability condition
(`proposals/06-locate.md:299-306`). These residuals align with Initiative 06
sequencing for read-only open and lock observation (`initiatives/06-session-override-contract.md:114-120`).

No same-label, same-family, fix-created, two-generation, or named three-generation oscillation is classified. All R1 chains are closed rather than returning as R2 findings.

## Citations spot-checked

- OK — Rev 1 prompt path `.tmp/06-locate-risk-audit.md` was absent; the Rev 2 prompt copy exists at `.tmp/06-locate-risk-audit-rev2.md`, and the prior Rev 1 report preserves the A-K checklist headings.
- OK — `proposals/06-locate.md:64-89` against current subcommands and dispatch at `src-tauri/src/main.rs:77-166` and `src-tauri/src/main.rs:287-338`.
- OK — `proposals/06-locate.md:125` against `StateDb::open_default()` at `src-tauri/src/state/db.rs:611-615`.
- OK — `proposals/06-locate.md:126` against resume config loading with `unwrap_or_default` at `src-tauri/src/main.rs:1079-1084`.
- OK — `proposals/06-locate.md:127` against `StateDb::resolve_resume` signature and behavior at `src-tauri/src/state/db.rs:2577-2670`.
- OK — `proposals/06-locate.md:127` ambiguity behavior against `src-tauri/src/state/db.rs:2713-2749`.
- OK — `proposals/06-locate.md:130` against `locate_transcript` signature and mkdir behavior at `src-tauri/src/sessions/mod.rs:171-199`.
- OK — `proposals/06-locate.md:131` against absent invocation working-directory storage in `InvocationRecord` / `InvocationStart` at `src-tauri/src/state/db.rs:205-233`.
- OK — `proposals/06-locate.md:132` against migration's Claude `cwd_hash` parent-directory precedent at `src-tauri/src/migration/mod.rs:155-188`.
- OK — `proposals/06-locate.md:133` against Codex locator source: current script checks filename suffix and `payload.id`, not workspace-root fields (`scripts/codex-locate-transcript:21-31`, `scripts/codex-locate-transcript:45-60`).
- OK — `proposals/06-locate.md:138` against partial DB candidate selection reading `session_chain_segments` only (`src-tauri/src/state/db.rs:2696-2711`).
- OK — `proposals/06-locate.md:289` and `proposals/06-locate.md:305` against `backfill_session_chains` skip behavior at `src-tauri/src/state/db.rs:2256-2271`.
- OK — README update targets still exist at `README.md:127-140`, `README.md:374-386`, `README.md:414-418`, and `README.md:500-512`.
- OK — Initiative constraints cited in §13 match `initiatives/06-session-override-contract.md:41-43` and `initiatives/06-session-override-contract.md:108-122`.
