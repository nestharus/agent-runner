# AGE-142 Provider-Termination Eval

This eval is the WRITE-state contract for normalizing provider child-process termination evidence into the AGE-139 terminal-signal vocabulary, fixture manifest rows, and the future line-oriented runner marker. It is an adapter document: the AGE-139 DTO and recognizer vocabulary remain the source contracts, while this eval owns only the provider-termination fixture corpus and the reader-facing translation shape for AGE-143/W5.

## Declared roles

Declared role set: `orchestration`, `validator`, `formatter`.

`orchestration` covers sequencing the AGE-139 DTO contract, provider recognizer contract, marker contract, and W5 reader interface into one eval surface. `validator` covers the fixture manifest and verifier expectations that rows are well-formed, resolvable, and classified into the allowed kind set. `formatter` covers the Markdown, YAML, and marker payload shapes exposed to reviewers and future tooling.

These are function-classification declared roles from `code-quality.md`. They are distinct from the coupling-role declarations below: `adapter` describes the cross-contract translation carrier, and `intrinsic-surface` describes ownership of the fixture corpus domain.

## Adapter declarations

```yaml
adapter_declarations:
  - component: evals/agent-runner-provider-termination/eval.md
    role: adapter
    Translates:
      - age-139-terminal-signal-dto-contract
      - age-139-provider-recognizer-contract
      - age-139-provider-vocabulary
      - oulipoly-terminal-signal-marker-contract
      - age-143-w5-reader-interface-contract
```

Every external reference in this eval is subordinate to one of those five `Translates:` contracts: AGE-139 DTO/status/evidence symbols, AGE-139 provider recognizer modules, the AGE-139 provider vocabulary owner, the marker payload contract, or the W5 reader interface contract.

## Intrinsic-surface declarations

```yaml
intrinsic_surface_declarations:
  - component: evals/agent-runner-provider-termination/fixtures/
    role: intrinsic-surface
    Domain: provider-termination-fixture-corpus
    Owns:
      - MANIFEST.yaml
      - fixture_bytes_path
      - sentinel_metadata_path
      - manifest-row-schema
      - bounded-evidence-excerpt-policy
      - fixture-provenance
      - per-fixture-expected-terminal-signal-kind
      - per-fixture-expected-marker-kind-label
```

The `provider-termination-fixture-corpus` domain owns the concrete manifest rows, relative raw-byte paths, sentinel metadata paths, bounded opaque evidence policy, fixture provenance, and expected terminal-signal and marker labels.

### AGE-139 reference contract

This eval references the AGE-139 terminal-signal DTO contract without redefining it.

Allowed DTO and trait symbols: `TerminalSignal`, `TerminalSignalKind`, `TerminalSignalEvidence`, `TerminalStatusEvidence`, and `TerminalSignalRecognizer`.

Allowed `TerminalSignalKind` variants: `CleanExit`, `NonzeroExit`, `SignalExit`, `SpawnError`, `QuotaExhaustedInband`, `ProlongedSilence`, and `Unknown`.

Allowed `TerminalStatusEvidence` variants and fields: `Exited { code: i32 }`, `SignalTerminated { signal: i32 }`, `SpawnError { reason: String }`, `ProlongedSilence { reason: String }`, and `Unknown`.

Allowed `TerminalSignalEvidence` fields: `provider_name`, `stdout`, `stderr`, `terminal_status`, and `observed_at`.

Allowed `TerminalSignal` fields: `kind`, `provider_name`, `evidence`, and `observed_at`.

Allowed precedence concepts: structured `SpawnError`, `ProlongedSilence`, and `SignalExit` are stronger than quota-token evidence; provider quota-token evidence may produce `QuotaExhaustedInband` only when no stronger structured status applies; `Exited { code: 0 }` maps to `CleanExit`; `Exited { code != 0 }` maps to `NonzeroExit`; incomplete or non-matching evidence maps to `Unknown`.

Allowed evidence-size concept: evidence excerpts are bounded opaque strings derived from provider bytes or structured terminal-status reasons. This eval cites bounded excerpt behavior and does not parse private provider transcript fields outside `TerminalSignalEvidence` and the fixture manifest metadata.

`network_error` is adjacent diagnostics vocabulary, not a terminal-signal kind. Network failures fall into `Unknown` or `NonzeroExit` according to terminal status and evidence, and the allowed kind set above remains the complete terminal-signal vocabulary for this eval.

### Per-provider recognizer reference contract

This eval references only the AGE-139 recognizer and vocabulary concepts below.

Allowed recognizer module concepts: Claude/Anthropic uses `crates/oulipoly-runtime/src/executor/providers/claude.rs::Recognizer`; Codex/OpenAI uses `crates/oulipoly-runtime/src/executor/providers/codex.rs::Recognizer`; OpenAI-compatible wrappers use `crates/oulipoly-runtime/src/executor/providers/openai_compat.rs::Recognizer`. Each implements `TerminalSignalRecognizer`, consumes `TerminalSignalEvidence`, and returns `TerminalSignal`.

Allowed vocabulary concepts: recognizers read only `stdout` and `stderr` byte slices from `TerminalSignalEvidence`; provider bytes are decoded lossily, lowercased, and matched by substring against the canonical token sets in `conventions/terminal-signal-provider-vocabulary.md`; fixture rows cite that canonical vocabulary owner rather than duplicating token lists as a second schema; non-quota provider output remains opaque bounded bytes.

