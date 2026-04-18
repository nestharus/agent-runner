# VALUES — agent-runner

Architectural and behavioral values the project is built around. These
are what proposals, implementations, and risk assessments must align to.

This document is **load-bearing**. When a risk reviewer flags a
violation, it cites a value. When a proposal makes a tradeoff, it
justifies against a value. When two reviewers disagree on a design call,
they appeal to values. If a value is wrong for the current situation,
amend the document — don't quietly route around it.

These values were extracted retroactively from architectural decisions
made through the project's history (multi-window quotas, sessions
adapter, density scoring, composite IDs). They're descriptive of how
the project actually works, not aspirational.

---

## How to use this document

- **Proposal authors**: cite values explicitly in the
  "Tradeoff justification" section of every proposal.
- **Risk reviewers**: when a finding is severity ≥ medium, name which
  value the proposal violates. Findings that don't trace to a value
  should be reframed or downgraded.
- **Implementers**: if a code change requires violating a value, surface
  it to the proposal author. Don't paper over.
- **Anyone**: if you're tempted to add a `match cli_name { "claude" =>
  ..., "codex" => ... }` in app code, stop and re-read value #1.

---

## Architecture values

### V1 — The runner is ignorant of any specific CLI/provider

Knowledge of how to talk to a specific CLI (Claude Code, Codex, GLM,
Gemini, Droid, future ones) lives in **user-replaceable adapter
scripts declared in TOML config**. The runner sees opaque commands.

If you find yourself writing `if command.contains("claude")` or
`match provider_name { "codex" => ..., }` in runner code, you're
violating this value. The fix is a declarative configuration field
that captures the per-provider behavior, dispatched generically.

This is why we have `providers.toml` (`quota_script` per provider) and
`sessions.toml` (`turn_script` per provider). Each new per-provider
behavior should follow the same pattern: add a declarative field,
not an enum branch.

### V2 — New CLIs are added by shipping a script + a TOML entry

Adding support for a new CLI (e.g. a third Claude account, a Gemini
adapter, a custom internal model) must NOT require editing or
recompiling the runner binary. The contract is: write a shell script
that meets the documented adapter contract, add a TOML entry pointing
at it, done.

This means: every per-CLI capability the runner offers must be
expressible through the adapter contract. If a feature can't be
extended without changing runner source, the feature isn't ready to
ship.

### V3 — Per-provider variation is declarative, not procedural

When two providers need different behavior, the difference is
configured in `providers.toml` / `models.toml` / `sessions.toml`, not
branched on in code. The runner reads the config and dispatches
generically.

Examples:
- Different quota APIs → each provider declares its own `quota_script`
- Different session log formats → each provider declares its own
  `turn_script`
- Different session-id capture mechanisms → each provider should
  declare its own `session_capture` strategy (declarative field), NOT
  be sniffed from the command name

### V4 — Adapter contracts evolve via optional fields

When the contract a script must produce gets richer, new fields are
**optional** with sensible defaults. Existing scripts that emit only
the older subset keep working. There is no versioning header, no
feature negotiation, no transitional dual-format support.

Examples:
- Quota script: was single `{used_percent, resets_at}`; now
  `{windows: [...]}`. Old shape still parses as one window.
- Turn script: was `{session_id, turn_id, timestamp, role}`; will
  add optional `parent_turn_id`, `is_sidechain`. Adapters that don't
  emit them get treated as linear.

### V5 — One source of truth per concern

Raw CLI session logs are the source of truth for transcript content.
SQLite is the source of truth for structural relationships
(invocations, parent edges, quota windows, turn metadata). We do not
duplicate content from raw logs into SQLite. We do not sync between
layers.

If a piece of data could live in two places, pick one and document
why. If you're tempted to copy data "for performance" or "for
caching," reconsider — derived/cached data introduces sync invariants
that decay.

### V6 — Storage discipline

The SQLite database holds metadata + indices, not content. Specifically:

- Quota numbers, refresh times, learned deltas — yes
- Invocation outcomes (success, exit code, timing) — yes
- Turn metadata (timestamps, roles, IDs) — yes
- Transcript content, tool call payloads, attachments — **no**, those
  live in their original files

Content is large, sensitive, and already exists where the user put it.
Don't move it.

---

## Runtime values

### V7 — No daemon, no background processes

The runner is a CLI binary. Multiple invocations coordinate through
SQLite WAL, not through a long-running process. State that needs to
survive across invocations lives on disk; in-memory state is
per-invocation.

This means: no service to install, no port to manage, no PID file, no
"the daemon is down" failure mode. The tradeoff is that work that
benefits from being persistent (caches, watches, scheduling) has to
be designed around per-invocation cost.

### V8 — Lazy on use, not eager

Quota refresh, session scan, and other expensive operations happen
when an invocation actually needs them — not periodically, not on
startup, not for providers we're not invoking right now.

Concretely: if a user runs `agents -m claude-haiku "..."`, the runner
refreshes only the providers serving claude-haiku. It does NOT refresh
codex/codex2 quotas, scan codex session logs, etc.

Corollary: if expensive work needs to happen in the background, the
right answer is usually "defer until next invocation needs it" rather
than "spawn a thread now."

### V9 — Stdout is the model's response; metadata goes on stderr

The runner's stdout pipes the wrapped CLI's stdout — that's the model
response, including binary content for image/video wrappers.
Anything the runner itself wants to emit (invocation IDs,
diagnostics, status lines) goes on stderr.

This preserves binary safety for `agents -m seedream-t2i "A cat" >
cat.jpeg` and any future binary-output models.

### V10 — Failures are observable, never silent

Errors get logged. Degraded modes (fell back to filesystem
correlation, quota script timed out, session capture unresolved) are
recorded — typically in a column like `session_capture_method` so the
user can see what happened.

If a fallback fires, the user can tell from inspection. If a value is
"unresolved," it shows as "unresolved," not as a silent zero or empty
string that looks like normal data.

### V11 — Explicit propagation over inference

When context needs to cross a process boundary, use environment
variables, command-line args, or structured stdin/stdout — explicit
mechanisms.

Inferring relationships from timestamps, cwd, filename patterns, or
proximity heuristics is **fallback-only** and must be marked as such.
A design that uses inference as the primary mechanism violates this
value.

This is why parent-invocation tracking propagates via
`OULIPOLY_PARENT_INVOCATION` env var, not via "find an invocation
that ran 200ms before me."

---

## Decision values

### V12 — Density over absolute when scoring scarce resources

When comparing usage across accounts that reset on different
schedules, normalize by time-until-reset rather than comparing
absolute values. An account at 50% with 1 hour until reset has less
headroom-per-hour than an account at 10% with 7 days until reset.

This applies wherever we're picking among constrained resources, not
just quota balancing.

### V13 — Composite identifiers when locality matters

If a value's interpretation depends on which provider/account
context it lives in, the identifier carries both pieces explicitly:

```json
{"source": "claude2", "id": "9e69e8cc-..."}
```

A bare ID that requires a lookup elsewhere to interpret is a leaky
abstraction. The composite is self-contained and extensible without
breaking parsers.

### V14 — No backwards-compat shims for internal code

Internal code (runner internals, schema migrations) does NOT carry
transitional dual-path support, deprecated aliases, feature flags
for "old behavior," or "v1 vs v2 route" branches. We change cleanly.

This is distinct from V4 (adapter contract backwards-compat). The
contract surface to user-replaceable scripts evolves additively
because we don't control those scripts. Internal code we control —
update all call sites, delete the old code, ship.

### V15 — Surface choice belongs to the caller

The runner emits useful structured data; the caller decides what to
do with it. Examples:

- Trace output: human ASCII default + `--json` for piping
- Invocation ID: emitted on stderr always; capture or ignore as you wish
- Transcript inspection: pointer-only by default, full inline only on
  request

The runner does NOT decide "this is too verbose, suppress it" or
"the user probably wants pretty colors here." Default to giving the
caller everything, with a flag to summarize.

### V16 — One PR per concern unless concerns are mutually load-bearing

When implementing a multi-part proposal, prefer the smallest PRs
that each ship independent value over one bundled diff. Each PR
should:

- Land cleanly against `main`
- Pass tests on its own
- Deliver visible user value (or be a strict prerequisite for one
  that does)
- Be reviewable in one pass

If a proposal covers N concerns and they could ship as N PRs in
dependency order, default to splitting. Bundle only when splitting
introduces real coupling pain (e.g. the migration touches two tables
that must change together).

---

## Anti-patterns this document explicitly rejects

- `match provider_name { "claude" => ..., }` in app code — violates V1, V3
- Copying CLI conversation content into SQLite — violates V5, V6
- Background threads / daemons / cron — violates V7
- Refreshing all providers on every CLI invocation — violates V8
- "Verbose mode for diagnostics" wrapped around stdout — violates V9
- Returning empty string / 0 / null where "unresolved" is the truth —
  violates V10
- Inferring parent invocation from `created_at` proximity — violates V11
- Comparing 5h-quota-30%-used to weekly-quota-30%-used as equal —
  violates V12
- Returning a session_id without saying which provider it's in —
  violates V13
- "We'll keep the old code path behind a feature flag for now" —
  violates V14
- Auto-summarizing structured output because "the user probably
  wants it short" — violates V15
- Bundling 5 independent features into one PR for "a single coherent
  release" — violates V16

---

## Amending this document

If a value is wrong for the current situation, propose an amendment.
Don't route around. The amendment must include:

- The current value text
- The proposed new text
- The concrete situation that motivated the amendment
- What previously-rejected designs would now be allowed (or
  vice-versa)

The change goes through the same risk-gate process as any other
substantive proposal.
