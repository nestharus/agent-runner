# 06-schema-probe - Phase 4 Audit Risk Report (Rev 1)

**Verdict: MEDIUM**

Rev 1 covers the main harness surface and most Phase 4 audit obligations:
the command shape is bounded, the side-effect contract is read-only, the
exit-code table uses the shared Initiative 06 namespace, and the test-intent
track has a row for every D-decision. Two contract gaps remain audit-relevant:
the JSON compatibility report does not fully pin the nested boolean-map shape,
and the reusable read-only open API names an error type without explicit
variants.

## Findings

### F01 - FLAG-MEDIUM - JSON schema leaves the compatibility-map shape partially ambiguous

The proposal requires `state_db.tables`, `state_db.required_columns`, and
`state_db.required_indexes`, but gives broad types such as "object boolean map"
and "grouped by table" (`proposals/06-schema-probe.md:117-119`). Required
structural items are enumerated later in the implementation-helper section
(`proposals/06-schema-probe.md:283-292`), but the public JSON schema does not
pin the exact object keys and nesting shape for the compatibility report.

The highest-risk field is `state_db.required_columns`: "object boolean map" and
"grouped by table" can be read as a flat map like
`"session_turns.parent_turn_id": true` or a nested map like
`"session_turns": { "parent_turn_id": true }`. The missing-DB and
incompatible-DB flows also refer to "all required table/column/index booleans"
without pinning the same serialized shape (`proposals/06-schema-probe.md:206-222`).

Why this matters for Phase 4: the harness contract requires a stable stdout
JSON object with schema/version/features/storage fields, and downstream tests
must assert exact compatibility-report fields without inferring serialization
from implementation (`05-session-schema-probe.md:15-43`;
`implementation-pipeline.md:152-157`).

### F02 - FLAG-MEDIUM - `ReadOnlyOpenError` is named but its variants are not explicit

Section 6 defines
`StateDb::open_read_only(path: &Path) -> Result<Self, ReadOnlyOpenError>`
(`proposals/06-schema-probe.md:246-254`). It describes behavior for missing
files, read-only SQLite opens, WAL sidecar failures, and operational inspection
failures (`proposals/06-schema-probe.md:260-272`), but does not spell out the
`ReadOnlyOpenError` enum variants or payloads.

The checklist requires reusable API types, signatures, and error variants to be
explicit. The proposal makes `Missing` visible only as a prose "typed `Missing`
result" and leaves the operational-open cases to be classified later through CLI
error codes (`proposals/06-schema-probe.md:263-270`,
`proposals/06-schema-probe.md:223-225`). Phase 6 contract work would have to
invent whether permission errors, invalid SQLite headers, missing files, and
WAL/shm failures are separate variants or one opaque operational variant.

Why this matters for Phase 4: `open_read_only` is the reusable API assigned to
06-schema-probe by the initiative (`06-session-override-contract.md:118-120`).
An underspecified error enum can still produce the right CLI exits, but it does
not satisfy the reusable API handoff standard.

## Checklist Review

### Section 3 JSON schema

FLAG-MEDIUM. Required top-level success fields are present, typed, and mostly
stable (`proposals/06-schema-probe.md:97-123`). F01 covers the remaining
schema ambiguity in the structural boolean maps.

### Section 5 exit codes

RESOLVED. The table covers harness-required exits `0`, `1`, and `14`, keeps
clap misuse at `2`, and reserves the shared Initiative 06 codes around
schema-probe's `14` slot (`proposals/06-schema-probe.md:229-244`;
`06-session-override-contract.md:106-122`).

### Section 2 clap shape

RESOLVED. The proposal specifies `agents session schema-probe`, no v1 flags, no
`--state-db`, one final `session` group, and no top-level aliases
(`proposals/06-schema-probe.md:69-95`). It accounts for extending 06-locate's
`SessionSubcommands` or introducing the same parent group if schema-probe lands
first.

### Section 6 reusable API

FLAG-MEDIUM. The signatures and ownership boundary are present
(`proposals/06-schema-probe.md:246-281`), but F02 covers the missing explicit
`ReadOnlyOpenError` variant contract.

### Section 9 test-intent track

RESOLVED. The table includes every required pipeline field: risk, intended
behavior, level, fixture source/application point, assumption link, observable
signal, and residual risk (`proposals/06-schema-probe.md:319-338`;
`implementation-pipeline.md:95-97`). Fixture infrastructure is also called out
(`proposals/06-schema-probe.md:339-342`).

### D-decision coverage

RESOLVED. At least one test row exists for every D-decision: D1 at
`proposals/06-schema-probe.md:325-326`, D2 at line 327, D3 at lines 328-329,
D4 at line 330, D5 at line 331, D6 at lines 332-334, and D7 at line 335.

### Section 1.1 assumption register

RESOLVED. Section 1.1 replaces the problem-map draft register and narrows A1-A6
with evidence, invalidators, and uses (`proposals/06-schema-probe.md:35-49`).
The invalidators are falsifiable: required mutating ensure, schema states not
mapping to one integer, feature safety depending on runtime config, GUI DB
divergence entering harness scope, required shared enum reuse, or compatibility
requiring data repair.

