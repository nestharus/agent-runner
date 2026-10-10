"""D2 G-A/G-B: owned residue, real tiny Popen/wait, private fake run trees.

No installed/native/provider/admin operation. Administrative guard selection
is mocked only against these owned fixtures; proof is never a PID inference.
"""
import contextlib
import copy
import fcntl
import json
import os
from pathlib import Path
import pwd
import select
import signal
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

import frontdoor as fd
import native_call as caller
from test_frontdoor import OWED, RETIRABLE


HEALTHY = '''import json,sys
for value in [
 {"entry":"setup-completed","launch":{}},
 {"event":"ack","index":0,"message_id":"m1"},
 {"event":"agent-message","input":0,"parent_message_id":"m1","text":"owned answer"},
 {"event":"turn-end","input":0,"stop_reason":"end_turn"},
 {"event":"terminal","status":"closed","session_control":{"retirement":{"eligible":True,"blocking":[]}}},
 {"entry":"terminal","relay":"complete"}]:
 print(json.dumps(value),flush=True)
sys.exit(87)
'''


class ResidueEvidence(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="fd-residue-evidence-")
        self.addCleanup(self.tmp.cleanup)
        self.base = Path(self.tmp.name)
        self.package = self.base / "package"
        self.package.mkdir()
        self.runs = self.base / "runs"
        self.runs.mkdir()
        self.user = pwd.getpwuid(os.getuid())
        self.user_dir = self.runs / str(self.user.pw_uid)
        self.user_dir.mkdir()
        self.site = {"run_base": str(self.runs), "cancel_grace_s": 1}
        self.store = {"schema": fd.STORE_ACCOUNT_SCHEMA, "requester": fd.control_requester(self.user.pw_uid),
                      "kind": "root_store_account", "complete": True, "inputs_total": 1,
                      "inputs_omitted": 0, "inputs": [{"index": 0, "state": "owed-insertion-unresolved"}],
                      "readings": {"owed": 1}}

    def run_tree(self, name, marker=None, terminal=OWED):
        run = self.user_dir / name
        (run / "private").mkdir(parents=True, mode=0o700)
        (run / "store").mkdir()
        (run / "launch").mkdir()
        (run / "launch" / "scratch").write_text("private scratch")
        (run / "private" / "lock").touch(mode=0o600)
        (run / "private" / "retention").write_text("discard")
        fd.write_private(run / "private" / "request.json", {"task": "private prompt"})
        fd.write_private(run / "private" / fd.ROOT_TERMINAL, terminal)
        if marker is not None:
            fd.write_private(run / "private" / fd.ENTRY_CUSTODY, dict(marker, run=name, run_dir=str(run)))
        return run

    def producer(self, code):
        entry = subprocess.Popen([sys.executable, "-I", "-B", "-c", code],
                                 stdin=subprocess.PIPE, stdout=subprocess.PIPE, close_fds=True)
        watch = os.pidfd_open(entry.pid)
        def collect():
            killed = not select.select([watch], [], [], 0)[0]
            if killed:
                signal.pidfd_send_signal(watch, signal.SIGKILL)
            status = entry.wait(timeout=5)
            self.assertTrue(select.select([watch], [], [], 5)[0])
            print(json.dumps({"owned_popen": entry.pid, "pidfd_end": True,
                              "wait_status": status, "cleanup_kill_requested": killed}))
            os.close(watch)
            entry.stdin.close()
            entry.stdout.close()
        self.addCleanup(collect)
        return entry

    def direct(self, fail_cleanup=False):
        prior = self.run_tree("prior-uncertain", {"entry": "possible", "entry_status": None})
        before = {str(p.relative_to(prior)): p.read_bytes() for p in prior.rglob("*") if p.is_file()}
        checked = {"live": False, "children": None, "retention": "discard", "deadline": 10,
                   "route_name": "owned-fixture", "route": {"executable": "/owned-unused", "settings": {}},
                   "policy": {"bash_authority": "trusted-task"}}
        entries = []
        def start_entry(*args):
            entry = self.producer(HEALTHY)
            entries.append(entry)
            read, alive = os.pipe()
            os.close(read)
            return entry, alive
        input_read, input_write = os.pipe()
        self.addCleanup(os.close, input_read)
        self.addCleanup(os.close, input_write)
        real_remove = fd.shutil.rmtree
        def remove(path, *args, **kwargs):
            if fail_cleanup and Path(path).parent == self.user_dir and Path(path) != prior:
                raise PermissionError("owned current cleanup failure")
            return real_remove(path, *args, **kwargs)
        with contextlib.ExitStack() as stack:
            output = stack.enter_context((self.base / "direct-output.jsonl").open("wb"))
            for patch in (
                mock.patch.object(fd, "check_owned"),
                mock.patch.object(fd, "admit", return_value=(str(self.package), self.site, self.user, checked, {}, b"")),
                mock.patch.object(fd, "entry_request", return_value={}),
                mock.patch.object(fd, "start_entry", side_effect=start_entry),
                mock.patch.object(fd, "OUT_FD", output.fileno()),
                mock.patch.object(fd.signal, "signal"),
                mock.patch.object(fd.shutil, "rmtree", side_effect=remove),
            ):
                stack.enter_context(patch)
            code = fd.run_locked(["frontdoor", "run"], {}, input_read)
        self.assertEqual(entries[0].wait(timeout=5), 87)
        lines = [json.loads(line) for line in (self.base / "direct-output.jsonl").read_bytes().splitlines()]
        self.assertEqual(before, {str(p.relative_to(prior)): p.read_bytes() for p in prior.rglob("*") if p.is_file()})
        return code, lines

    def test_prior_uncertainty_does_not_fail_healthy_direct_task_or_caller(self):
        code, lines = self.direct()
        self.assertEqual(code, 87)
        terminal = lines[-1]
        self.assertTrue(terminal["retire"]["ok"])
        self.assertEqual(terminal["swept"][0]["scope"], "previous-run")
        self.assertEqual(terminal["swept"][0]["disposition"], "retained-physical-uncertainty")
        self.assertFalse(terminal["swept"][0]["run_removed"])
        self.assertEqual(caller.classify(code, {"stdout_eof": True, "errors": []}, lines, {})[0], "answered")

    def test_actual_current_cleanup_failure_still_fails_with_prior_residue(self):
        code, lines = self.direct(fail_cleanup=True)
        self.assertEqual(code, fd.EXIT_CLEANUP_FAILED)
        self.assertFalse(lines[-1]["retire"]["ok"])
        self.assertEqual(caller.classify(code, {"stdout_eof": True, "errors": []}, lines, {})[0], "cleanup-failed")

    def route(self, op):
        lines = []
        with mock.patch.object(fd, "check_owned"), mock.patch.object(fd, "emit", side_effect=lambda v: lines.append(v) or True):
            code = fd.loss_route(self.site, self.user, op, str(self.package))
        self.assertEqual(code, 0)
        return lines

    def test_sweep_and_semantic_copy_retirement_preserve_actual_entry_wait(self):
        run = self.run_tree("waited")
        entry = self.producer("import sys;sys.exit(17)")
        status = entry.wait(timeout=5)
        self.assertEqual(status, 17)
        custody = fd.RunCustody()
        custody.run, custody.run_dir = run.name, str(run)
        custody.handle = {"token": "must-not-export", "socket": "private-address"}
        custody.ended(status)
        with mock.patch.object(fd, "check_owned"), mock.patch.object(fd, "store_account", return_value=(self.store, None)):
            result = fd.sweep(str(self.user_dir), str(self.package))[0]
        self.assertTrue(result["run_removed"])
        self.assertFalse(result["root_terminal"]["session_control"]["retirement"]["eligible"])
        record = self.route({"op": "loss-account", "account": run.name})[0]["record"]
        proof = record["physical_evidence"]
        self.assertEqual((proof["run"], proof["run_dir"], proof["entry_status"]), (run.name, str(run), status))
        self.assertEqual(proof["basis"]["kind"], "entry-poll-wait")
        self.assertEqual(record["store_account"]["readings"], {"owed": 1})
        self.assertNotIn("must-not-export", json.dumps(record))
        for _ in range(2):
            self.route({"op": "retire-loss-account", "account": run.name})
            archived = self.route({"op": "loss-account", "account": run.name})[0]["record"]
            self.assertEqual(archived["physical_evidence"], proof)
            self.assertTrue(archived["loss_account_retired"])
            self.assertIsNone(archived["store_account"])
        listing = self.route({"op": "loss-accounts"})
        self.assertEqual(listing[-1]["accounts"], 0)
        self.assertEqual(listing[-1]["physical_dispositions"], 1)

    def test_allocation_no_entry_fact_survives_removal_without_root_store(self):
        custody = fd.RunCustody()
        with mock.patch.object(fd, "check_owned"):
            _, run, _, _ = fd.make_run(self.site, self.user, custody)
            try:
                result = fd.retire(run, "discard", package=str(self.package))
            finally:
                custody.close_lock()
        self.assertTrue(result["run_removed"])
        proof = self.route({"op": "loss-account", "account": os.path.basename(run)})[0]["record"]["physical_evidence"]
        self.assertEqual((proof["entry"], proof["entry_status"]), ("not-started", None))
        self.assertEqual(proof["basis"]["kind"], "allocation-producer-no-entry")

    def test_proof_publication_failure_preserves_entire_source_before_prune(self):
        run = self.run_tree("publication", {"entry": "ended", "entry_status": 23})
        with mock.patch.object(fd, "check_owned"), mock.patch.object(fd, "write_durable", side_effect=OSError("owned evidence failure")):
            result = fd.sweep(str(self.user_dir), str(self.package))[0]
        self.assertFalse(result["ok"])
        self.assertTrue((run / "launch" / "scratch").exists())
        self.assertTrue((run / "private" / "request.json").exists())
        self.assertEqual(fd.physical_custody(str(run))[1]["entry_status"], 23)

    def test_unknown_invalid_and_semantically_retirable_are_not_physical_proof(self):
        for name, marker in (("absent", None), ("possible", {"entry": "possible", "entry_status": None}),
                             ("invalid", {"entry": "ended", "entry_status": True})):
            with self.subTest(name=name):
                run = self.run_tree(name, marker, RETIRABLE)
                with mock.patch.object(fd, "check_owned"):
                    result = fd.sweep(str(self.user_dir), str(self.package))
                self.assertFalse(next(r for r in result if r["run"] == str(run))["run_removed"])
                self.assertTrue((run / "launch" / "scratch").exists())
        run = self.user_dir / "invalid"
        (run / "private" / fd.ENTRY_CUSTODY).write_text("{}" + " " * (fd.HELLO_LIMIT + 1))
        self.assertFalse(fd.physical_custody(str(run))[0])

    def test_root_disposition_requires_identity_producer_basis_and_private_authority(self):
        run = self.run_tree("admin", {"entry": "possible", "entry_status": None})
        evidence = {"run": run.name, "run_dir": str(run), "entry": "not-started", "entry_status": None,
                    "basis": {"kind": "root-positive-producer-no-entry", "reference": "owned producer never spawned an entry"}}
        original = (run / "private" / fd.ENTRY_CUSTODY).read_bytes()
        with mock.patch.object(fd.os, "geteuid", return_value=1234):
            with self.assertRaises(fd.Refused):
                fd.record_physical_disposition(str(run), evidence)
        with mock.patch.object(fd.os, "geteuid", return_value=0):
            with self.assertRaises(fd.Refused):
                fd.record_physical_disposition(str(run), evidence)  # real ownership guard, nes-owned tree
            with mock.patch.object(fd, "check_owned"):
                for field, value in (("run", "someone-else"), ("run_dir", "/wrong"),
                                     ("entry_status", True), ("basis", {"kind": "free-lock", "reference": "not proof"})):
                    bad = copy.deepcopy(evidence)
                    bad[field] = value
                    with self.subTest(field=field), self.assertRaises(fd.Refused):
                        fd.record_physical_disposition(str(run), bad)
                self.assertEqual((run / "private" / fd.ENTRY_CUSTODY).read_bytes(), original)
                held = os.open(run / "private" / "lock", os.O_RDWR)
                try:
                    fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    with self.assertRaises(BlockingIOError):
                        fd.record_physical_disposition(str(run), evidence)
                finally:
                    os.close(held)
                (run / "private").chmod(0o755)
                with self.assertRaises(fd.Refused):
                    fd.record_physical_disposition(str(run), evidence)
                (run / "private").chmod(0o700)
                self.assertEqual(fd.record_physical_disposition(str(run), evidence), evidence)
                with self.assertRaises(fd.Refused):
                    fd.record_physical_disposition(str(run), evidence)
        # Publishing ROOT's explicit evidence does no removal itself.
        self.assertTrue((run / "launch" / "scratch").exists())
        with mock.patch.object(fd, "check_owned"), mock.patch.object(fd, "store_account", return_value=(self.store, None)):
            self.assertTrue(fd.sweep(str(self.user_dir), str(self.package))[0]["run_removed"])
        self.assertEqual(self.route({"op": "loss-account", "account": run.name})[0]["record"]["physical_evidence"], evidence)

    def test_wrong_marker_identity_and_retire_publication_error_preserve_facts(self):
        run = self.run_tree("identity", {"entry": "ended", "entry_status": 31})
        marker = run / "private" / fd.ENTRY_CUSTODY
        value = json.loads(marker.read_text())
        value["run"] = "another-run"
        marker.write_text(json.dumps(value))
        with mock.patch.object(fd, "check_owned"):
            self.assertFalse(fd.sweep(str(self.user_dir), str(self.package))[0]["run_removed"])
        self.assertTrue((run / "private" / "request.json").exists())
        value["run"] = run.name
        marker.write_text(json.dumps(value))
        with mock.patch.object(fd, "check_owned"), mock.patch.object(fd, "store_account", return_value=(self.store, None)):
            self.assertTrue(fd.sweep(str(self.user_dir), str(self.package))[0]["run_removed"])
        record = self.route({"op": "loss-account", "account": run.name})[0]["record"]
        with mock.patch.object(fd, "check_owned"), mock.patch.object(fd, "write_durable", side_effect=OSError("owned retire publication failure")):
            with self.assertRaises(OSError):
                fd.loss_route(self.site, self.user, {"op": "retire-loss-account", "account": run.name})
        self.assertEqual(self.route({"op": "loss-account", "account": run.name})[0]["record"], record)
        # Neither a semantic record's existence nor its complete store proves
        # entry end. A copy-only legacy reading has no positive physical field.
        directory = self.user_dir / fd.LOSS_ACCOUNTS
        legacy = dict(record, account="legacy", physical_evidence=None)
        fd.write_durable(str(directory), "legacy.json", legacy)
        self.assertIsNone(self.route({"op": "loss-account", "account": "legacy"})[0]["record"]["physical_evidence"])


if __name__ == "__main__":
    unittest.main()
