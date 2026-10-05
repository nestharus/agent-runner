# Native-root package (Linux x86_64)

A co-located native ACP v2 application payload: runner, per-root owner,
root PID 1, agent-bash, locked OpenCode dependencies, and the native
Claude receiver's locked dependencies (published Claude Agent SDK with its
unmodified Claude Code executable) with a pinned Node runtime, plus a
privileged front door and an explicit programmatic caller. Work runs with the
requester's normal host rights; the owner and root PID 1 run as root.
Host Python, its standard library and dynamic libraries remain trusted
runtime dependencies. This is not a static or verified privileged
security closure. No existing workflow's route is changed.

## Layout and construction

```
<package>/
  bin/oulipoly-agent-runner
  bin/oulipoly-root-supervisor
  bin/oulipoly-root-pid1
  bin/oulipoly-native-call
  libexec/oulipoly-native-frontdoor
  agent-bash/agent-bash
  agent-bash/bash.ts
  opencode/deps/
  claude/deps/          (Agent SDK, ACP SDK; Claude Code executable
                         node_modules/@anthropic-ai/claude-agent-sdk-linux-x64/claude)
  claude/node/bin/node  (official Node release, pinned sha256)
  share/install_package.py
  share/sudoers.template
  share/frontdoor.example.json
  share/README.md
  MANIFEST.json
```

All application assets are located relative to the package. The separate
`/opt/oulipoly-native/<id>` installation has no Broker dependency. The old
`/usr/local/libexec/oulipoly` prefix is refused.

```
python3 packaging/native-linux/build_package.py --build-dir B \
  --runner-repo W --agent-bash-repo T --agent-bash-commit C
```

Only B receives builds, dependency snapshots, cargo/npm caches, the Node
download, stage and archive. Cargo uses `--locked`; npm uses `ci
--ignore-scripts` for both lockfiles; Node is the official
`node-v24.21.0-linux-x64.tar.xz`, checked against its pinned sha256, of
which only `bin/node` and `LICENSE` are staged. The Claude Code and Node
executables are copied and hashed, never run; their dynamic dependencies
are read with `readelf`. MANIFEST also records the Claude lock hash, the
Claude Code package/version/sha256 and the Node release. Runner
features are the defaults. Dirty sources are refused unless explicitly
selected for construction. The id includes both commits; MANIFEST records
file hashes/modes, toolchain and Rust binaries' dynamic dependencies.
Sorted tar entries have 0:0 ownership and normalized timestamps/modes.
Warm same-host repeatability is the observed reproducibility scope; archive
ownership metadata does not establish installed root custody.

## Reviewed installation and recovery

ROOT reviews a signed source version, archive digest and root-owned copy
of the installer before running it. Nothing here authorizes installation.

```
/usr/bin/python3 -I <reviewed-installer> plan --archive A --sha256 H --user nes
/usr/bin/python3 -I <root-owned-reviewed-installer> install --archive A --sha256 H --user nes
```

`plan` makes a transient 0700 directory and 0600 archive copy, hashes that
copy, reads its package id, prints intended effects, and removes the copy.
It does not install into destination paths or establish manifest/custody
validation for them. Host-runtime construction requires Python 3.12+,
PID/mount namespaces, sudo and dynamic libraries listed in the manifest.

`install` validates literal and resolved existing destination ancestry.
Before the private copy or any other destination effect it creates a 0600
recovery record in the nearest existing prefix ancestor:
`<ancestor>/.oulipoly-native-<archive sha256 first 12>.install.json`.
Its exact location is printed before effects. The private copy lives in a
fresh 0700 directory alongside that record; only the hashed copy is read.
The record describes each planned effect before creation and records
created identities afterwards. Once the prefix exists it moves to
`<prefix>/<id>.install.json`. Both the relocation and package/rule rename
alternatives are recorded beforehand.

The installer extracts into `<prefix>/.<id>.partial`, verifies manifest
files and modes, renames into place, creates missing run-base parents and
`/var/lib/oulipoly-native/runs` (0711), and writes the site config (0644)
only if absent. Existing config is left after custody validation.
`visudo -cf` checks the staged 0440 rule. The enabling rename to
`/etc/sudoers.d/oulipoly-native` is the last enabling effect, after the
package/config/run base and their record are ready. Existing rules are
refused. No service or runtime is started.

Failures/interruption leave the current record for explicit recovery;
there is no implicit rollback/retry. An interrupted atomic record update
can also leave a root-private `<record>.writing`; its last complete record
remains the authoritative cleanup description and both names must be
reviewed. Recovery is:

```
/usr/bin/python3 -I <root-owned-reviewed-installer> uninstall \
  --record <exact-current-record> --purge-site
```

Uninstall acquires an exclusive package directory lock, also excluding
in-flight admission, and reports live/unknown run locks before changing
anything. It disables its unchanged rule first. It checks recorded
package content, including added paths, before removal; changed package,
site or sudoers files are left and reported. Only recorded empty
created directories are removed. It keeps the record until removals
finish. If that record blocks removal of its own parent directories, it
moves to a printed `<ancestor>/.<id>.removal.json` recovery path first.
Failures return 3 and retain a record. Without `--purge-site`, intentionally
retained site effects also retain the record and return 3.

