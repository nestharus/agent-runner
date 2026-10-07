"""Offline checks of the public caller: request production, capture, the
answer, close and deadline handling, and outcome classes. The front door
is a scripted stand-in run directly (no sudo, no root, no model)."""

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
    absent_account = mode in ("async-no-account-initial", "async-no-account-eventual")
    if mode.startswith("async-"):
        if not absent_account:
            say({"event": "async-owed", "change": "owed", "work": 2, "owed_async": 1})
        if mode in ("async-silent-completion", "async-no-account-initial", "async-loss-work-mismatch"):
            say({"event": "agent-message", "input": 0, "parent_message_id": "m1", "text": "the eventual answer"})
        say({"event": "turn-end", "input": 0, "stop_reason": "end_turn"})
    if mode == "answer":
        say({"event": "agent-message", "input": 0, "parent_message_id": "m1", "message_id": "a1", "text": "first"})
        say({"event": "agent-message", "input": 0, "parent_message_id": "m1", "message_id": "a2", "text": "the answer"})
        say({"event": "turn-end", "input": 0, "message_id": "m1", "stop_reason": "end_turn", "own_output": True})
    for raw in sys.stdin.buffer:
        cmd = json.loads(raw)["cmd"]
        if cmd == "close":
            if mode.startswith("async-"):
                lost = mode in ("async-trailing-loss", "async-loss-work-mismatch")
                if not lost:
                    work = 999 if mode == "async-completion-work-mismatch" else None if mode == "async-completion-work-null" else 2
                    completion = {"event": "bash-async-completion-admitted", "work": work, "input": 1}
                    if mode == "async-completion-work-null":
                        del completion["work"]
                    say(completion)
                    say({"event": "ack", "index": 1, "message_id": "m2"})
                    if mode not in ("async-silent-completion", "async-no-text", "async-no-account-initial"):
                        say({"event": "agent-message", "input": 1, "parent_message_id": "m2", "text": "the eventual answer"})
                    if mode != "async-unfinished":
                        say({"event": "turn-end", "input": 1, "stop_reason": "end_turn"})
                        if not absent_account and mode != "async-trailing-success":
                            say({"event": "async-owed", "change": "turn-ended", "work": 2, "owed_async": 0})
                terminal = {"event": "terminal", "status": "closed", "async": {
                    "accepted": 1, "turn_ended": 0 if lost or mode == "async-unfinished" else 1,
                    "undelivered": [{"work": 999 if mode == "async-loss-work-mismatch" else 2,
                                     "reason": "recipient-not-in-conversation"}] if lost else [],
                    "owed": 1 if mode == "async-unfinished" else 0}}
                if absent_account:
                    del terminal["async"]
                say(terminal)
            else:
                say({"event": "terminal", "status": "closed"})
            say({"entry": "terminal", "stage": "owner-ended", "relay": "complete"})
            say({"frontdoor": "terminal", "stage": "ended", "exit": 87, "retire": {"ok": True, "run_removed": True}})
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
        evidence = os.environ.get("NATIVE_CALL_EVIDENCE")
        if evidence:
            name = mode if self._testMethodName != "test_local_refusal_starts_nothing" else mode + "-local-refusal"
            shutil.copytree(out, os.path.join(evidence, name))
        with open(os.path.join(out, "result.json")) as file:
            return code, out, json.load(file)

    def seen(self):
        with open(self.path("fd", "seen.json")) as file:
            return json.load(file)

    def test_answer_close_and_capture(self):
        source = self.write("auth.json", {"openai": {"type": "oauth", "refresh": "fixture-refresh-grant", "access": SECRET,
                                                     "expires": int(NOW + 7200) * 1000}})
        code, out, result = self.call("answer",
                                      "--env", "PATH=/x:/usr/bin", "--deadline", "60")
        self.assertEqual((code, result["class"]), (0, "answered"))
        with open(os.path.join(out, "final.md")) as file:
            self.assertEqual(file.read(), "the answer")
        self.assertEqual(result["answer"]["linked_messages"], 2)
        self.assertEqual(list(result["sends"]), ["close"])
        seen = self.seen()
        self.assertEqual(seen["argv"], ["run"])
        self.assertEqual(seen["sudo_uid"], "1000")
        self.assertNotIn("credential", seen["request"])
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

    def test_async_eventual_answer_silent_turn_and_terminal_loss(self):
        for mode, expected in [("async-eventual", (0, "answered")),
                               ("async-silent-completion", (0, "answered")),
                               ("async-no-text", (1, "no-answer")),
                               ("async-unfinished", (6, "incomplete")),
                               ("async-trailing-loss", (10, "async-undelivered")),
                               ("async-trailing-success", (0, "answered"))]:
            with self.subTest(mode=mode):
                code, out, result = self.call(mode, "--deadline", "10")
                self.assertEqual((code, result["class"]), expected)
                self.assertEqual(list(result["sends"]), ["close"])
                final = os.path.join(out, "final.md")
                if mode in ("async-eventual", "async-silent-completion", "async-unfinished", "async-trailing-success"):
                    with open(final) as file:
                        text = file.read()
                    self.assertIn("the eventual answer", text)
                    self.assertLess(text.index("input 0"), text.index("input 1"))
                    self.assertTrue(result["answer"]["present"])
                    if mode == "async-unfinished":
                        self.assertIn("carrying turn end not observed", text)
                else:
                    self.assertFalse(os.path.exists(final))
                if mode == "async-trailing-loss":
                    self.assertEqual(result["async"]["unsettled"], 0)
                    self.assertEqual(result["async"]["reports"]["unsettled"], 1)
                    self.assertEqual(result["async"]["undelivered"][0]["reason"], "recipient-not-in-conversation")
                    self.assertTrue(result["async"]["evidence_gaps"])

    def test_async_work_and_absent_account_countercases(self):
        # U62 O5/O6 alterations played by the scripted front door through the
        # actual caller boundary; these are not actual owner exports.
        cases = [
            ("async-completion-work-mismatch", ["async-completion-work-not-in-owed-reports: 999"], False),
            ("async-completion-work-null", ["async-completion-work-not-in-owed-reports: None"], False),
            ("async-loss-work-mismatch", ["owner-terminal-undelivered-work-not-in-owed-reports: 999"], True),
            ("async-no-account-initial", ["async-completion-work-not-in-owed-reports: 2",
                                          "owner-terminal-async-account-missing"], False),
            ("async-no-account-eventual", ["async-completion-work-not-in-owed-reports: 2",
                                           "owner-terminal-async-account-missing"], False),
        ]
        for mode, errors, gap in cases:
            with self.subTest(mode=mode):
                code, out, result = self.call(mode, "--deadline", "10")
                self.assertEqual((code, result["class"]), (6, "incomplete"))
                self.assertEqual(result["async"]["errors"], errors)
                self.assertEqual(bool(result["async"]["evidence_gaps"]), gap)
                self.assertEqual(list(result["sends"]), ["close"])
                if mode == "async-loss-work-mismatch":
                    self.assertEqual(result["async"]["reports"]["unsettled"], 1)
                    self.assertEqual(result["async"]["undelivered"][0]["work"], 999)
                with open(os.path.join(out, "final.md")) as file:
                    text = file.read()
                self.assertIn("the eventual answer", text)
                if mode != "async-loss-work-mismatch":
                    self.assertLess(text.index("input 0"), text.index("input 1"))
                self.assertTrue(result["answer"]["present"])
                if mode.startswith("async-no-account-"):
                    self.assertEqual(result["async"]["reports"]["accepted"], 0)
                    self.assertIsNone(result["async"]["owner_summary"])

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
        # An ordinary call may include the owner's empty async summary.
        events.append({"event": "terminal", "async": {
            "accepted": 0, "turn_ended": 0, "undelivered": [], "owed": 0}})
        self.assertEqual(native_call.classify(87, {"stdout_eof": True, "errors": []}, events, {})[:3],
                         ("answered", "a", events[2]))
        self.assertEqual(native_call.classify(87, {"stdout_eof": False, "errors": []}, events, {})[0], "incomplete")
        self.assertEqual(native_call.classify(94, {"stdout_eof": True, "errors": []}, events, {})[0], "cleanup-failed")
        self.assertEqual(native_call.classify(73, {"stdout_eof": True, "errors": []}, events, {})[0], "ended-otherwise")


    def background_events(self, completion_turn=True, undelivered=False):
        events = [{"event": "ack", "index": 0, "message_id": "m1"},
                  {"event": "async-owed", "change": "owed", "work": 2, "owed_async": 1},
                  {"event": "agent-message", "input": 0, "parent_message_id": "m1", "text": "started"},
                  {"event": "turn-end", "input": 0, "stop_reason": "end_turn"}]
        if undelivered:
            events.append({"event": "async-owed", "change": "undelivered", "work": 2,
                           "reason": "recipient-not-in-conversation", "owed_async": 0})
        elif completion_turn:
            events += [{"event": "bash-async-completion-admitted", "work": 2, "input": 1},
                       {"event": "ack", "index": 1, "message_id": "m2"},
                       {"event": "agent-message", "input": 1, "parent_message_id": "m2", "text": "completed"},
                       {"event": "turn-end", "input": 1, "stop_reason": "end_turn"},
                       {"event": "async-owed", "change": "turn-ended", "work": 2, "owed_async": 0}]
        return events + [{"event": "terminal", "async": {
            "accepted": 1, "turn_ended": int(completion_turn and not undelivered),
            "undelivered": [{"work": 2, "reason": "recipient-not-in-conversation"}] if undelivered else [],
            "owed": int(not completion_turn and not undelivered)}},
            {"entry": "terminal", "relay": "complete"}, {"frontdoor": "terminal", "exit": 87}]

    def test_background_task_is_answered_only_after_its_completion_turn(self):
        ok = {"stdout_eof": True, "errors": []}
        self.assertEqual(native_call.classify(87, ok, self.background_events(), {"close": {}})[0], "answered")
        self.assertEqual(native_call.classify(87, ok, self.background_events(completion_turn=False), {"close": {}})[0],
                         "incomplete", "first-turn text alone is not a complete background task")
        self.assertEqual(native_call.classify(87, ok, self.background_events(undelivered=True), {"close": {}})[0],
                         "async-undelivered")
        # Owner ended-owed (3), mapped by the entry to 83, preserves the
        # rejected durable insertion while exposing its settled async loss.
        self.assertEqual(native_call.classify(83, ok, self.background_events(undelivered=True), {"close": {}})[0],
                         "async-undelivered")
        self.assertEqual(native_call.EXITS["async-undelivered"], 10)
        turns = native_call.turns_of(self.background_events())
        self.assertEqual([(t["input"], t["kind"], t["work"], t["text"]) for t in turns],
                         [(0, "initial", None, "started"), (1, "background-completion", 2, "completed")])
        summary = native_call.async_of(self.background_events(undelivered=True))
        self.assertEqual((summary["accepted"], summary["turn_ended"], summary["unsettled"]), (1, 0, 0))

    def test_async_missing_or_contradictory_terminal_evidence_is_incomplete(self):
        ok = {"stdout_eof": True, "errors": []}
        complete = self.background_events()
        missing = [e for e in complete if e.get("event") != "terminal"]
        contradictory = self.background_events()
        next(e for e in contradictory if e.get("event") == "terminal")["async"]["turn_ended"] = 0
        missing_turn = [e for e in complete if not (e.get("event") == "turn-end" and e.get("input") == 1)]
        for events in (missing, contradictory, missing_turn):
            self.assertEqual(native_call.classify(87, ok, events, {"close": {}})[0], "incomplete")
        # Trailing successful settlement reports may be absent, but its ACK and
        # carrying turn must still be observed; the owner account is not an ACK.
        trailing = [e for e in complete if e.get("change") != "turn-ended"]
        self.assertEqual(native_call.classify(87, ok, trailing, {"close": {}})[0], "answered")

    def test_close_with_owed_completion_defers_the_stop_bound(self):
        call = native_call.Call.__new__(native_call.Call)
        call.start = time.monotonic()
        call.sends, call.events, call.pending, call.stop_at, call.owed_async = {}, [], b"", None, 0
        call.log = lambda **fields: None
        proc = type("P", (), {"stdin": type("S", (), {"closed": False})()})()
        call.line(proc, b'{"event":"async-owed","change":"owed","work":2,"owed_async":1}')
        call.line(proc, b'{"event":"turn-end","input":0,"stop_reason":"end_turn"}')
        self.assertIn("close", call.sends)
        self.assertIsNone(call.stop_at, "no stop bound while a completion is owed")
        call.line(proc, b'{"event":"async-owed","change":"turn-ended","work":2,"owed_async":0}')
        self.assertIsNotNone(call.stop_at)


if __name__ == "__main__":
    unittest.main()
