# 06-schema-probe - Phase 4 Audit Risk Report (Rev 2)
**Verdict: LOW**

Rev 2 closes both Round 1 audit findings without weakening the contract. The
proposal now pins the public compatibility-map JSON shape and declares the
reusable read-only-open error variants. Fresh review found no new MEDIUM+
audit findings.

## R1 Closure Check

| R1 finding | Prior severity | Closure status | Evidence |
| --- | --- | --- | --- |
| R1-F01 / audit F01: compatibility-map JSON shape ambiguous. | MEDIUM | **closed** | §3 now requires `state_db.tables` as a flat table→boolean map, `required_columns` as table→column→boolean, `required_indexes` as table→index→boolean, and forbids dotted keys (`proposals/06-schema-probe.md:127-138`). The example serializes the canonical shape (`proposals/06-schema-probe.md:140-163`). The flow preserves canonical keys for absent structures (`proposals/06-schema-probe.md:247-263`), and D6 tests assert the same shape for missing/incompatible DBs (`proposals/06-schema-probe.md:397-399`). |
| R1-F02 / audit F02: `ReadOnlyOpenError` variants not explicit. | MEDIUM | **closed** | §6 now declares `Missing`, `NotADatabase`, `PermissionDenied`, `WalSidecarError`, and `Operational` variants with payloads (`proposals/06-schema-probe.md:293-310`). The variant table maps each trigger to CLI exit behavior and a §9.1 test row (`proposals/06-schema-probe.md:315-323`). |

Closure assessment: both R1 findings are closed, not weakened, regressed, or
partially deferred. The watch signals from the audit history are now addressed
by Rev 2 (`risk/06-schema-probe-audit-history.md:47-50`).

## Rev 2 Fresh Findings

No new MEDIUM+ findings.

## Fresh Checklist Review

### Scope and command surface

RESOLVED. The command remains one additive read-only surface:
`agents session schema-probe` (`proposals/06-schema-probe.md:3-7`).
It either extends 06-locate's `SessionSubcommands` or introduces the same
parent group independently, with no top-level aliases and no v1 flags
(`proposals/06-schema-probe.md:79-105`).

### JSON schema

RESOLVED. Required success fields cover binary identity, DB path/existence,
schema/user/current/minimum versions, compatibility maps, features,
supported storage types, and `safe_for_import_replace`
(`proposals/06-schema-probe.md:107-132`). R1-F01's ambiguity is closed by
the canonical map contract and example (`proposals/06-schema-probe.md:134-163`).

### Versioning and feature claims

RESOLVED. D1 chooses `PRAGMA user_version`, mirrors it into
`schema_version`, assigns versions `0`-`3`, and forbids the probe from
stamping (`proposals/06-schema-probe.md:165-186`). D2 uses a hardcoded
compiled feature map and keeps future sibling commands false until shipped
(`proposals/06-schema-probe.md:188-202`).

### Storage vocabulary and import-replace predicate

RESOLVED. D5 preserves the public `claude_code`, `codex_session`, `other`
vocabulary and rejects aliases to the internal config tag `codex`
(`proposals/06-schema-probe.md:204-218`). D4 keeps
`safe_for_import_replace` false unless compatibility, import-replace, and
pause-handshake support are all present (`proposals/06-schema-probe.md:220-237`).

### Resolution flow and exits

RESOLVED. The flow separates missing DB success, read-only open,
inspection, schema incompatibility, and operational failure
(`proposals/06-schema-probe.md:239-274`). The exit table covers `0`, `1`,
`2`, and shared Initiative 06 code `14`, with no use of sibling slots
`10`-`13` or `15`-`17` (`proposals/06-schema-probe.md:276-291`;
`/home/nes/projects/agent-runner/worktrees/06-locate/initiatives/06-session-override-contract.md:106-122`).

### Reusable API

RESOLVED. `StateDb::default_path`, `StateDb::open_read_only`, and
inspection helpers are assigned to `StateDb`; the CLI/probe module owns
JSON and exit mapping (`proposals/06-schema-probe.md:293-346`). R1-F02 is
closed by the explicit enum and variant mapping (`proposals/06-schema-probe.md:303-323`).

### Read-only semantics and side effects

