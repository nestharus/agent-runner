#!/usr/bin/env python3
"""Synthetic contract tests. NEVER evidence of a real runner compilation/review.

Usage: python3 -B scripts/tests/runner-provenance.test.py NEW_EXTERNAL_EVIDENCE_DIR
All fixtures, source checkouts and failed attempts remain in that directory.
"""

# Imports follow explicit fixture module path/bytecode configuration.
# ruff: noqa: E402
import copy
from datetime import datetime, timedelta, timezone
import importlib
import os
from pathlib import Path
import stat
import sys
import unittest
from unittest import mock

sys.dont_write_bytecode = True
REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts"))
from runner_provenance.common import Rejected, fingerprint, git, write_json, write_new
from runner_provenance.install import install, rollback

producer = importlib.import_module("runner_provenance.build")

EVIDENCE = Path(sys.argv.pop(1)).resolve()
if EVIDENCE.is_relative_to(REPO):
    raise ValueError("fixtures must be outside repository")
EVIDENCE.mkdir(mode=0o700)
COMMIT = git(REPO, "rev-parse", "HEAD")
SOURCE_ROOT = EVIDENCE / "clean-source-fixture"
SOURCE_ROOT.mkdir()
IDENTITY = {
    "commit": COMMIT,
    "tree": git(REPO, "rev-parse", "HEAD^{tree}"),
    "clean": True,
}
producer.snapshot(REPO, SOURCE_ROOT, IDENTITY)
SOURCE = SOURCE_ROOT / "source"


