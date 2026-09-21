# Runtime cap registry

`runtime-caps.json` is the source of truth for production timeouts, deadlines,
retry/attempt bounds, scan/page bounds, queue/payload/size guards, polling
cadences, and provisional stopgaps shipped by agent-runner. The Rust query API
is `oulipoly_core::runtime_cap::registry()`; `registry_json()` exposes the same
stable operator-readable document without requiring the State database.

Every entry owns one numeric/`Duration` source declaration, its exact tokenized
initializer, and a production reference in a named source scope. A
`direct_control` reference is the branch or API call that applies the cap. A
`configuration_source` reference initializes typed configuration; the checker
proves that source edge but does not claim whole-program dataflow through the
configured field. Same-file references are bound to the nearest lexically
visible declaration, including function-local and nested-block constants.
Cross-file references must use a `crate`/`self`/`super` path or a qualified
`oulipoly_*` workspace-crate path that the checker maps to the declaration's
source module; bare import resolution is deliberately outside this
syntax-level checker and is not claimed as whole-program name resolution.
Normal Rust compilation validates those qualified paths.

The machine-checked truth boundary is declaration coverage, exact initializer,
lexical or qualified-path reference identity, and script declaration/use
structure. Classification, protected-resource prose, exhaustion behavior,
observability, configurability, and rationale remain source-review obligations;
the validator rejects known placeholders but does not claim to prove prose
semantics from a symbol or function name.

The workspace `runtime_cap_registry` test verifies both directions over all
production numeric/`Duration` declarations, independent of their names. Every
declaration must be a registry entry or a source-bound entry in
`runtime-cap-exclusions.json`; exclusions cover numeric protocol/schema IDs,
OS flag encodings, presentation geometry, and business decision thresholds
that have no runtime exhaustion policy. The checker also rejects relevant
anonymous duration constructors, positive raw `libc::poll`/sleep timeouts,
bounded-channel capacities, `Read::take` ceilings, cap-shaped numeric fields in
typed configuration literals, and anonymous fixed allocations of at least one
KiB. Literal arithmetic is evaluated for these forms. It intentionally ignores
ordinary `Vec::truncate`, iterator `take`, and allocation `with_capacity`
calls, which do not establish a ceiling by themselves; sub-KiB fixed arrays are
treated as protocol/layout storage unless their named declaration is otherwise
registered.

Shipped top-level integration scripts are discovered from `scripts/` rather
than enumerated. Their numeric declarations and raw timeout/sleep forms are
checked in both directions. Cargo integration-test targets and source modules
owned only by test configurations are mechanically excluded. `cfg` expressions
are evaluated as Boolean production reachability with `test` and the declared
`test-support` feature disabled and other predicates left unknown, so
`cfg(any(target_os = "macos", test))` remains production-reachable while
`cfg(all(unix, test))` does not. Compiled fault hooks remain registered as
`test_only_patience` when their declaration is production-reachable but their
behavior is inert without explicit test environment.

Classes have deliberately distinct semantics:

- `structural_safety_bound`: fixed format, traversal, or correctness ceiling.
- `external_protocol_deadline`: a deadline required to bound an external peer
  or protocol exchange.
- `resource_guard`: memory, storage, queue, payload, or retained-work ceiling.
- `polling_cadence`: how often work is observed; never terminal by itself.
- `test_only_patience`: allowed by the schema but normally excluded by the
  production-source checker.
- `provisional_stopgap`: a temporary hard stop that still requires explicit
  ownership and must not masquerade as a protocol or resource invariant.

Terminal or degraded action must be preceded by the observability named in the
entry. Lifecycle cap diagnostics use the database-independent AGE-369 flight
recorder and include cap ID/class, operation and phase, configured value,
outcome certainty, and a trace correlation.