RESOLVED. D3 forbids read-write-create opens, parent directory creation,
schema ensure helpers, WAL mode mutation, and chain backfill
(`proposals/06-schema-probe.md:325-337`). §8 repeats the no-side-effect
contract across DB schema/data/PRAGMAs, config, transcripts, provider/quota/
setup/discovery/scan paths, invocation rows, telemetry, and cache state
(`proposals/06-schema-probe.md:371-382`).

### Anti-scope and existing-command behavior

RESOLVED. D7 explicitly avoids retrofitting `agents trace` or other
existing read-intent commands to `open_read_only` in v1
(`proposals/06-schema-probe.md:359-369`). The unchanged-surface list also
keeps trace, repl, resume, top-level resume, hidden resume-list, migrate-db,
migrate-config, sibling Initiative 06 commands, and provider/config/quota/
transcript work outside the PR (`proposals/06-schema-probe.md:28-33`).

### Test-intent track

RESOLVED. §9.1 includes rows for D1 through D7, side-effect contract, and
README truthfulness, with levels, fixture sources, assumption links,
observable signals, and residual risk (`proposals/06-schema-probe.md:384-407`).
Rev 2 specifically adds canonical-shape assertions to missing and
incompatible DB exit tests (`proposals/06-schema-probe.md:397-399`) and
variant coverage through the D3/D6 rows (`proposals/06-schema-probe.md:394-398`).

### README, supported surface, and residuals

RESOLVED. The README plan covers command listing, a dedicated schema-probe
section, JSON fields, version assignments, exit codes, missing-DB success,
unversioned-DB refusal, and `safe_for_import_replace` semantics
(`proposals/06-schema-probe.md:409-418`). Supported-surface notes bound the
consumer, migration path, rollback, and observability (`proposals/06-schema-probe.md:420-449`).
Residuals are explicit and not MEDIUM+ audit blockers (`proposals/06-schema-probe.md:451-466`).

### Cross-feature constraints

RESOLVED. The checklist carries the shared error namespace, non-applicable
ownership/lock items, the schema-probe-owned read-only open variant, no
auto-resume/provider/quota/config/migrate-config coupling, feature flags,
and the pre-write compatibility surface (`proposals/06-schema-probe.md:468-482`).

## Spot-Checked Citations

- OK - The local worktree still has no `Session` subcommand group; current
  subcommands are trace, repl, resume, hidden resume-list, migrate-db, and
  migrate-config (`src-tauri/src/main.rs:77-166`).
- OK - Local dispatch has no `Session` arm and routes existing subcommands
  directly before top-level resume/prompt handling (`src-tauri/src/main.rs:287-345`).
- OK - The stacked 06-locate branch has `Subcommands::Session` with
  `SessionSubcommands::Locate`, matching the proposal's branch-order plan
  (`/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/main.rs:156-185`).
- OK - Current `StateDb::open` creates parent directories, opens SQLite
  read-write, sets WAL mode, ensures schemas, and then backfills session
  chains (`src-tauri/src/state/db.rs:431-608`).
- OK - `StateDb::open_default` resolves the CLI DB path via
  `dirs::data_dir()/oulipoly-agent-runner/state.db` and immediately calls
  the mutating open (`src-tauri/src/state/db.rs:611-615`).
- OK - Required session tables, columns, and indexes named by the proposal
  are present in current schema/index bootstrap code
  (`src-tauri/src/state/db.rs:559-597`; `src-tauri/src/state/db.rs:826-877`).
- OK - Current source has no existing `schema_version`, `user_version`,
  `schema-probe`, `open_read_only`, or `ReadOnlyOpenError` implementation;
  the proposal is additive relative to current code.
- OK - Internal config storage uses `claude_code` and `codex`, while the
  locate branch's public enum serializes `claude_code`, `codex_session`,
  and `other` (`src-tauri/src/config/model.rs:195-228`;
  `/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/session_metadata/mod.rs:23-38`).
- OK - README currently documents the CLI persistent state path and still
  frames direct SQL as ad-hoc inspection, matching the proposed README
  migration (`README.md:222-224`; `README.md:500-523`).

## Decision

Rev 2 passes the audit risk gate at LOW. R1-F01 and R1-F02 are closed, and the
fresh Rev 2 assessment did not identify a regression or new MEDIUM+ finding.
