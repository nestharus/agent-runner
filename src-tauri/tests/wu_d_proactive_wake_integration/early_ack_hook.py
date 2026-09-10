"""Real provider child: independently receive, read producer log, then ACK.
No DB writes, PID prebinding, reconstructed prompts, or model workloads.
"""
import json
import os
from pathlib import Path
import re
import sqlite3
import subprocess
import sys
import time

work = Path(os.environ['work'])
runner = os.environ['AGENT_BASH_AGENT_RUNNER_BIN']
data = Path(os.environ['OULIPOLY_DATA_DIR'])


def rows(sql, args=()):
    with sqlite3.connect(f'file:{data / "pid-identity.db"}?mode=ro', uri=True) as db:
        return db.execute(sql, args).fetchall()


def await_rows(sql, args):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        found = rows(sql, args)
        if found:
            return found
        time.sleep(.02)
    raise AssertionError((sql, args))


if sys.argv[1] == 'initial':
    owner = json.loads(os.environ['OULIPOLY_PARENT_INVOCATION'])['id']
    await_rows('SELECT os_pid FROM pid_identity WHERE os_pid=? AND invocation_uuid=? AND session_id=?',
               (int(os.environ['FIXTURE_PROVIDER_PID']), owner, os.environ['session']))
    dispatch = subprocess.run([os.environ['AGENT_BASH_BIN'], 'run', '--completion-scope', 'tree',
                               '--delivery', 'async', '--', 'printf', 'early-ack-payload\n'],
                              capture_output=True, text=True, timeout=30)
    assert dispatch.returncode == 0, (dispatch.stdout, dispatch.stderr)
    result = json.loads(dispatch.stdout)
    assert result['dispatch_state'] == 'running', result
    (work / 'early-ack-dispatch.json').write_text(dispatch.stdout)
    found = await_rows('SELECT session_id,owner_invocation_uuid FROM mailbox WHERE handle=?', (result['handle'],))
    assert found == [(os.environ['session'], owner)], found
else:
    handle = json.loads((work / 'early-ack-dispatch.json').read_text())['handle']
    prompt = os.environ['last']
    assert re.findall(r'^   handle: (.+)$', prompt, re.M) == [handle], prompt
    advertised = re.findall(r'^   log: (.+)$', prompt, re.M)
    assert len(advertised) == 1, prompt
    payload = Path(json.loads(advertised[0])).read_text()
    assert payload == 'early-ack-payload\n', payload
    (work / 'early-ack-payload.txt').write_text(payload)
    found = rows('SELECT seq,session_id FROM mailbox WHERE handle=?', (handle,))
    assert len(found) == 1 and found[0][1] == os.environ['session'], found
    seq = str(found[0][0])
    ack = subprocess.run([runner, 'mailbox', 'ack', '--session-id', os.environ['session'],
                          '--from-seq', seq, '--to-seq', seq, '--delivered-by', 'real-fixture-consumer', '--json'],
                         capture_output=True, text=True, timeout=15)
    assert ack.returncode == 0 and json.loads(ack.stdout)['acknowledged_count'] == 1, (ack.stdout, ack.stderr)
    settled = rows('SELECT delivered_at,delivered_by_invocation_uuid FROM mailbox WHERE seq=?', (seq,))
    assert settled[0][0] and settled[0][1] == 'real-fixture-consumer', settled
    (work / 'early-ack-verified.json').write_text(json.dumps(settled))
