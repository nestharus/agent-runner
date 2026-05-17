#!/usr/bin/env python3
"""Contract tests for the AGE-126 AGE-89 provenance manifest surface."""

from __future__ import annotations

import hashlib
import json
import os
import re
import unittest
from pathlib import Path
from typing import Any, Iterator


EVAL_DIR = Path(__file__).resolve().parent
REPO_ROOT = EVAL_DIR.parents[1]

MANIFEST_PATH = EVAL_DIR / "age-89-dossier-manifest.json"
SCHEMA_PATH = EVAL_DIR / "age-89-dossier-manifest.schema.json"
README_PATH = EVAL_DIR / "README.md"
CARRY_FORWARD_PATH = EVAL_DIR / "CARRY_FORWARD.md"

DEFAULT_DOSSIER_ROOT = Path(
    "/home/nes/projects/agent-runner/planning/prototype-age-89-clarify/dossier"
)
BRANCH_TAG = "prototype-age-89-clarify-p2-stable"
CITATION_PREFIX = "evals/_provenance/age-89-dossier-manifest.json"
CITATION_SYNTAX = f"{CITATION_PREFIX}#<stable_id>"
STABLE_ID_PATTERN = r"^age-89-[a-z0-9-]+-v[0-9]+$"
SHA256_PATTERN = r"^[0-9a-f]{64}$"
ACCEPTED_STABLE_ID_EXAMPLES = ("age-89-answer-v1",)
REJECTED_STABLE_ID_EXAMPLES = ("AGE-89-answer-v1", "age-89-answer")
PRIVATE_PATH_FORBIDDEN_PHRASES = ("must not cite", "do not cite", "forbid", "forbidden")
FORBIDDEN_EXECUTABLE_KEYS = {
    "hook",
    "hooks",
    "loader",
    "loader_path",
    "runtime",
    "runtime_args",
    "replay",
    "replay_command",
    "script",
    "script_path",
    "command",
    "command_args",
    "executable",
    "argv",
    "launch",
    "launch_args",
    "test_hook",
}
STABLE_ID_SCHEMA_PATH = ["properties", "entries", "items", "properties", "stable_id"]

TOP_LEVEL_KEYS = {
    "manifest_id",
    "manifest_version",
    "purpose",
    "information_only",
    "owner",
    "source",
    "citation_syntax",
    "entries",
}

ENTRY_KEYS = {
    "stable_id",
    "source_path",
    "description",
    "sha256",
    "owner_chain",
    "branch_tag",
    "supersedes",
}

REQUIRED_INITIAL_ENTRIES = {
    "age-89-p1-a-truth-table-v1": {
        "source_path": "evidence/p1-A-truth-table.md",
        "sha256": "8fabfae4762d613606b882fdf2c718b3a38623440b567ea68061f19f8f87416d",
    },
    "age-89-p1-a-five-background-stress-v1": {
        "source_path": "evidence/p1-A-five-background-stress.md",
        "sha256": "cd28498f4744a635c9bbfada4f2d5251167a4fdf2442442dbf09e8ea6f5cc1c0",
    },
    "age-89-p1-a-scheduled-wakeup-v1": {
        "source_path": "evidence/p1-A-scheduled-wakeup.md",
        "sha256": "0504f27b2828aa098876fd37140749587417ed969da5356c6a46aa3795560654",
    },
    "age-89-answer-v1": {
        "source_path": "answer.md",
        "sha256": "6b5552b25ab456826307452fb1db71e9a0759f0b8b616397645fcca54e147032",
    },
}


def read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def parse_json(text: str) -> Any:
    return json.loads(text)


def manifest_raw() -> Any:
    return parse_json(read_text(MANIFEST_PATH))


def manifest() -> dict[str, Any]:
    value = manifest_raw()
    if not isinstance(value, dict):
        raise AssertionError(f"{MANIFEST_PATH} must contain a JSON object")
    return value


def schema_raw() -> Any:
    return parse_json(read_text(SCHEMA_PATH))


def schema() -> dict[str, Any]:
    value = schema_raw()
    if not isinstance(value, dict):
        raise AssertionError(f"{SCHEMA_PATH} must contain a JSON object")
    return value


