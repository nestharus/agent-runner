# AGE-319 paired Linux artifact (prerequisite only)

`build_paired_bundle.py` stages one versioned archive with the fixed Runner,
broker, thin launcher and Bash images, `install-v1.json`, broker service unit, `agents` and
`oulipoly-agent-runner` CLI links, and an `oulipoly-plane` GUI link and desktop
entry. The archive is inert: building or extracting it does not enable the
service or activate v30 State. The existing Tauri `.deb` and raw Runner release
assets continue to use their legacy paths and are **not** a paired deployment.

The manifest contains the workspace package version, a generation derived from
all four exact image digests, and those SHA-256 digests. The production broker
checks root ownership, path safety, all named image digests, its running image,
and the manifest before
binding `/run/oulipoly-kernel-broker/control.sock`. A Runner started from the
fixed `/usr/local/libexec/oulipoly/oulipoly-agent-runner` path checks its running
image and manifest, then asks the live broker
for that same generation and an open legacy entry route before any State work.
The `agents`, `oulipoly-agent-runner`, and `oulipoly-plane` links now enter the
exact installed launcher. It checks its own image and manifest, captures argv,
environment, stdio/TTY and cwd as bytes and descriptors, then submits a
challenged `oulipoly-installed-launch/v1` request. The broker pins that peer to
the launcher image, checks generation and descriptors, and refuses before
execution while the full supervisor and State routing are unfinished. A lost
reply is an error with no automatic retry. Direct invocation of the fixed
Runner also refuses unless it has one of the existing broker-owned entry
paths. An old or replaced image, absent or old broker, changed image, draining
gate, or v30 route refuses.

## Cutover requirements still open

There is **no supported installer activation** for this artifact yet. The
service currently owns only the broker and uses `KillMode=process`; it cannot
inventory or stop CLI/GUI/helper descendants. WSL here has no usable writable
unified cgroup-v2 subtree for that purpose. The broker currently admits only
help/offline diagnostic host entry and lacks a normal GUI/PTY or provider-work
custody path. The staged launcher does not start any workload. Terminal signal
and window-size relay, GUI display/socket lifecycle, lost-reply readback,
restart recovery, and cancellation still need a single broker-owned root/PID1
tree with exact physical drain. Standalone `agent-store`, `agent-scratchpad`, and
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

The fixed Bash image and digest are staged for future child admission. The
ordinary Bash `run` command still refuses a v30 owner before local handle or
workload creation. The private Bash child fixture is compiled under a separate
feature and is not part of this package.

Bundles with Bash use manifest schema 2 and a distinct `oulipoly-pair-v2`
generation derived from all four image digests. The reader still accepts
retained schema 1 manifests with no Bash field. A schema 1 bundle cannot admit
a Bash child; adding Bash to an old schema 1 manifest is invalid.
