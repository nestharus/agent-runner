# Synthesis: Observability Needs for Agent-Runner

This document maps `research/01-observability-problem.md` to
`oulipoly-agent-runner`'s specific situation and the user's stated
intent. The output is a scoped need-set the proposal phase will design
against, plus an explicit list of open questions the proposal must
resolve.

The synthesis is the user's last lightweight gate before the proposal
phase. After this is approved, the proposal author has a clear target
and the 3x risk gate becomes the next checkpoint.

---

## 1. The user's stated intent (verbatim)

The conversation framing this work was:

> "Return the ID of what was called and then we can call trace on that
> id to get the tree and then we can inspect the actual transcripts for
> each id in the tree? Something like that? That will work."

Decomposed:

- **Return the ID** — every invocation must emit a stable, durable
  identifier that the caller can capture without parsing the model's
  textual stdout (which is the model's response, not metadata).
- **Call trace on that ID** — a subcommand that takes the ID and walks
  outward to children, recursively, with their IDs.
- **Inspect transcripts for each ID** — from a node in the tree the user
  can get to the actual conversation: roles, content, tool calls.
- **"That will work"** — the user explicitly said this design is
  acceptable. We are not redesigning the contract. We are figuring out
  how to deliver it.

---

## 2. Scope decisions

The research enumerated seven use cases. For this initiative we are
committing to a subset. The rest are deferred and remain valid future
work but should not bloat the proposal.

### In scope

1. **"What did this run actually do?"** — given an invocation ID,
   reconstruct the transcript and tool activity for that invocation.
2. **"Show me the call tree."** — given an invocation ID, walk parent
   and child invocations, present a tree with each node's ID, model,
   provider/account, time, success/failure, and (if known) the
   correlated session ID.
3. **Returning the invocation ID to the caller** — emit it on a stable,
   parseable channel (stderr or a structured envelope, the proposal
   picks) so wrapping shell scripts and parent agents can capture it.

### Out of scope for this initiative

- "All calls a particular account made today across models" — useful but
  not what was asked. The data is mostly there already (`session_turns`
  is account-keyed); the gap is a query helper, which the proposal can
  document as an SQL recipe rather than a built-in subcommand.
- "Compare two parallel agents in real time" — requires streaming.
  Defer.
- "Was that a runaway loop?" — analytics over windows + transcripts.
  Defer.
- "Compliance: everything done with customer X's data" — requires
  customer scoping that the system doesn't model. Defer.
- Per-invocation stderr retention — useful, but separable. The current
  initiative's "what went wrong" answer is the failed invocation row +
  the transcript pointer (which contains the stderr-equivalent CLI
  output for chat-style CLIs). Stderr retention is its own feature.
- A web/TUI inspection surface — CLI subcommand + structured JSON
  output is enough for the stated need. A nicer UI is a separable PR.

---

## 3. The four schema-level gaps that block the in-scope use cases

Direct from the research, ranked by criticality:

1. **No invocation → caller channel** (`invocations.id` is auto-increment
   but never returned). This blocks "return the ID."
2. **No invocation → session correlation** (we know an invocation
   happened, we know turns landed in `session_turns`, but we have no
   stored link). This blocks "inspect the transcript" because we cannot
   identify which session a given invocation produced.
3. **No parent → child invocation edges**. This blocks "call tree"
   across invocations.
4. **No parent_uuid / sidechain capture in `session_turns`**. This blocks
   the within-session sub-branching part of the tree (Claude's Task tool
   subagent spans are invisible).

Other schema gaps the research flagged are real but not blocking the
in-scope work and will not be addressed here:

- `providers` table doesn't carry account name (would need
  account-level invocation queries, which we descoped above).
- `source_file` column exists but is unused (cosmetic; the
  reconstruction can recover the path from `session_id` + provider's
  base path).
- `last_error` is overwritten per failure (separable feature).

---

## 4. Tradeoff commitments (constraints on the proposal)

The proposal author is free to design the data model and CLI but should
honor these decisions made here:

- **Source of truth**: SQLite for the tree (IDs + edges + correlation),
  raw session log files for transcripts. We do NOT copy transcript
  content into SQLite. The trace command walks SQLite for structure,
  and dereferences each leaf to its raw `.jsonl` for content. Reasoning:
  storage pressure (DB already 215 MB), privacy (raw logs may be
  sensitive — keeping them in their original location preserves user
  expectations and existing OS-level access controls), and avoids
  duplicating data the user already has.
- **Cross-invocation correlation**: explicit parent-id propagation via
  environment variable. Inheritable, transparent, requires no
  cooperation from wrapped CLIs. NOT timestamp/cwd inference — too
  fragile. NOT modifying wrapped CLIs — out of our control.
- **Inspection surface**: a `trace` subcommand on the existing binary
  with both human-readable (default) and structured-JSON (`--json`)
  output. Plus README SQL recipes for ad-hoc questions outside the
  tree-walk pattern. NOT a separate viewer binary, NOT a TUI for now.
- **ID return channel**: stderr emission of a single, parseable line
  carrying a JSON object — NOT mixing into stdout (stdout is the model's
  response; callers pipe it). NOT a structured envelope wrapping stdout
  (would break binary-safe stdout for image/video models).
