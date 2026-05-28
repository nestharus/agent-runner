# spec-direct-model-cli - Direct model CLI dispatch

Declared roles: validator, formatter, accessor, mapper

## Intrinsic-surface declarations

```yaml
intrinsic_surface_declarations:
  - component: planning/coverage/spec-direct-model-cli.md
    role: intrinsic-surface
    Domain: direct_model_cli_coverage_spec
    Owns:
      - coverage-spec anchoring for direct model CLI dispatch
      - prompt-resolution pre-invocation failure expectations
      - direct-model prompt construction edge-case inventory
      - declared test-pattern references for the direct-model CLI surface
adapter_declarations:
  - component: planning/coverage/spec-direct-model-cli.md
    role: adapter
    Translates:
      - planning/coverage fixed-string source-file discovery -> direct-model CLI source ownership
      - direct-model prompt-resolution behavior contract -> declared test-pattern citations
```

## Source files

- `src-tauri/src/commands/direct_model.rs`

## Preconditions

- The `oulipoly-agent-runner` binary is built.
- Provider and model config loading can resolve the requested direct
  model name before prompt construction.
- Prompt input is supplied by positional args, `--file`, stdin, or
  `--agent-file` formatting.

## Input -> Expected output

| Input situation | Expected output |
|-----------------|-----------------|
| Direct `--model <name>` loads context, resolves the model, and receives a valid prompt. | Dispatch to provider balancing with the resolved `ModelConfig`, prompt, model map, working dir, and typed extra inputs. |
| Direct `--model <name>` cannot load provider/model context or cannot find the model. | Emit exactly one `OULIPOLY_FAILURE=` pre-invocation marker at `stage="provider_selection"` and return the original error. |
| Direct `--model <name>` prompt construction fails after model lookup and before balancing. | Emit exactly one stdout `OULIPOLY_FAILURE=` pre-invocation marker at `stage="prompt_resolution"`, preserve the original stderr `Error: ...`, emit no `OULIPOLY_RESULT`, and do not start provider balancing/execution. |
| Named-agent CLI resolution succeeds. | Resolve the named agent, look up its model, format the prompt with typed CLI inputs, and dispatch through the same balancing helper path. |

## Edge cases

- Unknown direct model names remain provider-selection failures.
- Unreadable `--file`, empty stdin, and unreadable direct `--agent-file`
  are prompt-resolution failures because the model is already known and
  balancing has not started.
- Direct `--agent-file` keeps loading through `load_agent_file` before
  formatting the raw prompt.
- Named-agent prompt construction uses typed input formatting and does
  not emit direct-model pre-invocation markers.

## Error conditions

- `Unknown model: {name}` - direct model lookup failed.
- `Unknown model for agent {agent.name}: {agent.model}` - named-agent
  model lookup failed.
- `Failed to read prompt file: {e}` - direct prompt file loading failed.
- `Empty prompt from stdin.` - direct stdin prompt resolved to empty.
- `Failed to read agent file {path}: {e}` - direct `--agent-file`
  loading failed.

## Boundaries

- Direct-model prompt-resolution markers do not synthesize invocation,
  provider, provider-session, chain, provider-index,
  attempted-provider, or result-envelope identity.
- This surface does not own provider process execution, terminal signal
  recognition, quota refresh, or session persistence.
- This surface does not own the named-agent config repository beyond
  invoking the configured repository adapter.

## Declared test patterns

- `src-tauri/tests/age181_prompt_resolution_failure.rs`
- `src-tauri/tests/age33_config_state_characterization.rs`
  (`age_33_deferred_one_shot_agent_file_site_remains_direct_loader_call`)
