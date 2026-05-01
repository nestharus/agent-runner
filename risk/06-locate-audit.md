# 06-locate — Phase 4 Audit Risk Report (Rev 1)

**Verdict: HIGH**

Rev 1 has most contract surfaces present, but it does not clear audit risk.
Two checklist/evidence gaps are blocking: D5 has no test-intent entry, and the
Codex workspace-root derivation is load-bearing for required JSON but is only
backed by a locator citation that inspects `payload.id`, not `payload.cwd` or
`payload.workspace_root`. A secondary drift exists in config-load semantics.
Prior-finding status: N/A for Rev 1.

## Concern-by-concern findings

### A1. Success JSON schema and exit-code contract — RESOLVED

§3 specifies the success fields, required flags, and stable types for
`session_id`, `chain_id`, `provider_name`, `storage_type`, `jsonl_path`,
`workspace_root`, `transcript_state`, and `mutable`
(`proposals/06-locate.md:85-97`). This covers the harness-required fields and
adds stable Initiative 06 fields (`01-session-locate.md:20-35`).

§5 covers every harness exit: `0`, `1`, `2`, `10`, `11`, and `12`
(`proposals/06-locate.md:128-139`; `01-session-locate.md:37-45`). Reserved
Initiative 06 sibling codes are not used (`proposals/06-locate.md:139`;
`initiatives/06-session-override-contract.md:108-111`).

### A2. Clap shape and API shape — RESOLVED

§2 gives the concrete nested clap shape:
`session locate <session-id> [--json]`, `SessionSubcommands::Locate`,
`session_id: String`, and `json: bool` (`proposals/06-locate.md:50-79`).
Dispatch placement is tied to the current match area
(`proposals/06-locate.md:77`; `src-tauri/src/main.rs:287-338`).

§6 names the module, public field names, enum variants, error variants, and
function signature (`proposals/06-locate.md:141-192`). The referenced current
types exist: `ModelStore`, `ResolvedResume`, `ProvidersConfig`, and
`locate_transcript` (`src-tauri/src/state/db.rs:128-138`;
`src-tauri/src/config/providers.rs:52-55`;
`src-tauri/src/sessions/mod.rs:171-175`).

### B1. Missing D5 test-intent coverage — FLAG-HIGH

Phase 3 requires each test-intent row to carry risk, intended behavior, level,
fixture source, assumption link, observable signal, and residual risk
(`implementation-pipeline.md:95-97`). The audit checklist also requires at
least one entry per D-decision D1 through D7.

Rev 1 defines D5 as "no `--state-db <path>` override in 06-locate" and no GUI
state DB support (`proposals/06-locate.md:113`, `proposals/06-locate.md:205`).
The §9.1 table has rows for D1, D2, D3, D4, D6, and D7, but no D5 row
(`proposals/06-locate.md:228-240`). Phase 6 would have to infer whether D5 is
untested, clap-only, documentation-only, or a non-applicability residual.

### B2. Fixture source and residual-risk fields — RESOLVED

§9.1 has fixture sources, observable signals, and residual risks per row:
temp DB state, provider config, sessions config, locator scripts, temp JSONL,
Unicode paths, and README checks (`proposals/06-locate.md:228-240`).

The fixture-infrastructure note is concrete enough to act on: temp state DB
seeder, temp config-root builder for `providers.toml`/`sessions.toml`, and tiny
locator scripts (`proposals/06-locate.md:242`).

### C1. Assumption register validation — RESOLVED

The proposal carries forward all eight problem-map §7 assumptions as A1-A8 and
adds A9 for quota exclusion from `mutable`
(`research/06-locate-problem-map.md:132-141`;
`proposals/06-locate.md:28-42`). The A6 narrowing is explicit: physical
read-only open is deferred to 06-schema-probe by initiative sequencing
(`proposals/06-locate.md:39`;
`initiatives/06-session-override-contract.md:118-120`).

Each assumption has a falsifiable invalidator; none is a bare `TBD`
(`proposals/06-locate.md:34-42`).

### D1. Cross-feature checklist citations — RESOLVED

§13 matches the initiative constraints:
shared codes at `initiatives/06-session-override-contract.md:108-111`,
resolver reuse at `initiatives/06-session-override-contract.md:112-113`, lock
observation at `initiatives/06-session-override-contract.md:114-117`, read-only
open sequencing at `initiatives/06-session-override-contract.md:118-120`,
anti-scope at `initiatives/06-session-override-contract.md:121-122`, and
`SessionMetadata` scope at `initiatives/06-session-override-contract.md:41-43`.
These align with `proposals/06-locate.md:288-301`.

