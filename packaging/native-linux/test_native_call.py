"""Offline checks of the public caller: credential staging from an
explicitly named source, refusals before anything starts, capture, the
answer, close and deadline handling, and outcome classes. The front door
is a scripted stand-in run directly (no sudo, no root, no model)."""

import base64
import json
import os
import shutil
import sys
import tempfile
import textwrap
import time
import unittest

import native_call

NOW = time.time()
SECRET = "fixture-access-marker"


def jwt(claims):
    part = lambda value: base64.urlsafe_b64encode(json.dumps(value).encode()).decode().rstrip("=")
    return f"{part({'alg': 'none'})}.{part(claims)}.sig"


# A scripted front door: checks the request line, then plays one scripted
# conversation. `answer`: ack, message, turn end, then end on close (87).
# `silent`: never ends the turn; ends on cancel with 92 like a kill.
FAKE_FRONTDOOR = textwrap.dedent("""
    import json, os, sys
    mode = os.environ.get("FAKE_MODE") or open(os.path.join(os.path.dirname(__file__), "mode")).read().strip()
    line = sys.stdin.buffer.readline()
    request = json.loads(line)
    seen = os.path.join(os.path.dirname(__file__), "seen.json")
    json.dump({"argv": sys.argv[1:], "sudo_uid": os.environ.get("SUDO_UID"), "request": request}, open(seen, "w"))
    def say(v):
        print(json.dumps(v), flush=True)
    if mode == "refuse":
        say({"frontdoor": "terminal", "stage": "refused", "reason": "x", "effects": "none"}); sys.exit(90)
    say({"frontdoor": "admitted", "run": "r1"})
    say({"entry": "setup-completed", "launch": {}})
    say({"event": "ack", "index": 0, "label": "accepted", "message_id": "m1"})
    if mode == "answer":
        say({"event": "agent-message", "input": 0, "parent_message_id": "m1", "message_id": "a1", "text": "first"})
        say({"event": "agent-message", "input": 0, "parent_message_id": "m1", "message_id": "a2", "text": "the answer"})
        say({"event": "turn-end", "input": 0, "message_id": "m1", "stop_reason": "end_turn", "own_output": True})
    for raw in sys.stdin.buffer:
        cmd = json.loads(raw)["cmd"]
        if cmd == "close":
            say({"event": "terminal", "status": "closed"})
            say({"entry": "terminal", "stage": "owner-ended", "relay": "complete"})
            say({"frontdoor": "terminal", "stage": "ended", "exit": 87, "retire": {"credentials": {"ok": True}}})
            sys.exit(87)
        if cmd == "cancel":
            say({"frontdoor": "terminal", "stage": "ended", "exit": 92, "killed": True})
            sys.exit(92)
    sys.exit(93)
""")


class Scratch(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="native-call-test-")
        self.addCleanup(shutil.rmtree, self.dir, True)

    def path(self, *parts):
        return os.path.join(self.dir, *parts)

    def write(self, name, value, mode=0o600):
        path = self.path(name)
        with open(path, "w") as file:
            file.write(value if isinstance(value, str) else json.dumps(value))
        os.chmod(path, mode)
        return path


