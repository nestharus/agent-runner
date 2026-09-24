# AGE-319 paired Linux artifact (prerequisite only)

## Separate old/new staging fixture

`stage_versioned_island.py` accepts ten explicit source paths: retained legacy
Runner/Bash images and adjacent configs, plus fresh Runner/Bash/broker/launcher
images and adjacent configs. It copies them into separate directories in one
new generation, records exact SHA-256/size/device/inode, syncs every file and
directory, renames the complete generation, syncs its parent, and verifies the
readback. It rejects an existing generation, mixed old/fresh State roots,
missing or changed images, and a fresh Bash config that does not point to the
staged fresh Runner. Every copied image is mode `0400` and cannot be executed
from the staging directory. It never modifies the old named image, config, State,
sidecar, WAL, handle, or installed service. `--require-root` requires a
root-owned, non-writable destination ancestry for installed staging.

The artifact is **inert**: its manifest says `admission=closed` and
`activation=inert-no-selector`; the script writes no `active-v2.json`, v1
manifest, public alias, service, or State database. Its fresh Bash image should
be built with Bash's `age319-closed-fresh` feature, which refuses every entry
before any helper or State effect. The fresh Runner has the matching
`age319-closed-fresh` feature, which refuses before its existing entry gate and
all CLI, GUI, maintenance, or State work. Staging alone does not attest those features
or prove new root admission. The old v1 `InstalledPair` cannot consume the
proposed v2 active record, and there is no installed common launcher for Runner
and Bash. Never place an active-v2 selector beside the v1 manifest or treat
this staging directory as an activated package.

On this joined source, the default Runner binary retains the broker-gated
offline root entry; `age319-closed-fresh` is a separate build feature. Pass an
explicit closed-feature image as `fresh_runner` when preparing v2 staging.
`stage_versioned_island.py` records exact bytes but does not attest Cargo
features. `build_paired_bundle.py` and the current Linux release job still emit
only the fixed schema-v1 bundle from their supplied default Runner/broker/
launcher images; they do not publish or install this v2 root. Do not reuse a
closed-feature Runner image as that bundle's old Runner image, or overwrite a
historical pinned v1 image during v2 preparation.

## Shared front-door source candidate (not installed)

`oulipoly-shared-front-door` is a separate Linux image. The installer helper
`shared_front_door.py` prepares a new `front-door-v2` root, a stable launcher
image, an immutable generation manifest, and non-executable fresh Runner/Bash
copies. It leaves the fixed v1 pair, its manifest/socket, original old Runner
and Bash/config paths, and all old State/WAL/sidecars untouched. The root must
be a child separate from the v1 manifest directory; the active file is named
`front-door-v2/active.json`, never `active-v2.json` beside `install-v1.json`.

The installer has explicit `prepare`, `check`, `publish`, `adopt`, and `read`
commands. Its CLI requires a root-owned control root, stage, launcher and broker
unless `--fixture` is supplied. Exact inventoried public symlinks may instead
live in a single user's `~/.local/bin`: their link owner must match their parent,
and every ancestor must be owned by root or that user and be free of group/other
write permissions. Their absolute link text, resolved old target and owner are
recorded before adoption; the root-owned manifest remains the source of truth.
Prepare checks all ten staged assets, the original adjacent old configs, and
the exact broker image. It does not change a public alias. Publish initializes
epoch zero as `legacy`; only then may an administrator adopt the named
aliases (`agents`, `oulipoly-agent-runner`, and `agent-bash`, plus
`oulipoly-plane` when an actual GUI entry exists)
one at a time. The exclusive publication lock and launcher shared lock
serialize the selector read through any old-image `exec`. A later
`fresh-closed` publication requires all aliases and the live root broker route
and image to match the generation. It uses a staged file, file and directory
fsync, atomic replacement, exact compare-and-swap digest, publication UUID,
and readback. A lost reply can be read back or retried with the same UUID.
Rollback to legacy or a second activation is refused. Production preparation
must declare the adapter disabled or bind both of its effective entries to the
inventoried Runner and Bash alias paths. An environment override to another
path remains outside this managed route. `check` reads every inventoried alias
back and reports `legacy`, `adopted` or `absent`; activation requires all present
aliases adopted. No command here has been run on installed paths.

The launcher recognizes the kernel `AT_EXECFN` of the exact Runner CLI, GUI,
or Bash alias. Before it touches old handle metadata or launches anything, it
reads the one active selector and verifies its own image, generation manifest,
old/fresh images and configs, and named broker image. Epoch zero calls each
alias's recorded old target image, so adoption before activation can create
late old debt.
After `fresh-closed`, Runner CLI/GUI and new Bash runs refuse. An exact old
Bash handle with matching old metadata may call old Bash for settlement;
unknown/ambiguous handles refuse. The fresh route reads the live broker's
generation and process image before refusing. The broker permits the launcher's
payload-free `I` route observation from a different image; fresh State
reservation and effect operations still require the pinned Runner image and
their physical checks. No fresh effect is opened by
this source. Direct/cached old binaries and adapter overrides that bypass the
named aliases are outside this front door and remain old-island debt.

The repository's `.desktop` archive entry names `/usr/local/bin/oulipoly-plane`,
but this host currently has no installed GUI alias. The installed OpenCode
adapter defaults to `~/.local/bin/agent-bash` and `~/.local/bin/agents`, with
`AGENT_BASH_BIN` and `AGENT_BASH_AGENT_RUNNER_BIN` overrides. On this host
`opencode.json` sets `tools.bash=true`, but the global custom tool file is named
`bash.ts.disabled`; the effective runtime adapter therefore needs a separate
inventory before declaring it bound or disabled. `PATH` lookup may resolve to the
user-local aliases; the launcher uses kernel `AT_EXECFN`, including relative
entries from the current directory, and accepts only exact inventoried names.
Direct launcher paths, alternate `PATH` hits, override paths, raw cached binaries
and deliberately changed local links are outside universal managed routing.
The fixed v1
Runner launcher and old Bash absolute helper cannot be replaced while pinned
old actors still need their images and paths. Fresh binaries stay mode `0444`
in this candidate; the copied fresh Bash config still points to the inert
stage. A later positive route needs a new compatible installation and separate
root/child/source/result proof, including host `sudo`/setuid/native access.

`build_paired_bundle.py` stages one versioned archive with the fixed Runner,
broker and thin launcher images, `install-v1.json`, broker service unit, `agents` and
`oulipoly-agent-runner` CLI links, and an `oulipoly-plane` GUI link and desktop
entry. The archive is inert: building or extracting it does not enable the
service or activate v30 State. The existing Tauri `.deb` and raw Runner release
assets continue to use their legacy paths and are **not** a paired deployment.

The manifest contains the workspace package version, a generation derived from
all three exact image digests, and those SHA-256 digests. The production broker
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
