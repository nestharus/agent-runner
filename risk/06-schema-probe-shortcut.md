# 06-schema-probe — Phase 4 Shortcut Risk Gate

**Verdict: LOW**

Each D-decision picks the branch that preserves the harness's
"refuse rather than corrupt" stance. None smuggles a problem
elsewhere by retitling it "anti-scope" or hides it behind a
deferred stub.

## Per-D judgments

### D1 — `schema_version` source = `PRAGMA user_version` (LOW)

Purpose-fit. SQLite already owns the integer slot, so the
proposal does not introduce a fresh metadata table that would
itself require bootstrap migration. D1b (metadata table) was
correctly rejected on that exact ground (§3.1, A2 invalidator).
The probe never stamps `user_version`; mutating schema-ensure
owns stamping (§1, §3.1).

The deliberate consequence — current DBs report
`user_version = 0` and are refused with exit `14` until a
mutating open stamps version `3` (§11, §12) — is disclosed,
not hidden. Bootstrap rides existing mutating paths
(`migrate-db` or any normal open), so no new bootstrap path
is created.

Residual: `CURRENT_SCHEMA_VERSION = 3` relies on every future
schema-touching PR remembering to bump it. §9.1 D1 acknowledges
this and pushes enforcement onto review. No probe-internal
mechanism would help.

### D2 — Feature flag enumeration = hardcoded list (LOW)

Purpose-fit and not documentation-only. §3.4 makes
`safe_for_import_replace` require both `session_import_replace`
and `session_pause_handshake` to be `true`, so a binary missing
either causes the predicate to report `false`. That is
functional gating, not advisory.

Clap-introspection rejection is correct: command presence does
not prove harness contract semantics. Cargo-feature rejection is
correct: these are ordinary product commands.

No deferred stubs. The `false` entries for unimplemented
siblings are truthful absence claims; §12 explicitly forbids
adding stub code for them and requires each sibling PR to update
the map when it ships.

### D3 — Read-only open semantics (LOW)

Purpose-fit. Partial-migration state is inspected structurally,
not repaired: missing tables/columns/indexes route to exit `14`
with failing booleans named on stderr (§4 step 9, §5). That is
the opposite of silent failure.

WAL handling is conservative. The proposal explicitly refuses
`immutable=1` because it can ignore live WAL content (§6.1);
inaccessible sidecars map to operational exit `1`, not schema
exit `14` (§9.1 D3 WAL row). The right distinction.

Observation: a DB with all required structures and
`user_version = 3` may still carry segmentless legacy
`session_turns` rows (Initiative 05 backfill skip), and the
probe will report `compatible = true`. Disclosed as a residual
in §12 — a known cross-feature limitation, not probe-side
silence.

### D4 — `safe_for_import_replace` predicate (LOW)

Conservative by construction. Seven §3.4 conditions must all
hold; failure of any returns `false`. Two are particularly
load-bearing:

- Requirement 6 (`session_pause_handshake == true`) prevents
  the predicate from going `true` until the lock primitive
  ships, even after import-replace lands. Without this guard, a
  future PR could ship import-replace alone and the predicate
  would advertise "safe" while concurrent writers raced.
- Requirement 7 (storage-type coverage) blocks reporting safety
  on a binary missing required storage support.

§3.4 explicitly notes this PR ships with the predicate expected
`false`. Pessimistic-in-doubt is the right default.

### D5 — Storage vocabulary = local public enum (LOW)

`{claude_code, codex_session, other}` matches 06-locate
verbatim (§3.3). Local duplication if schema-probe lands first
is acceptable because §3.3 explicitly forbids introducing a
second JSON vocabulary or aliases — pre-empting the
no-backwards-compatibility failure mode. §12 lists the
duplication as a Phase-5 reconciliation residual, not a shim.

### D6 — Exit code mapping (LOW)

Missing DB → exit `0` does not hide a degeneracy. The success
JSON carries `exists: false`, `schema_version: 0`,
`compatible: false`, `safe_for_import_replace: false` (§4 step
4, §5 row 2). The harness reads JSON, not exit alone; the
combination is unambiguous. The harness's own old-DB example
(spec lines 90-92, `/tmp/old-data`) is a different scenario
(unstamped DB present) and correctly routes to `14`.

The four-way split (`0` healthy / `0` missing / `1` operational
/ `14` incompatible) gives the harness more diagnostic
resolution. Operational errors (permission, invalid header,
WAL/shm access) stay on `1` and do not pollute `14` (§5 rows
3-4), keeping "schema mismatch" a clean signal.

### D7 — No retrofit of existing commands (LOW)

Correct. The proposal does not advertise that `agents trace` or
any other existing command is read-only; it only adds the
read-only path for schema-probe (§7, §9.1 D7). No claim, no
inconsistency to hide. Trace continues to mutate WAL state via
the existing mutating `StateDb::open`, unchanged.

The two opens have different purposes (mutating product paths
vs read-only inspection), not different vintages of the same
purpose, so this is not a parallel old/new shim under the
no-backwards-compatibility rule.

## Cross-cutting checks

- **Deferred stubs:** none. `false` feature flags are truthful
  absence claims; §12 forbids stubs for future siblings.
- **Backwards-compat shims:** none. Mutating `StateDb::open`
  is unchanged; `open_read_only` is additive and serves a
  different caller.
- **Anti-scope problem-shifting:** §7 exclusions (no retrofit,
  no GUI DB, no `--state-db` override, no probe-side stamping)
  are genuine boundaries. Each excluded item is either disclosed
  as a residual (§12) or routed to an existing mutating path
  (stamping → migrate-db). The harness's day-one need is
  satisfied, not deferred.

## LOW observations

1. `CURRENT_SCHEMA_VERSION = 3` depends on PR discipline; no
   probe-side mechanism enforces future bumps.
2. `compatible = true` does not validate complete chain
   backfill integrity; segmentless legacy turns can survive a
   "compatible" verdict. Disclosed §12.
3. `--state-db` override is anti-scope in v1, so harnesses
   cannot probe non-default paths without manipulating
   `XDG_DATA_HOME`. Disclosed §7, §11.
4. GUI/CLI DB-path divergence is preserved, not resolved.
   Disclosed §11.
