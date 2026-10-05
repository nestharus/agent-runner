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
FAR = (NOW + 86400) * 1000
OPENAI = {"openai": {"npm": "@ai-sdk/openai", "models": {"gpt-luna": {"options": {"reasoningEffort": "xhigh"}}}}}


def site(**extra):
    value = {
        "v": 1, "allowed_users": ["nes"], "run_base": "/x", "max_deadline_s": 3600,
        "cancel_grace_s": 30, "credential_margin_s": 300, "allow_keep": True,
        "default_path": "/usr/bin",
        "routes": {
            "sol": {"model": "openai/gpt-sol", "provider": OPENAI, "credential": "required", "children": ["luna-max"]},
            "opus": {"harness": "claude", "model": "claude-opus-5-5", "effort": "medium",
                     "config_dir": ".claude5", "credential": "none", "children": ["luna-max"]},
            "plain": {"harness": "claude", "model": "claude-opus-5-5", "effort": "medium",
                      "config_dir": ".claude5", "credential": "none"},
            "anthropic": {"model": "anthropic/x", "provider": {"anthropic": {}}, "credential": "required",
                          "children": ["luna-max"]},
        },
        "child_routes": {"luna-max": {"model": "openai/gpt-luna", "provider": OPENAI, "credential": "required"}},
    }
    value.update(extra)
    frontdoor.check_site_children(value)
    return value


def grant(provider="openai", expires=FAR):
    return {provider: {"type": "oauth", "refresh": "", "access": "tok-" + provider, "expires": expires}}


def request(route, **extra):
    value = {"v": 1, "route": route, "message": "m", "cwd": "/", "bash": {"authority": "trusted-task"}, "deadline_s": 600}
    value.update(extra)
    return value


class SiteAllowance(unittest.TestCase):
    def test_site_children_shape(self):
        with self.assertRaises(frontdoor.Refused):
            site(child_limits={"max_starts": 5})
        with self.assertRaises(frontdoor.Refused):
            site(child_limits={"max_concurrent": 3})
        bad = site()
        bad["routes"]["sol"]["children"] = ["nope"]
        with self.assertRaises(frontdoor.Refused):
            frontdoor.check_site_children(bad)
        self.assertEqual(site()["child_limits"], {"max_starts": 4, "max_concurrent": 2})
        # A site without children still loads, offering none.
        plain = {"routes": {"sol": {"model": "openai/x", "provider": OPENAI, "credential": "required"}}}
        frontdoor.check_site_children(plain)
        self.assertEqual(plain["child_routes"], {})

    def test_no_child_paths_unchanged(self):
        s = site()
        checked = frontdoor.check_request(request("plain"), s, NOW)
        self.assertIsNone(checked["children"])
        entry = frontdoor.entry_request("/pkg", "/run", SimpleNamespace(pw_name="nes", pw_dir="/home/nes"), checked, {}, False)
        self.assertNotIn("children", entry)
        # No-child Claude: a child grant is refused, never prepared.
        with self.assertRaisesRegex(frontdoor.Refused, "without children"):
            frontdoor.check_request(request("plain", child_credential=grant()), s, NOW)

    def test_route_must_offer_and_limits_are_ceilings(self):
        s = site()
        with self.assertRaisesRegex(frontdoor.Refused, "offers"):
            frontdoor.check_request(request("plain", children={"routes": ["luna-max"]}), s, NOW)
        with self.assertRaisesRegex(frontdoor.Refused, "max_starts"):
            frontdoor.check_request(request("opus", children={"routes": ["luna-max"], "max_starts": 5},
                                            child_credential=grant()), s, NOW)
        with self.assertRaisesRegex(frontdoor.Refused, "max_concurrent"):
            frontdoor.check_request(request("opus", children={"routes": ["luna-max"], "max_concurrent": 3},
                                            child_credential=grant()), s, NOW)
        with self.assertRaisesRegex(frontdoor.Refused, "children must be"):
            frontdoor.check_request(request("opus", children={"routes": ["luna-max"], "depth": 2}), s, NOW)


