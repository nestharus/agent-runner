"""Offline controls of live roots: the front door's detached owning
supervisor (`open_live`/`LiveRelay`) addressed by separate caller
processes (`native_call.py --root`). Unprivileged: no root, namespace,
native owner or model. Only `start_entry` is replaced (it needs a PID
namespace) by a scripted stand-in that speaks the owner's follow-up,
ACK, linked-message and turn-end records. Its PDEATHSIG did not end it
when the supervisor was killed; that control kills the stand-in explicitly.
What this does not establish: the
sudo/root path, the real owner/receiver, or namespace teardown."""

import ctypes
import fcntl
import json
import os
import pwd
import select
import signal
import socket
import subprocess
import sys
import tempfile
import threading
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
            say({"event": "terminal", "status": "closed", "async": {"accepted": 0, "turn_ended": 0, "undelivered": [], "owed": 0}})
            say({"entry": "terminal", "relay": "complete"}); sys.exit(87)
        elif cmd.get("cmd") == "cancel":
            say({"event": "terminal", "status": "cancelled", "async": {"accepted": 0, "turn_ended": 0, "undelivered": [], "owed": 0}})
            say({"entry": "terminal", "relay": "complete"}); sys.exit(82)
    sys.exit(70)
""")


def die_with_parent():
    ctypes.CDLL(None).prctl(1, int(signal.SIGKILL), 0, 0, 0)


def fake_start_entry(package, request_path, script=ENTRY):
    alive_r, alive_w = os.pipe()
    entry = subprocess.Popen([sys.executable, "-B", "-c", script], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
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

    def open(self, name="run1", deadline=60, grace=2, script=ENTRY):
        run = os.path.join(self.dir, name)
        os.makedirs(os.path.join(run, "private"))
        user = pwd.getpwuid(os.getuid())
        out_r, out_w = os.pipe()
        checked = {"deadline": deadline, "retention": "discard"}
        site = {"cancel_grace_s": grace}
        with mock.patch.object(frontdoor, "start_entry", lambda p, r: fake_start_entry(p, r, script)), \
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
        proc = subprocess.run([sys.executable, "-B", CALL, *argv, "--out", out], capture_output=True, timeout=60)
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

    def test_dead_supervisor_is_classified_dead_and_standin_is_explicitly_killed(self):
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


    def test_live_close_reconciles_async_across_earlier_attachments(self):
        for mode, expected in (("settled", (0, "closed")), ("trailing-settled", (0, "closed")),
                               ("lost", (10, "async-undelivered")), ("missing", (6, "incomplete")),
                               ("contradictory", (6, "incomplete")), ("owed", (6, "incomplete")),
                               ("no-entry-terminal", (6, "incomplete")), ("no-ack", (6, "incomplete")),
                               ("no-turn-end", (6, "incomplete")), ("status-conflict", (6, "incomplete"))):
            with self.subTest(mode=mode):
                script = async_entry(mode)
                run, terminal = self.open(name=mode, script=script)
                handle = self.handle_file(terminal)
                code, own = self.call("--root", handle, "--prompt-file", self.prompt("own input"))
                self.assertEqual((code, own["class"]), (0, "answered"))
                self.assertEqual(own["processing_completion"], "not-observed")
                code, closed = self.call("--root", handle, "--close")
                self.assertEqual((code, closed["class"]), expected, closed)
                print(json.dumps({"control": "live-async-close", "mode": mode, "actual_exit": code,
                                  "class": closed["class"], "async": closed["async"],
                                  "account_errors": closed["account_errors"], "physical_close": closed["physical_close"]}))
                self.assertTrue(closed["physical_close"])
                self.assertTrue(closed["front_door_terminal"]["retire"]["ok"])
                if mode == "lost":
                    self.assertEqual(closed["async"]["undelivered"][0]["work"], 2)
                if mode in ("settled", "trailing-settled"):
                    self.assertEqual(closed["async"]["accepted"], 1)
                    self.assertEqual(closed["async"]["turn_ended"], 1)
                self.assertEqual(waited(terminal["supervisor_pid"]), 87)


def async_entry(mode):
    script = ENTRY
    script = script.replace('n = 0\n', 'say({"event": "async-owed", "change": "owed", "work": 2, "owed_async": 1})\nn = 0\n')
    close_start = script.index('        say({"event": "terminal", "status": "closed"')
    close_end = script.index('    elif cmd.get("cmd") == "cancel":', close_start)
    lines = []
    if mode in ("settled", "trailing-settled", "contradictory", "no-entry-terminal", "no-ack", "no-turn-end", "status-conflict"):
        lines += [{"event": "bash-async-completion-admitted", "work": 2, "input": 2},
                  {"event": "ack", "index": 2, "message_id": "async2"},
                  {"event": "turn-end", "input": 2, "stop_reason": "end_turn"}]
        if mode != "trailing-settled":
            lines += [{"event": "async-owed", "change": "turn-ended", "work": 2, "owed_async": 0}]
    account = {"accepted": 1, "turn_ended": 1 if mode in ("settled", "trailing-settled", "no-entry-terminal", "no-ack", "no-turn-end", "status-conflict") else 0,
               "undelivered": [{"work": 2, "reason": "recipient-not-in-conversation"}] if mode == "lost" else [],
               "owed": 1 if mode == "owed" else 0}
    owner = {"event": "terminal", "status": "closed", "async": account}
    if mode == "missing":
        del owner["async"]
    if mode == "no-ack":
        lines = [e for e in lines if e.get("event") != "ack"]
    if mode == "no-turn-end":
        lines = [e for e in lines if e.get("event") != "turn-end"]
    if mode == "status-conflict":
        owner["status"] = "cancelled"
    lines.append(owner)
    if mode != "no-entry-terminal":
        lines.append({"entry": "terminal", "relay": "complete"})
    code = "".join("        say(" + repr(line) + ")\n" for line in lines) + "        sys.exit(87)\n"
    return script[:close_start] + code + script[close_end:]


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


class RecordBoundary(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.mkdtemp(dir=os.environ.get("TMPDIR"))
        self.listener, self.path = frontdoor.listen_live(self.directory, os.getuid())
        self.entry = subprocess.Popen([sys.executable, "-B", "-c", "import sys; sys.stdin.read()"],
                                      stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        self.relay = frontdoor.LiveRelay(self.entry, self.directory, 60, 1, self.listener, os.getuid(), "r", "t")
        self.clients = []

    def tearDown(self):
        self.relay.detach("test cleanup")
        for client in self.clients:
            client.close()
        self.listener.close()
        self.entry.kill()
        self.entry.wait(timeout=5)
        self.entry.stdin.close()
        self.entry.stdout.close()

    def attach(self):
        client = socket.socket(socket.AF_UNIX)
        self.clients.append(client)
        client.settimeout(5)
        client.connect(self.path)
        self.relay.accept()
        client.sendall(frontdoor.encoded({"v": 1, "attach": {"run": "r", "token": "t"}}))
        self.relay.hello(self.relay.waiting[0].recv(4096))
        return client

    def drain(self, client):
        while self.relay.output:
            self.relay.flush()
        data = b""
        while select.select([client], [], [], 0.05)[0]:
            data += client.recv(65536)
        return [json.loads(line) for line in data.splitlines()]

    def test_partial_send_then_eof_new_attach_has_whole_records_and_counted_loss(self):
        old = self.attach()
        self.drain(old)
        first = {"event": "agent-message", "text": "interrupted" * 20}
        later = {"event": "turn-end", "input": 1}
        self.relay.queue_output(first)
        self.relay.queue_output(later)
        conn = self.relay.client

        class InterruptedSend:
            def send(self, data):
                return conn.send(data[:17])
            def close(self):
                conn.close()

        self.relay.client = InterruptedSend()
        self.relay.flush()
        self.assertEqual(len(old.recv(4096)), 17)
        old.close()
        self.relay.client = conn
        self.relay.step(timeout=0.1)  # actual EOF/send failure boundary
        self.assertIsNone(self.relay.client)
        new = self.attach()
        records = self.drain(new)
        self.assertEqual(records[0]["frontdoor"], "attached")
        self.assertEqual(records[0]["backlog_dropped_total"], 1)
        self.assertEqual(records[0]["interrupted_records_total"], 1)
        self.assertEqual(records[0]["interrupted_prefix_bytes"], 17)
        self.assertEqual(records[0]["dropped_record_bytes_total"], len(frontdoor.encoded(first)))
        print(json.dumps({"control": "interrupted-record-new-attach", "partial_bytes_sent": 17, "records": records}))
        self.assertIn(later, records)
        self.assertNotIn(first, records)

    def test_overflow_and_invalid_entry_records_are_counted_without_invalid_framing(self):
        with mock.patch.object(frontdoor, "OUTPUT_LIMIT", 64):
            self.relay.queue_output({"text": "x" * 65})
        self.relay.entry_line(b"not-json")
        records = self.drain(self.attach())
        self.assertEqual(records[0]["backlog_dropped_total"], 2)
        self.assertIn("invalid-entry-record", self.relay.collection_errors)


class PublicLiveFailures(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.mkdtemp(dir=os.environ.get("TMPDIR"))

    def invoke(self, handle):
        path = os.path.join(self.directory, "handle.json")
        native_call.write_json(path, handle)
        out = os.path.join(self.directory, "out-" + str(time.monotonic_ns()))
        proc = subprocess.run([sys.executable, "-B", CALL, "--root", path, "--close", "--out", out, "--wait", "1"],
                              capture_output=True, timeout=15)
        self.assertNotIn(b"Traceback", proc.stderr)
        with open(os.path.join(out, "result.json")) as file:
            result = json.load(file)
        print(json.dumps({"control": "public-live-failure", "actual_exit": proc.returncode, "result": result, "stderr": proc.stderr.decode()}))
        self.assertEqual(proc.returncode, native_call.LIVE_EXITS[result["class"]])
        return proc, result

    def test_malformed_and_overlong_public_handle_have_declared_exit_and_stored_result(self):
        base = {"v": 1, "uid": os.getuid(), "run": "r", "token": "t", "socket": "/" + "x" * 108}
        for handle in ([], {**base, "socket": "/x\0y"}, {**base, "socket": "/bad\ud800"}, base):
            with self.subTest(handle=repr(handle)):
                proc, result = self.invoke(handle)
                self.assertEqual((proc.returncode, result["class"]), (3, "refused-locally"))

    def test_unusable_transport_maps_root_unavailable_without_claiming_root_state(self):
        # A regular file at the public address yields ECONNREFUSED on Linux,
        # so use a symlink loop to exercise an actual different connect error.
        address = os.path.join(self.directory, "loop")
        os.symlink("loop", address)
        proc, result = self.invoke({"v": 1, "uid": os.getuid(), "run": "r", "token": "t", "socket": address})
        self.assertEqual((proc.returncode, result["class"]), (17, "root-unavailable"))

    def test_peer_reset_during_hello_is_mapped_and_stored(self):
        address = os.path.join(self.directory, "reset")
        listener = socket.socket(socket.AF_UNIX)
        listener.bind(address)
        listener.listen(1)
        def reset():
            conn, _ = listener.accept()
            conn.close()
        worker = threading.Thread(target=reset)
        worker.start()
        try:
            proc, result = self.invoke({"v": 1, "uid": os.getuid(), "run": "r", "token": "t", "socket": address})
            self.assertIn(result["class"], ("root-unavailable", "root-refused"))
        finally:
            listener.close()
            worker.join(timeout=5)


    def test_malformed_terminal_transport_is_incomplete_without_traceback(self):
        address = os.path.join(self.directory, "malformed")
        listener = socket.socket(socket.AF_UNIX)
        listener.bind(address)
        listener.listen(1)
        def serve():
            conn, _ = listener.accept()
            with conn:
                conn.recv(4096)
                conn.sendall(frontdoor.encoded({"frontdoor": "attached"}))
                conn.recv(4096)
                conn.sendall(frontdoor.encoded({"frontdoor": "terminal", "exit": 87, "retire": []}))
        worker = threading.Thread(target=serve)
        worker.start()
        try:
            proc, result = self.invoke({"v": 1, "uid": os.getuid(), "run": "r", "token": "t", "socket": address})
            self.assertEqual((proc.returncode, result["class"]), (6, "incomplete"))
        finally:
            listener.close()
            worker.join(timeout=5)


class LiveAdmission(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.mkdtemp(dir=os.environ.get("TMPDIR"))
        self.site = {"run_base": self.directory, "cancel_grace_s": 1}
        self.user = pwd.getpwuid(os.getuid())
        self.locks = []
        self.listeners = []
        self.pids = []

    def tearDown(self):
        for pid in self.pids:
            try:
                os.kill(pid, signal.SIGKILL)
                os.waitpid(pid, 0)
            except (ProcessLookupError, ChildProcessError):
                pass
        for listener in self.listeners:
            listener.close()
        for lock in self.locks:
            os.close(lock)

    def existing(self):
        # Fixture roots have real held run locks and sockets. The tested new
        # admissions enter run_locked and its actual guard, never a manual gate.
        for _ in range(3):
            _, run, lock, _ = frontdoor.make_live_run(self.site, self.user)
            self.locks.append(lock)
            listener, _ = frontdoor.listen_live(run, os.getuid())
            self.listeners.append(listener)

    def spawn(self, notify, release, during_allocation=False):
        pid = os.fork()
        if pid == 0:
            try:
                checked = {"live": True, "credential": None, "children": None,
                           "retention": "discard", "deadline": 30, "route_name": "fixture",
                           "route": {"model": "fixture/scripted"}, "policy": {"bash_authority": "trusted-task"}}
                def paused_open(*args):
                    os.write(notify, b"in-progress\n")
                    os.read(release, 1)
                    return 0
                original = frontdoor.make_run
                def paused_allocate(*args):
                    os.write(notify, b"guard-held\n")
                    os.read(release, 1)
                    return original(*args)
                with mock.patch.object(frontdoor, "admit", return_value=("/pkg", self.site, self.user, checked, {}, b"")), \
                        mock.patch.object(frontdoor, "entry_request", return_value={}), \
                        mock.patch.object(frontdoor, "open_live", paused_open), \
                        mock.patch.object(frontdoor, "emit", lambda v: os.write(notify, frontdoor.encoded(v))):
                    if during_allocation:
                        with mock.patch.object(frontdoor, "make_run", paused_allocate):
                            code = frontdoor.run_locked(["frontdoor", "run"], {})
                    else:
                        code = frontdoor.run_locked(["frontdoor", "run"], {})
                os.write(notify, frontdoor.encoded({"exit": code}))
                os._exit(0)
            except BaseException:
                os._exit(1)
        self.pids.append(pid)
        return pid

    def line(self, fd):
        self.assertTrue(select.select([fd], [], [], 5)[0], "bounded child synchronization")
        return os.read(fd, 65536)

    def test_actual_run_locked_guard_and_inprogress_reservation_prevent_overbooking(self):
        with mock.patch.object(frontdoor, "check_owned"):
            self.existing()
            ready_r, ready_w = os.pipe()
            release_r, release_w = os.pipe()
            other_r, other_w = os.pipe()
            try:
                first = self.spawn(ready_w, release_r, during_allocation=True)
                self.assertIn(b"guard-held", self.line(ready_r))
                second = self.spawn(other_w, release_r)
                refused = self.line(other_r)
                while b'"exit"' not in refused:
                    refused += self.line(other_r)
                self.assertIn(b"requester admission busy", refused)
                self.assertIn(b'"exit": 90', refused)
                self.assertEqual(waited(second), 0)
                os.write(release_w, b"a")
                progressing = self.line(ready_r)
                while b"in-progress" not in progressing:
                    progressing += self.line(ready_r)
                # No fourth socket yet: the locked reservation itself counts.
                user_dir = os.path.join(self.directory, str(os.getuid()))
                self.assertEqual(frontdoor.live_roots(user_dir), 4)
                third = self.spawn(other_w, release_r)
                refused = self.line(other_r)
                while b'"exit"' not in refused:
                    refused += self.line(other_r)
                self.assertIn(b"4 already held", refused)
                self.assertIn(b'"exit": 90', refused)
                print(json.dumps({"control": "actual-guard-and-reservation", "guard_refusal_exit": 90,
                                  "reservation_refusal_exit": 90, "observed_locked_live_count": frontdoor.live_roots(user_dir)}))
                self.assertEqual(waited(third), 0)
                self.assertEqual(frontdoor.live_roots(user_dir), 4)
                os.write(release_w, b"b")
                self.assertEqual(waited(first), 0)
                self.assertEqual(frontdoor.live_roots(user_dir), 3, "dead reservation no longer counted")
            finally:
                for fd in (ready_r, ready_w, release_r, release_w, other_r, other_w):
                    os.close(fd)


class ClosePrecedence(unittest.TestCase):
    def test_cleanup_collection_cancel_and_loss_precedence_is_explicit(self):
        account = {"errors": ["missing-account"], "async": {"undelivered": [{"work": 2}]}}
        collection = {"eof": True, "errors": []}
        base = {"exit": 87, "retire": {"ok": True}}
        classify = lambda t, c=collection, cmd="close": native_call.end_class(t, account, c, cmd)
        self.assertEqual(classify({**base, "exit": 94}, cmd="cancel"), "cleanup-failed")
        self.assertEqual(classify(base, {"eof": False, "errors": []}, "cancel"), "incomplete")
        self.assertEqual(classify(base, cmd="cancel"), "cancelled")
        self.assertEqual(classify({**base, "exit": 92}), "cancelled")
        self.assertEqual(classify(base), "incomplete")
        account["errors"] = []
        self.assertEqual(classify(base), "async-undelivered")
        account["async"]["undelivered"] = []
        self.assertEqual(classify({**base, "live": {"backlog_dropped_total": 1}}), "incomplete")


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
