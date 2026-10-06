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
which only `bin/node` and `LICENSE` are staged. The locked musl Claude
Code platform package, never selected by the receiver, is not staged
(MANIFEST `claude_deps_not_staged`). The Claude Code and Node
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
optional inline access-only credential, `children`, separate Claude-parent
`child_credential`, and `retention` (`discard`, or
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
It collects at most N+65 s overall. Close waits for owed background
completion turns within that same deadline; its 65 s stop allowance starts
when that debt settles. Cancel retains its 65 s allowance,
with 30 s from terminal/partial EOF, plus a 0.2 s child exit attempt.
Lines are limited to 8 MiB and capture to 64 MiB; exceeding them gives
incomplete/unknown. Closing pipes requests abandonment; terminating its
own helper cannot prove a privileged namespace ended. It returns without
an unbounded wait. These limits cover normal programmatic I/O, not kernel
or filesystem stalls. The front door's site grace can exceed the caller's
collection allowance; then unknown is an intentional outcome.

Output directory (0700, must be new): `request.public.json` (credential
field reduced to provider/expiry), `events.jsonl`, `stderr.log`,
`caller.jsonl`, `final.md` when parent-linked text exists, `result.json`,
and `children.json` (also written as an empty inventory on no-child calls).
The final answer and automatic close ignore child-marked events. Bash
counts combine parent and child Bash. Child records are owner exports,
not parent consumption: the local parent result can precede owner export
while tracked Bash drains. Zero exported results does not mean zero local
parent results. Terminal children summaries cover the owner generation,
not a complete recovered history.
Type-only unexpected failures are machine-readable; if the directory
itself is unwritable, stderr supplies the class instead of promising a
result file that cannot be written.

Caller codes: 0 answered, 1 no-answer, 2 usage, 3 local refusal,
4 front-door refusal, 5 cancelled, 6 incomplete/stop unknown,
7 cleanup failed, 8 launch failed, 9 ended otherwise, 10 async-undelivered.
For tasks with accepted background work, `final.md` preserves ordered linked
turn text (including diagnostic text on failure); `result.json` retains the
report and owner-terminal async accounts and any gaps or inconsistencies.
`answer.present` covers task-linked text on async calls; `answer.linked_messages`
and `answer.turn_end` retain the initial-turn diagnostics. `turns` carries the
wider per-turn account, including silent or unfinished turns.
`answered` requires an eventual linked answer, the initial end_turn and every
owed completion's acknowledged tagged end_turn, a fully settled valid owner
account, complete transport and clean retirement. A silent completion turn
alone is not a failure, but no linked text anywhere remains no-answer;
undelivered or unknown debt cannot become success from text alone. Ordinary
non-background tasks keep their first-turn answer contract.
`answered` **does not prove task completion or correctness**. A denial
or tool echo can be answered/0; entry 87 means close followed through.

### Live roots (explicit, bounded)

The fresh per-call behavior above is unchanged and remains the default. A live
root is only opened on explicit request. In that case one front door process
stays the root's owning supervisor after the call that opened it has exited.
Later, independent calls by the same requester can then address that root.

- **Open.** Run `oulipoly-native-call ... --live-handle FILE` with the ordinary
  options. This sends request `live: true` through the same sudoers `run` rule.
  - Admission, routes, credentials, children and the namespace entry are the
    same as for a fresh call.
  - After admission the front door forks its supervisor into a new session.
    The supervisor closes the caller's stdio, unshares, starts the entry and
    reports readiness.
  - The front door then answers with terminal stage `live-opened`, which
    carries a handle (`run`, socket, random token, uid), and exits 0. This exit
    is **not** the root's end.
  - The caller writes the handle to a new file (mode 0600), attaches, waits for
    input 0's own tagged turn, writes `final.md` and detaches. It never sends
    the automatic close.
- **Address.** Run `oulipoly-native-call --root FILE --prompt-file P --out DIR
  [--wait S]`.
  - The caller connects to the root's `live.sock` in the root-owned run
    directory. The socket is owned by the requester, mode 0600, and the
    supervisor checks every peer's uid with `SO_PEERCRED`.
  - The caller proves the run and token, then sends `send` with a fresh `ref`.
    It correlates its own input in this order: `follow-up-admitted` (same
    `ref`, giving the input index), then `ack`, then linked `agent-message`,
    then that input's `turn-end`.
  - The caller then detaches. `--wait` bounds this call only; the root is
    unaffected by it.
  - One caller is attached at a time; a concurrent one is refused with `busy`.
    While nobody is attached, records wait in a bounded backlog (8 MiB).
    Overflow is dropped and counted (`backlog_dropped`), never silently.
    Each attachment begins on a whole JSON record. If a caller leaves during
    a partial send, that entire interrupted record is discarded and counted;
    whole unsent records remain queued. Attach and terminal records expose
    cumulative dropped records/bytes, interrupted records and prefix bytes
    sent to departed callers. Counts describe transport loss, not processing.
  - `answered`/0 covers only this input's correlated ACK, linked text and
    tagged `end_turn`; it establishes neither semantic processing nor
    settlement of the whole root or its background obligations.
- **Close or cancel.** Run `--root FILE --close` (or `--cancel`). This has the
  per-call close/cancel meaning, including deferral for owed background
  completions and the kill grace.
  - The supervisor stops listening and unlinks the socket. It retires the run
    as for a fresh call, sends the terminal record (`live` attaches,
    refusals, drops) to the attached closer, and exits with the entry's code.
  - Physical close and the caller's outcome are separate. `closed`/0 requires
    clean retirement, collected EOF, a complete entry relay terminal and a
    consistent owner terminal async account, with all accepted completion
    inputs covered by ACK/tagged turn ends and no counted transport loss.
    It establishes no semantic processing or task correctness.
  - The live supervisor keeps a runtime-only accounting witness across
    attachments (parent async reports/admissions, ACKs, turn ends and owner/
    entry terminals, without answer bodies). It is bounded at 8 MiB and
    exported in the final front-door terminal as `account_events`;
    `account_errors`, the reconciled `async`, and raw terminals remain visible.
    Missing, contradictory or limit-truncated evidence is `incomplete`/6.
    Known undelivered completions are `async-undelivered`/10, even after clean
    physical retirement. Counted transport loss also precludes `closed`/0.
  - Precedence: observed cleanup failure/7, then incomplete transport/stop
    collection/6, then explicit cancellation or owner kill/deadline/5, then
    accounting incompleteness/6, known async loss/10, and transport loss/6.
    Other owner exits remain `ended-otherwise`/9.
- **Bounds.**
  - `deadline_s` (site maximum) bounds the whole live root, not one call.
    When it expires the root is cancelled, then killed, even if nobody is
    attached.
  - At most 4 live roots per requester are allowed, including admissions
    still in progress before socket creation. A per-requester nonblocking
    admission guard covers count/allocation; the allocated run's locked
    reservation holds its slot until retirement. A competing guard holder
    is refused explicitly (`requester admission busy`); the cap refusal is
    `4 already held`. Dead ones (lock free) are not counted and are swept by
    the next run.
  - The supervisor holds the package's shared lock and the run lock for the
    root's life.
- **Outcomes (caller exits).**

  | Exit | Class | Meaning |
  |---|---|---|
  | 11 | `root-absent` | No address: the root ended and was retired, or never existed. |
  | 12 | `root-dead` | The address is present but nothing is listening; the supervisor died, and the lock is free for the sweep. |
  | 13 | `root-foreign` | The address is not accessible, or the peer is another uid. |
  | 14 | `root-refused` | The token or run is wrong (`handle-token`, `not-this-root`), or the reason is `busy` or `hello-timeout`. |
  | 15 | `follow-up-refused` | The owner refused the input (for example `input-closed`). |
  | 16 | `root-ended` | The root ended before this input's turn did. |
  | 17 | `root-unavailable` | Address/transport could not be used; no root state is inferred. |
  | 3 | `refused-locally` | Malformed/unusable handle (including overlong AF_UNIX address), or another uid. |

  `root-absent` after an unattended deadline or retirement is deliberately
  ambiguous: no terminal outcome can be inferred, including success or
  failure. There is no terminal history, automatic fallback, replay or restart.
  Failure before attachment uses the declared class and stores `result.json`
  in the caller output directory; an unwritable/uncreatable output directory
  can only expose its machine-readable refusal on stderr.
- **Not provided.**
  - Stopped-root delivery, restart, recovery and resume.
  - Dedup across owners.
  - Cross-supervisor messaging.
  - A global daemon.
  - Generic descendants.
  - The ordinary `agents` entry and the temporary dispatcher do not open live
    roots.

`test_live.py` contains unprivileged stand-in controls for separate callers,
constructor-initialized record framing/loss, the actual concurrent admission
boundary and in-progress reservations, malformed public handles, and honest
async close outcomes. The killed-supervisor stand-in did not itself die from
PDEATHSIG and is explicitly killed by the control: it supplies no containment
proof. Real owner/provider, privileged namespace/sudo/session policy and
package replacement while a live root holds its lock remain ROOT qualification.

## Ordinary `agents` entry (`native.toml`)

Ordinary Linux CLI launch requires `<runner config root>/native.toml`
(normally `~/.config/oulipoly-agent-runner/native.toml`). With valid
configuration, the launch forms
`agents -m MODEL PROMPT`, `agents AGENT PROMPT` and `--agent-file` run as one
call of the configured installed caller, before any legacy
maintenance/owner/State/provider path. The selected model name, from `-m` or
from the agent's frontmatter `model:`, must be mapped explicitly; the site
route then fixes provider, model and effort:

```toml
caller = "/opt/oulipoly-native/<id>/bin/oulipoly-native-call"
runs_dir = "/home/nes/.local/state/oulipoly-native-runs"   # prompt files + caller records
deadline_s = 1800                                            # optional; 1..7200

[models."codex~high"]
route = "sol-high"
bash = "trusted-task"                    # or bash_allow = ["whole command", ...]
credential_codex_profile = "/home/nes/.codex4"
children = ["luna-max"]                  # optional explicit explorer opt-in
child_max_starts = 1

[models."claude~medium"]
route = "opus-medium"
bash = "trusted-task"
```

The final answer goes to stdout; a status line (class, caller and front-door
exits, record directory) goes to stderr. Answer read/write/flush failures are
also reported there; after caller success they return entry exit 6, while a
nonzero caller exit is preserved. Caller 0/answered is not correctness.
Unmapped models, a missing/unreadable/invalid file, `--resume`, `repl`, `resume`, `--new`,
provider pinning/rotation and `-i`
with `-m` are refused with exit 3 and nothing launched: no legacy launch and
no substitute model. An unresolved config root is also refused with exit 3.
Missing configuration never falls through to the legacy owner/State path.
Help/usage, non-launch subcommands and GUI retain their separate paths; this
launch closure does not retire those or the wider legacy source.
The credential is the explicitly named profile only: no rotation, lease or
renewal (the caller
refuses a stale one). One attempt, synchronous, no streaming or resume yet.

## Native Claude routes

A route with `"harness": "claude"` names `model`, `effort`
(`low|medium|high|xhigh|max`), `config_dir` and `"credential": "none"`, and
may offer named `children`. The example site config carries `opus-medium` and
`opus-high` (`claude-opus-5-5`, `.claude5`), with `luna-max` offered on
these and `sol-high`.
`config_dir` is relative to the requester's passwd home (no absolute path,
no `..`): the requester's own Claude configuration directory. The front
door, runner and owner only name it; nothing of root reads it, and no
parent credential is accepted, staged or removed for these routes.
Opted-in explorers have a separate child-provider grant, described below.

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
runtime. For no-child calls, use `--route opus-medium` without a credential
option.

