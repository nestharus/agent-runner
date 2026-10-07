# Contract snapshot provenance

The provider contract's source of truth is `nestharus/agent-provider-sdk`
(`crates/provider-contract/contract/v1`). All 13 schema files here are
byte-for-byte equal to that path at SDK commit
`38acb566f985a77cd6a623257bfe7feb0302da62` (reachable from SDK `main`
`88ccaad04a18799aa9bfc612b5791ba36336799f`), the commit `Cargo.lock` resolves
for the `agent-provider-contract` dependency. The manifest names only the
repository; the lock carries the commit.

That SDK commit imported these files from this repository at
`5d025b82784556fe47521b1419c8eee574ddf13a` and changed two, adopted here
unchanged:

- `describe.schema.json`: the optional host-selected
  `capabilities.resident_session_v1`; capabilities open to unknown
  advertisements; advertised and preferred contract versions any
  `oulipoly.provider/vN`. Known capabilities stay boolean, and selected v1
  payloads and envelopes stay strict.
- `common.schema.json`: the `host.env` description naming the
  `OULIPOLY_HOST_RESIDENT_SESSION_V1=1` selector.

The other 11 files are byte-identical to `5d025b82`. SHA-256 of the two
changed files:

```text
a6760352a585883708d0eb538c1dd2cd7572995b8288c440ba2635fa7f7b2866  common.schema.json
a501bc9a83b602d47e8dde7c3b12f7e0d4a8296bed73e3da882749ab596da8e8  describe.schema.json
```

Raw schema validation cannot express that `preferred_contract` must be one
of `contract_versions`. The registered-provider receiver
(`src-tauri/src/commands/native_root/registered.rs`) therefore admits
`describe` through the SDK crate's own `SchemaRegistry` and typed decoding
(`DescribeCapabilities.additional` keeps unknown advertisements), then
selects with its `negotiation`/`resident_session` choosers. This crate's
generated `DescribeCapabilities` projection is unchanged: it ignores unknown
advertisements and does not carry `resident_session_v1`.

The resident-session extension's own schema, its `resident.prepare` params
and result, and common-version selection are used from the SDK crate, not
copied: `resident.prepare` travels in the base provider/v1 request and
success/error envelopes (`common.schema.json`), and the SDK's
`resident_session` decoders admit its params and result.

This snapshot is the SDK's semantic v1 realignment under fresh migration, not
a newer wire version. A later SDK release is adopted by replacing these bytes
from that commit and moving the lock with it.