A partially written extraction file that does not match the recorded
expected content is left for ROOT inspection; the record still names its
owned partial tree. Neither changed admin content nor unrelated data is
silently deleted. Nonempty retained run data requires separate explicit
ROOT review; uninstall does not stop tasks, purge runs, or remove caller
output. The root-owned installer copy is a separately owned ROOT effect.

## Front door and caller

The sudoers template pins one versioned front door and exactly `run`, with
`!use_pty`, `!log_input`, `!log_output`, `env_reset`, `!setenv`. The allowed
requester is selected from `SUDO_UID` and the root-owned site config. Real
sudo transport and host-root installation remain unexecuted construction
boundaries until ROOT separately runs them.

First stdin line (v1 JSON, at most 4 MiB): named site route, message,
absolute cwd, `bash` (`{"authority":"trusted-task"}` or
`{"allow":[whole commands]}`), extra env, positive site-bounded deadline,
optional inline access-only credential and `retention` (`discard`, or
site-allowed `keep`). Unknown fields are refused. No raw native-root
paths, user, provider config, credential path or recovery are exposed.
The cwd check drops to the requester identity.

The environment sets passwd identity values and site PATH/LANG. Known
unsafe/reserved names are refused (`LD_*`, glibc unsafe names, `MALLOC_*`,
`OULIPOLY_*`, `OPENCODE_*`, `AGENT_BASH_*`, `SUDO_*`, identity overrides).
Accepted requester extra names reach **root Rust processes as well as
work**. This is the accepted e2 denylist scope, not exhaustive assurance
against all privileged environment modifiers. Host/site trust remains a
condition. Work identity is dropped before cwd and PATH lookup.

The policy receipt describes constructed configuration, not the complete
effective host merge or immunity to same-user changes. Non-Bash native
tools are hidden/denied by that constructed configuration; a trusted
shell retains the requester's normal rights.

The entry is the direct child and PID 1 of a fresh PID/mount namespace,
with private /proc and parent-death SIGKILL. Killing it requests destruction
of the owned namespace. No global PID discovery or marker-based killing,
Broker or native recovery is used. Kernel teardown/uninterruptible I/O
has no finite guarantee; unknown stop is reported honestly.

Admission's first line and requester cwd check each have a 30 s bound.
Relay output and native controls use nonblocking bounded queues. Requester
cancel **or close** arms a kill after site grace G relative to that
control, even when native control/output does not drain. Signals, stdin
EOF, output loss/overflow likewise request cancel and arm kill. Without
an earlier control, the front door cancels at N+G and kills at N+2G.
After kill/partial EOF it collects only for a bounded interval; output
flush is limited to 2 s. It returns unknown (93) when stop/capture is not
observed rather than claiming a wait/reap. This bounds application I/O;
filesystem/kernel stalls are outside that guarantee.

After observed native exit, credential removal opens each ancestor with
`O_DIRECTORY|O_NOFOLLOW`, pins it by descriptor, then unlinks its own leaf.
A work-replaced symlink ancestor is reported as cleanup failure, preserving
the outside object. Descriptor-safe recursive removal handles discard.
Failed credential removal or discard residue yields 94. `keep` retains
noncredential state. The next same-user call may sweep free locks, using
these same cleanup rules. Sweep is not a wait witness for the previous
kernel namespace, and retention has no automatic time/quota bound.

Front-door codes: native entry status when observed, 90 admission refused,
91 setup failure, 92 namespace kill requested with observed collection,
93 stop/collection unknown, 94 cleanup/residue failure. No replay.

```
<package>/bin/oulipoly-native-call --route sol-high --prompt-file TASK \
  --cwd TREE --out NEW-DIR --trusted-task --deadline 1800 \
  --credential-codex-profile EXPLICIT-PROFILE
```

The caller selects `sudo -n <realpath frontdoor> run`; no profile search or
fallback. It nonblockingly sends request/control, captures stdout and
stderr, sends close on input 0's turn end, and cancel at deadline/signals.
It collects at most N+65 s overall, or 65 s from an early close/cancel,
with 30 s from terminal/partial EOF, plus a 0.2 s child exit attempt.
Lines are limited to 8 MiB and capture to 64 MiB; exceeding them gives
incomplete/unknown. Closing pipes requests abandonment; terminating its
own helper cannot prove a privileged namespace ended. It returns without
an unbounded wait. These limits cover normal programmatic I/O, not kernel
or filesystem stalls. The front door's site grace can exceed the caller's
collection allowance; then unknown is an intentional outcome.

Output directory (0700, must be new): `request.public.json` (credential
field reduced to provider/expiry), `events.jsonl`, `stderr.log`,
`caller.jsonl`, `final.md` when linked text exists, `result.json`.
Type-only unexpected failures are machine-readable; if the directory
itself is unwritable, stderr supplies the class instead of promising a
result file that cannot be written.

