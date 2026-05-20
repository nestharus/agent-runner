# Terminal-signal provider vocabulary — repo-owned schema declaration

This document is the **canonical schema owner** for the per-provider quota-exhausted and rate-limit token vocabularies that the AGE-139 W1 foundation slice (`crates/oulipoly-runtime/src/executor/providers/*`) reads. Any in-repo consumer that recognizes a `TerminalSignalKind::QuotaExhaustedInband` or `TerminalSignalKind::RateLimited` signal from a provider CLI's stdout or stderr MUST pull from the token sets declared here, not from the provider CLI's internal output layout.

AGE-162 partitioned the previously-merged token vocabulary into two disjoint sub-vocabularies per provider: **persistent quota** (upstream account-level exhaustion that warrants `mark_provider_exhausted`) and **transient rate-limit** (transport-layer throttling that must not flip routing state). Recognizers evaluate the persistent-quota set first; a stream that matches both sets is classified as persistent (it is the stronger claim).

The Claude, Codex/OpenAI, and OpenAI-compatible (Gemini/OpenCode/...) CLIs are external upstream-owned processes. This declaration does not change their behavior; it declares which subset of their output shape this repo treats as a stable contract for the purpose of A1 push-vs-pull system coupling (`~/ai/conventions/code-quality.md` § Push-vs-pull system coupling).

Per `~/ai/agents/push-pull-auditor.md` § Metric Binding "LOW canonical-doc-as-schema proof", a canonical workflow, convention, or orchestrator Markdown file declaring a `## Schema`, `## Format`, or `## Output Paths` section is the declared schema owner for the artifact. This file plays that role for the per-provider quota-token vocabulary within this repo. AGE-125 PP-001 established the project-local `<repo>/conventions/` placement precedent for the same role binding.

## Schema

### Common parse semantics

1. The recognizer reads `TerminalSignalEvidence.stdout` and `TerminalSignalEvidence.stderr` as `&[u8]` slices that may contain invalid UTF-8.
2. Each slice is decoded with `String::from_utf8_lossy(...)` (lossy UTF-8 decode; replacement characters allowed).
3. The decoded text is then converted to lowercase via `str::to_lowercase()` so token matching is case-insensitive.
4. The per-provider `contains_persistent_quota_token(text: &str) -> bool` predicate applies a substring match for the persistent-quota set; `contains_transient_rate_limit_token(text: &str) -> bool` applies the same shape for the transient-rate-limit set. `text.contains(<canonical_token>)` returns `true` for any one canonical token from the provider's declared sub-set below.
5. The persistent set is evaluated first. If any one persistent-quota token matches in either stdout or stderr, the evidence carries the `QuotaExhaustedInband` token shape and the transient set is NOT consulted (persistent wins because it is the stronger claim). If no persistent token matches but any transient token matches, the evidence carries the `RateLimited` shape. Precedence rules in the Step 6a contract decide whether the final `TerminalSignalKind` is `QuotaExhaustedInband` / `RateLimited` (no stronger structured terminal-status evidence) or one of the stronger signals (`SpawnError`, `ProlongedSilence`, `SignalExit`).
6. The first matching stream (stdout, then stderr) selected by `find_matching_stream` is used as the evidence excerpt; `bounded_excerpt(<bytes>, max_len)` formats the bounded human-readable representation that lands in `TerminalSignal.evidence`.
7. Tokens are matched in lowercase form. Producers and consumers below name the lowercase canonical form. The `contains_persistent_quota_token` and `contains_transient_rate_limit_token` implementations MUST construct their substring checks from exactly the canonical lowercase forms declared here.

### Required-token sets

The following per-provider token sets are the **only** quota-exhausted and rate-limit vocabulary this repo's terminal-signal recognizers pull from. Each token is a lowercase substring. Each provider declares two disjoint sub-sets: a **persistent-quota** set (matches map to `TerminalSignalKind::QuotaExhaustedInband`, which downstream triggers `mark_provider_exhausted`) and a **transient-rate-limit** set (matches map to `TerminalSignalKind::RateLimited`, which is routing-state-neutral). A recognizer's predicates MUST match exactly these sets and only these sets; adding or removing tokens requires updating this document and the recognizer in the same change.

#### Claude / Anthropic (`crates/oulipoly-runtime/src/executor/providers/claude.rs`)

Persistent-quota tokens (lowercase substrings; evaluated first):

- `claude usage limit reached`
- `usage limit reached`
- `monthly limit`
- `billing limit`
- `resets at`
- `reset_at`

Transient-rate-limit tokens (lowercase substrings):

- `rate_limit_error`
- `rate limit`
- `too many requests`

Source ownership: Anthropic Claude CLI (`https://docs.anthropic.com/claude/`). This document declares the **repo-side contract** for parsing a stable subset of Claude CLI quota-exhausted and rate-limit output; the upstream tool is not bound by this declaration. Fixture-string drift is recorded as a documented residual in `planning/age-139-terminal-signal-core/risk/age-139-test-residuals.md`.

#### Codex / OpenAI CLI (`crates/oulipoly-runtime/src/executor/providers/codex.rs`)

Persistent-quota tokens (lowercase substrings; evaluated first):

- `usage cap`
- `billing limit`
- `quota exceeded`
- `reset_at`
- `resets at`

Transient-rate-limit tokens (lowercase substrings):

- `http 429`
- `status: 429`
- `status 429`
- `rate limit`
- `rate_limit_exceeded`

