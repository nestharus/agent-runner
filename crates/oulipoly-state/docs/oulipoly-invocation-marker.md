# Oulipoly Invocation Marker

The canonical grammar owner is the Rust module
`crates/oulipoly-state/src/invocation_marker.rs`. This document is the
human-readable mirror for producers, consumers, fixtures, and helper crates.

Canonical producers emit compact JSON through
`CompositeInvocationId::stderr_line()` for stderr marker lines and
`serde_json::to_string(&CompositeInvocationId)` for
`OULIPOLY_PARENT_INVOCATION`. The legacy shell-mangled grammar is accepted
only for compatibility; repo-owned producers must not emit it.

```text
marker-line          <- "OULIPOLY_INVOCATION=" marker-payload
parent-env-value     <- marker-payload
marker-payload       <- canonical-json / legacy-shell-mangled

canonical-json       <- serde_json(CompositeInvocationId)
CompositeInvocationId:
  source: string, required
  id: uuid-string, required
  unknown fields: rejected

legacy-shell-mangled <- "{" legacy-field ("," legacy-field)* "}"
legacy-field         <- wsp? ("source" / "id" / legacy-unknown-key) wsp? ":" wsp? legacy-value wsp?
legacy-value         <- quoted-value / bare-value
quoted-value         <- '"' chars-without-unescaped-quote '"' / "'" chars-without-unescaped-quote "'"
bare-value           <- chars excluding "," and "}"
legacy semantics:
  source and id are required
  source/id values are trim-stripped of outer single or double quotes
  unknown legacy fields are ignored
  id must pass UUID validation after parse
```
