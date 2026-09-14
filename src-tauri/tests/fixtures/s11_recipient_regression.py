"""Fixture-only harmful neighbors: hidden prefix and nonce-selected controls."""
import contextlib
import copy
import hashlib
import io
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile

spec = importlib.util.spec_from_file_location("provider", Path(sys.argv[1]) / "external-provider.py")
provider = importlib.util.module_from_spec(spec)
spec.loader.exec_module(provider)
control = provider.recipient_control
with tempfile.TemporaryDirectory() as temp:
    root = Path(temp)
    work = root / "work"
    work.mkdir()
    control.CONTROL = root / "recipient-control.json"
    config = dict(work_dir=str(work), negative=True, env={"S11_MARKER_SESSION_MISMATCH": "1"})
    control.CONTROL.write_text(json.dumps(config))
    os.environ["S11_WORK_DIR"] = str(work)
    # Real fixture prompt page implementation sees the original appended text.
    original = "original actual logged prompt\n"
    (work / "resume-prompts.jsonl").write_text(json.dumps(original) + "\n")
    request = {"params": {"turn_projection": "user_observation", "after_token": "s11-anchor:0"}}
    visible = provider.session_turn_page(request)
    assert len(visible["result"]["turns"]) == 1
    hidden = control.read_page(request, copy.deepcopy(visible))
    assert hidden["result"]["turns"] == []
    assert hidden["result"]["resume_token"] == "s11-anchor:0"
    tail = {"params": {"turn_projection": "user_observation", "start_mode": "tail"}}
    assert control.read_page(tail, provider.session_turn_page(tail))["result"]["resume_token"] == "s11-anchor:0"
    (work / "expose-observation").touch()
    assert control.read_page(request, provider.session_turn_page(request)) == visible
    assert (work / "resume-prompts.jsonl").read_text() == json.dumps(original) + "\n"
    # DB access is not part of this fixture-unit control discrimination.
    control.projection_present = lambda _: False
    control.configure({"request_id": "manual", "params": {}}, "launch")
    assert "S11_MARKER_SESSION_MISMATCH" not in os.environ
    control.configure({"request_id": "recipient", "params": {"prompt_acceptance": {"delivery_nonce": "actual-nonce"}}}, "launch")
    assert os.environ["S11_MARKER_SESSION_MISMATCH"] == "1"
    from s11_diagnostics import capture
    elf = b"\x7fELF" + b"private source executable fixture" * 10
    (work / "source-executable").write_bytes(elf)
    captured = io.StringIO()
    with contextlib.redirect_stdout(captured):
        capture(root)
    records = [json.loads(line) for line in captured.getvalue().splitlines()]
    binary = next(r for r in records if r["path"].endswith("/source-executable"))
    assert binary["executable_sha256"] == hashlib.sha256(elf).hexdigest()
    assert binary["byte_len"] == len(elf) and "hex" not in binary
    prompt = next(r for r in records if r["path"].endswith("/resume-prompts.jsonl"))
    assert bytes.fromhex(prompt["hex"]) == (json.dumps(original) + "\n").encode()
print("fixture hidden-prefix, nonce-selection and executable-diagnostic regressions executed")

# Pure oracle inputs, not forged provider execution/ACK/receipt authority. Each
# expected contract tuple is independent of the implementation's mapper.
import subprocess
import s11_recipient_case as oracle


def rejected(label, check):
    try:
        check()
    except (AssertionError, KeyError):
        return
    raise AssertionError("harmful neighbor escaped: " + label)


contracts = [
    ("trusted", "success", "succeeded", 0, None, None),
    ("trusted", "no-assistant", "failed", 0, "resume_completion_unconfirmed", "resume_completion_unconfirmed"),
    ("trusted", "nonzero", "failed", 29, None, "resume_prompt_accepted_provider_failed"),
    ("trusted", "missing", "failed", -1, "resume_prompt_accepted_provider_failed", "resume_prompt_accepted_provider_failed"),
]
for marker in ("absent", "hash", "session", "nonce"):
    contracts.extend([
        (marker, "nonzero", "failed", 29, None, "exit_nonzero"),
        (marker, "missing", "failed", -1, "external_provider_missing_final_exit", "external_provider_missing_final_exit"),
    ])
