# Phase 6 expected process — 09-internal-unification

## Required child invocations

| Role | Model | Prompt | Output |
|---|---|---|---|
| Step 6b | gpt-high | `.tmp/phase6/step6b-prompt.md` | tests/initiative_09_internal_unification.rs + fixture migration + step6b-output-index.md |
| Step 6c | gpt-high | `.tmp/phase6/step6c-prompt.md` | step6c-reads.md (firstness) + delete internal/mod.rs + extend session_lock/session_metadata + lift session_replace |

## Firstness rule

- Step 6c writes `.tmp/phase6/step6c-reads.md` BEFORE editing product code.
- Test commit (6763500) precedes code commit (cfff4c4).

## Commit ordering

- 215fe7d plan(...): Phase 0 + Phase 3 proposal
- c0e1848 plan(...): Rev 2
- fac8e82 plan(...): Rev 3
- 7510bba rca(...): listing API alignment
- afd6628 risk(...): Phase 4 zero-risk gate
- 6763500 test(...): Phase 6 Step 6b
- cfff4c4 feat(...): Phase 6 Step 6c

Tests-first, code-after.
