"""Disposable proof of the preserved reinstall operator's filesystem control.

The service manager is simulated. Census inputs are real child processes read
from /proc, but only those children are enumerated. Nothing here is a host
replacement, UID split, restoration or live Broker proof.
"""

import hashlib
import io
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import unittest
import uuid
from contextlib import redirect_stdout
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import reinstall_preserved_v30 as operator
from first_install_v30 import IMAGES, build
from install_first_host_v30 import check, install


ATTEMPT = "b8e06d25-70fe-4f1f-bfef-3541913346e1"


class SimulatedService:
    """Stands in for systemd: one real Broker-image child, stop = terminate."""

    def __init__(self, broker_image):
        self.broker = subprocess.Popen([str(broker_image), "600"])
        self.active, self.others, self.stops, self.leftover = True, [], 0, None

    def show(self):
        return {"ActiveState": "active" if self.active else "inactive",
                "MainPID": self.broker.pid if self.active else 0, "SubState": "simulated"}

    def stop(self):
        self.stops += 1
        self.broker.terminate()
        self.broker.wait()
        self.active = False
        if self.leftover:
            self.others.append(subprocess.Popen(self.leftover))

    def pids(self):
        return [child.pid for child in (self.broker, *self.others) if child.poll() is None]

    def close(self):
        for child in (self.broker, *self.others):
            if child.poll() is None:
                child.kill()
                child.wait()


class PreservedReinstallTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        service = Path(__file__).with_name("oulipoly-kernel-broker.service")
        images = {}
        for role in IMAGES:
            images[role] = self.root / ("old-" + role)
            images[role].write_bytes(("old-" + role).encode())
        # The simulated Broker must be a real executable image.
        shutil.copyfile("/bin/sleep", images["broker"])
        self.old_package = self.root / "old.tar.gz"
        old = build(images, service, self.old_package, "0.1.0")
        images["runner"].write_bytes(b"new-runner")
        self.new_package = self.root / "new.tar.gz"
        new = build(images, service, self.new_package, "0.1.0")
        self.fixture = self.root / "host"
        self.fixture.mkdir()
        install(self.old_package, self.fixture)
        self.lay = operator.layout(self.fixture, old["generation"])
        state = self.lay["state"]
        state.parent.mkdir(parents=True)
        state.mkdir(mode=0o700)
        (state / "works").mkdir(mode=0o700)
        # Real SQLite container, with fixture-only contents; this does not
        # claim to implement the Rust State/sidecar/lane schemas.
        with sqlite3.connect(state / "state.db") as db:
            db.execute("CREATE TABLE fixture_record (attempt TEXT)")
            db.execute("INSERT INTO fixture_record VALUES (?)", (ATTEMPT,))
        (state / "state.db").chmod(0o600)
        (state / "entry-gate.lock").write_bytes(b"")
        (state / "installed-launches").mkdir(mode=0o700)
        (state / "installed-launches" / ATTEMPT).write_bytes(b"failed help attempt")
        (state / "works" / "link").symlink_to("../state.db")
        # Derived from fresh_lane.rs:80-97,162-203: metadata is taken from
        # state.db and fresh_lane has lane_id/domain_id/source_generation.
        # :246-264 compares this same pair to the sidecar's bound file.
        info = os.lstat(state / "state.db")
        source = {"source_generation": str(uuid.uuid4()), "domain_id": str(uuid.uuid4()),
                  "state_device": info.st_dev, "state_inode": info.st_ino,
                  "fresh_lane": {"lane_id": str(uuid.uuid4()), "domain_id": str(uuid.uuid4()),
                                 "source_generation": str(uuid.uuid4())}}
        (state / "empty-v30-bootstrap-v1.json").write_text(json.dumps(source))
        (state / "empty-v30-bootstrap-v1.json").chmod(0o600)
        # first_install_activation.rs:28-47,142-156,213-217: the source is
        # embedded unchanged; all five fingerprints come from installed files.
        def fingerprint(path):
            meta = os.lstat(path)
            return {"device": meta.st_dev, "inode": meta.st_ino,
                    "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}

        activation = {"schema": 1, "source": source, "pair_generation": old["generation"],
                      "manifest": fingerprint(self.lay["images"] / "install-v1.json"),
                      **{role: fingerprint(self.lay["images"] / name)
                         for role, name in IMAGES.items()}}
        (state / "first-install-activation-v1.json").write_text(json.dumps(
            activation))
        (state / "first-install-activation-v1.json").chmod(0o600)
        self.lay["runtime"].mkdir(parents=True)
        self.host = SimulatedService(self.lay["images"] / IMAGES["broker"])
        self.addCleanup(self.host.close)
        self.args = SimpleNamespace(
            old_package=self.old_package, new_package=self.new_package,
            old_generation=old["generation"], new_generation=new["generation"],
            source_generation=source["source_generation"], broker_pid=self.host.broker.pid)
        self.identities = {item: operator.capture(self.lay[item]) for item in operator.ITEMS}
        operator.CHANGES.clear()

    def run_op(self, command):
        return getattr(operator, command)(self.args, self.host, self.fixture)

    def cli_status(self):
        # Exercise the production JSON/exit-code adapter with fixture status.
        # No production layout, systemd command or root operation is executed.
        argv = ["reinstall_preserved_v30.py", "status", str(self.old_package),
                str(self.new_package), "--old-generation", self.args.old_generation,
                "--new-generation", self.args.new_generation,
                "--source-generation", self.args.source_generation,
                "--broker-pid", str(self.args.broker_pid)]
        report = self.run_op("status")
        output = io.StringIO()
        with patch.object(sys, "argv", argv), patch.object(operator.os, "geteuid", return_value=0), \
                patch.object(operator, "SystemdHost"), \
                patch.object(operator, "status", return_value=report), redirect_stdout(output):
            code = operator.main()
        return code, json.loads(output.getvalue())

    def assert_untouched(self):
        self.assertEqual(self.host.stops, 0)
        self.assertFalse(self.lay["preserved"].exists())
        for item in operator.ITEMS:
            self.assertEqual(operator.capture(self.lay[item]), self.identities[item])

    def test_preserve_moves_exact_identities_then_new_install_and_restore_refusal(self):
        before = self.run_op("status")
        self.assertEqual(set(before["locations"].values()), {"original"})
        self.assertEqual(before["readiness"], "ready")
        self.assertEqual(self.cli_status()[0], 0)
        self.assertEqual([a["reasons"] for a in before["census"]["actors"]],
                         [["image:oulipoly-kernel-broker"]])
        result = self.run_op("preserve")
        self.assertEqual(self.host.stops, 1)
        for item in operator.ITEMS:
            self.assertFalse(operator._exists(self.lay[item]))
            # Same bytes, device and inode at the preserved path.
            self.assertEqual(operator.capture(self.lay["moved"][item]), self.identities[item])
        self.assertEqual(result["drained"]["census"], {"actors": [], "unknown": []})
        baseline = operator.read_record(self.lay["preserved"] / operator.BASELINE)
        self.assertIn("installed-launches/" + ATTEMPT, baseline["trees"]["state"])
        self.assertEqual(os.stat(self.lay["preserved"]).st_mode & 0o777, 0o700)
        # The existing first-install path now publishes the new pair; the old
        # served State is not at the bootstrap path and is never reused.
        install(self.new_package, self.fixture)
        check(self.new_package, self.fixture)
        self.assertFalse(operator._exists(self.lay["state"]))
        # Stand in for activation's new State publication. Broker activation,
        # startup and readback themselves are not executed by this fixture.
        self.lay["state"].mkdir(mode=0o700)
        (self.lay["state"] / "state.db").write_bytes(b"new fixture State")
        after = self.run_op("status")
        self.assertEqual(after["locations"], {item: "both" for item in operator.ITEMS})
        self.assertEqual(after["matches_baseline"], {item: True for item in operator.ITEMS})
        self.assertEqual(after["verdict"], "preserved")
        self.assertEqual(after["preservation_verdict"],
                         "preserved old State, images and unit match baseline")
        self.assertEqual(after["comparison_paths"],
                         {item: str(self.lay["moved"][item]) for item in operator.ITEMS})
        self.assertEqual(self.cli_status()[0], 0)
        with self.assertRaisesRegex(operator.Refusal, "cannot restore unit"):
            self.run_op("restore")
        for item in operator.ITEMS:
            self.assertEqual(operator.capture(self.lay["moved"][item]), self.identities[item])

    def test_replaced_state_file_and_directory_identity_refuse_before_change(self):
        state = self.lay["state"]
        source_path = state / "empty-v30-bootstrap-v1.json"
        activation_path = state / "first-install-activation-v1.json"
        source = operator.read_record(source_path)
        activation = operator.read_record(activation_path)
        # Even records agreeing with each other must name the actual State
        # file. This old operator/fixture misconception must be rejected.
        directory = os.lstat(state)
        wrong_source = dict(source, state_device=directory.st_dev, state_inode=directory.st_ino)
        source_path.write_text(json.dumps(wrong_source))
        activation_path.write_text(json.dumps(dict(activation, source=wrong_source)))
        self.identities["state"] = operator.capture(state)
        self.assertEqual(self.cli_status()[0], 1)
        with self.assertRaisesRegex(operator.Refusal, "not the recorded served"):
            self.run_op("preserve")
        self.assert_untouched()
        source_path.write_text(json.dumps(source))
        activation_path.write_text(json.dumps(activation))
        # Same bytes in a different inode are no longer the served source.
        replacement = state / "replacement.db"
        shutil.copyfile(state / "state.db", replacement)
        replacement.chmod(0o600)
        replacement.replace(state / "state.db")
        self.identities["state"] = operator.capture(state)
        self.assertEqual(self.cli_status()[0], 1)
        with self.assertRaisesRegex(operator.Refusal, "not the recorded served"):
            self.run_op("preserve")
        self.assert_untouched()

    def test_final_status_reports_each_changed_or_missing_preserved_item(self):
        self.run_op("preserve")
        install(self.new_package, self.fixture)
        self.lay["state"].mkdir(mode=0o700)
        (self.lay["state"] / "state.db").write_bytes(b"new fixture State")
        for item in operator.ITEMS:
            with self.subTest(item=item):
                path = (self.lay["moved"][item] / "state.db" if item == "state" else
                        self.lay["moved"][item] / IMAGES["runner"] if item == "images" else
                        self.lay["moved"][item])
                mode = path.stat().st_mode & 0o777
                path.chmod(mode ^ 0o100)
                code, report = self.cli_status()
                self.assertEqual(code, 1)
                self.assertEqual(report["matches_baseline"][item], False)
                self.assertEqual(report["verdict"], "not ready")
                path.chmod(mode)
                self.assertEqual(self.cli_status()[0], 0)
                # Missing preservation must not borrow the new original.
                moved = self.lay["moved"][item]
                hidden = moved.with_name(moved.name + ".fixture-hidden")
                moved.rename(hidden)
                code, report = self.cli_status()
                self.assertEqual(code, 1)
                self.assertEqual(report["matches_baseline"][item], False)
                hidden.rename(moved)

    def test_partial_preservation_status_is_not_success(self):
        real = operator.rename_noreplace

        def stop_after_state(source, destination):
            if source == self.lay["images"]:
                raise OSError("fixture interruption")
            real(source, destination)

        with patch.object(operator, "rename_noreplace", stop_after_state):
            with self.assertRaisesRegex(OSError, "fixture interruption"):
                self.run_op("preserve")
        code, report = self.cli_status()
        self.assertEqual(code, 1)
        self.assertEqual(report["matches_baseline"], {item: True for item in operator.ITEMS})
        self.assertEqual(report["verdict"], "not ready")

    def test_restore_before_new_install_returns_same_inodes(self):
        self.run_op("preserve")
        self.run_op("restore")
        for item in operator.ITEMS:
            self.assertEqual(operator.capture(self.lay[item]), self.identities[item])
        check(self.old_package, self.fixture)
        self.assertEqual(operator.locations(self.lay),
                         {item: "original" for item in operator.ITEMS})

    def test_other_actor_refuses_before_stop(self):
        holder = subprocess.Popen([sys.executable, "-c",
            "import sys,time; f=open(sys.argv[1],'rb'); print(flush=True); time.sleep(600)",
            str(self.lay["state"] / "state.db")], stdout=subprocess.PIPE)
        self.host.others.append(holder)
        holder.stdout.readline()  # the State file is open
        holder.stdout.close()
        runner = subprocess.Popen([str(self.lay["images"] / IMAGES["broker"]), "600"])
        self.host.others.append(runner)
        with self.assertRaisesRegex(operator.Refusal, "other or unknown actor") as caught:
            self.run_op("preserve")
        self.assertIn("holds:", str(caught.exception))
        self.assert_untouched()
        # A census that cannot read a process is also a refusal.
        holder.kill(), runner.kill(), holder.wait(), runner.wait()
        with patch.object(operator.os, "readlink", side_effect=PermissionError("denied")):
            with self.assertRaisesRegex(operator.Refusal, "other or unknown") as caught:
                self.run_op("preserve")
        self.assertIn('"error": "denied"', str(caught.exception))
        self.assert_untouched()

    def test_unconfirmed_drain_keeps_stop_and_moves_nothing_until_retry(self):
        self.host.leftover = [str(self.lay["images"] / IMAGES["broker"]), "600"]
        with self.assertRaisesRegex(operator.Refusal, "drain not confirmed"):
            self.run_op("preserve")
        self.assertEqual(self.host.stops, 1)
        self.assertTrue(any("stopped" in change for change in operator.CHANGES))
        self.assertEqual(sorted(os.listdir(self.lay["preserved"])), [operator.PRE_STOP])
        for item in operator.ITEMS:
            self.assertEqual(operator.capture(self.lay[item]), self.identities[item])
        status = self.run_op("status")
        self.assertEqual(len(status["census"]["actors"]), 1)
        self.host.others[0].kill()
        self.host.others[0].wait()
        self.run_op("preserve")
        self.assertEqual(self.host.stops, 1)
        self.assertEqual(set(operator.locations(self.lay).values()), {"preserved"})

    def test_interrupted_move_is_visible_resumable_and_change_refuses(self):
        real = operator.rename_noreplace
        calls = []

        def interrupt_second(source, destination):
            calls.append(source)
            if len(calls) == 2:
                raise OSError("simulated interruption")
            real(source, destination)

        with patch.object(operator, "rename_noreplace", interrupt_second):
            with self.assertRaisesRegex(OSError, "simulated"):
                self.run_op("preserve")
        self.assertEqual(operator.locations(self.lay),
                         {"state": "preserved", "images": "original", "unit": "original"})
        status = self.run_op("status")
        self.assertEqual(status["matches_baseline"],
                         {"state": True, "images": True, "unit": True})
        # An image changed after the quiescent baseline refuses before moving.
        image = self.lay["images"] / IMAGES["bash"]
        body = image.read_bytes()
        image.chmod(0o755)
        image.write_bytes(b"changed")
        with self.assertRaisesRegex(operator.Refusal, "images changed since"):
            self.run_op("preserve")
        image.write_bytes(body)
        image.chmod(0o555)
        with self.assertRaisesRegex(operator.Refusal, "images changed since"):
            self.run_op("preserve")  # the mtime is identity too; no silent normalization
        self.assertEqual(operator.locations(self.lay)["images"], "original")

    def test_mismatched_install_or_expectation_refuses_untouched(self):
        cases = [("old_generation", "wrong", "old package generation"),
                 ("new_generation", "wrong", "new package generation"),
                 ("source_generation", "other", "not the recorded served"),
                 ("broker_pid", 1, "expected live Broker")]
        for field, value, message in cases:
            original = getattr(self.args, field)
            setattr(self.args, field, value)
            with self.assertRaisesRegex(operator.Refusal, message):
                self.run_op("preserve")
            setattr(self.args, field, original)
            if field != "old_generation":
                self.assert_untouched()
        stage = self.lay["state"].parent / ".empty-v30-bootstrap-abandoned"
        stage.mkdir()
        self.assertIn("interrupted install", self.run_op("status")["readiness"])
        with self.assertRaisesRegex(operator.Refusal, "interrupted install"):
            self.run_op("preserve")
        stage.rmdir()
        self.assert_untouched()


if __name__ == "__main__":
    unittest.main()
