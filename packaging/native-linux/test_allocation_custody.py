"""B3: real front-door allocation/fork/failure paths, owned Python actors.

No provider, installed entry, profile, root privilege or foreign inventory.
Actor signals use pidfds opened for this fixture's fork/Popen identities.
Default controls establish source behavior, not native namespace containment.
An explicit owned userns driver can select the real namespace startup helper.
"""
import contextlib
import ctypes
import json
import os
from pathlib import Path
import pwd
import select
import signal
import socket
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

import frontdoor as fd


ENTRY = '''import json,os,sys
print(json.dumps({"entry":"setup-completed","launch":{},"entry_pid":os.getpid(),"proc_pid":os.readlink("/proc/self")}),flush=True)
for line in sys.stdin:
    if json.loads(line).get("cmd")=="close":
        print(json.dumps({"event":"terminal","status":"closed","session_control":{"retirement":{"eligible":True,"blocking":[]}}}),flush=True)
        print(json.dumps({"entry":"terminal","relay":"complete"}),flush=True)
        sys.exit(87)
'''


class AllocationCustody(unittest.TestCase):
    namespace = False  # Selected only by an owned unshare test driver.

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="fd-custody-")
        self.addCleanup(self.tmp.cleanup)
        self.base = Path(self.tmp.name)
        self.package = self.base / "pkg"
        self.package.mkdir()
        if self.namespace:
            runner = self.package / fd.RUNNER
            runner.parent.mkdir()
            runner.write_text("#!/usr/bin/python3 -IB\n" + ENTRY)
            runner.chmod(0o755)
        self.runs = self.base / "runs"
        self.runs.mkdir()
        self.user = pwd.getpwuid(os.getuid())
        self.site = {"run_base": str(self.runs), "cancel_grace_s": 1}
        self.checked = {"live": True, "children": None, "retention": "discard",
                        "deadline": 30, "route_name": "owned-fixture",
                        "route": {"executable": "/owned/unused", "settings": {}},
                        "policy": {"bash_authority": "trusted-task"}}
        self.actors = []
        self.entry_watch = None
        self.entry_pid = None
        self.addCleanup(self.collect_actors)

    def collect_actors(self):
        # No numeric PID signals, even after a fixture assertion fails.
        for pid, watch in self.actors:
            signalled = not select.select([watch], [], [], 0)[0]
            if signalled:
                signal.pidfd_send_signal(watch, signal.SIGKILL)
            self.assertTrue(select.select([watch], [], [], 5)[0], "owned supervisor end")
            collected, status = os.waitpid(pid, 0)
            self.assertEqual(collected, pid)
            print(json.dumps({"fixture_actor": "supervisor", "pid_in_fixture_view": pid,
                              "pidfd_end": True, "kill_requested": signalled,
                              "wait_exit": os.waitstatus_to_exitcode(status)}))
            os.close(watch)
        self.actors.clear()
        if self.entry_watch is not None:
            if not select.select([self.entry_watch], [], [], 1)[0]:
                signal.pidfd_send_signal(self.entry_watch, signal.SIGKILL)
            self.assertTrue(select.select([self.entry_watch], [], [], 5)[0], "owned entry end")
            print(json.dumps({"fixture_actor": "entry", "pid_in_fixture_view": self.entry_pid,
                              "pidfd_end": True, "wait_status": "owned by supervisor, not inferred"}))
            os.close(self.entry_watch)
            self.entry_watch = None

    def exercise(self, boundary=None, fault=None):
        parent = os.getpid()
        real_fork, real_read, real_close = os.fork, os.read, os.close
        real_allocate, real_private, real_start = fd.make_run, fd.write_private, fd.start_entry
        lines, allocated, retired = [], [], []
        real_retire = fd.retire

        def fork():
            pid = real_fork()
            if pid:
                self.actors.append((pid, os.pidfd_open(pid)))
            return pid

        def allocate(*args):
            result = real_allocate(*args)
            allocated.append(result)
            return result

        def start_entry(package, request_path):
            run = Path(request_path).parent.parent
            (run / "store").mkdir()
            (run / "launch").mkdir()
            (run / "launch" / "scratch").write_text("protected writer scratch")
            (run / "private" / fd.ROOT_TERMINAL).write_text(json.dumps({
                "authority": {"root": "owned-root", "generation": 4, "claim": "owned-claim"},
                "session_control": {"retirement": {"eligible": False, "blocking": ["owed"]}}}))
            def parent_death():
                ctypes.CDLL(None).prctl(1, int(signal.SIGKILL), 0, 0, 0)
            if self.namespace:
                entry, write = real_start(package, request_path)
            else:
                entry = subprocess.Popen([sys.executable, "-I", "-B", "-c", ENTRY],
                                         stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                         preexec_fn=parent_death, close_fds=True)
                read, write = os.pipe()
                real_close(read)
            (run / "private" / "fixture-entry-popen-pid").write_text(str(entry.pid))
            return entry, write

        def read(number, size):
            chunk = real_read(number, size)
            if os.getpid() == parent and chunk == b"live\n":
                run = Path(allocated[0][1])
                # Exact Popen identity in the fixture's view, never an owner
                # namespace field or a discovered process.
                self.entry_pid = int((run / "private" / "fixture-entry-popen-pid").read_text())
                self.entry_watch = os.pidfd_open(self.entry_pid)
                if boundary in ("readiness", "double", "lock-close"):
                    raise OSError("owned readiness read failure after live fork")
            return chunk

        inner_failed = False
        def emit(value):
            nonlocal inner_failed
            if boundary == "double" and value.get("stage") == "front-door-failed" and not inner_failed:
                inner_failed = True
                raise OSError("owned inner failure-terminal capture failure")
            lines.append(value)
            return True

        def close(number):
            real_close(number)
            if boundary == "lock-close" and os.getpid() == parent and allocated and number == allocated[0][2]:
                raise OSError("owned lock finalization failure")

        def private(path, value):
            if fault == "reservation" and path.endswith(fd.LIVE_RESERVATION):
                raise OSError("owned reservation failure")
            return real_private(path, value)

        def retire(*args, **kwargs):
            retired.append(args[0])
            return real_retire(*args, **kwargs)

        with contextlib.ExitStack() as stack:
            for patch in (
                mock.patch.object(fd, "package_root", return_value=str(self.package)),
                mock.patch.object(fd, "check_owned"),
                mock.patch.object(fd, "admit", return_value=(str(self.package), self.site, self.user, self.checked, {}, b"")),
                mock.patch.object(fd, "make_run", side_effect=allocate),
                mock.patch.object(fd, "entry_request", return_value={}),
                mock.patch.object(fd, "start_entry", side_effect=start_entry),
                mock.patch.object(fd.os, "fork", side_effect=fork),
                mock.patch.object(fd.os, "read", side_effect=read),
                mock.patch.object(fd.os, "close", side_effect=close),
                mock.patch.object(fd, "write_private", side_effect=private),
                mock.patch.object(fd, "emit", side_effect=emit),
                mock.patch.object(fd, "retire", side_effect=retire),
            ):
                stack.enter_context(patch)
            if fault == "publication":
                stack.enter_context(mock.patch.object(fd, "write_durable", side_effect=OSError("owned recording failure")))
            code = fd.run(["frontdoor", "run"], {})
        return code, lines, allocated, retired

    def test_postfork_readiness_and_double_failure_preserve_writer_and_owned_facts(self):
        for boundary in ("readiness", "double", "lock-close"):
            with self.subTest(boundary=boundary):
                code, lines, allocated, retired = self.exercise(boundary)
                terminal = lines[-1]
                self.assertEqual(code, fd.EXIT_UNKNOWN)
                self.assertEqual(terminal["effects"], "possible")
                self.assertNotEqual(terminal["stage"], "refused")
                facts = terminal["custody"]
                run = Path(allocated[0][1])
                self.assertEqual(facts["run_dir"], str(run))
                self.assertEqual(facts["supervisor_pid"], self.actors[-1][0])
                self.assertEqual(facts["entry"], "possible")
                self.assertIsNone(facts["entry_status"])
                self.assertEqual(facts["handle"]["socket"], str(run / fd.LIVE_SOCKET))
                self.assertTrue((run / "private" / "request.json").exists())
                self.assertEqual((run / "launch" / "scratch").read_text(), "protected writer scratch")
                self.assertEqual(json.loads((run / "private" / fd.ROOT_TERMINAL).read_text())["authority"]["claim"], "owned-claim")
                self.assertEqual(retired, [], "no disposition without an entry end")
                self.assertFalse(select.select([self.entry_watch], [], [], 0)[0], "fake writer remains live")
                self.assertTrue(fd.run_lock_held(str(run)), "detached supervisor retains the actual lock")
                self.collect_actors()
                self.assertFalse(fd.run_lock_held(str(run)))
                # Even exact fixture cleanup is not a product end record.
                # A free lock cannot replace the missing collected entry end.
                with mock.patch.object(fd, "check_owned"):
                    swept = fd.sweep(str(run.parent))
                item = next(item for item in swept if item["run"] == str(run))
                self.assertFalse(item["ok"])
                self.assertFalse(item["run_removed"])
                self.assertEqual((run / "launch" / "scratch").read_text(), "protected writer scratch")

    def test_partial_allocation_recording_failure_is_possible_and_blocks_sweep(self):
        code, lines, allocated, retired = self.exercise(fault="publication")
        self.assertEqual(code, fd.EXIT_RUN_FAILED)
        self.assertEqual(lines[-1]["effects"], "possible")
        facts = lines[-1]["custody"]
        self.assertEqual(facts["entry"], "not-started")
        self.assertEqual(facts["capture_errors"], ["entry-custody:OSError"])
        self.assertEqual(self.actors, [])
        run = Path(facts["run_dir"])
        self.assertTrue((run / "private" / "lock").exists())
        self.assertFalse(fd.run_lock_held(str(run)))
        with mock.patch.object(fd, "check_owned"):
            result = fd.sweep(str(run.parent))[0]
        self.assertFalse(result["ok"])
        self.assertTrue(run.exists(), "missing physical publication is not authoritative no-start")

    def test_allocation_only_failure_allows_positive_no_entry_discard(self):
        code, lines, allocated, retired = self.exercise(fault="reservation")
        self.assertEqual(code, fd.EXIT_RUN_FAILED)
        self.assertEqual(lines[-1]["effects"], "possible")
        self.assertEqual(lines[-1]["custody"]["entry"], "not-started")
        self.assertEqual(self.actors, [])
        self.assertEqual(retired, [allocated[0][1]])
        self.assertFalse(Path(allocated[0][1]).exists())

    def test_admission_refusal_retains_legitimate_no_effect_terminal(self):
        lines = []
        with mock.patch.object(fd, "package_root", return_value=str(self.package)), \
                mock.patch.object(fd, "check_owned"), \
                mock.patch.object(fd, "admit", side_effect=fd.Refused("owned prelaunch refusal")), \
                mock.patch.object(fd, "emit", side_effect=lambda value: lines.append(value)):
            code = fd.run(["frontdoor", "run"], {})
        self.assertEqual(code, fd.EXIT_REFUSED)
        self.assertEqual((lines[-1]["stage"], lines[-1]["effects"]), ("refused", "none"))
        self.assertEqual(list(self.runs.iterdir()), [])

    def test_positive_live_close_collects_entry_end_before_discard(self):
        code, lines, allocated, retired = self.exercise()
        self.assertEqual(code, 0)
        handle = lines[-1]["handle"]
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
            client.settimeout(5)
            client.connect(handle["socket"])
            client.sendall(fd.encoded({"v": 1, "attach": {"run": handle["run"], "token": handle["token"]}}))
            client.sendall(fd.encoded({"cmd": "close"}))
            with client.makefile("rb") as output:
                records = [json.loads(line) for line in output]
        terminal = next(item for item in records if item.get("frontdoor") == "terminal")
        if self.namespace:
            setup = next(item for item in records if item.get("entry") == "setup-completed")
            self.assertEqual((setup["entry_pid"], setup["proc_pid"]), (1, "1"))
            print(json.dumps({"owned_namespace_fixture": "actual start_entry", "entry_pid1_verified": True}))
        self.assertEqual(terminal["entry_status"], 87)
        self.assertTrue(terminal["retire"]["run_removed"])
        self.assertFalse(Path(allocated[0][1]).exists())
        self.assertTrue(select.select([self.entry_watch], [], [], 5)[0])
        self.assertTrue(select.select([self.actors[0][1]], [], [], 5)[0])

    def test_live_cap_refusal_precedes_run_allocation(self):
        lines = []
        with mock.patch.object(fd, "package_root", return_value=str(self.package)), \
                mock.patch.object(fd, "check_owned"), \
                mock.patch.object(fd, "admit", return_value=(str(self.package), self.site, self.user, self.checked, {}, b"")), \
                mock.patch.object(fd, "live_roots", return_value=fd.MAX_LIVE_ROOTS), \
                mock.patch.object(fd, "emit", side_effect=lambda value: lines.append(value)), \
                mock.patch.object(fd, "make_run") as allocate:
            code = fd.run(["frontdoor", "run"], {})
        self.assertEqual(code, fd.EXIT_REFUSED)
        self.assertEqual((lines[-1]["stage"], lines[-1]["effects"]), ("refused", "none"))
        allocate.assert_not_called()
        self.assertEqual(self.actors, [])


if __name__ == "__main__":
    unittest.main()
