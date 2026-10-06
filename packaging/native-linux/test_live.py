"""Offline controls of live roots: the front door's detached owning
supervisor (`open_live`/`LiveRelay`) addressed by separate caller
processes (`native_call.py --root`). Unprivileged: no root, namespace,
native owner or model. Only `start_entry` is replaced (it needs a PID
namespace) by a scripted stand-in that speaks the owner's follow-up,
ACK, linked-message and turn-end records and dies with its supervisor
(PDEATHSIG), as the real entry does. What this does not establish: the
sudo/root path, the real owner/receiver, or namespace teardown."""

import ctypes
import json
import os
import pwd
import signal
import socket
import subprocess
import sys
import tempfile
import textwrap
import time
import types
import unittest
from unittest import mock

import frontdoor
import native_call

HERE = os.path.dirname(os.path.abspath(__file__))
CALL = os.path.join(HERE, "native_call.py")

ENTRY = textwrap.dedent("""
    import json, os, sys
    def say(v):
        print(json.dumps(v), flush=True)
    def turn(i, text):
        mid = "m%d" % i
        say({"event": "ack", "index": i, "message_id": mid, "label": "accepted", "durable": True})
        say({"event": "agent-message", "input": i, "parent_message_id": mid, "text": "%s pid=%d" % (text, os.getpid())})
        say({"event": "turn-end", "input": i, "stop_reason": "end_turn"})
    say({"entry": "setup-completed", "launch": {}})
    turn(0, "first")
    n = 0
    for line in sys.stdin:
        cmd = json.loads(line)
        if cmd.get("cmd") == "send":
            n += 1
            say({"event": "follow-up-received", "ref": cmd.get("ref"), "stage": "queued-not-admitted"})
            say({"event": "follow-up-admitted", "ref": cmd.get("ref"), "input": n, "durable": True})
            turn(n, "echo:" + cmd["text"])
        elif cmd.get("cmd") == "close":
            say({"event": "terminal", "status": "closed"}); sys.exit(87)
        elif cmd.get("cmd") == "cancel":
            say({"event": "terminal", "status": "cancelled"}); sys.exit(82)
    sys.exit(70)
""")


def die_with_parent():
    ctypes.CDLL(None).prctl(1, int(signal.SIGKILL), 0, 0, 0)


def fake_start_entry(package, request_path):
    alive_r, alive_w = os.pipe()
    entry = subprocess.Popen([sys.executable, "-c", ENTRY], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                             preexec_fn=die_with_parent, close_fds=True)
    os.close(alive_r)
    return entry, alive_w


def waited(pid, bound=15):
    """The supervisor's exit status within a bound, else None."""
    end = time.monotonic() + bound
    while time.monotonic() < end:
        done, status = os.waitpid(pid, os.WNOHANG)
        if done:
            return os.waitstatus_to_exitcode(status)
        time.sleep(0.05)
    return None