Allowed provider-family concepts: the Claude family includes `claude`, `claude2`, and Anthropic Claude CLI account names; the Codex family includes `codex` and OpenAI Codex CLI account names; the OpenAI-compatible family includes `gemini`, `opencode`, and other configured OpenAI-compatible wrappers.

### Provider-family dispatch table

| provider_name | recognizer_module_path | terminal_signal_kind_set |
|---|---|---|
| claude | crates/oulipoly-runtime/src/executor/providers/claude.rs | seven kinds |
| claude2 | crates/oulipoly-runtime/src/executor/providers/claude.rs | seven kinds |
| codex | crates/oulipoly-runtime/src/executor/providers/codex.rs | seven kinds |
| gemini | crates/oulipoly-runtime/src/executor/providers/openai_compat.rs | seven kinds |
| opencode | crates/oulipoly-runtime/src/executor/providers/openai_compat.rs | seven kinds |
| other OpenAI-compatible wrappers | crates/oulipoly-runtime/src/executor/providers/openai_compat.rs | seven kinds |

The `seven kinds` label refers to the AGE-139 set named in `### AGE-139 reference contract`.

### Fixture manifest schema

The fixture corpus is declared in [MANIFEST.yaml](fixtures/MANIFEST.yaml). Fixture rows provide values for this schema and raw fixture files contain provider bytes only.

```yaml
schema_id: agent-runner-provider-termination-fixture-manifest-v1
schema_owner: evals/agent-runner-provider-termination/eval.md
adapter_surface:
  raw_fixture_files: "provider bytes only"
  sentinel_metadata_files: "minimal structured metadata only"
  expected_rows: "machine-readable contract for W5"
rows:
  - id: string
    provider_name: string
    provider_family: claude | codex | openai_compat
    recognizer_module_path: string
    fixture_bytes_path: string | null
    fixture_bytes_role: stdout | stderr | combined | none
    sentinel_metadata_path: string | null
    terminal_status:
      kind: exited | signal_terminated | spawn_error | prolonged_silence | unknown
      code: integer | null
      signal: integer | null
      reason: string | null
    observed_at: string
    expected_terminal_signal_kind: CleanExit | NonzeroExit | SignalExit | SpawnError | QuotaExhaustedInband | ProlongedSilence | Unknown
    expected_marker_kind_label: clean_exit | nonzero_exit | signal_exit | spawn_error | quota_exhausted_inband | prolonged_silence | unknown
    evidence_excerpt_policy:
      max_chars: 160
      opaque: true
      parsed_fields: TerminalSignalEvidence-only
    provenance:
      source: string
      privacy_reviewed: boolean
      notes: string
```

### Marker payload schema

```text
OULIPOLY_TERMINAL_SIGNAL=<json>
```

The runtime line above carries the product wire payload with exactly `kind`, `evidence`, `invocation_id`, and `session_id`. The YAML below is the eval driver's enriched internal marker-event schema; the eval verifier consumes the runtime marker and may attach provider and fixture metadata from the manifest before scoring.

```yaml
schema_id: agent-runner-terminal-signal-marker-v1
provider_name: string
provider_family: claude | codex | openai_compat
kind: clean_exit | nonzero_exit | signal_exit | spawn_error | quota_exhausted_inband | prolonged_silence | unknown
evidence:
  fixture_row_id: string | null
  fixture_path: string | null
  excerpt: string
  excerpt_max_chars: 160
  opaque: true
terminal_status:
  kind: exited | signal_terminated | spawn_error | prolonged_silence | unknown
  code: integer | null
  signal: integer | null
  reason: string | null
observed_at: string
exit_disposition:
  process_exit_code: integer | null
  synthetic: boolean
  reason: string
```

The marker is a runner control record, not provider text. It is expected as one final line after provider-visible output has been drained and before wrapper process exit; `kind` is the lower-snake label for one allowed AGE-139 terminal-signal kind, and `evidence.excerpt` remains bounded and opaque.

### W5 reader interface schema

```yaml
schema_id: agent-runner-provider-termination-w5-reader-v1
inputs:
  eval_path: evals/agent-runner-provider-termination/eval.md
  manifest_path: evals/agent-runner-provider-termination/fixtures/MANIFEST.yaml | evals/agent-runner-provider-termination/fixtures/MANIFEST.md
  fixture_root: evals/agent-runner-provider-termination/fixtures/
reader_steps:
  - read_manifest_rows
  - resolve_fixture_bytes_path_relative_to_fixture_root
  - resolve_sentinel_metadata_path_relative_to_fixture_root
  - build_TerminalSignalEvidence_from_manifest_row
  - select_recognizer_from_provider_family_dispatch_table
  - assert_expected_terminal_signal_kind
  - render_or_parse_marker_payload_using_marker_payload_schema
  - assert_network_rows_use_adjacent_network_error_diagnostics_only
outputs:
  finding_fields:
    - eval_id
    - severity
    - evidence_paths
    - summary
    - suggested_action
    - confidence
```

The W5 reader must not require live provider CLIs, network calls, sleeps, or absolute machine-local paths. It reads raw fixture bytes only to build `TerminalSignalEvidence.stdout` and `TerminalSignalEvidence.stderr`, uses the dispatch table above for recognizer selection, and treats `network_error` as adjacent diagnostics rather than a terminal-signal kind.

### Current main status

Current main does not yet emit `OULIPOLY_TERMINAL_SIGNAL=<json>`. This eval documents the target contract and fixture corpus for later implementation and AGE-143/W5 reader work.
