# spec-kernel-broker — Linux kernel ownership source slice

## Source files

- `crates/oulipoly-kernel-broker/src/identity.rs`
- `crates/oulipoly-kernel-broker/src/installed_launch.rs`
- `crates/oulipoly-kernel-broker/src/installed_launcher.rs`
- `crates/oulipoly-kernel-broker/src/installed_pair.rs`
- `crates/oulipoly-kernel-broker/src/accepted_grant.rs`
- `crates/oulipoly-kernel-broker/src/cutover_gate.rs`
- `crates/oulipoly-kernel-broker/src/entry_registry.rs`
- `crates/oulipoly-kernel-broker/src/lib.rs`
- `crates/oulipoly-kernel-broker/src/linux_main.rs`
- `crates/oulipoly-kernel-broker/src/main.rs`
- `crates/oulipoly-kernel-broker/src/protocol.rs`
- `crates/oulipoly-kernel-broker/src/private_installed_exec.rs`
- `crates/oulipoly-kernel-broker/src/registry.rs`
- `crates/oulipoly-kernel-broker/src/root_join.rs`
- `crates/oulipoly-kernel-broker/src/source_candidate.rs`
- `crates/oulipoly-kernel-broker/src/source_launch.rs`
- `crates/oulipoly-kernel-broker/src/source_physical.rs`
- `crates/oulipoly-kernel-broker/src/work_registry.rs`
- `crates/oulipoly-kernel-broker/src/writer_census.rs`
- `crates/oulipoly-kernel-broker/tests/private_root_join.rs`
- `crates/oulipoly-kernel-broker/tests/private_source_physical.rs`
- `crates/oulipoly-kernel-broker/tests/private_accepted_h_frame.rs`
- `crates/oulipoly-kernel-broker/tests/private_installed_exec.rs`
- `crates/oulipoly-kernel-broker/tests/age319_fresh_lane_socket.rs`
- `src-tauri/src/kernel_entry.rs`
- `src-tauri/src/completion_owner/linux.rs`
- `src-tauri/src/completion_owner/driver.rs`
- `src-tauri/src/completion_owner/broker_route.rs`
- `src-tauri/src/completion_owner/mod.rs`
- `src-tauri/src/completion_owner/original_work.rs`
- `src-tauri/src/main.rs`
- `src-tauri/tests/age319_persistent_guardian.rs`

## Preconditions

- The broker source remains uninstalled. The opt-in Runner host entry starts a pinned completion guardian after a positive broker grant and requests a one-use root child join for CLI help or offline diagnostics.
- State and mailbox domains are initialized separately before this opt-in entry; preflight is read-only.
- An installed service would require host root in the initial user/PID namespaces, protected binary and state paths, and a fixed Runner image.
- Only trusted in-process broker code can call `insert_prepared`, after a separate positive accepted-work check. There is no work-registration socket opcode.
- The grant ledger is recovery-validated at broker startup. Challenged `H` prepares a positive guardian grant but cannot consume it or launch work. A prepared/consumed record remains debt until an exact physical settlement exists.

## Input → Expected output

