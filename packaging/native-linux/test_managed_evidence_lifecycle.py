"""C2/C3 source-to-installer controls, not installed physical qualification.

The source frontdoor runs a tiny owned Python entry through the existing
healthy fixture. Admission/root ownership and provider launch are fixture
seams; no packaged executable, provider, sudo or host installation runs.
Physical records are facts, not installer drain/settlement authority.
"""
import contextlib
import fcntl
import io
import json
import os
from pathlib import Path
import shutil
import unittest
from unittest import mock

import frontdoor as fd
import install_package as ins
from test_build_install import Scratch, PACKAGE_ID
import test_residue_evidence as residue_fixtures


class ManagedEvidenceLifecycle(Scratch):
    def install_fixture(self):
        archive, digest = self.archive(self.stage())
        with mock.patch.object(ins, "VISUDO", os.path.join(self.dir, "no-visudo")), \
             contextlib.redirect_stdout(io.StringIO()):
            code, dest = self.install(archive, digest)
        self.assertEqual(code, 0)
        self.dest = Path(dest)
        self.package = self.dest / "opt/oulipoly-native" / PACKAGE_ID
        self.rule = self.dest / "etc/sudoers.d/oulipoly-native"
        self.record = self.dest / "opt/oulipoly-native" / (PACKAGE_ID + ".install.json")
        self.runs = self.dest / "var/lib/oulipoly-native/runs"

    def produced_records(self):
        fixture = residue_fixtures.ResidueEvidence(methodName="runTest")
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        # Separate source-fixture producer package, never an installed binary.
        fixture.runs = self.runs
        fixture.user_dir = self.runs / str(fixture.user.pw_uid)
        fixture.user_dir.mkdir(mode=0o700)
        fixture.site["run_base"] = str(self.runs)
        code, lines = fixture.direct()
        self.assertEqual(code, 87)
        self.assertTrue(lines[-1]["retire"]["run_removed"])
        # direct() also creates prior physical uncertainty for its own
        # frontdoor test. Remove only that authored setup here: installer
        # fencing is package/run locks, not physical-marker interpretation.
        shutil.rmtree(fixture.user_dir / "prior-uncertain")
        custody = fd.RunCustody()
        for name in ("semantic", "active"):
            entry = fixture.producer("import sys;sys.exit(17)")
            status = entry.wait(timeout=5)
            run = fixture.run_tree(name)
            custody.run, custody.run_dir = run.name, str(run)
            custody.ended(status)
            with mock.patch.object(fd, "check_owned"), \
                 mock.patch.object(fd, "store_account", return_value=(fixture.store, None)):
                result = fd.retire(str(run), "discard", package=str(fixture.package))
            self.assertTrue(result["run_removed"])
            self.assertTrue(result["loss_account"]["retained"])
            if name == "semantic":
                fixture.route({"op": "retire-loss-account", "account": name})
                directory = fixture.user_dir / fd.LOSS_ACCOUNTS
                self.assertEqual(fd.loss_names(directory), [])
                self.assertEqual(len(fd.disposition_names(directory)), 2)
        self.directory = fixture.user_dir / fd.LOSS_ACCOUNTS
        self.assertEqual(len(fd.loss_names(self.directory)), 1)
        self.assertEqual(len(fd.disposition_names(self.directory)), 3)
        return {p.name: p.read_bytes() for p in self.directory.iterdir()}

    def uninstall_fixture(self, purge=False):
        output = io.StringIO()
        argv = ["uninstall", "--record", f"/opt/oulipoly-native/{PACKAGE_ID}.install.json",
                "--dest-root", str(self.dest), "--unprivileged-test"]
        if purge:
            argv.append("--purge-site")
        with contextlib.redirect_stdout(output):
            code = ins.main(argv)
        return code, json.loads(output.getvalue())

    def assert_blocked(self, purge=False):
        before = {p.name: p.read_bytes() for p in self.directory.iterdir() if p.is_file()}
        rule, record = self.rule.read_bytes(), self.record.read_bytes()
        code, report = self.uninstall_fixture(purge)
        self.assertEqual((code, report["stopped"]), (3, "live-or-unknown-runs"))
        self.assertTrue(self.package.exists())
        self.assertEqual(self.rule.read_bytes(), rule)
        self.assertEqual(self.record.read_bytes(), record)
        self.assertEqual({p.name: p.read_bytes() for p in self.directory.iterdir() if p.is_file()}, before)

    def test_nonpurging_removal_preserves_produced_semantic_and_physical_bytes(self):
        self.install_fixture()
        before = self.produced_records()
        code, report = self.uninstall_fixture()
        self.assertEqual(code, 3, "retained site effects keep the existing recovery record")
        self.assertNotIn("stopped", report)
        self.assertFalse(self.package.exists())
        self.assertFalse(self.rule.exists())
        self.assertTrue(self.record.exists())
        self.assertEqual({p.name: p.read_bytes() for p in self.directory.iterdir()}, before)
        evidence, = report["loss_accounts"]
        self.assertEqual((evidence["accounts"], evidence["physical_dispositions"]), (1, 3))
        self.assertEqual(evidence["disposition"], "preserved across package removal")
        self.assertTrue((self.dest / "etc/oulipoly-native/frontdoor.json").exists())

    def test_corrected_helper_removes_later_package_over_earlier_producer_records(self):
        self.dest = Path(self.dir) / "root"
        self.runs = self.dest / "var/lib/oulipoly-native/runs"
        self.runs.mkdir(parents=True, mode=0o711)
        before = self.produced_records()  # target package does not yet exist
        self.install_fixture()
        code, report = self.uninstall_fixture()
        self.assertEqual(code, 3)
        self.assertNotIn("stopped", report)
        self.assertFalse(self.package.exists())
        self.assertFalse(self.rule.exists())
        self.assertEqual({p.name: p.read_bytes() for p in self.directory.iterdir()}, before)

    def test_explicit_purge_reports_distinct_kinds_and_journals_before_deletion(self):
        self.install_fixture()
        before = self.produced_records()
        actual_unlink = ins.os.unlink
        witnessed = {}
        def unlink(path, *args, **kwargs):
            if Path(path).parent == self.directory:
                record = json.loads(self.record.read_text())
                items = {item["path"]: item for item in record["loss_account_disposition"]}
                item = items[str(path)]
                self.assertFalse(self.rule.exists(), "owned entry disabled before deletion")
                self.assertEqual(item["state"], "purge-intended")
                self.assertEqual(item["meaning"], "evidence deletion, not settlement")
                witnessed[Path(path).name] = item["kind"]
            return actual_unlink(path, *args, **kwargs)
        with mock.patch.object(ins.os, "unlink", side_effect=unlink):
            code, report = self.uninstall_fixture(purge=True)
        self.assertEqual(code, 0)
        self.assertEqual(set(witnessed), set(before))
        self.assertEqual(witnessed["active.json"], "semantic-account")
        self.assertEqual(witnessed["semantic.json.retired"], "physical-only-disposition")
        self.assertEqual(list(witnessed.values()).count("physical-only-disposition"), 3)
        evidence, = report["loss_accounts"]
        self.assertEqual((evidence["accounts"], evidence["physical_dispositions"]), (1, 3))
        self.assertEqual(evidence["disposition"], "purged-not-settled")
        self.assertFalse(self.directory.exists())
        self.assertFalse(self.package.exists())
        self.assertFalse(self.rule.exists())

    def test_unknown_pending_types_and_modes_block_even_explicit_purge(self):
        self.install_fixture()
        self.produced_records()
        self.assertEqual(ins.live_runs(str(self.runs), True), [])
        for name, kind in (("unknown", "file"), (".next-deadbeef", "file"),
                           ("future.json.other", "file"), ("directory.json.retired", "directory"),
                           ("link.json.retired", "symlink"), ("public.json.retired", "mode")):
            with self.subTest(name=name):
                path = self.directory / name
                if kind == "directory":
                    path.mkdir(mode=0o700)
                elif kind == "symlink":
                    path.symlink_to(self.directory / "active.json")
                else:
                    path.write_bytes(b"unknown fixture object")
                    path.chmod(0o644 if kind == "mode" else 0o600)
                try:
                    self.assert_blocked(purge=True)
                finally:
                    path.rmdir() if kind == "directory" else path.unlink()
        self.directory.chmod(0o755)
        try:
            self.assert_blocked()
        finally:
            self.directory.chmod(0o700)
        self.assertNotEqual(os.geteuid(), 0)
        with self.assertRaisesRegex(ins.Stop, "root custody"):
            ins.managed_loss_directory(str(self.directory), False)

    def test_changed_rule_blocks_purge_of_both_produced_kinds(self):
        self.install_fixture()
        before = self.produced_records()
        self.rule.chmod(0o600)
        self.rule.write_bytes(b"changed owned-fixture rule")
        code, report = self.uninstall_fixture(purge=True)
        self.assertEqual(code, 3)
        self.assertTrue(report["left"])
        self.assertTrue(self.package.exists())
        self.assertEqual(self.rule.read_bytes(), b"changed owned-fixture rule")
        self.assertEqual({p.name: p.read_bytes() for p in self.directory.iterdir()}, before)

    def test_package_and_run_locks_still_fence_removal(self):
        self.install_fixture()
        self.produced_records()
        self.assertEqual(ins.live_runs(str(self.runs), True), [])
        lock = os.open(self.package, os.O_RDONLY | os.O_DIRECTORY)
        try:
            fcntl.flock(lock, fcntl.LOCK_SH | fcntl.LOCK_NB)
            code, report = self.uninstall_fixture(purge=True)
            self.assertEqual((code, report["stopped"]), (3, "package-in-use-or-admitting"))
            self.assertTrue(self.rule.exists())
            self.assertTrue(self.package.exists())
        finally:
            os.close(lock)
        private = self.directory.parent / "live" / "private"
        private.mkdir(parents=True, mode=0o700)
        lock = os.open(private / "lock", os.O_CREAT | os.O_RDWR, 0o600)
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            self.assert_blocked(purge=True)
        finally:
            os.close(lock)


if __name__ == "__main__":
    unittest.main()
