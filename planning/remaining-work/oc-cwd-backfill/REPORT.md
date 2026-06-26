# OpenCode cwd_script Backfill Report

No live `providers.toml` or live `opencode.db` files were modified. All database checks used read-only SQLite access or the read-only `opencode-cwd` adapter path.

## Live Config Snapshot

Current live values from `~/.config/oulipoly-agent-runner/providers.toml`:

```toml
[opencode.session_storage]
cwd_script = "/bin/false"

[opencode2.session_storage]
cwd_script = "opencode-cwd ~/.opencode2/opencode"

[opencode3.session_storage]
cwd_script = "/bin/false"

[opencode4.session_storage]
cwd_script = "/bin/false"

[opencode5.session_storage]
cwd_script = "/bin/false"
```

`opencode6` discrepancy: the live config has no `[opencode6]`, `[opencode6.session_storage]`, or `[opencode6.session_capture]` block. `~/.local/bin/opencode6` is also absent, and `~/.opencode6/opencode/opencode.db` does not exist.

## Per-Account Verification

| Account | Wrapper finding | DB dir | DB exists | Verified adapter command | Output |
|---|---|---:|---:|---|---|
| `opencode` | `~/.local/bin/opencode1` does not set `XDG_DATA_HOME`; default opencode data dir applies. | `~/.local/share/opencode` | yes | `opencode-cwd ~/.local/share/opencode ses_0f9d21fbfffe5lZrSNrkxRjiaT` | `{"found":true,"cwd":"/home/nes/projects/agent-runner/trunk"}` |
| `opencode2` | `~/.local/bin/opencode2` sets `XDG_DATA_HOME="$HOME/.opencode2"`; already fixed in live config. | `~/.opencode2/opencode` | yes | `opencode-cwd ~/.opencode2/opencode ses_0f9d25adcffeHGAp2BGgLLgxra` | `{"found":true,"cwd":"/home/nes/projects/agent-runner/trunk"}` |
| `opencode3` | `~/.local/bin/opencode3` sets `XDG_DATA_HOME="$HOME/.opencode3"`. | `~/.opencode3/opencode` | yes | `opencode-cwd ~/.opencode3/opencode ses_0f9d26b4affeff3526WQQxAqb2` | `{"found":true,"cwd":"/home/nes/projects/agent-runner/trunk"}` |
| `opencode4` | `~/.local/bin/opencode4` sets `XDG_DATA_HOME="$HOME/.opencode4"`. | `~/.opencode4/opencode` | yes | `opencode-cwd ~/.opencode4/opencode ses_0f9d23444ffe0AwyfK08Gik3BK` | `{"found":true,"cwd":"/home/nes/projects/agent-runner/trunk"}` |
| `opencode5` | `~/.local/bin/opencode5` sets `XDG_DATA_HOME="$HOME/.opencode5"`. | `~/.opencode5/opencode` | yes | `opencode-cwd ~/.opencode5/opencode ses_0f9d247c4ffeLAh5r1FF07W1W5` | `{"found":true,"cwd":"/home/nes/projects/agent-runner/trunk"}` |
| `opencode6` | no `~/.local/bin/opencode6` wrapper found. | `~/.opencode6/opencode` | no | `opencode-cwd ~/.opencode6/opencode __oulipoly_cwd_probe__` | `{"found":false}` |

The live `ses_1012` session id was not used.

## providers.toml Diff

This is the exact diff produced against a copy of the live config. No `opencode6` hunk exists because the live config has no `opencode6` provider block.

```diff
--- planning/remaining-work/oc-cwd-backfill/dry-run-before.toml
+++ planning/remaining-work/oc-cwd-backfill/dry-run-after.toml
@@ -473,7 +473,7 @@
 [opencode.session_storage]
 kind = "script"
 storage_type = "claude_code"
-cwd_script = "/bin/false"
+cwd_script = "opencode-cwd ~/.local/share/opencode"
 transcript_script = "/bin/false"
 
 [opencode2]
@@ -512,7 +512,7 @@
 [opencode3.session_storage]
 kind = "script"
 storage_type = "claude_code"
-cwd_script = "/bin/false"
+cwd_script = "opencode-cwd ~/.opencode3/opencode"
 transcript_script = "/bin/false"
 
 [opencode4]
@@ -533,7 +533,7 @@
 [opencode4.session_storage]
 kind = "script"
 storage_type = "claude_code"
-cwd_script = "/bin/false"
+cwd_script = "opencode-cwd ~/.opencode4/opencode"
 transcript_script = "/bin/false"
 
 [opencode5]
@@ -554,7 +554,7 @@
 [opencode5.session_storage]
 kind = "script"
 storage_type = "claude_code"
-cwd_script = "/bin/false"
+cwd_script = "opencode-cwd ~/.opencode5/opencode"
 transcript_script = "/bin/false"
 
 [opencode.session_capture]
```

## Apply Script

Saved as `planning/remaining-work/oc-cwd-backfill/apply-cwd-backfill.sh`.

The script defaults to `~/.config/oulipoly-agent-runner/providers.toml`, accepts an optional path argument for copy-testing, creates a timestamped `.bak`, edits a temp copy, applies only `cwd_script = "/bin/false"` substitutions inside the target `[*.session_storage]` blocks, refuses unexpected current values, refuses to apply a present target without its `opencode.db`, validates through `oulipoly-config` before install, then validates the installed file again.

Copy dry-run output:

```text
backup: planning/remaining-work/oc-cwd-backfill/dry-run-work.toml.20260626T231516Z.bak
updated: opencode
updated: opencode3
updated: opencode4
updated: opencode5
skipped: opencode6: session_storage block missing
validating edited temp file
providers.toml parsed: 22 provider entries
validating installed file
providers.toml parsed: 22 provider entries
cwd backfill complete
```

## opencode-cwd Diff

`scripts/opencode-cwd` is identical to the installed `~/.local/bin/opencode-cwd`:

```text
$ diff -u scripts/opencode-cwd ~/.local/bin/opencode-cwd
# no output; exit 0
```

No repo copy sync was needed.

## Test

Targeted adapter test passed:

```text
$ cargo test --manifest-path src-tauri/Cargo.toml --test cwd_scripts
running 4 tests
test cwd_scripts_unchanged ... ok
test claude_code_cwd_decodes_project_directory_name ... ok
test codex_cwd_reads_payload_cwd_from_rollout_first_line ... ok
test opencode_cwd_reads_directory_from_opencode_db ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

STATUS: DONE