| Input | Expected output |
|-------|-----------------|
| Live peer in a registered root PID namespace. | Exact root ID. |
| Live peer in a registered direct or deeper work PID namespace. | Nearest exact work incarnation and its root ID. |
| Peer in an unrelated host branch, with no registry debt. | Outside classification. |
| Work PID1 with exact live root and direct parent namespace, registered before release. | Fsynced record binding root/work/parent and PID1 incarnation. |
| Exact bound host guardian sends H after exclusive acceptance, with pinned executable, intent, cwd, state and accepted descriptors. | Exact receipt bytes, live source incarnation and namespace, owner generation, request digest, supervisor, root PID1, work ID, and consumed parent grant/live namespace bind to a fsynced prepared record. No worker executes. |
| H succeeds, fails, or loses its response at the pinned guardian. | The accepted request remains no-replay and never forks through the legacy `Command::spawn` path; no execution grant or physical drain is inferred from H. |
| Prepared grant consumed before a future namespace fork, then broker restart. | Fsynced consumed record remains and replay is refused. |
| Root or work PID1 missing/changed on restart. | Durable unknown debt, never a drain receipt. |
| Exact live v30 driver requests the broker-selected reserved source. | The retained sidecar supplies admitted registration/listener bytes. The broker checks original registration, environment and image inodes/hashes, holds one nested PID1 worker before exec, consumes the exact grant once in FULL WAL, fsyncs the physical record and confirmation, rechecks the original names, then opens the worker gate. A lost response never permits a second launch. The Runner driver does not call this route yet. |
| Fresh D request UUID is sent to the versioned broker socket. | Broker commits an exact new-lane State session admission before the mailbox allocation. It returns only when both rows match the request, lane, source generation, session and allocation UUIDs. A lost reply returns the same pair; a State-first interruption is reconciled only by the same D key. d refuses the incomplete pair and never repairs it. Neither pair grants an invocation, owner, effect, result or ACK. |
| Broker-owned consumed source grant, pinned root/entry/driver/guardian/held worker, and empty root-only capture files. | One fsynced source physical record binds exact process incarnations, nested PID1 lineage, worker local PID and output inodes before the worker gate opens. A duplicate grant or orphan output blocks another physical binding. The joined child's prior broker stamp remains valid after that child exits. |
| Source worker exits while an adopted descendant remains live. | Source PID1 retains the worker wait, pumps each pipe to root-only disk with bounded memory and backpressure, reaps to ECHILD without a lifetime cap, syncs and hashes complete stdout/stderr, and writes one terminal receipt. Post-owner readback remains live/pending until the exact PID1 incarnation ends. Read/write/sync failure records incomplete diagnostic debt when storage permits. |
| Broker restarts after a durable source cancellation intent but before a signal reply. | The fixed root-only registry reopens, reissues the request through the exact PID1 pidfd, and observes the same terminal/drain receipt. A changed PID never receives the signal. |
| Unsolicited descriptor in a challenged socket request. | Descriptor closed and request refused. |
| Entry reservation from an unrelated child PID namespace classified `outside`. | Denied because the connector is not in the broker's host PID namespace. |
| Host entry reserves before guardian fork, then prepares a gated exact child and binds its domain. | Fsynced root/entry/prepared-guardian/domain binding; repeated bind refused. |
| Exact Runner host entry asks the live broker for its State route before any user-side sidecar read or root reservation. | Legacy route retains v29 preflight. A broker-owned v30 generation refuses this incomplete production client path, even when the retired user-side copy is present or corrupt; another executable cannot select or inspect the route. |
| Private v30 guardian prepares/releases an owner and its exact child gate; the pinned driver reads, reserves and observes acceptance. | Both actors derive the same source from broker I/Y, use challenged R/W and exact PK readback, and leave the retired user sidecar corrupt and unused. A second W cannot add another owner or attempt. Production entry remains closed while wider State repair and wake/custody readers are direct. |
| An execed product driver receives a pinned root and a v30 broker route. | It reads the exact running owner through broker I/R, then refuses before bounded State repair, reservation, wake, or any retired user-side sidecar open. A supplied root never falls back to v29; a legacy guardian omits it and retains its existing direct route. |
| Broker verifies a distinct live driver and writes or withdraws an activation reservation. | The retained State transaction compares the pinned driver identity, not the broker PID. Withdrawal is limited to the original unaccepted revision and clears the exact wake claim; accepted attempts cannot be withdrawn. |
| Broker activates v30 after a legacy route observation or restarts between that observation and entry reservation. | The broker refuses legacy E/P/G/A/J independently of the client's earlier observation. |
| Pinned guardian publishes a completion owner after broker G/A. | The owner row stores the exact root UUID, domain, supervisor UUID, and guardian incarnation; the driver remains gated until a second A and durable row comparison. |
| P or G is refused, or the guardian dies. | No child Runner is released; debt remains and later readback/admission is denied. |
| Exact original entry sends J after durable owner readback with help/offline-diagnostics argv/environment and five validated descriptors. | Join consumption is fsynced once, a separate root PID namespace has persistent PID1, and only the fixed Runner child is released after broker pre-exec identity verification. |
| Exact joined child verifies a connected native-owner socket through challenged V. | Broker checks the fsynced child stamp and root PID1 ancestry, the host guardian and driver boot/starttime, and host-side socket peer credentials before returning a read-only verification receipt. |
| A sealed Runner helper in a consumed K work verifies the connected owner socket. | A v3 H grant must pin the helper inode, Runner SHA-256, session, invocation and native registration-authority digest from the accepted intent. V must match the exact live work/grant, root, owner generation, authority digest, executable and guardian socket. The same bytes at another inode, altered witness, or missing/unconsumed grant refuse; an unchanged live work reattaches across broker restart. |
| V carries a changed guardian incarnation or unrelated connected socket. | Refused without changing the one-use join or granting work. |
| Production provider/recovery CLI, GUI or TTY entry. | Refused before broker reservation while host/local PID plumbing and descriptor handoff are incomplete. |
| Private feature-gated installed CLI entry with fixed Runner image, supported offline args, cwd and stdio/PTY descriptors. | One fsynced request identity; broker forks a host guardian and gated root PID1, pins the Runner child before release, and the guardian persists exit plus physical drain. Replay, unsettled second entrant, wrong image/generation and GUI refuse. Exact PID1 cancel survives broker restart and targets only that namespace. Production L remains closed. |
| Loader-controlled environment, forged completion socket, sibling J, or replay J. | Refused without an arbitrary command launch. |
| Broker restarts after a spent join. | Exact PID1 reattaches if live; spent join remains debt and no second child is launched. |
| A sibling child tries to bind a prepared guardian's root ID. | Refused on pinned guardian incarnation mismatch. |
| Legacy `L` request. | Refused; no ungated Runner is released. |
| Exact paired launcher sends a versioned L request with argv/environment bytes, present stdio/TTY descriptors and cwd. | Broker pins launcher executable and validates generation, request ID and descriptor shape; current production route refuses before Runner or State work. An absent broker or lost reply is an error, never an automatic new launch. |
| CLI and GUI paired links are staged from one archive. | Both point to the thin launcher; direct fixed Runner remains gated. Legacy `.deb` and raw Runner are not converted. |
| Host root closes broker ingress with X, then broker restarts. | Durable draining marker refuses ordinary broker opcodes; challenged i reports draining from broker-owned state. |
| A prerequisite fails before fixed sidecar publication, or fixed sidecar exists. | Explicit host-root x can resume legacy admission only before publication; publication makes abort refuse. |
| A direct SQLite writer retains main/WAL/SHM handles after ingress closes. | Writer remains live, so the latch gives no `QuiescedCutoverProof` and cannot authorize migration. |
| A pinned process retains main/WAL/SHM handles or an unlinked old WAL handle. | Broker-side census reports exact PID incarnation, executable inode and each handle as a blocker; a dead or changed PID does not count as the same writer. An empty scan is not a certificate. |
| Host-connected socket inherited by a privileged child PID namespace process. | Ancestor PID claim fails in the kernel; a real child send fails the broker's per-request credential equality check. |

