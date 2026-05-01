# 06-locate — Phase 8 Supported-Surface Verification (PR)

**Termination signal:** `none`
**LOW / MEDIUM / HIGH:** **LOW**

Re-capture against post-fix-pass tip `2605b37`. The only change since
the prior LOW verdict is `2605b37` (README addition documenting the
`agents session locate` surface), which does not touch any source
file. The implementation under `src-tauri/src/session_metadata/mod.rs`,
the wiring in `src-tauri/src/main.rs`, and the `TranscriptState` move
out of `src-tauri/src/trace/mod.rs` all remain consistent with Rev 3
of the proposal and the Phase 4 Rev 3 LOW verdict. A1–A9 hold against
the diff; A4 holds in its Rev 3 form (Codex `payload.cwd` parser
matches `scripts/codex-locate-transcript`'s line-walk + `type ==
"session_meta"` discriminator + `payload` sub-object pattern, with
fail-closed exit `12` for missing/absent/non-canonical/non-UTF-8).
No adjacent path is broken or degraded. §11.1's three load-bearing
claims (no user state one-shot, uninstall rollback, no telemetry) are
verifiable from the diff. No new findings; the prior cosmetic R3-F01
remained closed by the implementation following the existing locator
script precedent. Termination signals do not fire.

## A1-A9 verification against diff

| ID | Verdict | Diff evidence |
| --- | --- | --- |
| A1 (resolver returns one owner for common case) | HOLDS | `session_metadata/mod.rs:92-94` calls existing `state.resolve_resume(models, input, None)`; resolver code in `src-tauri/src/state/db.rs` is untouched by the diff. |
| A2 (ambiguity = `ResumeError::Ambiguous`) | HOLDS | `map_resume_error` at `session_metadata/mod.rs:138` maps `ResumeError::Ambiguous` → `MetadataError::AmbiguousSession`; no second ownership query exists in the diff. Resolver-collapsed multi-chain inputs reach success path, matching D1a. |
| A3 (`transcript_locator` suffices for canonical local JSONL) | HOLDS | `available_jsonl_path` at `session_metadata/mod.rs:200-242` uses existing `locate_transcript` only; no provider spawn, no DB mutation, no network. |
| A4 (Claude path-hash + Codex `payload.cwd`) | HOLDS in Rev 3 form | `derive_claude_workspace_root` (lines 244-323) enumerates ALL decompositions (recursive split on `-`), succeeds iff exactly one decoded path exists (pure Rev 3 prose), else `unsupported-storage`. `derive_codex_workspace_root` (lines 377-443) line-walks the located JSONL, accepts only records with `type == "session_meta"`, extracts `payload.cwd`, requires absolute + exists + canonical + UTF-8, fails closed on missing `session_meta`, absent `cwd`, malformed JSON, non-absolute, non-existing, or non-UTF-8. All three Rev 3 forward-looking invalidator clauses still falsifiable. |
| A5 (`session_storage` is source of storage-type discrimination) | HOLDS | `From<&Option<SessionStorage>> for SessionStorageType` at `session_metadata/mod.rs:31-39` maps `ClaudeCode`→`ClaudeCode`, `Codex`→`CodexSession`, `None`→`Other`. Internal `SessionStorage` enum in `config/model.rs` is untouched by the diff. `codex_session` boundary translation occurs at JSON serialization only. |
| A6 (logically read-only after open) | HOLDS | `session_metadata/mod.rs` performs zero writes: no `INSERT`/`UPDATE`/`DELETE`, no `scan_provider`, no `migrate_chain_segment`, no quota or auth refresh, no provider spawn. `StateDb::open_default()` retains its existing physical write cost (deferred to 06-schema-probe per §12). |
| A7 (direct CLI sessions are resolver-visible after Initiative-05 chain mint) | HOLDS | Locate calls `resolve_resume`; partial DBs without segment membership map to `ResumeError::NoChainFound` → `SessionNotFound` → exit `10`, matching D4a. No `session_turns` fallback in the diff. |
| A8 (mutable approximated from active segment + storage + resume + transcript + workspace) | HOLDS | `session_metadata/mod.rs:116-119` ANDs: storage_type ≠ Other, `provider.resume.is_some()`, `jsonl_path.is_absolute()`, `workspace_root.is_absolute()`. The active-segment condition is implicit (success path requires `resolve_resume` returning `Ok`, which requires an active segment per `state/db.rs`). Path canonicalization + existence + UTF-8 are gated upstream in `available_jsonl_path` and the workspace deriver. |
| A9 (mutable does NOT consult quotas) | HOLDS | No reference to `provider_quotas`, `exhausted_at`, or quota-script invocation anywhere in `session_metadata/mod.rs` or the new `run_session_locate` in `src-tauri/src/main.rs`. |

**Termination signal #1 (`invalidated-assumption`) does not fire.**

## Adjacent-path blast-radius

| Path | Verdict | Diff evidence |
| --- | --- | --- |
| `agents resume` | PRESERVED | `src-tauri/src/main.rs` diff adds only the `Session` subcommand variant + dispatch arm; existing `Resume`, top-level `--resume`, `run_resume`, and `resume_execution_target` paths are unchanged. |
| `agents repl --resume` | PRESERVED | `Repl` subcommand and `run_repl` untouched in the diff. |
| Top-level `--resume` | PRESERVED | Top-level dispatch arm at `src-tauri/src/main.rs:341-389` not modified by this diff. |
| `agents trace --json` | PRESERVED + bit-stable | `src-tauri/src/trace/mod.rs` diff is purely the removal of the local `TranscriptState` enum and a `use crate::session_metadata::TranscriptState` import. The new enum has identical variants in identical order with identical `#[serde(rename_all = "snake_case")]` and identical `as_str` mappings (only its visibility changed from private to `pub(crate)`, which is JSON-output-irrelevant). `TraceSession`'s serialized JSON shape is bit-stable. |
| `agents migrate-config` | UNCOUPLED | `MigrateConfig` arm and `run_migrate_config` not touched by the diff. |
| `agents migrate-db` | UNCOUPLED | `MigrateDb` arm and `run_migrate_db` not touched by the diff. |
| Hidden `agents resume-list` | PRESERVED | `ResumeList` arm and `run_resume_list` not touched by the diff. |
| Direct CLI ingestion (`scan_provider`, adapter scripts) | PRESERVED | `src-tauri/src/sessions/mod.rs` not touched by the diff; locate only consumes `locate_transcript`, which §8 / README already permit. |

No path is BROKEN or DEGRADED.

## §11.1 claims verification

- **No user state one-shot**: VERIFIED. The diff adds no schema migration, no DB column, no config rewrite. `StateDb::open_default()` retains its existing schema-ensure + WAL + backfill behavior; `migrate-db` is uncoupled.
- **Existing partial DBs map to exit `10 session-not-found`**: VERIFIED. `map_resume_error` at `session_metadata/mod.rs:137` routes `ResumeError::NoChainFound` to `SessionNotFound`, and `metadata_error_exit_code` at `src-tauri/src/main.rs` maps `SessionNotFound` to exit `10`. No `session_turns`-direct fallback exists in the diff.
- **Uninstall/revert rollback**: VERIFIED. Removing the `Session` subcommand variant from `Subcommands` and deleting `src-tauri/src/session_metadata/` would leave the rest of the binary functionally unchanged. The `TranscriptState` move would need to be reverted in lockstep, but trace continues to consume the same enum shape.
- **No telemetry**: VERIFIED. Implementation paths are: `StateDb::open_default`, `load_models`, `ProvidersConfig::load`, `SessionsConfig::load`, `resolve_resume`, `locate_transcript`, file reads, `serde_json::to_string`, `println!`, `eprintln!`. No invocation rows, no trace records, no quota reads, no provider spawn, no `auth_refresh_command`, no JSONL copy/create/rename/truncate.

## Codex parser cross-check vs `scripts/codex-locate-transcript`

A4's Rev 3 invalidator names "upstream Codex schema drift" as the
falsifying condition. The bundled locator script
`scripts/codex-locate-transcript:21-60` already encodes the same
schema invariant the new Rust parser depends on:

- The script's content fallback at lines 36-42 walks the JSONL
  line-by-line, parses each line as JSON, filters by
  `obj.get("type") != "session_meta"`, then reads `payload.id` from
  the same `payload` sub-object that the Rust parser reads
  `payload.cwd` from. The shared shape is `{ "type": "session_meta",
  "payload": { ... } }`.
- `derive_codex_workspace_root` at `session_metadata/mod.rs:386-437`
  uses identical line-walk + `serde_json::from_str` + `type`
  discriminator + `payload`-then-field access, then layers four
  fail-closed checks (absolute, exists, canonicalize, UTF-8) and
  one fail-closed loop terminator (no `session_meta` found →
  `codex_missing_session_meta`).
- This means Codex schema drift would invalidate both the existing
  locator script and the new workspace-root parser simultaneously,
  producing a single coherent fail-closed behavior for the harness
  rather than divergent partial-success states.

R3-F01 (multi-record `session_meta` discriminator + first-match
behavior) is functionally addressed: `derive_codex_workspace_root`
filters by `type == "session_meta"` and returns on the first valid
`payload.cwd` extraction, matching the line-walk precedent. No new
finding.

## Trace JSON bit-stability check (TranscriptState move)

The `TranscriptState` enum was moved out of `src-tauri/src/trace/mod.rs`
into `src-tauri/src/session_metadata/mod.rs` and re-imported via
`use crate::session_metadata::TranscriptState`. Verification that
trace's serialized JSON output is bit-stable:

- Variant set unchanged: `Unresolved`, `NoLocator`, `Missing`,
  `Available` in identical declaration order.
- `#[serde(rename_all = "snake_case")]` preserved verbatim, so
  serde will emit the same lowercase strings (`unresolved`,
  `no_locator`, `missing`, `available`).
- `as_str` mapping preserved verbatim; only the function visibility
  changed from private to `pub(crate)`, which has no effect on JSON
  output.
- `Debug, Clone, Copy, Serialize` derives preserved; `PartialEq, Eq`
  added for cross-module equality testing — these do not affect
  serialization.
- `TraceSession` continues to embed `TranscriptState` as a typed
  field; serde tag/format remain unchanged.

Net: `trace --json` output bytes are bit-stable across this PR.

## Net value vs Rev 3

Net value is unchanged from the Rev 3 LOW verdict. Problem-map §6
entries #1-11 remain retired, with #8 retired for both Claude
(path-hash inversion) and Codex (`payload.cwd` parsing).
`SessionMetadata` field set unchanged; downstream Initiative-06
sequencing forward-compat preserved (06-export, 06-import-replace,
06-pause-handshake, 06-schema-probe). `MetadataError` reserved
sibling codes 13-17 unused, as required by §13.

**Termination signal #2 (`non-positive-value`) does not fire**: the
PR still delivers the stable JSON metadata surface the harness needs
to stop reading raw `state.db`/JSONL layouts directly. Blast radius
remains small and additive; rollback remains low-cost (delete the
`session_metadata` module + remove the `Session` clap variant +
restore `TranscriptState` to `trace`).

## Findings

- No new MEDIUM or HIGH surface. R3-F01 cleanly addressed by the
  implementation following the existing locator-script line-walk
  precedent.
- The README addition in `2605b37` documents the surface accurately
  (success requires canonical file-backed transcript;
  `transcript_state` is `available` on success only; `mutable: true`
  is a read-time eligibility hint, not a write lock; SQL is
  positioned as ad-hoc debugging, not the supported path). No
  contract drift between README and implementation.

**Final verdict: LOW. Termination signal: none.**