class Credentials(Scratch):
    def codex_profile(self, exp, account=True):
        os.mkdir(self.path("profile"))
        tokens = {"access_token": jwt({"exp": exp}), "refresh_token": "fixture-refresh-grant", "id_token": "x"}
        if account:
            tokens["account_id"] = "acct"
        self.write("profile/auth.json", {"tokens": tokens})
        return self.path("profile")

    def test_codex_profile_gives_access_only(self):
        exp = int(NOW) + 7200
        entry = native_call.from_codex_profile(self.codex_profile(exp))["openai"]
        self.assertEqual(entry["refresh"], "")
        self.assertEqual(entry["expires"], exp * 1000)
        self.assertEqual(entry["accountId"], "acct")
        self.assertNotIn("fixture-refresh-grant", json.dumps(entry))

    def test_opencode_auth_drops_refresh(self):
        source = self.write("auth.json", {"openai": {"type": "oauth", "refresh": "fixture-refresh-grant", "access": SECRET,
                                                     "expires": int(NOW + 7200) * 1000, "accountId": "acct"}})
        entry = native_call.from_opencode_auth(source, "openai")["openai"]
        self.assertEqual(entry, {"type": "oauth", "refresh": "", "access": SECRET, "expires": int(NOW + 7200) * 1000, "accountId": "acct"})

    def test_refusals_quote_nothing(self):
        api = self.write("api.json", {"openai": {"type": "api", "key": SECRET}})
        with self.assertRaisesRegex(native_call.LocalRefusal, "not oauth"):
            native_call.from_opencode_auth(api, "openai")
        broken = self.write("broken.json", '{"openai": "' + SECRET)
        with self.assertRaises(native_call.LocalRefusal) as caught:
            native_call.from_opencode_auth(broken, "openai")
        self.assertNotIn(SECRET, str(caught.exception))
        os.symlink(api, self.path("link.json"))
        with self.assertRaisesRegex(native_call.LocalRefusal, "not a regular file"):
            native_call.from_opencode_auth(self.path("link.json"), "openai")
        with self.assertRaisesRegex(native_call.LocalRefusal, "schema"):
            native_call.from_opencode_auth(api, "anthropic")

    def test_freshness_against_deadline(self):
        credential = native_call.access_only("openai", SECRET, int(NOW + 1000) * 1000, None)
        with self.assertRaisesRegex(native_call.LocalRefusal, "deadline needs 1200s"):
            native_call.check_fresh(credential, 600, 600, NOW)
        native_call.check_fresh(credential, 300, 600, NOW)


class Calls(Scratch):
    def setUp(self):
        super().setUp()
        os.mkdir(self.path("fd"))
        self.frontdoor = self.path("fd", "frontdoor.py")
        with open(self.frontdoor, "w") as file:
            file.write(FAKE_FRONTDOOR)
        self.prompt = self.write("prompt.md", "look at it")
        os.mkdir(self.path("cwd"))

    def call(self, mode, *extra):
        with open(self.path("fd", "mode"), "w") as file:
            file.write(mode)
        out = self.path("out-" + mode)
        argv = ["--route", "fixture", "--prompt-file", self.prompt, "--cwd", self.path("cwd"), "--out", out,
                "--trusted-task", "--frontdoor", self.frontdoor, "--direct-requester-uid", "1000", *extra]
        code = native_call.main(argv)
        with open(os.path.join(out, "result.json")) as file:
            return code, out, json.load(file)

    def seen(self):
        with open(self.path("fd", "seen.json")) as file:
            return json.load(file)

    def test_answer_close_and_capture(self):
        source = self.write("auth.json", {"openai": {"type": "oauth", "refresh": "fixture-refresh-grant", "access": SECRET,
                                                     "expires": int(NOW + 7200) * 1000}})
        code, out, result = self.call("answer", "--credential-opencode-auth", source, "--credential-provider", "openai",
                                      "--env", "PATH=/x:/usr/bin", "--deadline", "60")
        self.assertEqual((code, result["class"]), (0, "answered"))
        with open(os.path.join(out, "final.md")) as file:
            self.assertEqual(file.read(), "the answer")
        self.assertEqual(result["answer"]["linked_messages"], 2)
        self.assertEqual(list(result["sends"]), ["close"])
        seen = self.seen()
        self.assertEqual(seen["argv"], ["run"])
        self.assertEqual(seen["sudo_uid"], "1000")
        self.assertEqual(seen["request"]["credential"]["openai"]["refresh"], "")
        self.assertEqual(seen["request"]["credential"]["openai"]["access"], SECRET)
        self.assertEqual(seen["request"]["env"], {"PATH": "/x:/usr/bin"})
        self.assertEqual(seen["request"]["bash"], {"authority": "trusted-task"})
        for name in os.listdir(out):
            with open(os.path.join(out, name), "rb") as file:
                data = file.read()
            self.assertNotIn(SECRET.encode(), data, name)
            self.assertNotIn(b"fixture-refresh-grant", data, name)
        with open(os.path.join(out, "events.jsonl"), "rb") as file:
            self.assertEqual(len(file.read().splitlines()), result["counts"]["lines"])
        self.assertEqual(oct(os.stat(out).st_mode & 0o777), "0o700")

    def test_deadline_cancels_without_replay(self):
        started = time.monotonic()
        code, out, result = self.call("silent", "--deadline", "1")
        self.assertEqual((code, result["class"]), (5, "cancelled"))
        self.assertEqual(list(result["sends"]), ["cancel"])
        self.assertLess(time.monotonic() - started, 20)
        self.assertFalse(os.path.exists(os.path.join(out, "final.md")))

    def test_front_door_refusal(self):
        code, out, result = self.call("refuse")
        self.assertEqual((code, result["class"]), (4, "front-door-refused"))

    def test_local_refusal_starts_nothing(self):
        source = self.write("auth.json", {"openai": {"type": "oauth", "refresh": "", "access": SECRET,
                                                     "expires": int(NOW + 100) * 1000}})
        code, out, result = self.call("answer", "--credential-opencode-auth", source, "--credential-provider", "openai")
        self.assertEqual((code, result["class"], result["started"]), (3, "refused-locally", False))
        self.assertFalse(os.path.exists(self.path("fd", "seen.json")))
        self.assertEqual(os.listdir(out), ["result.json"])

    def test_existing_out_is_refused(self):
        os.mkdir(self.path("taken"))
        argv = ["--route", "r", "--prompt-file", self.prompt, "--cwd", self.dir, "--out", self.path("taken"), "--trusted-task"]
        self.assertEqual(native_call.main(argv), 2)


