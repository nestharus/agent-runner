# Kernel broker source slice (AGE-319)

This Linux-only executable is an **uninstalled, opt-in building block**.
Runner's `OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1` branch calls the broker
before maintenance or owner startup. It reserves a root ID, prepares the exact
host guardian while that child waits on a gate, and binds its domain before
the guardian starts its driver. The branch then fails closed: no root Runner
or provider is released. Ordinary Runner and Bash admission and allocated
attempt custody remain in force. The paired AGE-319 work is not deployable.

## Installed paths and protocol

The unit artifact is `packaging/linux/oulipoly-kernel-broker.service`. It is
source only and must not be enabled before paired integration and privileged
host validation. The binary requires host root in the initial user and PID
namespaces, a root-owned installation at
`/usr/local/libexec/oulipoly/oulipoly-kernel-broker`, a root-owned fixed Runner
image at `/usr/local/libexec/oulipoly/oulipoly-agent-runner`, the systemd-created
root-owned `/var/lib/oulipoly-kernel-broker`, and a root-owned runtime directory
at `/run/oulipoly-kernel-broker`. The unit uses the `oulipoly` group for socket
access. It is not an installer and creates no group or host configuration.

The Unix stream socket accepts one challenged request per connection:

1. Broker sets `SO_PASSCRED`, reads `SO_PEERCRED`, and pins the connector with
   pidfd, starttime, boot ID, and PID namespace before sending a 16-byte challenge.
2. Caller sends one message: 17 bytes for `C` (classify), `E` (reserve entry),
   or disabled `L`; 37 bytes for `P` (prepare exact guardian PID); 49 bytes for
   `G` (bind root and domain UUIDs). The broker requires per-message
   `SCM_CREDENTIALS` to match the pinned connector and rejects passed FDs.
3. Responses include `outside`, `inside`, `uncertain`, `reserved <root_id>`,
   `prepared <root_id>`, `bound <root_id> <domain_id>`, and `error <reason>`.
   Classification is never an authorization grant. `L` always refuses because
   its former child launch lacked guardian and entry binding.

`E` requires an outside non-system UID in the broker's exact host PID namespace.
It fsyncs a broker-generated root UUID and pinned entry incarnation. The host
entry forks a waiting guardian, then `P` pins that exact direct child and fsyncs
its incarnation before releasing the child gate. `G` accepts only that prepared
guardian, binds its domain once, and fsyncs the binding. A sibling, repeated
bind, dead guardian, wrong entry, or changed incarnation cannot bind. The
`entries/` records remain as debt across broker restart; there is no timeout
or automatic retirement. This slice creates **no PID namespace or Runner child**.
No new `no_new_privs` or seccomp is applied.

The challenged request still relies on equality of connect-time
`SO_PEERCRED` and per-message `SCM_CREDENTIALS`, with the connector pinned by
pidfd/starttime/namespace. `SCM_CREDENTIALS` is configurable by a sender with
`CAP_SYS_ADMIN`; equality alone is not a general sender proof. For a socket
transferred from a host connector into a child PID namespace, Linux resolves an
explicitly claimed PID in the sender's own PID namespace before reporting it to
the host receiver. The child cannot name an ancestor-only connector. The
private adversarial test supplies `CAP_SYS_ADMIN` in a child user namespace,
tries the outside PID, observes `ESRCH`, and then verifies that a real send is
rejected by the broker's production request reader. This is a checked exclusion
for that specific transfer path, not a host-root sudo test or a general proof
for arbitrary IPC provenance. A host namespace process with host-root authority
remains outside this trust boundary and must be addressed by installation
policy and an authenticated host guardian.

The root registry uses one fsynced file per root and reattaches only to the
same boot, PID starttime, namespace inode, and live namespace PID1. A missing
or changed PID1 is retained as unknown debt. Any debt makes classification
`uncertain` and blocks new reservations. There is deliberately no automatic
retirement, timeout, or deletion of a root record.

The source also has a `works/` registry under that state directory. The broker
loads it before serving requests. Its in-process `insert_prepared` API requires
a live, directly nested namespace PID1 and binds a separate broker work
incarnation to the exact recorded root incarnation, accepted work ID, and
optional direct parent work incarnation. Trusted broker code must call it
**after** positive accepted-work authorization by the guardian and **before**
releasing a gated worker. The current service has no such call site or socket
operation: it does not create a work namespace or accept work. A peer inside a
registered work, including an adopted descendant, classifies to the nearest
registered work namespace. Sibling namespaces remain separate. Vanished work
PID1s and failed record writes remain uncertainty; there is no drain receipt
or retirement claim. The service rejects and closes unsolicited passed FDs on
challenged requests.

## Required next integration

- Bind the published `RootAuthorityGrant` to the broker record including
  supervisor authority and exact entry/guardian incarnation. Create a gated
  root PID1 and launch a Runner only after an authenticated one-use join.
  Validate CLI arguments, TTY descriptors, and GUI environment before release.
  The current host branch stops before dispatch.
- Add a broker-authenticated accepted-work handoff from the guardian, then
  broker-controlled nested PID namespace launch and a PID1/reaper for each
  accepted work or allocated attempt. Call `insert_prepared` while the worker
  remains gated. Add physical drain receipts and reconciliation before any
  record can retire. A bare work opcode or a claimed work ID is insufficient.
- Migrate shared PID fields, sidecars, signals, and SQLite readers to explicit
  host/local PID domains. Preserve source/ACK/physical drain distinctions.
- Replace allocated-attempt `no_new_privs`/seccomp custody only together with
  per-attempt physical drain proof. Test ordinary host-root sudo/setuid, service
  restart, long-lived work, and installed host-root behavior in an authorized
  private host fixture. Unprivileged tests do not prove these outcomes.