Caller codes: 0 answered, 1 no-answer, 2 usage, 3 local refusal,
4 front-door refusal, 5 cancelled, 6 incomplete/stop unknown,
7 cleanup failed, 8 launch failed, 9 ended otherwise.
`answered` requires linked text, its end_turn, complete transport and clean
retirement; it **does not prove task completion or correctness**. A denial
or tool echo can be answered/0; entry 87 means close followed through.

## Native Claude routes

A route with `"harness": "claude"` names `model`, `effort`
(`low|medium|high|xhigh|max`), `config_dir` and `"credential": "none"`;
nothing else. The example site config carries `opus-medium` and
`opus-high` (`claude-opus-5-5`, `.claude5`); `sol-high` is unchanged.
`config_dir` is relative to the requester's passwd home (no absolute path,
no `..`): the requester's own Claude configuration directory. The front
door, runner and owner only name it; nothing of root reads it, and no
credential is accepted, staged or removed for these routes.

The owner runs the receiver (`claude/node/bin/node` with the owner crate's
`native/claude/acp-v2-receiver.mjs`, a `stdio` harness) as the requester.
It drives the packaged Claude Code executable through the Agent SDK with
`CLAUDE_CONFIG_DIR` set to that store, so Claude Code's own login there
pays and refreshes, and its transcripts land there, outside the run's
discard. Inherited `ANTHROPIC_*`/`CLAUDE*` variables (API keys, OAuth
tokens, base URL and provider redirects) are withheld from Claude Code.
No user/project/local settings, hooks, plugins, CLAUDE.md or auto memory
load; MCP is strict (only the receiver's server); permission mode is
`dontAsk`; built-in Bash, Agent/Task, Monitor, background, web and skill
tools are not offered. `bash` is the receiver's tool running `agent-bash
run --delivery sync` into the root's Bash ingress (attributed, recorded,
killed on cancel); an allow list is enforced by the tool before anything
runs; `trusted-task` also offers built-in Read, Write and Edit, whose edits
are the harness's own, not Bash records. A trusted shell keeps the
requester's host rights: this is constructed configuration, not a sandbox
or absence certificate. Claude Code's managed settings still apply.

Each input gets an ascending `messageId` and a random SDK uuid; the ACK
waits for Claude Code's consumption echo (`--replay-user-messages`) or a
reply/result naming that uuid. Answers and turn ends are linked only
through Claude Code's explicit `user_message_uuid(s)` mapped back to the
receiver's ids. No echo within 120 s, Claude Code exiting, or an SDK
failure is a visible error notice and a bounded end of the harness (no
deadline-long wait, no replay); error results, unattributed results, a
different reported model and permission denials are visible notices and
`_claude_*` stop reasons. No CLI version gate: support is observed at
runtime. The caller works unchanged: `--route opus-medium` without any
credential option.

The offline controls use a stand-in for the Claude Code executable
(the crate's `tests/fixtures/fake-claude.mjs`); they check the receiver,
SDK wiring, owner contract and package path, not Claude Code, a login, a
subscription or a model. A real native Claude run remains ROOT's separate
witness.

## Credential and evidence scope

The explicit caller-selected Codex profile or OpenCode auth source is
read as the caller, checked again on the opened fd, and reduced to access,
expiry and optional account id. No refresh grant is sent, issuer POST,
login, probe or copyback occurs. Root reads no requester credential path.
Expiry is a local unverified declaration (decoded JWT exp or supplied
expires), not issuer attestation. Out-of-range expiry is refused.
Front-door freshness is checked after admission/cwd and just before
launch, and must cover N+2G+2+site margin. Caller margin defaults to 600 s;
the site-side check is authoritative for its actual grace/margin.

The staged access entry is root-private until setup-completed, then removed;
launch copies are removed after observed exit. A killed front door can
leave staged copies until the next same-user sweep. Access-only staging
intentionally does not put the access token in argv/env/capture.
**There is no output redactor.** Arbitrary task output, environment values,
diagnostics or model text can contain secrets. The native server password
is present in the work environment. Auth-at-close, default-plugin egress,
and ChatGPT subscription OAuth behavior remain unknown. The fixture's
provider uses an openai-compatible scripted key, not a real subscription.

Offline unittest controls use stand-in peers and explicit timer/fault seams.
The user-namespace fixture uses actual packaged runtime components and a
scripted loopback SSE model: namespace uid 0 is host nes, with an explicit
TRUSTED_OWNERS seam. Earlier 25 asserted rows plus one OBSERVED cancellation
row remain preserved history. New outside-file cleanup controls exercise
inner uid separation, without establishing host-root semantics. OBSERVED
is not an asserted pass; summary.passed merely excludes false assertions.
Native cancel may still need outer kill; C6/E1–E3, native owner deadlines,
store growth, tag trust and other carried native debts are not repaired.
No host install, real Sol task, wider Core/CRW cohort, cold import/loading,
security approval/efficacy, workflow cutover or migration completion is
established by this package construction.
