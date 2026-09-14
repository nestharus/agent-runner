# Sync MCP completion regression

Run `python3 src-tauri/tests/fixtures/age360/sync_mcp_e2e.py --help` for the
required explicit runner, Bash, Bun, Codex source, shared Bash source and evidence
paths. Linux user/mount/PID/network namespaces, Python 3.11+, GCC and mount are
required. No installed runner/Bash fallback is allowed. No real agents execute.

The external TypeScript files are copied byte-for-byte into the fixture; vendored
and shared Bash adapter hashes must agree. The tested runner and Bash executables
are copied explicitly and hashed. The caller must establish their build/source
correspondence; a hash alone does not do that. No product rebuild is performed.
Only the small passive process-entry audit shared library is compiled.

A fake native provider launches the actual Codex MCP bridge, submits an ordinary
controlled shell command through the actual shared adapter and receives JSON-RPC
responses. Sync compares the exact returned body, not a substring. Async waits
for the real runner's resumed fake recipient to receive the same-handle prompt,
read exact payload bytes and ACK its exact row. The native session-marker binding
is real; this is **not** the Codex host's live metadata handshake or embedded
provider-asset selection. No remote host-result ACK is required or inferred.

Every case uses fresh synthetic configuration and DBs. `/home`, `/root` and
`/run` are masked before starting runtime processes; environment is cleared;
private PID/network namespaces and teardown contain the controlled workloads.
The test does not inspect or copy production databases, sessions or providers.

Passive SQL audit triggers record listener insertion/active updates. Executable
entry instrumentation records PID, PPID, executable and argv, not environment
capabilities. These are test-only diagnostics, not product logging changes. A SQL
row update alone does not identify the executing Rust function; source mapping
and executable-entry evidence must be reported with that limitation.

Sync acceptance, exact inactive/unacknowledged/no-mailbox listener state and a
private guardian's driver replacement supply the no-notification oracle, rather
than a short quiet interval. This does not establish every crash/host-loss order.
A timeout is a failed/incomplete experiment, never proof of suppression. The
process-local manual-recovery policy retains unacknowledged debt intentionally.

Inspect `run.stdout`, `run.stderr`, `identities.json`, `isolation.json` and each
case's `mcp-wire.jsonl`, `mcp-result.txt`, `mcp.stderr`, `process-audit.tsv`,
`terminal.json`, native logs, source records and recipient receipt. Test fixture
failures and product-policy failures must be distinguished. Never patch product
behavior or weaken exact-body/state assertions just to obtain green.

## Independent-owner executable contrast

Add `--prior-runner PATH --prior-sha256 SHA256` to run four cases: fixed-owner
sync/async and prior-owner sync/async. Use explicitly identified local build
artifacts, not installed paths. Historical equality requires the exact recorded
hash; a pre-policy source rebuild must instead be labeled as a candidate.
The normal `--runner` remains the foreground/helper executable in every case;
`--agent-bash` remains the recovery executable. Each case founds a fresh private
domain with a held synthetic native context, then the fixed foreground joins that
same domain. Domains are fresh between cases, never production-derived.

Identical scheduling stops only the identity-checked private driver before new
source admission. The fixed helper accepts the actual new source and the MCP host
receives its response before the driver is continued. No listener is seeded and
no product row is rewritten. Founding context flags are set inside the fake
provider launch, not in the guardian environment, to avoid contaminating resumed
recipient session reads. No metadata-host contrast is selected here.

The audit library requires SQLite and OpenSSL development headers/library;
contrast mode additionally requires `nm`. The harness discovers
`sqlite3_open`/`sqlite3_auto_extension` offsets in each copied executable, then its
private preload constructor calls SQLite's auto-extension registration API through
that exact mapped image. It does **not** patch executable bytes, intercept/replace
product SQL, or require a product build. Unavailable symbols are an experiment
gap, not authority to modify product instrumentation. The extension registers an
innocuous PID scalar used only by test audit triggers and a SQL PROFILE callback.
Both compared binaries receive identical instrumentation; its timing overhead is
a limitation, not an uninstrumented production observation.

`sql-audit.tsv` contains completed relevant SQL with PID/connection/error/transaction
state. `sql-audit.tsv.writers` records actual `/proc/self/exe` hashes (including
forked drivers and sealed helpers). Audit trigger rows join previous/new active
state to writer PID and current domain generation. `repair-barrier.json` requires
an event-specific activation SQL PROFILE in the independently identified driver
and a subsequent successful COMMIT on the same connection, after fixed-helper
acceptance. This observes triggered-event repair even when fixed policy updates
zero rows; replacement or silence cannot satisfy it. It establishes that repair
transaction, not all future driver work or a stack trace of every Rust function.

A duplicate is `DUPLICATE_RED` only with exact sync MCP output, actual same-handle
recipient notification/output/ACK, and the 0→1 transition attributed to the owner
driver. `duplicate-proof.json` preserves this join. Async still requires actual
recipient delivery. Exit 0 means no duplicate observed with completed assertions;
exit 2 means the duplicate was observed; exit 1 means an incomplete/failed harness
assertion. All four cases execute so an expected historical red cannot hide a
failed async control. A fixture red does not attest any production activation
writer or grant fix/deployment authority.

## Owner-upgrade conditional operational search

`--coordinated-idle-replacement` (requires both prior flags) runs all four
contrast cases, but releases the sole founding native context **before** the
fixed foreground can admit work. It probes the existing six-byte `drain\n`
request (unsupported: EOF), then uses normal fixture-provider exit, not a
signal or product-row rewrite. The private PID-namespace controller collects
an actual successful terminal wait of the original guardian. The new owner
must preserve the domain, change generation and run the fixed executable.
Normal sync exact-byte/no-notification and async recipient/ACK oracles remain.
`coordinated-retirement.json` records the operation and original owner.

This is a conditional deployment experiment with an explicitly **empty**
completion inventory and an externally closed admission population (one fake
entry). It is not an implemented domain drain command, active-work transfer,
a production admission fence, or a fix for inactive sync debt. No forced owner
kill is part of this mode. Observation time bounds fail the experiment; they do
not authorize replacement. The historical mixed-owner schedule remains
available without the new flag, including its exit-2 duplicate signal.

Copy provenance: `planning/age353-queue/resume-post-outage/owner-upgrade-search-evidence/provenance.md`
in the machine-local project planning tree, not this repository's planning tree.

## Qualified settlement candidate (sidecar 19)

For the fixed-only sync case, the fixture now captures the guardian before the
native caller exits and collects that exact guardian's successful terminal wait.
Accepted response-only notification settlement permits normal retirement without
ACK; retained source/output and zero transient activation remain required. This
is normal candidate-owner retirement, not a populated old-image cutover. The
older-schema contrast/replacement branches remain historical controls and must
not be run as a migration beneath old writers.

The synthetic async MCP host stays open until original snapshot publication.
Closing it immediately after the async handle reply can cancel still-running
work; that is a different host-loss experiment, not the exact-output async
control. Exact recipient bytes and genuine sequence ACK are still required.
