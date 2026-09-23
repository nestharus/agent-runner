# Manual selected completion recovery

An interrupted local caller can inspect an accepted `completion-continuation-v2`
event without its Bash handle, latest local receipt, synchronous response, or
mailbox row. This is a read-only local CLI path using normal filesystem access.
It does not activate a listener, notify an agent, acknowledge a mailbox item,
rerun a workload, or record physical drain.

```sh
oulipoly-agent-runner notify agent-bash-recovery-list --session-id SESSION_ID
oulipoly-agent-runner notify agent-bash-recovery-list
oulipoly-agent-runner notify agent-bash-recovery-list --session-id SESSION_ID --cursor NEXT_CURSOR
oulipoly-agent-runner notify agent-bash-recovery-read --event-id EVENT_ID --output /private/recovered.bin
oulipoly-agent-runner notify agent-bash-recovery-attempts --event-id EVENT_ID --cursor NEXT_ATTEMPT_CURSOR
```

List pages contain up to 100 **accepted** event identities, newest triggered
first. Pass the opaque `next_cursor` returned by a page to fetch the next page;
when it is null, that traversal is complete. A cursor is bound to the same
session filter and sidecar domain. A malformed, mismatched, or stale boundary
cursor fails explicitly; restart without `--cursor`. Each page is bounded to
101 rows and emits at most 100 identities. New acceptances at the head cannot
shift previously returned pages or skip records already accepted when the walk
started, provided their trigger sort keys remain unchanged. This is a keyset
walk, not a frozen SQLite snapshot: a later acceptance with a sort key above
the current cursor, or an administrator's timestamp repair, may require a
fresh walk. The direct `--event-id` read remains available when the event ID is
known.
Omitting `--session-id` searches all locally retained accepted events under
the same OS permissions. An event ID from a lost response may be used directly.
Read verifies the committed
event JSON against its content-addressed path, byte length and SHA-256. For a
raw artifact it also checks the selected descriptor against the Runner-owned
copy, then streams at most 1 GiB through a 64 KiB buffer and verifies exact EOF
and SHA-256. `--output` publishes only a verified copy and refuses to replace an
existing file. Omitting `--output` verifies and reports metadata without copying.

`selected_output.kind` distinguishes `raw_bytes`, `lossy_inline_legacy`, and
`selected_missing_output`. New Bash snapshots use raw artifacts for every size.
Older inline strings are preserved as text with a digest of their **retained
text**, but cannot recover original invalid UTF-8 bytes; `--output` rejects
them. A validated missing-original-output event has no selected body to copy.

`source_acceptance` is the committed source acceptance. `presentation_and_ack`
reports each registered listener's policy, mailbox placement, transport
evidence and ACK separately, including response-only listeners with no row.
`physical_drain` distinguishes the source's reported tree-drain flag from
Runner attempt phases and exact receipts. No receipt or `drained` phase is
inferred from acceptance, presentation, a missing Bash source,
or the manual read itself.

`physical_drain.attempts` is one page of exact attempt rows, not necessarily
the whole history. The read and each `agent-bash-recovery-attempts` call scan at
most 128 retained attempt keys; a linked association page may contain up to
128 exact rows. If `attempt_search_complete` is false, pass
`next_attempt_cursor` to the attempts command, even when `attempts` is empty.
Only a null cursor ends this traversal for the observed attempt generation.
If attempts, receipts, or source links change between pages, the next call
rejects the cursor as stale; restart with `agent-bash-recovery-read` and follow
its new cursor. The generation covers the sidecar, so an unrelated attempt
change can also require a restart. Even an already returned terminal page
describes its own read snapshot, not changes committed afterward. The
source-level
`association_completeness: "unknown"` remains unknown for pre-v25 sources even
after every retained attempt has been scanned: a deleted old claim cannot prove
which other sources shared an activation. A scalar source match is positive
evidence for that source; it is not used to assign the activation to another
source. Each page is one SQLite read snapshot; the durable generation check
prevents a changed attempt set from silently completing a multi-page walk.
The attempts command reads only sidecar metadata, so following
the cursor does not rehash or recopy the selected output artifact.

Accepted v2 event payloads remain protected from terminal payload reclamation
by the accepted-source reference. Runner-owned raw output copies have no
automatic day-30 deletion path. Open/unacknowledged completions therefore do
not silently expire at day 30. Closed-record retirement is a separate policy;
this command does not implement cleanup or delegated cross-session ACK.
