# Contract snapshot provenance

The provider contract's source of truth is `nestharus/agent-provider-sdk`
(`crates/provider-contract/contract/v1`). Runner admits provider/v1 wire
values through that crate (`agent-provider-contract`): its embedded schemas,
operation table, admission rules and DTOs. The 13 files here are a readable
snapshot of exactly those embedded bytes, and
`crates/oulipoly-provider/tests/schema_inventory.rs` fails if they differ.
The manifest names only the repository; `Cargo.lock` carries the commit
(currently `44f1bbe814cd27d4e5e89afad1c6fb4627a493b6`).

The bytes were first adopted at SDK commit
`38acb566f985a77cd6a623257bfe7feb0302da62` and are unchanged at the locked
commit. SDK commit `38acb566` imported these files from this repository at
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
of `contract_versions`; the SDK registry adds that rule, so every Runner
`describe` admission applies it. Runner's `oulipoly_provider::generated` and
`oulipoly_provider::schemas` re-export the SDK definitions and add only
host-owned items: the host-selected extension identifiers and selectors, and
the `resident.prepare` envelope row its provider client carries.

The resident-session extension's own schema, its `resident.prepare` params
and result, and common-version selection are used from the SDK crate, not
copied: `resident.prepare` travels in the base provider/v1 request and
success/error envelopes (`common.schema.json`), and the SDK's
`resident_session` decoders admit its params and result.

This snapshot is the SDK's semantic v1 realignment under fresh migration, not
a newer wire version. A later SDK release is adopted by moving the lock and
replacing these bytes from that commit.