The offline controls use a stand-in for the Claude Code executable
(the crate's `tests/fixtures/fake-claude.mjs`); they check the receiver,
SDK wiring, owner contract and package path, not Claude Code, a login, a
subscription or a model. A real native Claude run remains ROOT's separate
witness.

## Registered orientation explorers (explicit opt-in)

The site owns `child_routes`, each parent route's `children` offer list,
and `child_limits` (at most **4 starts / 2 funded concurrent**, depth 1).
The requester opts in with `--child-route NAME` (repeatable), optionally
lowering `--child-max-starts` and `--child-max-concurrent`. A request can
name only offered routes; it cannot select models, providers, paths or
credentials' locations for the owner. A child never offers children.
The root owns its children's lifetime; close, cancel, requester loss,
parent-work end and the outer deadline stop them, without replay.

`frontdoor.example.json` offers `luna-max`: **openai/gpt-6-luna** with
model `options.reasoningEffort: "max"`, `reasoningSummary: "auto"`,
`include: ["reasoning.encrypted_content"]` and `store: false`, using
`@ai-sdk/openai` Responses rather than Chat Completions. The provider's
[model page](https://developers.openai.com/api/docs/models/gpt-6-luna)
lists this model id and `max`; tool use at non-`none` effort requires
Responses. The locked OpenCode 1.18.30
[provider](https://github.com/anomalyco/opencode/blob/v1.18.30/packages/opencode/src/provider/provider.ts)
selects Responses, merges model options, and its
[transform](https://github.com/anomalyco/opencode/blob/v1.18.30/packages/opencode/src/provider/transform.ts)
forwards explicit reasoning options with `forceReasoning`. Its generated
OpenAI variants do not enumerate `max`: this example uses explicit model
options, not variant inference. OpenCode's bundled `@ai-sdk/openai` 3.0.88
accepts a string effort and serializes it as `reasoning.effort`.
The built-in [Codex plugin](https://github.com/anomalyco/opencode/blob/v1.18.30/packages/opencode/src/plugin/openai/codex.ts)
admits GPT major versions above 5 and redirects OAuth Responses to its
Codex endpoint. These are public configuration/source facts, **not** a
runtime witness of Luna, effort, subscription or account-limit support.
First ordinary opted-in use after ROOT's paired deployment is that witness;
no probe, forced renewal, substituted model or fallback is implied.

An OpenCode/Sol parent reuses its admitted matching-provider access-only
grant. A Claude parent opts in with a separate explicit
`--child-credential-codex-profile PROFILE`, or
`--child-credential-opencode-auth AUTH --child-credential-provider openai`.
Claude's own login is not read by the caller/front door for this grant.
No-child Claude calls stage no child grant. The grant is snapshotted once
into root-private `private/child-auth.json` and copied per child launch;
it is retained past parent setup-completed for later admissions. A child
grant that ages during admission can leave setup files before its final
freshness refusal; local freshness is not issuer or account validation.

After ROOT's review and installation, example caller opt-ins are:

```bash
<package>/bin/oulipoly-native-call --route sol-high --prompt-file TASK \
  --cwd TREE --out NEW-DIR --trusted-task --deadline 1800 \
  --credential-codex-profile EXPLICIT-PROFILE --child-route luna-max

<package>/bin/oulipoly-native-call --route opus-medium --prompt-file TASK \
  --cwd TREE --out NEW-DIR --trusted-task --deadline 1800 \
  --child-route luna-max --child-credential-codex-profile EXPLICIT-PROFILE
```

The parent's `explore` / `mcp__oulipoly__explore` tool returns orientation
and separate lifecycle text. `answered` is content only. Known child end
and Bash drain are separate observations. Unknown possible child/Bash ends
stay charged until positive end (untracked unknowns conservatively for the
owner's remaining life); tracked Bash holds the slot through its waiter's
end. Even the ordinary local result says **release pending** while still
live, rather than promising an immediately available slot. A stalled Bash
waiter can hold the slot and owner export until the root's outer backstop;
no local drain timer or unbounded-progress guarantee is supplied. A setup
refusal starts no child process but may leave a shared base or partial
files; an exec failure can follow a positively started work. Neither
permission configuration nor the read-only brief is an arbitrary-shell
write barrier.

The offline package fixture invokes the real OpenCode custom-tool loader
and the published Claude SDK with a fake executable and scripted loopback
provider on both parent kinds. The separate published-SDK MCP control
checks explore registration/invocation and abort closing the ingress.
To run it without adding dependencies to the product worktree, copy
`crates/oulipoly-root-supervisor/native/` into a B-owned source overlay,
copy its `explore-client.mjs` beside `claude/acp-v2-receiver.mjs` (the actual
provisioned layout), link the published Claude deps' `node_modules` at the
overlay's `native/node_modules`, and run the packaged Node as:
`OULIPOLY_CLAUDE_RECEIVER_NO_MAIN=1 <node> --test <overlay>/native/test/claude-explore-sdk.test.mjs`.

Their captures must be collected for the candidate: source/control
existence alone is not execution or supported activation. ROOT separately
owns installed proc/identity readback and AI caller pin/opt-in/site rollout.

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

The parent staged access entry is root-private until setup-completed, then
removed;
the child snapshot stays for later admissions until normal retirement;
normal-path launch copies are removed after known entry end when cleanup
succeeds. Unknown entry stop skips retirement; abrupt front-door loss can
leave staged and derived copies until the next same-user stale-run sweep.
The access-only expiry window is accepted residue exposure, not a universal
deletion or provider-validity guarantee. Access-only staging
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