class ChildGrant(unittest.TestCase):
    def test_claude_parent_needs_separate_access_only_grant(self):
        s = site()
        asked = {"routes": ["luna-max"], "max_starts": 3}
        with self.assertRaisesRegex(frontdoor.Refused, "need child_credential"):
            frontdoor.check_request(request("opus", children=asked), s, NOW)
        refresh = grant()
        refresh["openai"]["refresh"] = "r"
        with self.assertRaisesRegex(frontdoor.Refused, "refresh"):
            frontdoor.check_request(request("opus", children=asked, child_credential=refresh), s, NOW)
        with self.assertRaisesRegex(frontdoor.Refused, "expires in"):
            frontdoor.check_request(request("opus", children=asked, child_credential=grant(expires=(NOW + 700) * 1000)), s, NOW)
        with self.assertRaisesRegex(frontdoor.Refused, "one openai entry"):
            frontdoor.check_request(request("opus", children=asked, child_credential=grant("anthropic")), s, NOW)
        checked = frontdoor.check_request(request("opus", children=asked, child_credential=grant()), s, NOW)
        children = checked["children"]
        self.assertEqual((children["max_starts"], children["max_concurrent"]), (3, 2))
        self.assertEqual(children["grant"][0], grant())
        self.assertEqual(children["grant"][1]["source"], "separate-child-grant")
        self.assertNotIn("tok-", json.dumps(children["grant"][1]))
        # The Claude parent itself still takes no credential.
        self.assertIsNone(checked["credential"])
        entry = frontdoor.entry_request("/pkg", "/run", SimpleNamespace(pw_name="nes", pw_dir="/home/nes"), checked, {}, False)
        self.assertEqual(entry["children"]["auth"], "/run/private/child-auth.json")
        self.assertEqual(entry["children"]["routes"], {"luna-max": {"model": "openai/gpt-luna", "provider": OPENAI}})
        self.assertEqual(entry["children"]["opencode"]["deps"], "/pkg/opencode/deps")
        self.assertIn("claude", entry)
        self.assertNotIn("tok-", json.dumps(entry))

    def test_opencode_parent_children_reuse_its_grant(self):
        s = site()
        asked = {"routes": ["luna-max"]}
        with self.assertRaisesRegex(frontdoor.Refused, "reuse its own grant"):
            frontdoor.check_request(request("sol", children=asked, credential=grant(), child_credential=grant()), s, NOW)
        checked = frontdoor.check_request(request("sol", children=asked, credential=grant()), s, NOW)
        self.assertEqual(checked["children"]["grant"][0], checked["credential"][0])
        self.assertEqual(checked["children"]["grant"][1]["source"], "reused-parent-grant")
        # A parent grant of another provider is not reused for the child's.
        with self.assertRaisesRegex(frontdoor.Refused, "own openai grant"):
            frontdoor.check_request(request("anthropic", children=asked, credential=grant("anthropic")), s, NOW)


class Retirement(unittest.TestCase):
    def make_run(self, children=("c1", "c2")):
        run = tempfile.mkdtemp(prefix="child-retire-")
        self.addCleanup(shutil.rmtree, run, True)
        files = list(frontdoor.CREDENTIAL_FILES)
        for child in children:
            files += [f"launch/children/{child}/{name}" for name in frontdoor.CHILD_CREDENTIAL_FILES]
        for name in files:
            os.makedirs(os.path.dirname(os.path.join(run, name)), exist_ok=True)
            with open(os.path.join(run, name), "w") as file:
                file.write("secret")
        return run, files

    def test_keep_removes_every_staged_and_child_copy(self):
        run, files = self.make_run()
        # A partial child setup: its directory without a credential yet.
        os.makedirs(os.path.join(run, "launch/children/c3/xdg"))
        # Not a child launch: never traversed.
        os.makedirs(os.path.join(run, "launch/children/other/secret"))
        with open(os.path.join(run, "launch/children/other/secret/server-password"), "w") as file:
            file.write("x")
        result = frontdoor.retire(run, "keep")
        self.assertTrue(result["ok"], result)
        for name in files:
            self.assertFalse(os.path.exists(os.path.join(run, name)), name)
        removed = {os.path.relpath(r["path"], run) for r in result["credentials"]["files"] if r["result"] == "removed"}
        self.assertIn("private/child-auth.json", removed)
        self.assertIn("launch/children/c2/xdg/data/opencode/auth.json", removed)
        absent = {os.path.relpath(r["path"], run) for r in result["credentials"]["files"] if r["result"] == "absent"}
        self.assertIn("launch/children/c3/secret/server-password", absent)
        self.assertTrue(os.path.isdir(run))

    def test_symlinked_children_dir_is_a_failure_not_followed(self):
        run, _ = self.make_run(children=())
        elsewhere = tempfile.mkdtemp(prefix="child-elsewhere-")
        self.addCleanup(shutil.rmtree, elsewhere, True)
        os.makedirs(os.path.join(elsewhere, "c1/secret"))
        target = os.path.join(elsewhere, "c1/secret/server-password")
        with open(target, "w") as file:
            file.write("x")
        os.symlink(elsewhere, os.path.join(run, "launch/children"))
        result = frontdoor.retire(run, "discard")
        self.assertFalse(result["ok"], result)
        self.assertTrue(os.path.exists(target))
        self.assertIsNone(result["run_removed"])

    def test_discard_removes_the_run(self):
        run, _ = self.make_run()
        result = frontdoor.retire(run, "discard")
        self.assertTrue(result["ok"], result)
        self.assertFalse(os.path.exists(run))


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
        self.assertIsNone(public["child_credential"])
        self.assertNotIn("child_credential", req)


if __name__ == "__main__":
    unittest.main()