## Edge cases

- Sibling roots and works never share identity; a nested work must name its exact direct parent work incarnation.
- A double-forked, session-detached process with no inherited environment or socket still classifies by PID namespace membership.
- Registry read order does not matter for nested parent reattachment.
- A write or fsync failure poisons the running registry until restart/reconciliation.
- A grant ID may bind only one work namespace record; old classifier records have no grant authority.
- Source capture has no arbitrary output cutoff. A positive terminal requires exact full-file length and SHA-256 readback; I/O failure, changed bytes or incomplete storage remains unknown debt.

## Error conditions

- Unknown, duplicated, malformed, stale, changed, or ambiguous records fail closed.
- `NS_GET_PARENT`, pidfd, boot/starttime, or namespace errors produce uncertain classification.
- Wrong root/parent, duplicate accepted work ID, or non-PID1 work target fails registration.
- Unknown root ID, wrong direct child, mismatched prepared guardian, duplicate prepare/bind, or dead pinned incarnation fails entry binding.

## Boundaries

- Classification is never positive work authority.
- Fresh v30 session allocation is broker-owned; production source effects, source acceptance/release, recipient delivery/ACK, and native K/Q remain closed. Private fixtures exercise the source launch and native physical paths separately. The installed launcher and normal v30 entry remain gated pending combined lineage and host-root proof.
- The unprivileged user-namespace fixture cannot establish host-root sudo/setuid behavior.
- Ordinary Runner/Bash entry and allocated-attempt NNP/seccomp remain. The opt-in entry keeps the host guardian outside the root PID namespace and releases only the fixed Runner for help/offline diagnostics.

