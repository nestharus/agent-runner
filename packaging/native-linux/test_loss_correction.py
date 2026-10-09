"""D1-D8 finite controls. Stand-in owner/caller output where explicit;
real namespace/fork/containment controls are in test_loss_namespace.py.
No model, sudo, install into the host, or private profile effects."""
import contextlib
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import types
import unittest
from unittest import mock

import build_package
import frontdoor as fd
import install_package as ins
import native_call as caller
import test_frontdoor as fixtures
OWED, SITE = fixtures.OWED, fixtures.SITE
from test_build_install import Scratch as InstallScratch, PACKAGE_ID
from test_native_call import Scratch as CallerScratch


class RetainedCorrection(fixtures.Retention):
    def test_large_terminal_is_bounded_and_private_fields_are_not_retained(self):
        terminal = {"status": "lost", "session_control": {
            "retirement": {"eligible": False, "blocking": ["owed"]},
            "subjects": [{"reading": "x" * 250} for _ in range(22000)],
            "lineage": {"authorities": [{"owner": "SYNTHETIC-TOKEN"}]},
            "recorded_actor_custody": {"works_unknown": 1, "incarnations_open": 2}},
            "required_account": {"records": ["SYNTHETIC-PRIVATE-BODY"]}}
        run = self.make_run("large", "discard", False, terminal)
        with mock.patch.object(fd, "check_owned"):
            result = fd.retire(run, "discard", package=self.package)
        self.assertTrue(result["run_removed"])
        directory = os.path.join(self.user_dir, fd.LOSS_ACCOUNTS)
        record = fd.read_loss_account(directory, "large")
        text = json.dumps(record)
        self.assertLess(len(text.encode()), fd.LOSS_RECORD_LIMIT)
        for secret in ("SYNTHETIC-TOKEN", "SYNTHETIC-PRIVATE-BODY", "subjects", "lineage"):
            self.assertNotIn(secret, text)
        self.assertEqual(record["last_root_terminal"]["session_control"]["recorded_actor_custody"]["works_unknown"], 1)

    def test_oversized_total_publication_preserves_source(self):
        run = self.make_run("oversized", "discard", False)
        account = {"schema": fd.STORE_ACCOUNT_SCHEMA, "requester": f"uid:{self.uid}",
                   "inputs": [], "complete": True, "root": "x" * fd.LOSS_RECORD_LIMIT}
        with mock.patch.object(fd, "check_owned"), mock.patch.object(fd, "store_account", return_value=(account, None)):
            result = fd.retire(run, "discard", package=self.package)
        self.assertFalse(result["run_removed"])
        self.assertTrue(os.path.isdir(os.path.join(run, "store")))
        self.assertFalse(result["loss_account"]["retained"])

    def test_incomplete_256_listing_keeps_omitted_input_evidence(self):
        run = self.make_run("partial", "discard", False)
        account = {"schema": fd.STORE_ACCOUNT_SCHEMA, "requester": f"uid:{self.uid}",
                   "inputs": [{"index": i} for i in range(256)], "inputs_total": 257,
                   "inputs_omitted": 1, "complete": False}
        with mock.patch.object(fd, "check_owned"), mock.patch.object(fd, "store_account", return_value=(account, None)):
            result = fd.retire(run, "discard", package=self.package)
        self.assertFalse(result["run_removed"])
        self.assertTrue(result["loss_account"]["retained"])
        self.assertEqual(self.loss_account("partial")["store_account"]["inputs_omitted"], 1)
        self.assertTrue(os.path.isdir(os.path.join(run, "store")))

    def test_malformed_terminal_stays_unknown_and_keeps_store(self):
        for index, terminal in enumerate(({"session_control": ["malformed"]},
                                          {"session_control": {"retirement": []}})):
            run = self.make_run(f"malformed{index}", "discard", False, terminal)
            with mock.patch.object(fd, "check_owned"):
                result = fd.retire(run, "discard", package=self.package)
            self.assertFalse(result["run_removed"])
            self.assertEqual(self.loss_account(f"malformed{index}")["last_root_terminal"]["knowledge"], "unknown")

    def test_refresh_preserves_known_ending_and_terminal_summary(self):
        run = self.make_run("refresh", "discard", False, OWED)
        with mock.patch.object(fd, "check_owned"), mock.patch.object(fd, "store_account", return_value=(None, "fixture-unavailable")):
            first = fd.retire(run, "discard", package=self.package,
                              capture={"by": "run-end", "entry_status": -9, "killed": True})
        self.assertFalse(first["run_removed"])
        os.unlink(os.path.join(run, "private", fd.ROOT_TERMINAL))
        with mock.patch.object(fd, "check_owned"):
            second = fd.sweep(self.user_dir, self.package)[0]
        self.assertTrue(second["run_removed"])
        record = self.loss_account("refresh")
        self.assertEqual(record["captured"]["by"], "sweep")
        self.assertEqual(record["ending_capture"]["entry_status"], -9)
        self.assertTrue(record["ending_capture"]["killed"])
        self.assertEqual(record["last_root_terminal"]["status"], "closed")
        self.assertIn("not observed", record["run"])

    def test_malformed_nested_and_identity_are_partial_not_uncaught_outage(self):
        run = self.make_run("nested", "discard", False)
        with mock.patch.object(fd, "check_owned"):
            fd.retire(run, "discard", package=self.package)
        path = Path(self.user_dir) / fd.LOSS_ACCOUNTS / "nested.json"
        record = json.loads(path.read_text())
        record["store_account"] = ["bad"]
        path.write_text(json.dumps(record))
        code, lines = self.route({"v": 1, "op": "loss-accounts"})
        self.assertEqual(code, fd.EXIT_UNKNOWN)
        self.assertEqual(lines[0]["error"], "ValueError")
        with self.assertRaises(fd.Refused):
            self.route({"v": 1, "op": "loss-account", "account": "nested"})
        record["store_account"] = None
        record["requester"] = "uid:foreign"
        path.write_text(json.dumps(record))
        with self.assertRaisesRegex(ValueError, "identity"):
            fd.read_loss_account(str(path.parent), "nested")

    def test_explicit_capture_only_stale_owned_runs_no_task_admission(self):
        lost = self.make_run("lost", "discard", False)
        live = self.make_run("live", "discard", True)
        user = types.SimpleNamespace(pw_uid=self.uid)
        lines = []
        with mock.patch.object(fd, "check_owned"), mock.patch.object(fd, "emit", side_effect=lambda v: lines.append(v) or True):
            code = fd.loss_route({"run_base": self.dir}, user,
                                 fd.loss_op({"v": 1, "op": "capture-loss-accounts"}), self.package)
        self.assertEqual(code, 0)
        self.assertFalse(os.path.exists(lost))
        self.assertTrue(os.path.exists(live))
        self.assertEqual(lines[-1]["runs"], 1)
        self.assertIn("capture/prune/discard", lines[-1]["effects"])
        self.assertEqual(self.accounts(), ["lost.json"])

    def test_explicit_capture_respects_keep_retention_but_produces_reading(self):
        run = self.make_run("kept", "keep", False)
        with mock.patch.object(fd, "check_owned"), mock.patch.object(fd, "emit", return_value=True):
            code = fd.loss_route({"run_base": self.dir}, types.SimpleNamespace(pw_uid=self.uid),
                                 {"op": "capture-loss-accounts"}, self.package)
        self.assertEqual(code, 0)
        self.assertTrue(os.path.isdir(os.path.join(run, "store")))
        self.assertEqual(self.loss_account("kept")["store_account"]["inputs_total"], 1)
        self.assertIn("retention policy", self.loss_account("kept")["run"])

    def test_first_directory_creation_syncs_parent_and_failed_sync_keeps_source(self):
        run = self.make_run("sync", "discard", False)
        with mock.patch.object(fd, "check_owned"), mock.patch.object(fd, "sync_directory", side_effect=OSError("fixture fsync failure")) as sync:
            result = fd.retire(run, "discard", package=self.package)
        self.assertEqual(sync.call_args.args[0], self.user_dir)
        self.assertFalse(result["run_removed"])
        self.assertTrue(os.path.exists(os.path.join(run, "store")))


