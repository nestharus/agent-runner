"""Offline checks of the front door's admission, environment,
control, retention and relay rules. Unprivileged: nothing here is root,
no namespace is made, no native root or model is started. The relay runs
against a scripted stand-in for the entry."""

import fcntl
import json
import os
import pwd
import shutil
import subprocess
import sys
import tempfile
import textwrap
import time
import types
import unittest
from unittest import mock

import frontdoor

NOW = 1_800_000_000
from test_provider_route import SITE as PROVIDER_SITE, PROVIDER
SITE = dict(PROVIDER_SITE, routes={"sol-high": PROVIDER, "fixture": PROVIDER})


def request(**overrides):
    value = {
        "v": 1,
        "route": "sol-high",
        "message": "look",
        "cwd": "/home/nes/project",
        "bash": {"authority": "trusted-task"},
        "deadline_s": 1800,
    }
    value.update(overrides)
    return value


def refused(test, value, fragment, site=SITE):
    with test.assertRaises(frontdoor.Refused) as caught:
        frontdoor.check_request(value, json.loads(json.dumps(site)), NOW)
    test.assertIn(fragment, str(caught.exception))
    test.assertNotIn("fixture-access-marker", str(caught.exception))


class Admission(unittest.TestCase):

    def test_refuses_shape(self):
        refused(self, request(user="root"), "unknown fields")
        refused(self, request(store="/tmp/x"), "unknown fields")
        refused(self, request(v=2), "v must be 1")
        refused(self, request(route="raw"), "route")
        refused(self, request(message="  "), "message")
        refused(self, request(cwd="relative"), "cwd")
        refused(self, request(deadline_s=3601), "deadline_s")
        refused(self, request(deadline_s=0), "deadline_s")
        refused(self, request(deadline_s=True), "deadline_s")
        refused(self, request(retention="keep"), "retention")
        refused(self, request(bash={"authority": "trusted-task", "allow": ["ls"]}), "bash")
        refused(self, request(bash={"allow": []}), "bash")
        refused(self, request(bash={"authority": "root"}), "bash")

    def test_keep_only_when_site_allows(self):
        site = dict(SITE, allow_keep=True)
        self.assertEqual(frontdoor.check_request(request(retention="keep"), site, NOW)["retention"], "keep")

    def test_allow_list_policy(self):
        checked = frontdoor.check_request(request(bash={"allow": ["git status"]}), SITE, NOW)
        self.assertEqual(checked["policy"], {"bash_allow": ["git status"]})






class Environment(unittest.TestCase):
    user = types.SimpleNamespace(pw_name="nes", pw_uid=1000, pw_gid=1000, pw_dir="/home/nes", pw_shell="/bin/bash")

    def test_fixed_identity_and_passable_names(self):
        env = frontdoor.root_env(self.user, SITE, {"PATH": "/home/nes/.cargo/bin:/usr/bin", "NODE_OPTIONS": "x", "LC_ALL": "C"})
        self.assertEqual(env["HOME"], "/home/nes")
        self.assertEqual(env["USER"], "nes")
        self.assertEqual(env["PATH"], "/home/nes/.cargo/bin:/usr/bin")
        self.assertEqual(frontdoor.root_env(self.user, SITE, {})["PATH"], SITE["default_path"])

    def test_refuses_loader_libc_reserved_and_identity_names(self):
        for name in ("LD_PRELOAD", "LD_LIBRARY_PATH", "GLIBC_TUNABLES", "GCONV_PATH", "TMPDIR", "MALLOC_ARENA_MAX",
                     "HOME", "USER", "OULIPOLY_ROOT_BASH_V1", "OULIPOLY_ACP_V2_SOCKET", "OULIPOLY_CONFIG",
                     "AGENT_BASH_BIN", "SUDO_UID", "BAD-NAME", "", "1X"):
            with self.subTest(name=name), self.assertRaises(frontdoor.Refused):
                frontdoor.root_env(self.user, SITE, {name: "v"})
        with self.assertRaises(frontdoor.Refused):
            frontdoor.root_env(self.user, SITE, {"X": "a\0b"})
        with self.assertRaises(frontdoor.Refused):
            frontdoor.root_env(self.user, SITE, {f"N{i}": "" for i in range(65)})