### E1. Anti-scope closure — RESOLVED

§7 closes the harness anti-scope list: no export/import/replace/append,
auto-resume, provider spawn, quota refresh, config edit, credential/quota/raw
config exposure, or sibling Initiative 06 subcommands
(`proposals/06-locate.md:196-208`; `01-session-locate.md:66-71`).

D1b and D4b are explicitly rejected (`proposals/06-locate.md:203-204`). §8 is
consistent with the major anti-scope items: no DB row writes, provider
commands, quota scripts, config writes, or transcript rewrites
(`proposals/06-locate.md:211-220`).

### E2. Locator `state_dir` side effect is omitted — FLAG-MEDIUM

§4 step 7 requires calling existing `locate_transcript`
(`proposals/06-locate.md:118`), and that function creates the adapter
`state_dir` before invoking the locator (`src-tauri/src/sessions/mod.rs:183-187`).

§8 accounts for `StateDb::open` side effects, but not this locator-directory
side effect (`proposals/06-locate.md:211-220`). The harness says
"Side effects: none" while allowing configured transcript locators only as part
of the existing trace/session contract (`01-session-locate.md:46`). Phase 6
would have to decide whether directory creation is accepted, avoided, or
documented.

### F1. State DB open path and resolver signature — RESOLVED

`StateDb::open_default()` exists and opens
`dirs::data_dir()/oulipoly-agent-runner/state.db`
(`src-tauri/src/state/db.rs:611-615`), matching §4 step 2
(`proposals/06-locate.md:113`).

`StateDb::resolve_resume(&models, input, None)` is valid: the fourth parameter
is `model_override: Option<&str>` (`src-tauri/src/state/db.rs:2577-2582`),
matching §4 step 4 (`proposals/06-locate.md:115`).

### F2. Provider/session config load semantics citation drift — FLAG-MEDIUM

§4 step 3 says malformed config is an operational error unless the existing
loader treats absence as empty (`proposals/06-locate.md:114`). The cited resume
path calls `ProvidersConfig::load(...).unwrap_or_default()` and
`SessionsConfig::load(...).unwrap_or_default()`, swallowing malformed-load
errors as defaults (`src-tauri/src/main.rs:1079-1084`).

The provider loader itself distinguishes missing file from malformed TOML:
missing file returns default, malformed TOML returns `Err`
(`src-tauri/src/config/providers.rs:81-90`). The proposal may intentionally
choose stricter locate semantics, but the cited "same way resume-adjacent code"
evidence does not match current resume behavior.

### F3. Transcript locator signature and trace state — RESOLVED

`locate_transcript(sessions_cfg, provider_name, active_session_id)` matches the
actual signature:
`(&SessionsConfig, &str, &str) -> Result<Option<PathBuf>, String>`
(`src-tauri/src/sessions/mod.rs:171-175`; `proposals/06-locate.md:118`).

Trace's states are correctly cited as `unresolved`, `no_locator`, `missing`,
and `available` (`src-tauri/src/trace/mod.rs:73-80`).

### F4. Codex workspace-root derivation is speculative against cited source — FLAG-HIGH

§3 makes `workspace_root` a required success field; failure to derive it is
`unsupported-storage` (`proposals/06-locate.md:93-95`). §4 step 8 says Codex
should scan the located JSONL for `session_meta` and use absolute
`payload.cwd` or `payload.workspace_root` if present
(`proposals/06-locate.md:119-122`).

The cited Codex locator establishes only that current script logic inspects
`session_meta.payload.id`; it does not inspect or validate `payload.cwd` or
`payload.workspace_root` (`scripts/codex-locate-transcript:45-60`). The audit
prompt explicitly called this derivation speculative and required a flag if the
script does not look at `payload.cwd`.

Because the field is required on success and §9.1 expects Codex
`session_meta` cwd to produce a canonical root (`proposals/06-locate.md:237`),
Phase 6 would have to invent the Codex transcript-shape assumption or reject
Codex success cases more broadly than the proposal states.

### F5. Claude migration `cwd_hash` convention — RESOLVED

