# AGE-319 paired Linux artifact (prerequisite only)

`build_paired_bundle.py` stages one versioned archive with the fixed Runner
image, broker image, `install-v1.json`, broker service unit, `agents` and
`oulipoly-agent-runner` CLI links, and an `oulipoly-plane` GUI link and desktop
entry. The archive is inert: building or extracting it does not enable the
service or activate v30 State. The existing Tauri `.deb` and raw Runner release
assets continue to use their legacy paths and are **not** a paired deployment.

The manifest contains the workspace package version, a generation derived from
the two exact image digests, and those SHA-256 digests. The production broker
checks root ownership, path safety, both named image digests, its running image,
and the manifest before
binding `/run/oulipoly-kernel-broker/control.sock`. A Runner started from the
fixed `/usr/local/libexec/oulipoly/oulipoly-agent-runner` path (including via
either link) checks its running image and manifest, then asks the live broker
for that same generation and an open legacy entry route before any State work.
An old or replaced image, absent or old broker, changed image, draining gate,
or v30 route refuses. Direct invocation through the staged CLI and GUI links
also refuses: the only currently admitted fixed-image entry is an explicit
broker-owned host or child path, and host mode accepts only help/offline
diagnostics. Each fixed Runner holds a read lease on the durable root-owned
`/var/lib/oulipoly-kernel-broker/entry-admission.lock` for its process lifetime.
The broker closes X durably before checking that all such leases have exited;
the root-only `D` observation reports only this fixed-image drain. The state
directory grants the `oulipoly` group search access for this lock, without
granting access to broker State. Old Runner images and already-running old
helpers do not hold this lease and remain separate cutover debt.
Handle-local copies whose bytes match the installed Runner digest take the
same lease and check the broker's legacy route before doing direct work. A
different older cached helper remains outside this binary protocol and must
be found in the old actor inventory.

## Cutover requirements still open

There is **no supported installer activation** for this artifact yet. The
service currently owns only the broker and uses `KillMode=process`; it cannot
inventory or stop CLI/GUI/helper descendants. WSL here has no usable writable
unified cgroup-v2 subtree for that purpose. The broker currently admits only
help/offline diagnostic host entry and lacks a normal GUI or provider-work
custody path. Standalone `agent-store`, `agent-scratchpad`, and
`agent-messenger` assets have no paired ingress contract. Existing `.deb`,
user-local links, historical binaries and already-running State writers can
still address the retired user State path. A manifest or process-name scan
cannot make them join one supervision boundary. No process is signalled by
this artifact.

An eventual administrator cutover must first hold persistent ingress across
service restart, stop new launches through every supported entry, and obtain
an exact owned process tree for every admitted CLI, GUI and helper. Stop new
entry before stopping the supervisor; join its known processes and descendants
by pinned incarnation, and treat any unowned or uncertain writer as a blocker.
Only after a held sidecar fence, root-observed main/WAL/SHM census, and
recheck through offline publication may root-only State migration proceed.
Rollback before publication may reopen the old gate only after the new
supervisor is stopped and known children are joined. After publication, do not
reopen legacy writers or roll back to old State. Never kill an arbitrary
same-UID or process-name match. Host `sudo`/setuid behavior inside workloads
must remain unrestricted.

The manifest check is an image compatibility prerequisite only. It does not
prove a global writer census, persistent ingress, process custody, State v30
routing, or delivery readiness.

## Cold WSL reboot route

A changed Linux boot ID would end old PID namespaces and their open file
descriptors. It would not settle source, recipient, notification, ACK, or
terminal obligations recorded by those processes. The old v2 files must be
retained for reconciliation.

This archive cannot currently establish a cold cutover. The unit is wanted by
`multi-user.target`, with no early storage-fence unit or installed activation.
It does not order WSL user commands, existing `.deb`/raw binaries, or user-local
links behind a broker-held gate. The retired data directory may be selected by
the old executable's adjacent `config.toml` or `OULIPOLY_DATA_DIR`; it and its
parent may be user-owned. Changing ownership on only the sidecar file does not
fence a user who can rename the parent entry or create a fresh sidecar and
return an old-protocol success. Old Bash helper snapshots with a different
digest also bypass the paired-image lease.

A future installer must durably arm the preboot gate, establish a root-held
fence on **every resolved retired State and sidecar main/WAL/SHM path before
any user entry**, and keep that fence through snapshot, publication,
activation, and readback. The fence must cover directory replacement and
aliases as well as writes to existing inodes, while leaving normal provider
work and host `sudo` unrestricted. The broker must verify the arm record, a
different boot ID, the same held storage fence, and a closed admission gate
before a private cold proof can be constructed. An interrupted startup must
keep admission closed; a published v29 copy requires forward-only recovery.
The current broker refuses restart when a published `sidecar` exists without
a closed durable entry gate, but that check alone does not establish the cold
proof.