## Declared test patterns

- `crates/oulipoly-kernel-broker/tests/private_pidns.rs` exercises root sibling/nested classification and root debt.
- `crates/oulipoly-kernel-broker/tests/private_work_pidns.rs` exercises root/work binding, sibling separation, adopted peer classification, persistence poisoning, and restart uncertainty.
- `crates/oulipoly-kernel-broker/tests/private_root_join.rs` runs the actual opt-in Runner and broker binaries in a private user namespace, holds the child at the pre-exec gate, and checks exact root/guardian placement, persisted child stamp, V acceptance and changed-incarnation/socket refusal, replay denial and broker restart debt. Its normal v30 modes use an independently admitted StateDb suffix, a corrupted retired sidecar, and the retained broker connection. The execed driver projects the bounded suffix, rejects wrong root/source/owner and stale/duplicate repair requests, reconciles a lost reply by exact cursor readback, and stops at the missing source grant. Restart retains the repaired cursor without admitting another root.
- `crates/oulipoly-kernel-broker/tests/private_source_physical.rs` runs a Rust nested PID1 reaper with a held Python worker and an adopted descendant longer than five seconds. It exercises post-owner readback, complete bounded output, duplicate and orphan debt, in-place output change, missing receipt, and cancellation intent replay after registry reopen. Its consumed grant is a private fixture surrogate, not a Bash-created source or production consume.
- `crates/oulipoly-kernel-broker/src/linux_main.rs` unit tests exercise challenged credentials, a `CAP_SYS_ADMIN` child namespace socket handoff, exact host namespace policy, descriptor rejection, and production dispatch E/P/G ordering.
- `crates/oulipoly-kernel-broker/src/entry_registry.rs` exercises persisted exact prepare/bind and sibling/replay denial.
- `crates/oulipoly-kernel-broker/src/installed_launch.rs` and `protocol.rs` unit fixtures capture CLI/GUI argument bytes, environment, cwd, absent stdio, PTY FDs/window size, second entrant, missing socket and a lost reply without an automatic retry. The fixture has no workload execution.
- `crates/oulipoly-kernel-broker/src/accepted_grant.rs` exercises receipt/intent binding, consumed replay refusal across reopen, and malformed recovery refusal.
- `src-tauri/src/kernel_entry.rs` exercises read-only preflight and reserve-before-guardian ordering.
- `src-tauri/tests/age319_persistent_guardian.rs` uses an opt-in private user-namespace broker-like endpoint and the production Runner binary to verify refusal before grant, exact durable owner/root identity, sibling/replay denial, and dead guardian debt.
- `src-tauri/src/completion_owner/linux_admission_tests.rs` exercises prepare-before-release of the guardian gate.
- `src-tauri/src/completion_owner/original_work.rs` exercises exact broker root ID propagation into a one-use root authority.

## Cross-references

- `crates/oulipoly-kernel-broker/README.md` describes the opt-in service and missing integration.
- `AGENTS.md` § Test-audit infrastructure.