class InstallerLoss(InstallScratch):
    def loss(self, dest, populated):
        directory = Path(dest) / "var/lib/oulipoly-native/runs/1000/loss-accounts"
        directory.mkdir(parents=True, mode=0o700)
        if populated:
            path = directory / "lost.json"
            path.write_text('{"opaque_fixture_evidence":true}')
            path.chmod(0o600)
        return directory

    def test_empty_and_populated_loss_directories_not_live_or_unknown(self):
        for populated in (False, True):
            base = Path(self.dir) / str(populated)
            directory = self.loss(base, populated)
            self.assertEqual(ins.live_runs(str(directory.parent.parent), True), [])
            (directory / "unexpected").mkdir()
            self.assertEqual(ins.live_runs(str(directory.parent.parent), True), [str(directory)])

    def test_nonpurging_replacement_preserves_loss_evidence(self):
        archive, digest = self.archive(self.stage())
        with contextlib.redirect_stdout(io.StringIO()):
            code, dest = self.install(archive, digest)
            self.assertEqual(code, 0)
            directory = self.loss(dest, True)
            code = ins.main(["uninstall", "--record", f"/opt/oulipoly-native/{PACKAGE_ID}.install.json",
                             "--dest-root", dest, "--unprivileged-test"])
        self.assertEqual(code, 3, "intentionally retained site effects retain cleanup record")
        self.assertTrue((directory / "lost.json").exists())
        self.assertFalse((Path(dest) / "opt/oulipoly-native" / PACKAGE_ID).exists())
        self.assertFalse((Path(dest) / "etc/sudoers.d/oulipoly-native").exists())
        stage = Path(self.stage())
        second = PACKAGE_ID + "-replacement"
        newstage = stage.with_name(second)
        stage.rename(newstage)
        manifest = json.loads((newstage / "MANIFEST.json").read_text())
        manifest["id"] = second
        (newstage / "MANIFEST.json").write_text(json.dumps(manifest))
        archive2 = str(Path(self.dir) / "replacement.tar.gz")
        build_package.write_archive(str(newstage), second, archive2, 1700000000)
        digest2 = hashlib.sha256(Path(archive2).read_bytes()).hexdigest()
        with contextlib.redirect_stdout(io.StringIO()):
            code, _ = self.install(archive2, digest2)
        self.assertEqual(code, 0)
        self.assertTrue((directory / "lost.json").exists())

    def test_explicit_purge_reports_evidence_deletion_never_settlement(self):
        archive, digest = self.archive(self.stage())
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            code, dest = self.install(archive, digest)
            directory = self.loss(dest, True)
            code = ins.main(["uninstall", "--record", f"/opt/oulipoly-native/{PACKAGE_ID}.install.json",
                             "--dest-root", dest, "--unprivileged-test", "--purge-site"])
        self.assertEqual(code, 0)
        self.assertFalse(directory.exists())
        self.assertIn("purged-not-settled", output.getvalue())

    def test_changed_rule_blocks_removal_and_does_not_purge_evidence(self):
        archive, digest = self.archive(self.stage())
        with contextlib.redirect_stdout(io.StringIO()):
            code, dest = self.install(archive, digest)
            directory = self.loss(dest, True)
            rule = Path(dest) / "etc/sudoers.d/oulipoly-native"
            rule.chmod(0o600)
            rule.write_text("changed admin rule")
            code = ins.main(["uninstall", "--record", f"/opt/oulipoly-native/{PACKAGE_ID}.install.json",
                             "--dest-root", dest, "--unprivileged-test", "--purge-site"])
        self.assertEqual(code, 3)
        self.assertTrue((directory / "lost.json").exists())