def entries_raw() -> Any:
    return manifest_raw()["entries"]


def entries(value: Any) -> list[Any]:
    if not isinstance(value, list):
        raise AssertionError("manifest entries must be an array")
    for index, entry in enumerate(value):
        if not isinstance(entry, dict):
            raise AssertionError(f"entry {index} must be a JSON object")
    return value


def entry_by_stable_id() -> dict[str, dict[str, Any]]:
    return {str(entry["stable_id"]): entry for entry in entries(entries_raw())}


def missing_top_level_keys() -> set[str]:
    return TOP_LEVEL_KEYS - set(manifest())


def entry_missing_required_fields(entry: dict[str, Any]) -> set[str]:
    return ENTRY_KEYS - set(entry)


def stable_ids() -> list[Any]:
    return [entry["stable_id"] for entry in entries(entries_raw())]


def supersedes_target_present(entry: dict[str, Any]) -> bool:
    return entry.get("supersedes") is not None


def missing_required_initial_stable_ids() -> set[str]:
    return set(REQUIRED_INITIAL_ENTRIES) - set(stable_ids())


def dossier_root_override_text() -> str | None:
    return os.environ.get("AGE89_DOSSIER_ROOT")


def path_from_text_or_default(text: str | None, default: Path) -> Path:
    return Path(text) if text else default


def is_override_present(text: str | None) -> bool:
    return text is not None


def is_readable_dir(path: Path) -> bool:
    return path.is_dir() and os.access(path, os.R_OK | os.X_OK)


def dossier_root() -> Path:
    return path_from_text_or_default(dossier_root_override_text(), DEFAULT_DOSSIER_ROOT)


def dossier_skip_reason() -> str:
    return (
        f"AGE-89 dossier root is unreadable: {dossier_root()}. "
        "Set AGE89_DOSSIER_ROOT to a readable dossier checkout."
    )


def dossier_root_is_readable() -> bool:
    return is_readable_dir(dossier_root())


def dossier_root_override_is_present() -> bool:
    return is_override_present(dossier_root_override_text())


def entry_dossier_path(entry: dict[str, Any]) -> Path:
    return dossier_root() / entry["source_path"]


def entry_dossier_file_exists(entry: dict[str, Any]) -> bool:
    return entry_dossier_path(entry).is_file()


def entry_dossier_file_missing_message(entry: dict[str, Any]) -> str:
    return f"missing dossier file for {entry['stable_id']}: {entry_dossier_path(entry)}"


def read_bytes(path: Path) -> bytes:
    return path.read_bytes()