### Section 13 cross-feature constraints

RESOLVED. The checklist cites the initiative's shared error namespace and
read-only-open assignment, and marks ownership/lock constraints not applicable
where schema-probe does not resolve or mutate sessions
(`proposals/06-schema-probe.md:403-417`; `06-session-override-contract.md:106-122`).

### Section 7 anti-scope

RESOLVED. The anti-scope excludes existing-command retrofits, sibling Initiative
06 features, provider spawn, auto-resume, auth/quota/setup/discovery paths,
scans, config edits, migration/backfill/stamping, `--state-db`, GUI DB support,
and third-party write compatibility (`proposals/06-schema-probe.md:294-304`).

### Section 8 side-effect contract

RESOLVED. The side-effect contract forbids state directory or DB creation,
schema/data/PRAGMA mutation, backfill, migration, provider/quota/setup/discovery
/scan/locator code, transcript reads, config edits, invocation rows, telemetry,
and cache state (`proposals/06-schema-probe.md:306-317`).

### Section 10 README plan

RESOLVED. The README plan names the command-list update, new documentation
section location, JSON fields, version assignments, exit codes, missing-DB
success, unversioned-DB refusal, and `safe_for_import_replace` semantics
(`proposals/06-schema-probe.md:344-353`).

### Section 12 residuals

RESOLVED. Residuals are concrete and bounded: unversioned current-shape DB
refusal, structural-only compatibility, GUI DB divergence, unknown build
commit, branch-order storage vocabulary duplication, and future feature-map
updates (`proposals/06-schema-probe.md:386-401`).

### Deferred stubs / backwards-compatibility shims

RESOLVED. I found no deferred implementation stubs. The proposal rejects
top-level aliases, does not add old/new command aliases, and frames sibling
feature flags as false until implemented rather than callable placeholders
(`proposals/06-schema-probe.md:86-92`, `proposals/06-schema-probe.md:156-161`,
`proposals/06-schema-probe.md:400-401`; `no-deferred-stubs.md:1-18`;
`no-backwards-compatibility.md:1-24`).

## Spot-Checked Citations

- OK - local `Subcommands` has no `Session` group; it contains `Trace`, `Repl`,
  `Resume`, hidden `ResumeList`, `MigrateDb`, and `MigrateConfig`
  (`research/06-schema-probe-problem-map.md:8`; `src-tauri/src/main.rs:77-166`).
- OK - local dispatch has no `Session` arm and matches existing subcommands
  directly (`research/06-schema-probe-problem-map.md:10`; `src-tauri/src/main.rs:287-338`).
- OK - stacked 06-locate has `Subcommands::Session` and
  `SessionSubcommands::Locate` (`research/06-schema-probe-problem-map.md:9`;
  `/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/main.rs:156-185`).
- OK - stacked 06-locate maps metadata errors to exits `10`, `11`, and `12`
  while operational remains `1` (`research/06-schema-probe-problem-map.md:42`;
  `/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/main.rs:561-568`).
- OK - `StateDb::open` creates parent directories, opens SQLite read-write,
  sets WAL mode, ensures schemas, and runs `backfill_session_chains`
  (`research/06-schema-probe-problem-map.md:14`; `src-tauri/src/state/db.rs:431-608`).
- OK - `StateDb::open_default` resolves `dirs::data_dir()/oulipoly-agent-runner/state.db`
  and then calls the mutating open (`research/06-schema-probe-problem-map.md:15`;
  `src-tauri/src/state/db.rs:611-615`).
- OK - required session tables, columns, and indexes cited by the proposal are
  present in current schema bootstrap/index helpers (`proposals/06-schema-probe.md:283-292`;
  `src-tauri/src/state/db.rs:559-597`, `src-tauri/src/state/db.rs:826-877`).
- OK - current code does not expose `user_version`, `schema_version`,
  `schema-probe`, or `open_read_only`; grep only found proposal/research text
  and older proposal references.
- OK - Cargo package metadata is `oulipoly-agent-runner` version `0.1.0`, and
  `build.rs` does not embed a commit (`research/06-schema-probe-problem-map.md:33`;
  `src-tauri/Cargo.toml:1-4`, `src-tauri/build.rs:1-3`).
- OK - internal config storage uses `claude_code` and `codex`, while stacked
  06-locate's public enum serializes `claude_code`, `codex_session`, and
  `other` (`research/06-schema-probe-problem-map.md:31-33`;
  `src-tauri/src/config/model.rs:195-228`;
  `/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/session_metadata/mod.rs:23-38`).
- OK - README documents the CLI persistent state path and presents manual SQL
  inspection as ad-hoc debugging (`README.md:222-224`, `README.md:500-523`;
  `proposals/06-schema-probe.md:346-353`).
- OK - GUI/Tauri state DB path is derived beside `models_dir`, not from the CLI
  data-dir resolver (`research/06-schema-probe-problem-map.md:79`; `src-tauri/src/lib.rs:525-533`).