class CallerLoss(CallerScratch):
    def test_actual_cli_routes_without_prompt_or_provider_and_preserves_nonzero(self):
        script = self.write("frontdoor.py", '''import json,sys
r=json.loads(sys.stdin.readline())
assert set(r) <= {"v","op","account"}
print(json.dumps({"frontdoor":"loss-account","account":"a","fixture":True}),flush=True)
code=93 if r.get("account")=="unknown" else 0
print(json.dumps({"frontdoor":"terminal","stage":r["op"],"exit":code}),flush=True)
sys.exit(code)
''')
        for index, op in enumerate(caller.LOSS_OPS):
            out = self.path(str(index))
            argv = [sys.executable, caller.__file__, "--loss-op", op, "--out", out,
                    "--frontdoor", script, "--direct-requester-uid", "4242", "--deadline", "3"]
            if op in ("loss-account", "retire-loss-account"):
                argv += ["--account", "a"]
            result = subprocess.run(argv, capture_output=True, timeout=8)
            self.assertEqual(result.returncode, 0, result.stderr.decode())
            report = json.loads(Path(out, "result.json").read_text())
            self.assertEqual(report["class"], "loss-operation-complete")
            self.assertEqual(report["request"]["op"], op)
            self.assertIn("not semantic use", report["meaning"])
        out = self.path("unknown")
        code = caller.main(["--loss-op", "loss-account", "--account", "unknown", "--out", out,
                            "--frontdoor", script, "--direct-requester-uid", "4242", "--deadline", "3"])
        self.assertNotEqual(code, 0)
        self.assertEqual(json.loads(Path(out, "result.json").read_text())["front_door_exit"], 93)

    def test_public_admission_capture_does_not_check_provider_or_cwd(self):
        r, w = os.pipe()
        try:
            os.write(w, b'{"v":1,"op":"capture-loss-accounts"}\n')
            os.close(w)
            user = types.SimpleNamespace(pw_uid=4242)
            with mock.patch.object(fd, "check_package"), mock.patch.object(fd, "load_site", return_value=SITE), \
                 mock.patch.object(fd, "requester", return_value=user), \
                 mock.patch.object(fd, "check_provider_executable") as provider, \
                 mock.patch.object(fd, "check_cwd_as") as cwd:
                result = fd.admit(["frontdoor", "run"], {}, r, 0)
            self.assertEqual(result[3], {"op": "capture-loss-accounts"})
            provider.assert_not_called()
            cwd.assert_not_called()
        finally:
            os.close(r)
