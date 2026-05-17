# AGE-89 Dossier Provenance

This directory is the repo-owned provenance interface for AGE-89 dossier evidence. It lets eval consumers cite stable manifest IDs while AGE-89 remains the owner of the source evidence and its meaning.

## Manifest Path

The manifest lives at `evals/_provenance/age-89-dossier-manifest.json`.

## Citation Syntax

Consumers MUST cite AGE-89 dossier evidence with `evals/_provenance/age-89-dossier-manifest.json#<stable_id>`.

Consumers must not cite the AGE-89 dossier through absolute private planning paths. Those paths are verification metadata only, not the consumer-facing provenance boundary.

## Ownership Rule

The AGE-89 chain owns the source evidence. Downstream consumers cite stable IDs from this manifest and do not rewrite or reinterpret AGE-89 dossier files as their own evidence.

## Addition Rule

New evidence files get a new `entries[*]` object with a new `stable_id` following `^age-89-[a-z0-9-]+-v[0-9]+$`, a source-relative `source_path`, a SHA-256 hash, the AGE-89 owner chain, and the stable source branch tag.

## Supersession Rule

Superseded entries either bump the version suffix, such as `-v1` to `-v2`, with `supersedes` pointing at the predecessor stable ID, or remain in place with `supersedes: null`.

## Information-Only Rule

Manifest entries MUST NOT carry executable hooks, runtime loader references, replay payloads, scripts, or test-hook-dependent paths. This manifest is inert provenance data.

## Verification

Use `contract_tests.py` and `run-tests.sh` for local verification:

```bash
bash evals/_provenance/run-tests.sh
```
