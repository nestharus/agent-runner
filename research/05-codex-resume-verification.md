# Codex `experimental_resume` Verification

Date: 2026-04-30

## Verdict

`NOT_FOUND`

`experimental_resume` is not documented and is not present in the Codex CLI source at the installed version (`codex-cli 0.125.0`) or in fetched `openai/codex` `main`. Codex does have an internal app-server resume-by-path field (`ThreadResumeParams.path`), but the installed CLI does not expose it as `-c experimental_resume=...`.

## Installed CLI Evidence

- `which codex`: `/home/nes/.npm-global/bin/codex`
- `codex --version`: `codex-cli 0.125.0`
- `codex --help | rg -i "resume|config|-c"` shows only generic `-c, --config <key=value>` and the documented `resume` subcommand.
- `codex resume --help`: documents `codex resume [OPTIONS] [SESSION_ID] [PROMPT]`, `--last`, `--all`, `--include-non-interactive`, and generic `-c`; no path option.
- `codex exec resume --help`: documents `codex exec resume [OPTIONS] [SESSION_ID] [PROMPT]`, `--last`, `--all`, and generic `-c`; no path option.
- `codex help config`: exits 2 with `error: unrecognized subcommand 'config'`.

Unknown config keys are tolerated: `codex -c definitely_not_a_real_codex_key=foo --version` prints `codex-cli 0.125.0` and exits 0. `codex -c definitely_not_a_real_codex_key=foo features list` and `codex -c experimental_resume='"/tmp/no-such-rollout.jsonl"' features list` also exit 0. That proves the parser accepts arbitrary `-c` keys; it does not prove any code reads them.

## Rollout Probe

Recent rollout: `/home/nes/.codex/sessions/2026/04/23/rollout-2026-04-23T16-51-20-019dbcc1-54de-70e1-ac83-167be034498a.jsonl`. The first `session_meta` line contains `"id":"019dbcc1-54de-70e1-ac83-167be034498a"`. The Codex state DB also has a matching `threads` row with `source=exec`, `cwd=/home/nes/projects/agent-runner`, and `model=gpt-5.5-high`.

Controlled interactive probe, with no prompt sent:

```bash
TERM=xterm codex --no-alt-screen \
  -c experimental_resume='"/home/nes/.codex/sessions/2026/04/23/rollout-2026-04-23T16-51-20-019dbcc1-54de-70e1-ac83-167be034498a.jsonl"' \
  resume
```

Observed behavior: Codex opened the normal resume picker. Typing `q` entered `q` into the picker search field and showed `No results for your search`, then I aborted with Ctrl-C. This is strong evidence that `experimental_resume` is ignored by the interactive CLI path.

`TERM=xterm codex --no-alt-screen resume 019dbcc1-54de-70e1-ac83-167be034498a` was also started without a prompt and aborted before any LLM call. It reported `ERROR: No saved session found with ID ...`. That appears to be because this rollout is an `exec` source while `codex resume` is the interactive surface; the non-interactive surface is `codex exec resume <SESSION_ID> [PROMPT]`. I did not run that with a prompt because it would start an LLM call.

## Source Evidence

Source checked from `openai/codex` tag `rust-v0.125.0`, commit `637f7dd6d737f3961e6bf32fbb3861c4953269c5`. The installed npm package is only a JS shim plus native binary.

```text
rg -n "experimental_resume" /tmp/codex-src
exit=1
```

The same search against fetched `origin/main` returned no matches.

Relevant source refs:

- `codex-rs/cli/src/main.rs:278`, `:281`, `:285`, `:289`, `:293`: `ResumeCommand` accepts session id plus `--last`, `--all`, `--include-non-interactive`; no path/config resume field.
- `codex-rs/cli/src/main.rs:1569`, `:1581`, `:1583`: TUI resume is built from picker/session-id state only.
- `codex-rs/utils/cli/src/config_override.rs:19`, `:29`, `:42`, `:74`: generic `-c` parsing accepts keys without validating that the destination field exists.
- `codex-rs/config/src/config_toml.rs:71`, `:408-411`: `ConfigToml` has other legacy experimental fields but no `experimental_resume`.
- `codex-rs/app-server-protocol/src/protocol/v2.rs:3445-3468`: internal app-server protocol defines unstable `path: Option<PathBuf>` for resume-by-path.
- `codex-rs/tui/src/app_server_session.rs:1103-1125`: TUI resume builds `ThreadResumeParams` and leaves `path` unset.
- `codex-rs/exec/src/cli.rs:141`, `:154`, `:177`: `codex exec resume [SESSION_ID] [PROMPT]`.
- `codex-rs/exec/src/lib.rs:1275`, `:1322`, `:1340`: exec resume resolves by UUID or metadata lookup, not by config key.

## Docs And Discussion

- CLI reference: https://developers.openai.com/codex/cli/reference
- Config reference: https://developers.openai.com/codex/config-reference
- GitHub discussion: https://github.com/openai/codex/discussions/3827

The CLI reference says it catalogs documented commands/flags and describes generic `-c key=value` (`L575-L579`). It documents interactive resume as `codex resume` with `--all`, `--last`, and `SESSION_ID` only (`L2189-L2238`). It documents exec resume as `codex exec resume [SESSION_ID]` with `--all`, `--last`, `PROMPT`, and `SESSION_ID` only (`L1790-L1823`).

The config reference has no match for `experimental_resume`.

Discussion #3827 asks whether `codex exec` can specify a rollout filename or session id (`L203-L208`), then proposes a `-c session_filename_path=...` style mechanism (`L269-L276`). The reply says Codex CLI does not support manually naming rollout files or setting custom session ids (`L297-L305`). The discussion does not mention `experimental_resume`.

## Implications For Proposal Section 7

Do not ship this Codex provider block:

```toml
[providers.resume]
kind        = "config"
config_key  = "experimental_resume"
argument    = "absolute_path"
```

The live CLI accepts unknown `-c` keys without error, so argv-shape tests would pass while the real CLI ignores the key. Codex migration must use a supported UUID resume surface:

```bash
codex exec resume <UUID> [PROMPT]
codex resume <UUID>
```

For migration, copy the rollout to the target `$CODEX_HOME` canonical `sessions/YYYY/MM/DD/` path preserving the rollout UUID, then ensure the target Codex home can resolve that UUID. The deterministic choices are: trigger/verify Codex's scan-and-repair path, or explicitly upsert the target `state_5.sqlite` `threads` row. The SQLite upsert is more reliable but couples agent-runner to Codex private schema.

## Recommended Proposal Edit

Replace §7 title with `Codex resume uses supported UUID subcommand`. Delete `ResumeStrategyKind::Config`, `ConfigArgument`, `config_key`, and `argument` from the proposed `ResumeConfig`.

Replace the Codex example with:

```toml
[providers.resume]
kind       = "subcommand"
subcommand = ["exec", "resume"]
argument   = "session_id"
```

Add to §6:

```text
For Codex migrations, copy the rollout to the target CODEX_HOME canonical sessions path preserving the rollout UUID. Before spawning Codex, ensure the target Codex state can resolve that UUID. Do not use `-c experimental_resume`; Codex 0.125.0 ignores that key.
```

Update A7 and Q3 to say Codex rollout migration depends on supported UUID resume (`codex exec resume <UUID>` / `codex resume <UUID>`) plus target home/index visibility, not `experimental_resume`.

## Experiments Not Run

No question artifact was emitted. The decisive probes did not send a prompt. A stronger smoke test would run `CODEX_HOME=<target-home> codex exec resume <UUID> "minimal follow-up"`; that would start an LLM call, so it was intentionally not run.
