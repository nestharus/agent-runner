# spec-kernel-broker — Linux kernel ownership source slice

## Source files

- `crates/oulipoly-kernel-broker/src/identity.rs`
- `crates/oulipoly-kernel-broker/src/lib.rs`
- `crates/oulipoly-kernel-broker/src/linux_main.rs`
- `crates/oulipoly-kernel-broker/src/main.rs`
- `crates/oulipoly-kernel-broker/src/protocol.rs`
- `crates/oulipoly-kernel-broker/src/registry.rs`
- `crates/oulipoly-kernel-broker/src/work_registry.rs`

## Preconditions

- The broker source remains uninstalled and is not a Runner or Bash entry path.
- An installed service would require host root in the initial user/PID namespaces, protected binary and state paths, and a fixed Runner image.
- Only trusted in-process broker code can call `insert_prepared`, after a separate positive accepted-work check. There is no work-registration socket opcode.

## Input → Expected output

| Input | Expected output |
|-------|-----------------|
| Live peer in a registered root PID namespace. | Exact root ID. |
| Live peer in a registered direct or deeper work PID namespace. | Nearest exact work incarnation and its root ID. |
| Peer in an unrelated host branch, with no registry debt. | Outside classification. |
| Work PID1 with exact live root and direct parent namespace, registered before release. | Fsynced record binding root/work/parent and PID1 incarnation. |
| Root or work PID1 missing/changed on restart. | Durable unknown debt, never a drain receipt. |
| Unsolicited descriptor in a challenged socket request. | Descriptor closed and request refused. |

## Edge cases

- Sibling roots and works never share identity; a nested work must name its exact direct parent work incarnation.
- A double-forked, session-detached process with no inherited environment or socket still classifies by PID namespace membership.
- Registry read order does not matter for nested parent reattachment.
- A write or fsync failure poisons the running registry until restart/reconciliation.

## Error conditions

- Unknown, duplicated, malformed, stale, changed, or ambiguous records fail closed.
- `NS_GET_PARENT`, pidfd, boot/starttime, or namespace errors produce uncertain classification.
- Wrong root/parent, duplicate accepted work ID, or non-PID1 work target fails registration.

## Boundaries

- Classification is never positive work authority.
- The source has no authenticated guardian acceptance handoff, work launch, clean physical drain receipt, or retirement operation.
- The unprivileged user-namespace fixture cannot establish host-root sudo/setuid behavior.
- Existing Runner/Bash entry, guardian, allocated-attempt NNP/seccomp, and CLI/GUI paths are unchanged.

## Declared test patterns

- `crates/oulipoly-kernel-broker/tests/private_pidns.rs` exercises root sibling/nested classification and root debt.
- `crates/oulipoly-kernel-broker/tests/private_work_pidns.rs` exercises root/work binding, sibling separation, adopted peer classification, persistence poisoning, and restart uncertainty.
- `crates/oulipoly-kernel-broker/src/linux_main.rs` unit tests exercise challenged credentials and descriptor rejection.

## Cross-references

- `crates/oulipoly-kernel-broker/README.md` describes the opt-in service and missing integration.
- `AGENTS.md` § Test-audit infrastructure.
