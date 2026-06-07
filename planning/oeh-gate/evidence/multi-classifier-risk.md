# OEH Multi-Classifier Risk Profile

Coordinator pre-gate review found no genuine multi-classifier risk requiring behavior-identical splitting before auditor dispatch.

The only pre-gate remediation was validation-only commit `bdbb9e3`: two OpenCode recognizer unit tests were added to bind the F4 parity proof claim that quota/rate substrings in ordinary output do not classify as quota or rate-limit terminal signals.

Current split-sensitive surfaces for the function-classification auditor:

| Surface | Disposition |
|---|---|
| `supervised_exit_code` | Single mapper from terminal signal plus real status to final exit code. |
| `unknown_terminal_reason` | Single mapper from `Unknown` signal evidence to terminal reason. |
| `json_error_signal_from_stream` | Single mapper from stream bytes to optional terminal signal tuple; parsing/filtering/evidence are delegated. |
| `json_error_line_evidence` | Formatter for bounded terminal evidence; parse/value extraction are delegated helpers. |
| `json_error_evidence_from_value` | Single mapper from parsed JSON value to provider-error evidence. |
| `json_error_evidence` | Single formatter for provider-error evidence text. |
| `terminal_signal_kind_from_json_error` | Single mapper from structured error object and normalized message to terminal signal kind. |