def sha256_hex(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def entry_dossier_sha256(entry: dict[str, Any]) -> str:
    return sha256_hex(read_bytes(entry_dossier_path(entry)))


def path_has_parent_segment(path: str) -> bool:
    return ".." in Path(path).parts


def iter_keys(value: Any) -> Iterator[str]:
    if isinstance(value, dict):
        for key, nested in value.items():
            yield str(key)
            yield from iter_keys(nested)
    elif isinstance(value, list):
        for nested in value:
            yield from iter_keys(nested)


def forbidden_keys_in_entry(entry: dict[str, Any]) -> set[str]:
    return {key for key in iter_keys(entry) if key.lower() in FORBIDDEN_EXECUTABLE_KEYS}


def schema_property_raw(path: list[str]) -> Any:
    current: Any = schema_raw()
    for segment in path:
        current = current[segment]
    return current


def schema_property(current: Any, path: list[str]) -> dict[str, Any]:
    if not isinstance(current, dict):
        raise AssertionError(f"schema path {'.'.join(path)} must resolve to an object")
    return current


def compile_pattern(pattern_text: str) -> re.Pattern[str]:
    return re.compile(pattern_text)


def missing_schema_top_level_keys() -> set[str]:
    return TOP_LEVEL_KEYS - set(schema().get("required", []))


def missing_schema_entry_keys() -> set[str]:
    path = ["properties", "entries", "items"]
    return ENTRY_KEYS - set(schema_property(schema_property_raw(path), path).get("required", []))


def stable_id_schema_pattern() -> Any:
    return schema_property(schema_property_raw(STABLE_ID_SCHEMA_PATH), STABLE_ID_SCHEMA_PATH).get(
        "pattern"
    )


def pattern_matches_examples(
    pattern: re.Pattern[str], examples: tuple[str, ...]
) -> dict[str, bool]:
    return {example: pattern.fullmatch(example) is not None for example in examples}


def stable_id_schema_pattern_match_results(examples: tuple[str, ...]) -> dict[str, bool]:
    return pattern_matches_examples(compile_pattern(stable_id_schema_pattern()), examples)


def readme_text() -> str:
    return README_PATH.read_text(encoding="utf-8")


def lower_text(text: str) -> str:
    return text.lower()


def text_contains_phrase(text: str, phrase: str) -> bool:
    return phrase in text


def text_contains_any_phrase(text: str, phrases: tuple[str, ...]) -> bool:
    return any(text_contains_phrase(text, phrase) for phrase in phrases)


class ProvenanceContractTests(unittest.TestCase):
    def test_manifest_is_valid_json(self) -> None:
        self.assertIsNotNone(manifest_raw())

    def test_top_level_fields_present(self) -> None:
        missing = missing_top_level_keys()
        self.assertFalse(missing, f"missing top-level manifest keys: {sorted(missing)}")

    def test_information_only_is_true(self) -> None:
        self.assertIs(manifest()["information_only"], True)

    def test_owner_chain_is_age_89(self) -> None:
        self.assertEqual(manifest()["owner"]["chain"], "age-89")

    def test_source_branch_tag(self) -> None:
        self.assertEqual(manifest()["source"]["branch_tag"], BRANCH_TAG)

    def test_citation_syntax_present(self) -> None:
        citation_syntax = manifest()["citation_syntax"]
        self.assertIn("#", citation_syntax)
        self.assertTrue(
            citation_syntax.startswith(CITATION_PREFIX),
            f"citation syntax must start with {CITATION_PREFIX}",
        )

    def test_entries_non_empty(self) -> None:
        self.assertGreaterEqual(len(entries(entries_raw())), 4)

    def test_entry_required_fields(self) -> None:
        for entry in entries(entries_raw()):
            missing = entry_missing_required_fields(entry)
            self.assertFalse(
                missing,
                f"{entry.get('stable_id', '<missing stable_id>')} missing keys: {sorted(missing)}",
            )

    def test_entry_stable_id_pattern(self) -> None:
        for entry in entries(entries_raw()):
            self.assertRegex(str(entry["stable_id"]), STABLE_ID_PATTERN)

    def test_entry_stable_id_unique(self) -> None:
        self.assertEqual(
            len(stable_ids()),
            len(set(stable_ids())),
            f"duplicate stable_id values: {stable_ids()}",
        )

    def test_entry_sha256_format(self) -> None:
        for entry in entries(entries_raw()):
            self.assertRegex(str(entry["sha256"]), SHA256_PATTERN)

    def test_entry_source_path_relative_safe(self) -> None:
        for entry in entries(entries_raw()):
            source_path = str(entry["source_path"])
            self.assertFalse(source_path.startswith("/"), source_path)
            self.assertFalse(path_has_parent_segment(source_path), source_path)

    def test_entry_owner_chain_age_89(self) -> None:
        for entry in entries(entries_raw()):
            self.assertEqual(entry["owner_chain"], "age-89", entry["stable_id"])

    def test_entry_branch_tag_matches_source(self) -> None:
        source_branch_tag = manifest()["source"]["branch_tag"]
        for entry in entries(entries_raw()):
            self.assertEqual(entry["branch_tag"], source_branch_tag, entry["stable_id"])

    def test_entry_supersedes_resolves(self) -> None:
        for entry in entries(entries_raw()):
            supersedes = entry["supersedes"]
            if supersedes_target_present(entry):
                self.assertIn(supersedes, stable_ids(), entry["stable_id"])
                self.assertNotEqual(supersedes, entry["stable_id"])

    def test_required_initial_four_entries_present(self) -> None:
        by_id = entry_by_stable_id()
        missing = missing_required_initial_stable_ids()
        self.assertFalse(missing, f"missing required initial entries: {sorted(missing)}")
        for stable_id, expected in REQUIRED_INITIAL_ENTRIES.items():
            self.assertEqual(by_id[stable_id]["source_path"], expected["source_path"])
            self.assertEqual(by_id[stable_id]["sha256"], expected["sha256"])

    def test_entry_sha256_matches_dossier_file(self) -> None:
        if not dossier_root_is_readable():
            if dossier_root_override_is_present():
                self.fail(dossier_skip_reason())
            self.skipTest(dossier_skip_reason())

        for entry in entries(entries_raw()):
            self.assertTrue(
                entry_dossier_file_exists(entry),
                entry_dossier_file_missing_message(entry),
            )
            self.assertEqual(
                entry_dossier_sha256(entry),
                entry["sha256"],
                entry["stable_id"],
            )

    def test_no_executable_hook_keys(self) -> None:
        for entry in entries(entries_raw()):
            bad_keys = sorted(forbidden_keys_in_entry(entry))
            self.assertFalse(bad_keys, f"{entry['stable_id']} has executable keys: {bad_keys}")

    def test_schema_is_valid_json(self) -> None:
        self.assertIsNotNone(schema_raw())

    def test_schema_declares_draft_07(self) -> None:
        schema_uri = str(schema()["$schema"]).rstrip("#")
        self.assertTrue(
            schema_uri.endswith("draft-07/schema"),
            f"schema must declare draft-07, got {schema_uri}",
        )

    def test_schema_required_top_level_keys(self) -> None:
        missing = missing_schema_top_level_keys()
        self.assertFalse(missing, f"schema missing top-level required keys: {sorted(missing)}")

    def test_schema_per_entry_required_keys(self) -> None:
        missing = missing_schema_entry_keys()
        self.assertFalse(missing, f"schema missing entry required keys: {sorted(missing)}")

    def test_schema_constrains_information_only_true(self) -> None:
        path = ["properties", "information_only"]
        information_only = schema_property(schema_property_raw(path), path)
        constrained = information_only.get("const") is True or information_only.get("enum") == [True]
        self.assertTrue(constrained, "schema must constrain information_only to true")

    def test_schema_constrains_owner_chain_age_89(self) -> None:
        path = ["properties", "owner", "properties", "chain"]
        owner_chain = schema_property(schema_property_raw(path), path)
        constrained = owner_chain.get("const") == "age-89" or owner_chain.get("enum") == ["age-89"]
        self.assertTrue(constrained, "schema must constrain owner.chain to age-89")

    def test_schema_constrains_stable_id_pattern(self) -> None:
        pattern = stable_id_schema_pattern()
        self.assertIsInstance(pattern, str)
        accepted = stable_id_schema_pattern_match_results(ACCEPTED_STABLE_ID_EXAMPLES)
        rejected = stable_id_schema_pattern_match_results(REJECTED_STABLE_ID_EXAMPLES)
        self.assertTrue(accepted["age-89-answer-v1"])
        self.assertFalse(rejected["AGE-89-answer-v1"])
        self.assertFalse(rejected["age-89-answer"])

    def test_readme_present(self) -> None:
        self.assertTrue(README_PATH.is_file(), f"missing {README_PATH}")

    def test_readme_documents_citation_syntax(self) -> None:
        text = readme_text()
        lowered = lower_text(text)
        self.assertIn(CITATION_SYNTAX, text)
        self.assertTrue(
            text_contains_any_phrase(lowered, PRIVATE_PATH_FORBIDDEN_PHRASES),
            "README must forbid absolute private-path citation",
        )
        self.assertIn("absolute", lowered)
        self.assertTrue(
            text_contains_any_phrase(lowered, ("private", "planning")),
            "README must name private/planning path citation as forbidden",
        )

    def test_readme_documents_ownership_addition_supersession(self) -> None:
        lowered = lower_text(readme_text())
        self.assertRegex(lowered, r"\bowner(ship)?\b|\bowns?\b")
        self.assertRegex(lowered, r"\baddition\b|\badd\b|\bnew evidence\b")
        self.assertRegex(lowered, r"\bsupersession\b|\bsupersedes\b|\bsuperseded\b")

    def test_no_carry_forward_md_in_dir(self) -> None:
        self.assertFalse(CARRY_FORWARD_PATH.exists(), f"must not create {CARRY_FORWARD_PATH}")


if __name__ == "__main__":
    unittest.main()
