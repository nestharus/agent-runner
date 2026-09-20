# Runtime cap registry

`runtime-caps.json` is the source of truth for production timeouts, deadlines,
retry/attempt bounds, scan/page bounds, queue/payload/size guards, polling
cadences, and provisional stopgaps shipped by agent-runner. The Rust query API
is `oulipoly_core::runtime_cap::registry()`; `registry_json()` exposes the same
stable operator-readable document without requiring the State database.

Every entry owns one named source declaration. The workspace
`runtime_cap_registry` test verifies both directions: every entry resolves to a
real production declaration, and every cap-shaped production declaration has
an entry. It also rejects duration literals used outside named declarations,
literal `take`/`truncate` resource ceilings, literal bounded-channel capacities,
and literal durations passed to terminal APIs. Shipped integration-script
constants are checked in both directions. Test modules and integration-test
source trees are mechanically excluded; compiled fault hooks remain registered
as `test_only_patience` even though they are inert without explicit test
environment.

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