Source ownership: OpenAI Codex / `gpt`-CLI (`https://platform.openai.com/docs/`). Same upstream-not-bound disclaimer as Claude.

#### OpenAI-compatible (`crates/oulipoly-runtime/src/executor/providers/openai_compat.rs`)

Persistent-quota tokens (lowercase substrings; evaluated first):

- `quota exhausted`
- `quota exceeded`

Transient-rate-limit tokens (lowercase substrings):

- `rate_limit_exceeded`
- `429`
- `too many requests`
- `rate limit exceeded`

Source ownership: generic OpenAI-compatible providers (Gemini, OpenCode, and other wrappers exposing an OpenAI-compatible API). The token set is intentionally narrower than Claude/Codex because OpenAI-compatible wrappers vary; the residual class `non-canonical-provider-drift` covers per-provider deviations.

### Output Paths

The terminal-signal recognizers do not read provider output from files; they read from in-memory `&[u8]` slices supplied by the upstream caller (AGE-141 W2 bounded-silence detector, AGE-140 W3 executor result mapper, or the in-tree unit tests). No filesystem path is part of this schema.

## Consumers

| Consumer | Pull site | Tokens pulled |
|---|---|---|
| `crates/oulipoly-runtime/src/executor/providers/claude.rs::contains_persistent_quota_token` and `::contains_transient_rate_limit_token` (via `Recognizer::recognize`) | Reads lowercase-lossy stdout / stderr supplied through `TerminalSignalEvidence` | Claude/Anthropic persistent + transient sets above |
| `crates/oulipoly-runtime/src/executor/providers/codex.rs::contains_persistent_quota_token` and `::contains_transient_rate_limit_token` (same shape) | Same | Codex/OpenAI persistent + transient sets above |
| `crates/oulipoly-runtime/src/executor/providers/openai_compat.rs::contains_persistent_quota_token` and `::contains_transient_rate_limit_token` (same shape) | Same | OpenAI-compatible persistent + transient sets above |

Any new consumer that pulls from provider quota-exhausted or rate-limit vocabulary within this repo MUST be added to this table and MUST pull only from the per-provider sets declared above.

## Source

External vendor CLIs (Anthropic Claude, OpenAI Codex/`gpt`-CLI, generic OpenAI-compatible providers including Gemini and OpenCode) are upstream-owned. This document declares the **repo-side contract** for parsing a stable subset of their quota-exhausted output; the upstream tools are not bound by this declaration. The accepted residual is documented in the AGE-139 Step 6b residuals artifact at `planning/age-139-terminal-signal-core/risk/age-139-test-residuals.md` as `fixture-string-drift` (Claude / Codex / OpenAI-compat) and `non-canonical-provider-drift` (OpenAI-compat).

## Stability contract

This repo treats the per-provider token sets above as a stable contract surface for the purpose of A1 push-vs-pull system coupling. If any vendor CLI changes its quota-exhausted output in a way that breaks these tokens, the breakage point is this declaration: update both this document and the corresponding `crates/oulipoly-runtime/src/executor/providers/<vendor>.rs::contains_quota_token` in the same change. Consumers MUST NOT silently fall back to reading other stdout/stderr substrings or to other internal CLI artifacts.

## Out of scope

- Any other provider stdout/stderr content (diagnostic messages, progress markers, warnings, error backtraces, tool-use output) is opaque. This document does not declare a contract for it. Consumers that need to surface raw stdout/stderr (e.g. for failure reporting in the `evidence` field of `TerminalSignal`) MUST do so as a single bounded opaque excerpt (`bounded_excerpt`) and MUST NOT parse it for additional fields.
- The structured terminal-status path (`TerminalStatusEvidence::Exited`, `::SignalTerminated`, `::SpawnError`, `::ProlongedSilence`, `::Unknown`) is declared by the AGE-139 Step 6a contract (`planning/age-139-terminal-signal-core/contracts/age-139-terminal-signal-core.md`) and the `TerminalSignalEvidence` / `TerminalStatusEvidence` Rust types in `crates/oulipoly-runtime/src/executor/terminal_signal.rs`. That structured path is in-component schema and is not covered by this document.

## Audit binding

For A1 push-vs-pull system coupling (`~/ai/conventions/code-quality.md` § Push-vs-pull system coupling), this file plays the role of declared schema owner per `~/ai/agents/push-pull-auditor.md` § Metric Binding "LOW canonical-doc-as-schema proof":

> emit LOW when the pulled generated artifact's shape is declared inline by a canonical … workflow, convention, or orchestrator Markdown file in a dedicated `## Schema`, `## Format`, `## Output Paths`, or phase-specific schema-declaration section.

Consumers pulling only from the declared per-provider token sets above score LOW under A1 by canonical-doc-as-schema proof. Pulls that mix declared shape with undeclared adjacent stdout / stderr content split: the declared-shape portion scores LOW, the undeclared portion scores HIGH under the private-source recipe.

Project-local placement at `<repo>/conventions/` follows the AGE-125 PP-001 precedent (`worktrees/age-125-setup-agent-pipe-deadlock/conventions/claude-cli-output-format.md`) under ACR-251.

## Declared roles

`mapper, accessor`.

Rationale: this file declares (mapper) the per-provider canonical token vocabulary that the recognizers map provider output to, and exposes (accessor) named token sets for in-tree consumers to reference. The file is documentation only and contains no executable code or state mutation.
