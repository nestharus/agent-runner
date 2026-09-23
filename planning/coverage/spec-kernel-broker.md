# spec-kernel-broker — Linux kernel ownership source slice

## Source files

- `crates/oulipoly-kernel-broker/src/identity.rs`
- `crates/oulipoly-kernel-broker/src/entry_registry.rs`
- `crates/oulipoly-kernel-broker/src/lib.rs`
- `crates/oulipoly-kernel-broker/src/linux_main.rs`
- `crates/oulipoly-kernel-broker/src/main.rs`
- `crates/oulipoly-kernel-broker/src/protocol.rs`
- `crates/oulipoly-kernel-broker/src/registry.rs`
- `crates/oulipoly-kernel-broker/src/work_registry.rs`
- `src-tauri/src/kernel_entry.rs`
- `src-tauri/src/completion_owner/linux.rs`
- `src-tauri/src/completion_owner/mod.rs`
- `src-tauri/src/completion_owner/original_work.rs`
- `src-tauri/src/main.rs`
- `src-tauri/tests/age319_persistent_guardian.rs`

## Preconditions

- The broker source remains uninstalled. The opt-in Runner host entry starts a pinned completion guardian after a positive broker grant but does not release a child Runner.
- State and mailbox domains are initialized separately before this opt-in entry; preflight is read-only.
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
| Entry reservation from an unrelated child PID namespace classified `outside`. | Denied because the connector is not in the broker's host PID namespace. |
| Host entry reserves before guardian fork, then prepares a gated exact child and binds its domain. | Fsynced root/entry/prepared-guardian/domain binding; repeated bind refused. |
| Pinned guardian publishes a completion owner after broker G/A. | The owner row stores the exact root UUID, domain, supervisor UUID, and guardian incarnation; the driver remains gated until a second A and durable row comparison. |
| P or G is refused, or the guardian dies. | No child Runner is released; debt remains and later readback/admission is denied. |
| A sibling child tries to bind a prepared guardian's root ID. | Refused on pinned guardian incarnation mismatch. |
| Legacy `L` request. | Refused; no ungated Runner is released. |
| Host-connected socket inherited by a privileged child PID namespace process. | Ancestor PID claim fails in the kernel; a real child send fails the broker's per-request credential equality check. |

## Edge cases

- Sibling roots and works never share identity; a nested work must name its exact direct parent work incarnation.
- A double-forked, session-detached process with no inherited environment or socket still classifies by PID namespace membership.
- Registry read order does not matter for nested parent reattachment.
- A write or fsync failure poisons the running registry until restart/reconciliation.

## Error conditions

- Unknown, duplicated, malformed, stale, changed, or ambiguous records fail closed.
- `NS_GET_PARENT`, pidfd, boot/starttime, or namespace errors produce uncertain classification.
- Wrong root/parent, duplicate accepted work ID, or non-PID1 work target fails registration.
- Unknown root ID, wrong direct child, mismatched prepared guardian, duplicate prepare/bind, or dead pinned incarnation fails entry binding.

## Boundaries

- Classification is never positive work authority.
- The source has no authenticated Runner join, CLI/GUI descriptor handoff, guardian acceptance handoff, work launch, clean physical drain receipt, or retirement operation.
- The unprivileged user-namespace fixture cannot establish host-root sudo/setuid behavior.
- Ordinary Runner/Bash entry and allocated-attempt NNP/seccomp remain. The opt-in host entry holds the live completion guardian while waiting for the still-unimplemented one-use child join; it releases no child Runner.

## Declared test patterns

- `crates/oulipoly-kernel-broker/tests/private_pidns.rs` exercises root sibling/nested classification and root debt.
- `crates/oulipoly-kernel-broker/tests/private_work_pidns.rs` exercises root/work binding, sibling separation, adopted peer classification, persistence poisoning, and restart uncertainty.
- `crates/oulipoly-kernel-broker/src/linux_main.rs` unit tests exercise challenged credentials, a `CAP_SYS_ADMIN` child namespace socket handoff, exact host namespace policy, descriptor rejection, and production dispatch E/P/G ordering.
- `crates/oulipoly-kernel-broker/src/entry_registry.rs` exercises persisted exact prepare/bind and sibling/replay denial.
- `src-tauri/src/kernel_entry.rs` exercises read-only preflight and reserve-before-guardian ordering.
- `src-tauri/tests/age319_persistent_guardian.rs` uses an opt-in private user-namespace broker-like endpoint and the production Runner binary to verify refusal before grant, exact durable owner/root identity, sibling/replay denial, and dead guardian debt.
- `src-tauri/src/completion_owner/linux_admission_tests.rs` exercises prepare-before-release of the guardian gate.
- `src-tauri/src/completion_owner/original_work.rs` exercises exact broker root ID propagation into a one-use root authority.

## Cross-references

- `crates/oulipoly-kernel-broker/README.md` describes the opt-in service and missing integration.
- `AGENTS.md` § Test-audit infrastructure.