class Requester(unittest.TestCase):
    site = SITE

    def test_requires_root_and_sudo_named_allowed_user(self):
        me = pwd.getpwuid(os.getuid())
        with self.assertRaisesRegex(frontdoor.Refused, "not running as root"):
            frontdoor.requester(self.site, {"SUDO_UID": "1000"})
        with mock.patch.object(frontdoor.os, "geteuid", return_value=0), mock.patch.object(frontdoor.os, "getuid", return_value=0):
            with self.assertRaisesRegex(frontdoor.Refused, "no requester"):
                frontdoor.requester(self.site, {})
            with self.assertRaisesRegex(frontdoor.Refused, "requester is root"):
                frontdoor.requester(self.site, {"SUDO_UID": "0"})
            allowed = dict(self.site, allowed_users=[me.pw_name])
            self.assertEqual(frontdoor.requester(allowed, {"SUDO_UID": str(me.pw_uid)}).pw_name, me.pw_name)
            with self.assertRaisesRegex(frontdoor.Refused, "not allowed"):
                frontdoor.requester(dict(self.site, allowed_users=["someone-else"]), {"SUDO_UID": str(me.pw_uid)})

    def test_argv_is_pinned(self):
        self.assertEqual(frontdoor.parse_argv(["fd", "run"]), frontdoor.SITE_CONFIG)
        self.assertEqual(frontdoor.parse_argv(["fd", "--site-config", "/x", "run"]), "/x")
        for argv in (["fd"], ["fd", "run", "x"], ["fd", "--recover", "x"], ["fd", "--site-config", "/x"]):
            self.assertIsNone(frontdoor.parse_argv(argv))


class Scratch(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="frontdoor-test-")
        self.addCleanup(shutil.rmtree, self.dir, True)


class Custody(Scratch):
    def test_own_files_are_not_root_custody(self):
        path = os.path.join(self.dir, "frontdoor.json")
        with open(path, "w") as file:
            json.dump(SITE, file)
        with self.assertRaisesRegex(frontdoor.Refused, "not owned by root"):
            frontdoor.load_site(path)



    def test_writable_and_escaping_entries_refused(self):
        owners = frozenset({os.getuid()})
        tree = os.path.join(self.dir, "pkg")
        os.makedirs(os.path.join(tree, "bin"))
        with mock.patch.object(frontdoor, "TRUSTED_OWNERS", owners):
            os.chmod(tree, 0o755)
            os.chmod(os.path.join(tree, "bin"), 0o755)
            frontdoor.check_tree(tree)
            os.symlink("/etc/passwd", os.path.join(tree, "bin", "out"))
            with self.assertRaisesRegex(frontdoor.Refused, "leaves the package"):
                frontdoor.check_tree(tree)
            os.unlink(os.path.join(tree, "bin", "out"))
            os.chmod(os.path.join(tree, "bin"), 0o775)
            with self.assertRaisesRegex(frontdoor.Refused, "writable"):
                frontdoor.check_tree(tree)


class Retention(Scratch):
    def make_run(self, name, retention, locked):
        run = os.path.join(self.dir, name)
        for sub in ("private", "launch/provider", "store"):
            os.makedirs(os.path.join(run, sub))
        for rel in ("launch/provider/adapter-state",):
            with open(os.path.join(run, rel), "w") as file:
                file.write("fixture-secret")
        with open(os.path.join(run, "private", "retention"), "w") as file:
            file.write(retention)
        fd = os.open(os.path.join(run, "private", "lock"), os.O_RDWR | os.O_CREAT)
        if locked:
            fcntl.flock(fd, fcntl.LOCK_EX)
            self.addCleanup(os.close, fd)
        else:
            os.close(fd)
        return run

    def test_discard_removes_package_run(self):
        run = self.make_run("a", "discard", False)
        record = frontdoor.retire(run, "discard")
        self.assertTrue(record["ok"])
        self.assertTrue(record["run_removed"])
        self.assertFalse(os.path.exists(run))

    def test_keep_preserves_adapter_owned_state(self):
        run = self.make_run("b", "keep", False)
        record = frontdoor.retire(run, "keep")
        self.assertIsNone(record["run_removed"])
        self.assertTrue(os.path.isdir(os.path.join(run, "store")))
        for rel in ("launch/provider/adapter-state",):
            self.assertTrue(os.path.lexists(os.path.join(run, rel)))

    def test_sweep_takes_only_runs_whose_lock_is_free(self):
        live = self.make_run("live", "discard", True)
        dead = self.make_run("dead", "discard", False)
        kept = self.make_run("kept", "keep", False)
        with mock.patch.object(frontdoor, "check_owned"):
            swept = frontdoor.sweep(self.dir)
        self.assertEqual(sorted(os.path.basename(r["run"]) for r in swept), ["dead", "kept"])
        self.assertTrue(os.path.exists(os.path.join(live, "launch/provider/adapter-state")))
        self.assertFalse(os.path.exists(dead))
        self.assertTrue(os.path.isdir(kept))
        self.assertTrue(os.path.lexists(os.path.join(kept, "launch/provider/adapter-state")))