The migration code derives `cwd_hash` from the source transcript parent
directory name and uses it as the target Claude projects subdirectory
(`src-tauri/src/migration/mod.rs:155-188`). This matches §4 step 8's Claude
path-convention citation (`proposals/06-locate.md:120`).

### G1. README plan integrity — RESOLVED

The README update plan cites current ranges accurately: subcommands at
`README.md:127-140`, transcript locator docs at `README.md:374-386`, trace
state docs at `README.md:414-418`, and SQL paragraph at `README.md:500-512`.

The listed README changes cover synopsis, locate section, success JSON fields,
exit codes, trace-vs-locate behavior, and SQL positioning
(`proposals/06-locate.md:244-253`).

### H1. Implementation residuals — FLAG-LOW

§12 names concrete residuals: physical read-only DB open, GUI/CLI DB
divergence, resolver-parity ambiguity, workspace-root rejection, and limited
`other` storage success (`proposals/06-locate.md:278-286`). These are bounded,
but they do not cover the locator `state_dir` creation noted in E2.

### I1. Migration and rollback against partial DBs — RESOLVED

§11.1 says no user state one-shot is required and existing partial DBs remain
partial (`proposals/06-locate.md:272-274`). That is defensible against the
problem map's partial-DB finding because resolver candidate selection reads
`session_chain_segments`, not raw `session_turns`, and segmentless turns return
`NoChainFound` (`research/06-locate-problem-map.md:105-113`;
`src-tauri/src/state/db.rs:2696-2711`; `proposals/06-locate.md:126`).

### J1. Deferred-stub discipline — RESOLVED

No §3 or §6 type signature contains a TODO placeholder, fake variant, or
half-built runtime-error stub (`proposals/06-locate.md:85-97`,
`proposals/06-locate.md:149-190`).

Sibling features are scheduled Initiative 06 PRs, not locate stubs
(`initiatives/06-session-override-contract.md:41-56`).

### K1. Backwards-compatibility discipline — RESOLVED

The proposal does not preserve a legacy path for the same behavior. It adds a
new subcommand, keeps existing resume/trace/migrate behavior unchanged, and
rejects duplicate ownership paths (`proposals/06-locate.md:20-26`,
`proposals/06-locate.md:203-205`).

The `codex_session` mapping is a public output vocabulary choice, not a config
alias or internal enum rename (`proposals/06-locate.md:98`;
`src-tauri/src/config/model.rs:195-229`).

## Citations spot-checked

- OK — `proposals/06-locate.md:52` against current subcommands at
  `src-tauri/src/main.rs:77-166`.
- OK — `proposals/06-locate.md:77` against dispatch at
  `src-tauri/src/main.rs:287-338`.
- OK — `proposals/06-locate.md:113` against `StateDb::open_default()` at
  `src-tauri/src/state/db.rs:611-615`.
- OK — `proposals/06-locate.md:115` against `StateDb::resolve_resume` at
  `src-tauri/src/state/db.rs:2577-2582`.
- OK — `proposals/06-locate.md:115` resolver ambiguity behavior against
  `src-tauri/src/state/db.rs:2713-2749`.
- OK — `proposals/06-locate.md:118` against `locate_transcript` signature at
  `src-tauri/src/sessions/mod.rs:171-175`.
- DRIFTED — `proposals/06-locate.md:114` cites resume-adjacent config loading
  for operational malformed-config behavior, but resume uses
  `unwrap_or_default` at `src-tauri/src/main.rs:1079-1084`.
- OK — `proposals/06-locate.md:120` against migration `cwd_hash` parent-dir
  derivation at `src-tauri/src/migration/mod.rs:155-188`.
- DRIFTED — `proposals/06-locate.md:121` uses the Codex locator as precedent
  for `payload.cwd`/`payload.workspace_root`; current script only checks
  `payload.id` at `scripts/codex-locate-transcript:45-60`.
- OK — `proposals/06-locate.md:248` against README subcommand range
  `README.md:127-140`.
- OK — `proposals/06-locate.md:252` against README transcript locator/state
  ranges `README.md:374-386` and `README.md:414-418`.
- OK — `proposals/06-locate.md:253` against SQL paragraph `README.md:500-512`.
- OK — `proposals/06-locate.md:292-301` against initiative constraints at
  `initiatives/06-session-override-contract.md:41-43` and
  `initiatives/06-session-override-contract.md:108-122`.
