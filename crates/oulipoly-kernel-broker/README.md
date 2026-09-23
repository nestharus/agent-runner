# Kernel broker source slice (AGE-319)

This Linux-only executable is an **uninstalled, opt-in building block**. No
Runner or Bash production path calls it. The current Runner and Bash admission,
grants, and allocated-attempt custody remain in force. The paired AGE-319 work
is not ready to deploy or merge on the basis of this slice.

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

The Unix stream socket accepts one request per connection:

1. Broker sets `SO_PASSCRED`, reads `SO_PEERCRED`, and opens a pidfd plus
   `/proc/<host-pid>/ns/pid` before sending a fresh 16-byte challenge.
2. Caller sends exactly 17 bytes in one send: `C` (classify) or `L` (launch the
   fixed Runner), followed by the challenge. The broker reads per-message
   `SCM_CREDENTIALS`, rejects a sender different from the pinned connector,
   and rechecks boot ID, process starttime, pidfd liveness, and namespace.
3. Broker closes the connection after one newline-terminated response:
   `inside <root_id>`, `inside-work <root_id> <work_incarnation>`, `outside`,
   `uncertain`, `released <root_id>`, or `error <reason>`. `protocol::request`
   is the matching client API. Classification is never an authorization grant.
   `released` confirms the
   namespace PID1 gate opened; Runner exec status is not yet reported.

The request has no executable, UID, mount, namespace, root ID, or PID fields.
For `L`, the broker takes the UID/GID from kernel credentials, refuses system
UIDs, checks the fixed root-owned image path, and admits only a peer classified
outside all managed roots **whose pinned connector is in the broker's exact
host PID namespace**. An unrelated child PID namespace can classify `outside`
but cannot launch. It creates a new mount and PID namespace without a
user namespace, mounts private procfs, durably records a new root UUID and
host PID1 incarnation before releasing the fixed Runner child. The protected
root PID1 reaps adopted children and has no idle exit timer. The Runner child
drops to the authenticated UID/GID and kernel-reported supplementary groups; it receives a
small fixed environment and no caller-supplied arguments or file descriptors.
The broker does not set `no_new_privs` or install seccomp.

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
`uncertain` and blocks new launches. There is deliberately no automatic
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

- Move the shared completion guardian outside all root namespaces and gate
  both CLI and GUI entry before their first possible fork. Bind the guardian's
  positive `RootAuthorityGrant.root_id` to this broker's exact root UUID and
  host/local process identities; never use classification as a grant.
- Add a broker-authenticated acceptance handoff from the guardian, then
  broker-controlled nested PID namespace launch and a PID1/reaper for each
  accepted work or allocated attempt. Call `insert_prepared` while the exact
  worker remains gated; place Bash workers and provider launchers before their
  first fork. Add clean physical drain receipts and reconciliation before any
  record can be retired. The current `L` operation creates roots only.
- Migrate all shared PID fields, sidecars, signals, and SQLite readers to
  explicit host/local PID domains. Add result/notification/ACK settlement and
  deliberate retirement; a live namespace or dead process alone proves none of
  these outcomes.
- Replace allocated-attempt `no_new_privs`/seccomp custody only together with
  the new per-attempt physical drain proof. Test ordinary host-root sudo/setuid,
  service restart, long-lived work, and actual installed host-root behavior in
  an authorized private host fixture. Those are not proven by unprivileged
  namespace tests.
