# Claude CLI output format — repo-owned schema declaration

This document is the **canonical schema owner** for the subset of the
Claude CLI's stdout and stderr token vocabulary that this repo reads.
Any in-repo consumer (e.g. `crates/oulipoly-setup`) that parses Claude
CLI output MUST pull from the shapes declared here, not from the CLI's
internal layout.

The Claude CLI itself is an external upstream-owned process. This
declaration does not change its behavior; it declares which subset of
its output shape this repo treats as a stable contract for the purpose
of A1 push-vs-pull system coupling
(`~/ai/conventions/code-quality.md` § Push-vs-pull system coupling).

Per `~/ai/agents/push-pull-auditor.md` § Metric Binding "LOW
canonical-doc-as-schema proof", a canonical workflow, convention, or
orchestrator Markdown file declaring a `## Schema`, `## Format`, or
`## Output Paths` section is the declared schema owner for the
artifact. This file plays that role for the Claude CLI artifact within
this repo.

## Schema

### Session-token vocabulary (Claude CLI stderr)

The Claude CLI emits its session identifier on stderr using one of two
line-prefix forms. The shape declared here is the **only** stderr
subset this repo pulls from; any other stderr content is treated as
opaque diagnostic text.

#### Forms

| Form | Line prefix | Suffix shape | Semantics |
|---|---|---|---|
| 1 | `Session: ` | `<id>` (trimmed) | Primary session-id announcement |
| 2 | `session_id: ` | `<id>` (trimmed) | Alternate session-id announcement |

Both forms are equally recognized; the consumer must accept either.
`<id>` is the opaque session identifier produced by the Claude CLI and
is treated as a string token by the consumer; this declaration makes
no claim about its internal structure (length, character set, UUID
shape, etc.).

#### Parse semantics

1. Iterate stderr line-by-line.
2. For each line, `trim()` whitespace.
3. If the trimmed line `starts_with("Session: ")`, the session id is
   the remainder of the line after the prefix, with surrounding
   whitespace trimmed.
4. Otherwise, if the trimmed line `starts_with("session_id: ")`, the
   session id is the remainder after that prefix, trimmed.
5. Otherwise, the line contributes no session id.
6. The first matching line wins; subsequent matches in the same
   stderr buffer are not consumed by the parser.
7. If no line matches either form, the consumer treats the session id
   as absent (`Option::None`).

#### Stability contract

This repo treats both forms as a stable contract surface for the
purpose of A1 push-vs-pull system coupling. If the Claude CLI changes
either prefix or the meaning of the suffix, the breakage point is
this declaration: update both this document and the consumer in the
same change. Consumers MUST NOT silently fall back to reading other
stderr substrings or to other internal CLI artifacts.

#### Out of scope

- Any other stderr content (diagnostic messages, progress markers,
  warnings, error backtraces) is opaque. This document does not
  declare a contract for it. Consumers that need to surface raw
  stderr (e.g. for failure reporting) MUST do so as a single
  truncated opaque blob and MUST NOT parse it for additional fields.
- Stdout JSON is declared by the `--json-schema` argument supplied at
  invocation; that schema is the schema owner for stdout. This
  document covers stderr only.

### Output Paths

This repo does not read Claude CLI output from files; it reads from
piped child stdout and stderr. No filesystem path is part of this
schema.

## Consumers

| Consumer | Pull site | Pulls form(s) |
|---|---|---|
| `crates/oulipoly-setup/src/agent.rs::extract_session_id` (via `SetupAgent::update_session_id`) | Reads piped child stderr after `send_turn` drains the child | Both forms 1 and 2 |

Any new consumer that pulls from Claude CLI output within this repo
MUST be added to this table and MUST pull only from the shapes
declared above.

## Source

Claude CLI is owned by Anthropic
(`https://docs.anthropic.com/claude/`). This document declares the
**repo-side contract** for parsing a stable subset of its output; the
upstream tool is not bound by this declaration.

## Audit binding

For A1 push-vs-pull system coupling
(`~/ai/conventions/code-quality.md` § Push-vs-pull system coupling),
this file plays the role of declared schema owner per
`~/ai/agents/push-pull-auditor.md` § Metric Binding "LOW
canonical-doc-as-schema proof":

> emit LOW when the pulled generated artifact's shape is declared
> inline by a canonical … workflow, convention, or orchestrator
> Markdown file in a dedicated `## Schema`, `## Format`, `## Output
> Paths`, or phase-specific schema-declaration section.

Consumers pulling only from the declared shape above score LOW under
A1 by canonical-doc-as-schema proof. Pulls that mix declared shape
with undeclared adjacent stderr content split: the declared-shape
portion scores LOW, the undeclared portion scores HIGH under the
private-source recipe.