class ContractTests(unittest.TestCase):
    def setUp(self):
        self.root = EVIDENCE / self._testMethodName
        self.root.mkdir()
        self.target = self.root / "installed-runner"
        self.candidate = self.root / "oulipoly-agent-runner"
        write_new(self.target, b"old synthetic runner", 0o751)
        write_new(self.candidate, b"new synthetic runner", 0o755)
        evidence = {}
        for name in (
            "004-rustc.log",
            "005-cargo.log",
            "006-vendor.log",
            "007-build.log",
        ):
            write_new(self.root / name, b"SYNTHETIC UNIT FIXTURE; NO REAL BUILD\n")
            evidence[name] = fingerprint(self.root / name)
        (self.root / "control").mkdir()
        write_new(self.root / "control/vendor.toml", b"SYNTHETIC CONFIGURATION")
        self.manifest = {
            "schema": 1,
            "profile": producer.PROFILE,
            "source": IDENTITY,
            "output": fingerprint(self.candidate),
            "build": {
                "argv": producer.BUILD,
                "environment": producer.ENVIRONMENT,
                "exit_code": 0,
                "network": False,
                "sandbox_argv": ["SYNTHETIC PROCESS MODEL"],
            },
            "producer": {"review_approval": False, "script_sha256": "1" * 64},
            "toolchain": {
                "rustc": evidence["004-rustc.log"],
                "cargo": evidence["005-cargo.log"],
            },
            "evidence": evidence,
            "inputs": {
                "cargo_lock": fingerprint(SOURCE / "Cargo.lock"),
                "vendor_sha256": "1" * 64,
                "source_snapshot_sha256": "1" * 64,
                "generated_sha256": "1" * 64,
                "configuration": fingerprint(self.root / "control/vendor.toml"),
                "host": {
                    "system_trees": {"SYNTHETIC": "1" * 64},
                    "toolchain": "1" * 64,
                    "alternatives": "1" * 64,
                    "ld_cache": fingerprint(self.root / "control/vendor.toml"),
                    "bin": "synthetic",
                    "lib": "synthetic",
                    "lib64": "synthetic",
                    "kernel": "synthetic",
                    "machine": "synthetic",
                },
            },
        }
        self.manifest_path = self.root / "manifest.json"
        write_json(self.manifest_path, self.manifest)
        self.custody = fingerprint(self.manifest_path)["sha256"]
        self.auth = {
            "schema": 1,
            "decision": "approved-for-install",
            "review_evidence": "SYNTHETIC UNIT FIXTURE; NOT REVIEW APPROVAL",
            "expires_at": (datetime.now(timezone.utc) + timedelta(hours=1)).isoformat(),
            "source": IDENTITY,
            "target": str(self.target),
            "current": fingerprint(self.target),
            "current_mode": 0o751,
            "candidate": fingerprint(self.candidate),
            "producer_manifest_sha256": self.custody,
        }
        self.auth_path = self.root / "authorization.json"
        write_json(self.auth_path, self.auth)
        self.auth_pin = fingerprint(self.auth_path)["sha256"]

    def repin(self):
        # Create a new fixture generation, never overwrite old evidence.
        generation = len(list(self.root.glob("manifest*.json")))
        self.manifest_path = self.root / f"manifest-{generation}.json"
        write_json(self.manifest_path, self.manifest)
        self.custody = fingerprint(self.manifest_path)["sha256"]
        self.auth["producer_manifest_sha256"] = self.custody
        self.auth_path = self.root / f"authorization-{generation}.json"
        write_json(self.auth_path, self.auth)
        self.auth_pin = fingerprint(self.auth_path)["sha256"]

    def invoke(self, apply=False, **overrides):
        args = dict(
            source=SOURCE,
            manifest=self.manifest_path,
            custody_pin=self.custody,
            authorization_path=self.auth_path,
            authorization_pin=self.auth_pin,
            target=self.target,
            verify_only=not apply,
        )
        args.update(overrides)
        return install(**args)

    def rejected(self, pattern, **overrides):
        before = fingerprint(self.target)
        with self.assertRaisesRegex((Rejected, OSError, KeyError, TypeError), pattern):
            self.invoke(apply=True, **overrides)
        self.assertEqual(fingerprint(self.target), before)
        self.assertFalse(list(self.root.glob(".runner-install-*")))

    def test_verify_only_is_read_only(self):
        before = sorted(self.root.iterdir())
        result = self.invoke()
        self.assertFalse(result["changed"])
        self.assertEqual(before, sorted(self.root.iterdir()))
        self.assertEqual(fingerprint(self.target), self.auth["current"])

    def test_install_and_rollback_preserve_bytes_and_mode(self):
        result = self.invoke(apply=True)
        self.assertEqual(fingerprint(self.target), self.auth["candidate"])
        tx = Path(result["transaction"])
        self.assertEqual(fingerprint(tx / "previous"), self.auth["current"])
        rollback(self.auth_path, self.auth_pin, self.target, tx)
        self.assertEqual(fingerprint(self.target), self.auth["current"])
        self.assertEqual(stat.S_IMODE(self.target.stat().st_mode), 0o751)
        self.assertTrue((tx / "previous").exists())
        with self.assertRaisesRegex(Rejected, "rollback current mismatch"):
            rollback(self.auth_path, self.auth_pin, self.target, tx)

    def test_tampered_binary(self):
        self.candidate.write_bytes(b"tampered synthetic runner")
        self.rejected("tampered candidate")

    def test_tampered_manifest_independent_pin(self):
        self.manifest_path.write_bytes(b"{}")
        self.rejected("independent digest mismatch")

    def test_manifest_source_mismatch_even_when_rehashed(self):
        self.manifest = copy.deepcopy(self.manifest)
        self.manifest["source"]["tree"] = "0" * 40
        self.repin()
        self.rejected("manifest/source authorization mismatch")

    def test_wrong_selected_source(self):
        self.auth = copy.deepcopy(self.auth)
        self.auth["source"]["commit"] = "0" * 40
        self.manifest["source"] = self.auth["source"]
        self.repin()
        self.rejected("source commit mismatch")

    def test_dirty_source(self):
        fixture = self.root / "dirty-fixture"
        fixture.mkdir()
        producer.snapshot(REPO, fixture, IDENTITY)
        (fixture / "source/Cargo.toml").write_text(
            "# deliberately dirty retained fixture\n"
        )
        self.rejected("dirty source", source=fixture / "source")

    def test_wrong_target(self):
        other = self.root / "wrong-target"
        write_new(other, b"old synthetic runner", 0o751)
        self.rejected("wrong independently authorized target", target=other)
        self.assertEqual(fingerprint(other), self.auth["current"])

    def test_wrong_current_bytes(self):
        self.target.write_bytes(b"other synthetic installed runner")
        self.rejected("wrong current target bytes")

    def test_wrong_mode(self):
        self.target.chmod(0o777)
        self.rejected("wrong current target mode")

    def test_missing_review(self):
        self.auth["decision"] = "not-reviewed"
        self.repin()
        self.rejected("missing root review approval")

    def test_missing_custody(self):
        self.rejected("custody/authorization mismatch", custody_pin="0" * 64)

    def test_tampered_authorization(self):
        self.auth_path.write_bytes(b"{}")
        self.rejected("independent digest mismatch")

    def test_expired_approval(self):
        self.auth["expires_at"] = "2000-01-01T00:00:00+00:00"
        self.repin()
        self.rejected("expired authorization")

    def test_forged_build_configuration(self):
        self.manifest = copy.deepcopy(self.manifest)
        self.manifest["build"]["argv"] = ["true"]
        self.repin()
        self.rejected("unexpected build command")

    def test_missing_build_evidence(self):
        self.manifest["evidence"]["007-build.log"]["sha256"] = "0" * 64
        self.repin()
        self.rejected("build evidence mismatch")

    def test_lock_mismatch(self):
        self.manifest["inputs"]["cargo_lock"]["sha256"] = "0" * 64
        self.repin()
        self.rejected("locked dependency mismatch")

    def test_missing_toolchain_identity(self):
        self.manifest["toolchain"] = {}
        self.repin()
        self.rejected("rustc")

    def test_producer_configuration_tamper(self):
        (self.root / "control/vendor.toml").write_bytes(b"changed configuration")
        self.rejected("producer configuration mismatch")

    def test_extended_metadata_rejected(self):
        os.setxattr(self.target, "user.synthetic-provenance-test", b"fixture")
        self.rejected("extended metadata")

    def test_symlink_target_rejected(self):
        link = self.root / "link"
        link.symlink_to(self.target)
        self.rejected("symlink path", target=link)

    def test_atomic_replace_failure_keeps_current_and_backup(self):
        with mock.patch(
            "runner_provenance.install.os.replace",
            side_effect=OSError("injected rename failure"),
        ):
            with self.assertRaisesRegex(OSError, "injected rename failure"):
                self.invoke(apply=True)
        self.assertEqual(fingerprint(self.target), self.auth["current"])
        (tx,) = self.root.glob(".runner-install-*")
        self.assertEqual(fingerprint(tx / "previous"), self.auth["current"])
        self.assertEqual(fingerprint(tx / "candidate"), self.auth["candidate"])

    def test_rollback_tampering_rejected(self):
        result = self.invoke(apply=True)
        tx = Path(result["transaction"])
        (tx / "previous").write_bytes(b"tampered backup")
        with self.assertRaisesRegex(Rejected, "tampered rollback bytes"):
            rollback(self.auth_path, self.auth_pin, self.target, tx)
        self.assertEqual(fingerprint(self.target), self.auth["candidate"])

    def producer_case(
        self,
        name,
        output=True,
        fail=False,
        mutate=None,
        host_changed=False,
        symlink_output=False,
    ):
        destination = self.root / name
        toolchain = self.root / "toolchain"
        (toolchain / "bin").mkdir(parents=True)
        write_new(toolchain / "bin/cargo", b"SYNTHETIC")
        write_new(toolchain / "bin/rustc", b"SYNTHETIC")

        def snapshot(repo, root, identity):
            (root / "source").mkdir()
            write_new(root / "source/Cargo.lock", b"SYNTHETIC LOCK")

        def execute(argv, cwd, log, env):
            write_new(log, b"SYNTHETIC PROCESS MODEL; NOT REAL BUILD\n")
            if argv[-len(producer.BUILD) :] == producer.BUILD:
                if output:
                    binary = (
                        cwd
                        / "output/target/x86_64-unknown-linux-gnu/release/oulipoly-agent-runner"
                    )
                    binary.parent.mkdir(parents=True)
                    if symlink_output:
                        binary.symlink_to(self.candidate)
                    else:
                        write_new(binary, b"\x7fELF SYNTHETIC MODEL OUTPUT")
                if fail:
                    raise Rejected("injected failed compiler")
                if mutate:
                    write_new(cwd / mutate / "MUTATED-INPUT", b"unexpected input")

        with (
            mock.patch.object(producer, "source_identity", return_value=IDENTITY),
            mock.patch.object(producer, "snapshot", side_effect=snapshot),
            mock.patch.object(
                producer,
                "host_identity",
                side_effect=[{"test_model": True}, {"test_model": not host_changed}],
            ),
            mock.patch.object(producer, "command", side_effect=execute),
        ):
            return producer.build(REPO, COMMIT, destination, toolchain)

    def test_producer_model_records_actual_selected_output(self):
        manifest = self.producer_case("model-build")
        output = self.root / "model-build/oulipoly-agent-runner"
        self.assertEqual(manifest["output"], fingerprint(output))
        self.assertIs(manifest["producer"]["review_approval"], False)

    def test_stale_preexisting_output_rejected(self):
        destination = self.root / "stale-build"
        destination.mkdir()
        write_new(destination / "oulipoly-agent-runner", b"stale old bytes")
        with self.assertRaises(FileExistsError):
            self.producer_case("stale-build")
        self.assertFalse((destination / "manifest.json").exists())
        self.assertEqual(
            (destination / "oulipoly-agent-runner").read_bytes(), b"stale old bytes"
        )

    def test_successful_command_without_output_cannot_attest(self):
        with self.assertRaises(FileNotFoundError):
            self.producer_case("no-output", output=False)
        self.assertFalse((self.root / "no-output/manifest.json").exists())

    def test_failed_build_cannot_attest(self):
        with self.assertRaisesRegex(Rejected, "injected failed compiler"):
            self.producer_case("failed-build", fail=True)
        self.assertFalse((self.root / "failed-build/manifest.json").exists())

    def test_symlink_build_output_cannot_attest(self):
        with self.assertRaisesRegex(Rejected, "not a regular file"):
            self.producer_case("symlink-output", symlink_output=True)
        self.assertFalse((self.root / "symlink-output/manifest.json").exists())

    def test_changed_source_input_cannot_attest(self):
        with self.assertRaisesRegex(Rejected, "source snapshot changed"):
            self.producer_case("changed-source", mutate="source")
        self.assertFalse((self.root / "changed-source/manifest.json").exists())

    def test_changed_vendor_input_cannot_attest(self):
        with self.assertRaisesRegex(Rejected, "dependency inputs changed"):
            self.producer_case("changed-vendor", mutate="vendor")
        self.assertFalse((self.root / "changed-vendor/manifest.json").exists())

    def test_changed_host_cannot_attest(self):
        with self.assertRaisesRegex(Rejected, "host/toolchain inputs changed"):
            self.producer_case("changed-host", host_changed=True)
        self.assertFalse((self.root / "changed-host/manifest.json").exists())

    def test_sandbox_is_closed_and_inputs_readonly(self):
        args = producer.sandbox(self.root, self.root / "toolchain", False)
        self.assertIn("--clearenv", args)
        self.assertIn("--unshare-all", args)
        self.assertNotIn("--share-net", args)
        self.assertNotIn("/home/nes", args)
        index = args.index("/source")
        self.assertEqual(args[index - 2], "--ro-bind")
        index = args.index("/vendor")
        self.assertEqual(args[index - 2], "--ro-bind")
        self.assertNotIn("RUSTFLAGS", producer.ENVIRONMENT)
        self.assertNotIn("TAURI_CONFIG", producer.ENVIRONMENT)


if __name__ == "__main__":
    unittest.main(verbosity=2)