class Controls(unittest.TestCase):
    def test_only_entry_controls_pass(self):
        self.assertEqual(frontdoor.control(b'{"cmd":"cancel"}'), {"cmd": "cancel"})
        self.assertEqual(frontdoor.control(b'{"cmd":"close"}'), {"cmd": "close"})
        self.assertEqual(frontdoor.control(b'{"cmd":"send","text":"more","ref":"r1"}')["text"], "more")
        for line in (b'{"cmd":"cancel","x":1}', b'{"cmd":"send","text":" "}', b'{"cmd":"send","text":"a","harness":"h"}',
                     b'{"cmd":"recover"}', b'not json', b'[]', b'{"cmd":"send","text":"a","ref":1}'):
            with self.subTest(line=line):
                self.assertIsNone(frontdoor.control(line))


# A stand-in for `native-root`: prints scripted lines, echoes each control
# it gets as an owner line, ends on close (87) or cancel (82), or never
# when told to ignore controls.
FAKE_ENTRY = textwrap.dedent("""
    import json, sys, time
    mode = sys.argv[1]
    def say(v):
        print(json.dumps(v), flush=True)
    say({"entry": "setup-completed", "launch": {}})
    say({"event": "ack", "index": 0, "message_id": "m1"})
    for line in sys.stdin:
        cmd = json.loads(line)
        say({"event": "control-seen", "cmd": cmd})
        if mode == "ignore":
            continue
        if cmd.get("cmd") == "close":
            say({"event": "terminal", "status": "closed"}); sys.exit(87)
        if cmd.get("cmd") == "cancel":
            say({"event": "terminal", "status": "cancelled"}); sys.exit(82)
    if mode == "ignore":
        time.sleep(60)
    sys.exit(70)
""")


