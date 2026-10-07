"""Offline controls of the registered-children bridge: the site's and
requester's allowance, the children's one grant (reused or separate, never
Claude's login), the entry request, retirement of every staged and child
copy, and the caller's parent-only answer/close. Unprivileged; nothing is
started, no credential is real."""

import json
import os
import shutil
import sys
import tempfile
import unittest
from types import SimpleNamespace

import frontdoor
import native_call

NOW = 1_800_000_000


class CallerParentOnly(unittest.TestCase):
    PARENT = [
        {"harness": "p", "event": "ack", "index": 0, "message_id": "m1"},
        {"harness": "child-1", "event": "ack", "index": 0, "message_id": "c1", "child": {"parent": "p"}},
        {"harness": "child-1", "event": "agent-message", "input": 0, "parent_message_id": "c1", "text": "child says", "child": {"parent": "p"}},
        {"harness": "child-1", "event": "turn-end", "input": 0, "stop_reason": "end_turn", "child": {"parent": "p"}},
        {"harness": "p", "event": "agent-message", "input": 0, "parent_message_id": "m1", "text": "parent says"},
        {"harness": "p", "event": "turn-end", "input": 0, "stop_reason": "end_turn"},
    ]

    def test_answer_is_the_parents_only(self):
        answer, turn, linked = native_call.answer_of(self.PARENT)
        self.assertEqual((answer, turn["harness"], linked), ("parent says", "p", 1))
        # A child's turn end alone is no parent answer.
        answer, turn, _ = native_call.answer_of(self.PARENT[:4])
        self.assertEqual((answer, turn), (None, None))

    def test_child_turn_end_does_not_close(self):
        out = tempfile.mkdtemp(prefix="caller-")
        self.addCleanup(shutil.rmtree, out, True)
        call = native_call.Call(SimpleNamespace(), out)
        proc = SimpleNamespace(stdin=SimpleNamespace(closed=False))
        call.line(proc, json.dumps(self.PARENT[3]))
        self.assertEqual(call.sends, {})
        call.line(proc, json.dumps(self.PARENT[5]))
        self.assertIn("close", call.sends)
        call.actions.close()
        call.capture.close()

    def test_children_file_and_deadline_bounds(self):
        events = self.PARENT + [
            {"event": "child-accepted", "child": "child-1"},
            {"event": "child-result", "child": {"parent": "p"}, "outcome": "answered", "lifecycle": {"end": "observed"}},
            {"event": "terminal", "children": {"starts": 1, "children": []}},
        ]
        children = native_call.children_of(events)
        self.assertEqual(len(children["results"]), 1)
        self.assertEqual(children["owner_summary"]["starts"], 1)
        base = ["--route", "r", "--prompt-file", "p", "--cwd", "/", "--out", "o", "--trusted-task"]
        for bad in ("0", "7201", "1e9", "1" + "0" * 400, "nan"):
            with self.assertRaises(SystemExit):
                native_call.parse_args(base + ["--deadline", bad])
        self.assertEqual(native_call.parse_args(base + ["--deadline", "7200"]).deadline, 7200)
        with self.assertRaises(SystemExit):
            native_call.parse_args(base + ["--child-max-starts", "2"])
        args = native_call.parse_args(base + ["--child-route", "luna-max", "--child-max-concurrent", "1"])
        prompt = os.path.join(tempfile.mkdtemp(prefix="caller-p-"), "p")
        self.addCleanup(shutil.rmtree, os.path.dirname(prompt), True)
        with open(prompt, "w") as file:
            file.write("q")
        args.prompt_file = prompt
        req, public = native_call.build_request(args, NOW)
        self.assertEqual(req["children"], {"routes": ["luna-max"], "max_concurrent": 1})
        self.assertNotIn("child_credential", public)
        self.assertNotIn("child_credential", req)


if __name__ == "__main__":
    unittest.main()