mutations = 0
for marker, shape, status, code, category, reason in contracts:
    expected = dict(status=status, exit_code=code, error_category=category, terminal_reason=reason)
    assert oracle.expected_outcome(marker, shape) == expected
    for projection in (False, True):
        shell_exit = 1 if projection or shape == "no-assistant" else {"success": 0, "nonzero": 29, "missing": 255}[shape]
        durable = dict(expected, invocation_uuid="original-fixture-invocation")
        envelope = dict(expected, id=durable["invocation_uuid"], success=status == "succeeded")
        native = dict(result=envelope, stdout=oracle.PAYLOAD if shape == "success" else "",
                      stderr="missing_final_exit;provider_process=exited:0" if shape == "missing" else "",
                      receipt=dict(root_exit_code=shell_exit, root_wait_status=shell_exit * 256))
        negative = marker != "trusted" and shape != "success"
        def check(n=native, d=durable):
            oracle.assert_outcome(n, d, marker, shape, projection, negative)
        check()
        for field, value in dict(status="contradictory", exit_code=73, error_category="wrong-category",
                                 terminal_reason="wrong-terminal").items():
            for target in ("envelope", "state"):
                n, d = copy.deepcopy(native), dict(durable)
                (n["result"] if target == "envelope" else d)[field] = value
                rejected(f"{marker}/{shape}/{target}/{field}", lambda: check(n, d))
                mutations += 1
            after = dict(durable, **{field: value})
            rejected("recovery/" + field, lambda: oracle.assert_preserved_outcome(durable, after))
            mutations += 1
        for field in (*oracle.OUTCOME_FIELDS, "success"):
            n = copy.deepcopy(native)
            del n["result"][field]
            rejected("missing envelope/" + field, lambda: check(n))
            mutations += 1
        n = copy.deepcopy(native)
        n["result"]["success"] = not envelope["success"]
        rejected("contradictory success", lambda: check(n))
        n = copy.deepcopy(native)
        n["result"]["id"] = "manual-successor"
        rejected("wrong result identity", lambda: check(n))
        # Timestamps and ongoing projection/progress are intentionally ignored.
        after = dict(durable, finished_at="later", last_progress_at="later", progress_epoch=2)
        oracle.assert_preserved_outcome(durable, after)
        if status == "failed":
            after = dict(durable, **oracle.SUCCESS)
            rejected("recovery replaced failure with manual success", lambda: oracle.assert_preserved_outcome(durable, after))
        mutations += 2

manual = dict(id="manual-fixture-invocation", status="succeeded", success=True,
              exit_code=0, error_category=None, terminal_reason=None)


def manual_output(value, rc=0, stdout=oracle.PAYLOAD, copies=1):
    stderr = ("OULIPOLY_RESULT=" + json.dumps(value) + "\n") * copies
    return subprocess.CompletedProcess(["private-source-runner", "resume"], rc, stdout.encode(), stderr.encode())


assert oracle.assert_manual_outcome(manual_output(manual), "original-fixture-invocation") == manual
for field, value in dict(status="failed", success=False, exit_code=29, error_category="resume_completion_unconfirmed",
                         terminal_reason="exit_nonzero", id="original-fixture-invocation").items():
    wrong = dict(manual, **{field: value})
    rejected("manual/" + field, lambda: oracle.assert_manual_outcome(manual_output(wrong), "original-fixture-invocation"))
    mutations += 1
for options in (dict(rc=1), dict(stdout="no assistant completion\n"), dict(copies=0), dict(copies=2)):
    rejected("manual output/" + str(options), lambda: oracle.assert_manual_outcome(manual_output(manual, **options), "original-fixture-invocation"))
    mutations += 1
print(f"fixture exact outcome, recovery preservation and manual-envelope regressions executed; {mutations} counted harmful neighbors rejected")