- **ID is composite, not opaque**: the returned identifier is
  `{"source": "<provider>", "id": "<uuid>"}`. The `source` is the
  provider/account name the balancer picked (e.g. `claude2`), and
  `id` is the per-invocation UUID. Reasoning:
  - The same `id` is meaningless without knowing which account it ran
    against (raw session logs live in account-scoped directories like
    `~/.claude2/projects/...`).
  - Composite JSON is extensible — we can add `model_name`,
    `started_at`, etc. later without breaking parsers.
  - JSON over delimiter-joined string ("`claude2:uuid`") avoids
    parsing fragility if a provider name ever contains the delimiter.
  - Children of this invocation receive the same composite via env-var
    propagation, so the tree consistently identifies parents.
  - Stable line prefix (e.g. `OULIPOLY_INVOCATION=`) makes shell
    capture trivial: `INV=$(agents ... 2> >(grep ^OULIPOLY_INVOCATION=))`.
- **ID format**: UUIDv4 for the `id` field. Strings survive
  backups/restores cleanly, are independent of insert order, and don't
  collide across machines/databases. The proposal decides whether to
  keep `invocations.id` integer as internal PK and add a UUID column,
  or replace.
- **Adapter contract evolution**: the existing `turn_script` contract
  is currently `{session_id, turn_id, timestamp, role}`. To capture
  within-session parentage we will extend it with optional
  `parent_turn_id` and `is_sidechain` fields. Existing scripts that
  don't emit them keep working (unset means "linear, no parent
  information available"). Reasoning: same backwards-compat principle
  as the multi-window quota refactor (legacy single-window output still
  parses).

---

## 5. Critical unknown the proposal must resolve

**How do we capture the session_id of an invocation we just spawned?**

This is the single hardest design problem in scope and the proposal
must address it head-on. The candidates are:

a) **Parse the wrapped CLI's stdout/stderr** for a session marker. Some
   CLIs print a session ID; some don't. Different CLIs use different
   markers. Brittle.
b) **Post-hoc cwd + spawn-time correlation** with files appearing in
   `~/.claude*/projects/<encoded-cwd>/`. Works if the file is created
   immediately (we can stat for files modified after our spawn time).
   Brittle for CLIs that don't write per-session files synchronously.
c) **Force the wrapped CLI to use a session ID we generate** via its
   own resume/replay flag (Claude Code has `--session-id`/`-c
   <id>`-style flags; Codex similar). Most reliable IF the CLI accepts
   it. Requires per-CLI knowledge.
d) **Pre-create an empty session file at a known path and pass a flag
   forcing the CLI to use it**. Variant of (c).
e) **Adapter-side correlation**: extend `turn_script` to optionally
   emit a "this session was spawned at TIME by PID" hint, then match
   to invocations by spawn time + pid. Works if adapters cooperate.

The proposal must pick one and document the fallback when it doesn't
apply. This is not just a CLI design question — it determines whether
the trace tree's "session" leaves are reliable or best-effort.

---

## 6. Other open questions the proposal must resolve

- **Where does parent_invocation_id come from when an invocation has
  no agent-runner parent (top-level)?** Null vs sentinel vs
  self-referencing. (Likely null; the proposal should justify.)
- **What does the tree look like when correlation fails?** A child
  invocation with no resolvable session, or a session with no resolved
  parent invocation, must still appear sensibly in the tree.
- **How do user-direct CLI sessions (no agent-runner involvement at
  all) appear?** They land in `session_turns` but have no parent
  invocation. Probably they appear as "orphan" sessions when listed by
  account/timestamp, but are not traversed by the tree command.
- **What's the trace-output format for a leaf transcript?** Pointer
  (path + session_id) only? Plus turn count? Plus first-line preview?
  Plus full content? Full content via a separate flag?
- **Do we add `parent_uuid` and `is_sidechain` columns to
  `session_turns` now, or only when the corresponding adapter changes
  ship?** The schema migration is essentially free; the question is
  whether the proposal includes the `claude-code-turns` adapter
  update in this initiative or splits it.
- **Do we model image/video model invocations the same way?** They have
  no transcript and binary stdout. Probably they appear in the tree
  with model name + timing + exit code only, no session_id, no
  transcript dereference. The proposal should commit to a behavior.
- **Backwards-compat for existing `invocations` rows** that have no
  UUID assigned (the 61 currently in the DB). Proposal must handle the
  migration cleanly (lazy backfill on read? backfill on first run?
  leave them as integer-only "legacy" rows?).

---

## 7. What the proposal will look like

The proposal author should produce `proposals/01-trace-inspection.md`
covering:

- Data model: column additions, new tables if any, migration path
- Cross-invocation parent propagation: env-var name, format,
  inheritance behavior across `sh -c`, security considerations
  (whether we sanitize anywhere)
- Session-ID capture mechanism: chosen approach + per-CLI fallback
  matrix (Claude Code, Codex, anything else we know about)
- Adapter contract delta: new optional fields, exact JSON shape,
  how the existing reference adapters change
- The `trace` subcommand: argument shape, default output, JSON output
  shape, how it walks the tree, how it dereferences transcripts
- ID return contract: exact stderr line format, conditions under which
  it's emitted (always? flag-gated?)
- README updates: "How to inspect a run" section, SQL recipes for
  ad-hoc questions
- Anti-scope: an explicit list of things NOT in this initiative
  (web UI, stderr retention, account-level queries, streaming, etc.)
- Each tradeoff committed in section 4 above must be re-stated and
  justified in the proposal — the risk gate will check that the
  proposal is consistent with the synthesis, and any deviation must be
  explicit.

---

## 8. Confidence note

The user explicitly approved the design shape ("That will work").
This synthesis assumes that approval still holds. If the proposal
phase surfaces a blocker that forces deviation (e.g. the session-ID
capture problem turns out to have no acceptable solution), the
proposal must escalate back to a synthesis revision rather than
silently scope-creep.