class Classes(unittest.TestCase):
    def test_answer_needs_its_own_turn_end(self):
        events = [
            {"event": "ack", "index": 0, "message_id": "m1"},
            {"event": "agent-message", "input": 0, "parent_message_id": "m1", "text": "a"},
            {"event": "agent-message", "input": 0, "parent_message_id": "other", "text": "not linked"},
            {"event": "turn-end", "input": 0, "stop_reason": "end_turn"},
            {"event": "agent-message", "input": 0, "parent_message_id": "m1", "text": "after"},
        ]
        answer, turn_end, linked = native_call.answer_of(events)
        self.assertEqual((answer, linked), ("a", 1))
        self.assertEqual(native_call.answer_of(events[:2]), (None, None, 1))

    def test_incomplete_capture_is_not_answered(self):
        events = [{"event": "ack", "index": 0, "message_id": "m1"},
                  {"event": "agent-message", "input": 0, "parent_message_id": "m1", "text": "a"},
                  {"event": "turn-end", "input": 0, "stop_reason": "end_turn"},
                  {"entry": "terminal", "relay": "incomplete"},
                  {"frontdoor": "terminal", "exit": 87}]
        cls = native_call.classify(87, {"stdout_eof": True, "errors": []}, events, {"close": {}})[0]
        self.assertEqual(cls, "incomplete")
        events[3]["relay"] = "complete"
        self.assertEqual(native_call.classify(87, {"stdout_eof": True, "errors": []}, events, {"close": {}})[0], "answered")
        self.assertEqual(native_call.classify(87, {"stdout_eof": False, "errors": []}, events, {})[0], "incomplete")
        self.assertEqual(native_call.classify(94, {"stdout_eof": True, "errors": []}, events, {})[0], "cleanup-failed")
        self.assertEqual(native_call.classify(73, {"stdout_eof": True, "errors": []}, events, {})[0], "ended-otherwise")


if __name__ == "__main__":
    unittest.main()