class LiveRoot(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp(dir=os.environ.get("TMPDIR"))
        self.pids = []

    def tearDown(self):
        for pid in self.pids:
            try:
                os.kill(pid, signal.SIGKILL)
                os.waitpid(pid, 0)
            except (ProcessLookupError, ChildProcessError):
                pass

    def open(self, name="run1", deadline=60, grace=2):
        run = os.path.join(self.dir, name)
        os.makedirs(os.path.join(run, "private"))
        user = pwd.getpwuid(os.getuid())
        out_r, out_w = os.pipe()
        checked = {"deadline": deadline, "retention": "discard"}
        site = {"cancel_grace_s": grace}
        with mock.patch.object(frontdoor, "start_entry", fake_start_entry), \
                mock.patch.object(frontdoor, "OUT_FD", out_w):
            code = frontdoor.open_live("/pkg", site, checked, name, run, os.path.join(run, "private", "request.json"), user)
        os.close(out_w)
        with os.fdopen(out_r, "rb") as file:
            lines = [json.loads(line) for line in file.read().splitlines()]
        self.assertEqual(code, 0)
        terminal = lines[-1]
        self.assertEqual(terminal["stage"], "live-opened")
        self.pids.append(terminal["supervisor_pid"])
        return run, terminal

    def call(self, *argv):
        out = os.path.join(self.dir, "out-%d" % time.monotonic_ns())
        proc = subprocess.run([sys.executable, CALL, *argv, "--out", out], capture_output=True, timeout=60)
        with open(os.path.join(out, "result.json")) as file:
            return proc.returncode, json.load(file)

    def prompt(self, text):
        path = os.path.join(self.dir, "p-%d" % time.monotonic_ns())
        with open(path, "w") as file:
            file.write(text)
        return path

    def opener(self, terminal, handle_path):
        out = os.path.join(self.dir, "opener")
        os.mkdir(out)
        call = types.SimpleNamespace(actions=open(os.path.join(out, "caller.jsonl"), "w"),
                                     capture=open(os.path.join(out, "events.jsonl"), "wb"), start=time.monotonic())
        args = types.SimpleNamespace(live_handle=handle_path, out=out, deadline=30)
        code = native_call.opened_live(args, call, terminal, {"errors": [], "exit": 0})
        with open(os.path.join(out, "result.json")) as file:
            return code, json.load(file)

    def test_one_supervisor_outlives_its_opening_call_and_takes_later_callers_inputs(self):
        run, terminal = self.open()
        handle = os.path.join(self.dir, "handle.json")
        code, opened = self.opener(terminal, handle)
        self.assertEqual((code, opened["class"]), (0, "answered"))
        self.assertEqual(opened["turn"]["input"], 0)
        self.assertTrue(opened["root"].startswith("live"))
        pid = terminal["supervisor_pid"]
        self.assertIsNone(os.waitpid(pid, os.WNOHANG)[0] or None, "supervisor stays after the opening call")
        entry_pids = set()
        for n, text in ((1, "second"), (2, "third")):
            code, result = self.call("--root", handle, "--prompt-file", self.prompt(text))
            self.assertEqual((code, result["class"]), (0, "answered"), result)
            turn = result["turn"]
            self.assertEqual((turn["input"], turn["admitted"], turn["ack"]["message_id"]), (n, True, "m%d" % n))
            self.assertEqual(turn["turn_end"]["input"], n)
            final = open(os.path.join(self.latest_out(), "final.md")).read()
            self.assertTrue(final.startswith("echo:" + text))
            entry_pids.add(final.rsplit("pid=", 1)[1])
            self.assertEqual(os.waitpid(pid, os.WNOHANG)[0], 0, "same supervisor still live")
        self.assertEqual(len(entry_pids), 1, "both later inputs reached the same live entry")
        code, closed = self.call("--root", handle, "--close")
        self.assertEqual((code, closed["class"]), (0, "closed"), closed)
        self.assertEqual(closed["front_door_terminal"]["exit"], 87)
        self.assertTrue(closed["front_door_terminal"]["retire"]["ok"])
        self.assertEqual(closed["front_door_terminal"]["live"]["attaches"], 4)
        self.assertEqual(waited(pid), 87)
        self.assertFalse(os.path.exists(run))
        code, stale = self.call("--root", handle, "--prompt-file", self.prompt("after close"))
        self.assertEqual((code, stale["class"]), (11, "root-absent"))

    def latest_out(self):
        outs = sorted(n for n in os.listdir(self.dir) if n.startswith("out-"))
        return os.path.join(self.dir, outs[-1])

    def handle_file(self, terminal, **changes):
        path = os.path.join(self.dir, "h-%d" % time.monotonic_ns())
        native_call.write_handle(path, {**terminal["handle"], **changes})
        return path

    def test_wrong_token_or_root_is_refused_and_the_root_continues(self):
        run, terminal = self.open()
        for changes, reason in (({"token": "0" * 32}, "handle-token"), ({"run": "other"}, "not-this-root")):
            code, result = self.call("--root", self.handle_file(terminal, **changes), "--prompt-file", self.prompt("x"))
            self.assertEqual((code, result["class"], result["reason"]), (14, "root-refused", reason))
        code, result = self.call("--root", self.handle_file(terminal), "--prompt-file", self.prompt("ok"))
        self.assertEqual((code, result["turn"]["input"]), (0, 1))
        code, result = self.call("--root", self.handle_file(terminal, uid=os.getuid() + 1), "--close")
        self.assertEqual((code, result["class"]), (3, "refused-locally"))
        code, closed = self.call("--root", self.handle_file(terminal), "--close")
        self.assertEqual(closed["front_door_terminal"]["live"]["refusals"], ["handle-token", "not-this-root"])

    def test_concurrent_second_caller_is_refused_busy(self):
        run, terminal = self.open()
        held = native_call.Attached(terminal["handle"], self.dir)
        self.assertIsNone(held.connect())
        code, result = self.call("--root", self.handle_file(terminal), "--prompt-file", self.prompt("x"))
        self.assertEqual((code, result["reason"]), (14, "busy"))
        held.write({"cmd": "close"})
        while held.next_event(time.monotonic() + 10) is not None:
            pass
        self.assertEqual(held.events[-1]["frontdoor"], "terminal")
        self.assertEqual(waited(terminal["supervisor_pid"]), 87)

    def test_dead_supervisor_is_classified_dead_and_its_entry_dies_with_it(self):
        run, terminal = self.open()
        handle = self.handle_file(terminal)
        code, result = self.call("--root", handle, "--prompt-file", self.prompt("x"))
        entry_pid = int(open(os.path.join(self.latest_out(), "final.md")).read().rsplit("pid=", 1)[1])
        os.kill(terminal["supervisor_pid"], signal.SIGKILL)
        self.assertEqual(waited(terminal["supervisor_pid"]), -9)
        code, result = self.call("--root", handle, "--prompt-file", self.prompt("x"))
        self.assertEqual((code, result["class"]), (12, "root-dead"))
        # Whether the entry tree follows its dead supervisor is the real
        # namespace entry's (unchanged `contained` PDEATHSIG, PID-namespace
        # init): a privileged qualification, not this stand-in's. Here the
        # stand-in's own PDEATHSIG was observed not to end it, so it is
        # killed explicitly rather than claimed.
        try:
            os.kill(entry_pid, signal.SIGKILL)
        except ProcessLookupError:
            pass

    def test_deadline_ends_an_unattended_root(self):
        run, terminal = self.open(deadline=1, grace=1)
        self.assertEqual(waited(terminal["supervisor_pid"]), 82)
        self.assertFalse(os.path.exists(run))
        code, result = self.call("--root", self.handle_file(terminal), "--close")
        self.assertEqual((code, result["class"]), (11, "root-absent"))


class ForeignPeer(unittest.TestCase):
    def test_peer_of_another_uid_is_refused_before_hello(self):
        directory = tempfile.mkdtemp(dir=os.environ.get("TMPDIR"))
        listener, path = frontdoor.listen_live(directory, os.getuid())
        entry = subprocess.Popen([sys.executable, "-c", "import sys; sys.stdin.read()"], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        self.addCleanup(lambda: (entry.kill(), entry.wait(), entry.stdin.close(), entry.stdout.close()))
        relay = frontdoor.LiveRelay(entry, directory, 60, 1, listener, os.getuid() + 1, "r", "t")
        client = socket.socket(socket.AF_UNIX)
        client.connect(path)
        relay.step(timeout=1)
        client.settimeout(5)
        self.assertEqual(json.loads(client.recv(4096))["reason"], "foreign-requester")
        self.assertEqual(relay.refusals, ["foreign-requester"])
        self.assertIsNone(relay.client)
        self.assertEqual(os.stat(path).st_mode & 0o777, 0o600)
        listener.close()


class LiveRequest(unittest.TestCase):
    def test_live_must_be_boolean(self):
        import test_frontdoor
        site = test_frontdoor.SITE
        base = {"v": 1, "route": "fixture", "message": "m", "cwd": "/tmp", "bash": {"authority": "trusted-task"}, "deadline_s": 10}
        self.assertFalse(frontdoor.check_request(base, site, test_frontdoor.NOW)["live"])
        self.assertTrue(frontdoor.check_request(dict(base, live=True), site, test_frontdoor.NOW)["live"])
        with self.assertRaises(frontdoor.Refused):
            frontdoor.check_request(dict(base, live="yes"), site, test_frontdoor.NOW)


if __name__ == "__main__":
    unittest.main()