class RelayTest(Scratch):
    def relay(self, mode, deadline, grace):
        run = os.path.join(self.dir, "run")
        os.makedirs(os.path.join(run, "private"))
        entry = subprocess.Popen([sys.executable, "-c", FAKE_ENTRY, mode], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        out_r, out_w = os.pipe()
        in_r, in_w = os.pipe()

        def close():
            if entry.poll() is None:
                entry.kill()
            entry.wait()
            entry.stdin.close()
            entry.stdout.close()
            for fd in (out_r, out_w, in_r, in_w):
                try:
                    os.close(fd)
                except OSError:
                    pass

        self.addCleanup(close)
        relay = frontdoor.Relay(entry, run, deadline, grace)
        return relay, entry, run, (out_r, out_w), (in_r, in_w)

    def drain(self, fd):
        os.set_blocking(fd, False)
        data = b""
        try:
            while True:
                chunk = os.read(fd, 65536)
                if not chunk:
                    break
                data += chunk
        except BlockingIOError:
            pass
        return [json.loads(line) for line in data.splitlines()]

    def test_relays_valid_controls_and_refuses_others(self):
        relay, entry, run, (out_r, out_w), (in_r, in_w) = self.relay("obey", 60, 5)
        os.write(in_w, b'{"cmd":"recover"}\n{"cmd":"send","text":"more"}\n{"cmd":"close"}\n')
        with mock.patch.object(frontdoor, "OUT_FD", out_w):
            status = relay.run_to_end(in_r)
        lines = self.drain(out_r)
        self.assertEqual(status, 87)
        self.assertFalse(relay.killed)
        seen = [l["cmd"] for l in lines if l.get("event") == "control-seen"]
        self.assertEqual(seen, [{"cmd": "send", "text": "more"}, {"cmd": "close"}])
        self.assertIn({"frontdoor": "control-refused", "reason": "not cancel, close or send"}, lines)
        self.assertEqual([l["entry"] for l in lines if "entry" in l], ["setup-completed"])

    def test_stdin_eof_is_abandonment_cancel(self):
        relay, entry, run, (out_r, out_w), (in_r, in_w) = self.relay("obey", 60, 5)
        os.close(in_w)
        in_w = -1
        with mock.patch.object(frontdoor, "OUT_FD", out_w):
            status = relay.run_to_end(in_r)
        lines = self.drain(out_r)
        self.assertEqual(status, 82)
        self.assertFalse(relay.killed)
        self.assertTrue(any(l.get("frontdoor") == "cancel" and "abandoned" in l["why"] for l in lines))

    def test_deadline_cancels_then_kills_an_entry_that_ignores_it(self):
        relay, entry, run, (out_r, out_w), (in_r, in_w) = self.relay("ignore", 0.5, 0.5)
        started = time.monotonic()
        with mock.patch.object(frontdoor, "OUT_FD", out_w):
            status = relay.run_to_end(in_r)
        lines = self.drain(out_r)
        self.assertTrue(relay.killed)
        self.assertEqual(frontdoor.entry_exit(status, relay.killed), frontdoor.EXIT_KILLED)
        self.assertLess(time.monotonic() - started, 10)
        self.assertEqual([l["frontdoor"] for l in lines if "frontdoor" in l], ["cancel", "killed"])
        self.assertEqual(relay.why, "deadline")

    def test_close_with_owed_background_completions_arms_kill_only_once_none_is_owed(self):
        relay, entry, run, (out_r, out_w), (in_r, in_w) = self.relay("ignore", 60, 5)
        clock = [100.0]
        relay.clock = lambda: clock[0]
        with mock.patch.object(frontdoor, "OUT_FD", out_w):
            relay.entry_line(b'{"event":"async-owed","change":"owed","work":2,"owed_async":1}')
            relay.requester_line(b'{"cmd":"close"}')
            self.assertIsNone(relay.kill_at, "no kill grace while a completion is owed")
            self.assertTrue(relay.close_waiting)
            # A registered child's events never move the parent's count.
            relay.entry_line(b'{"event":"async-owed","change":"turn-ended","owed_async":0,"child":{"id":"c"}}')
            self.assertIsNone(relay.kill_at)
            clock[0] = 500.0
            relay.entry_line(b'{"event":"async-owed","change":"turn-ended","work":2,"owed_async":0}')
            self.assertEqual(relay.kill_at, 505.0)
            self.assertEqual(relay.why, "requester close")
        # Without owed work, close arms at once as before.
        plain = frontdoor.Relay(entry, run, 60, 5, clock=lambda: 7.0)
        plain.requester_line(b'{"cmd":"close"}')
        self.assertEqual(plain.kill_at, 12.0)

    def test_signal_is_abandonment(self):
        relay, entry, run, (out_r, out_w), (in_r, in_w) = self.relay("obey", 60, 5)
        relay.signals.append(15)
        with mock.patch.object(frontdoor, "OUT_FD", out_w):
            status = relay.run_to_end(in_r)
        self.assertEqual(status, 82)
        self.assertEqual(relay.why, "abandoned: signal 15")


class EntryRequest(unittest.TestCase):
    def test_paths_and_user_are_the_front_doors(self):
        user = types.SimpleNamespace(pw_name="nes", pw_uid=1000, pw_gid=1000, pw_dir="/home/nes", pw_shell="/bin/bash")
        checked = frontdoor.check_request(request(), SITE, NOW)
        value = frontdoor.entry_request("/opt/p", "/var/r/1000/run", user, checked, {"PATH": "/usr/bin"})
        self.assertEqual(value["store"], "/var/r/1000/run/store")
        self.assertEqual(value["launch_dir"], "/var/r/1000/run/launch")
        self.assertEqual(value["workload"], {"isolation": "host-root", "user": "nes"})
        self.assertEqual(value["provider"]["bash_authority"], "trusted-task")
        self.assertEqual(value["provider"]["executable"], PROVIDER["executable"])
        self.assertNotIn("fixture-access-marker", json.dumps(value))
        self.assertEqual(value["messages"], ["look"])
        self.assertNotIn("claude", value)



if __name__ == "__main__":
    unittest.main()
