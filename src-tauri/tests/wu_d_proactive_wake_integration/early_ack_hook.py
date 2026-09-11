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
    # The generation creator is the runner; the provider executable is a
    # separately custodied process. Join by invocation, then independently
    # verify the live identity rather than trusting ancestry or a wake claim.
    creators = await_rows(
        'SELECT creator_identity_os_pid, creator_identity_os_boot_id, '
        'creator_identity_os_pid_starttime_ticks FROM runtime_generation '
        'WHERE spawn_invocation_uuid=?', (owner,))
    assert len(creators) == 1, creators
    pid, boot, start = creators[0]
    assert Path('/proc/sys/kernel/random/boot_id').read_text().strip() == boot
    stat = Path(f'/proc/{pid}/stat').read_text().rsplit(') ', 1)[1].split()
    assert int(stat[19]) == start
    assert os.path.samefile(f'/proc/{pid}/exe', runner)
    found = await_rows(
        'SELECT os_pid FROM pid_identity WHERE os_pid=? AND invocation_uuid=? AND session_id=?',
        (pid, owner, os.environ['session']))
    assert found == [(pid,)], found
    if UNPAUSE_MODE != 'none':
        pause = subprocess.run([runner, 'mailbox', 'pause', '--session-id', os.environ['session'], '--json'],
                               capture_output=True, text=True, timeout=15)
        assert pause.returncode == 0 and json.loads(pause.stdout)['paused'], (pause.stdout, pause.stderr)
    dispatch = subprocess.run([os.environ['AGENT_BASH_BIN'], 'run', '--completion-scope', 'tree',
                               '--delivery', 'async', '--', 'printf', 'early-ack-payload\n'],
                              capture_output=True, text=True, timeout=30)
    assert dispatch.returncode == 0, (dispatch.stdout, dispatch.stderr)
    result = json.loads(dispatch.stdout)
    assert result['dispatch_state'] == 'running', result
    (work / 'early-ack-dispatch.json').write_text(dispatch.stdout)
    found = await_rows('SELECT session_id,owner_invocation_uuid FROM mailbox WHERE handle=?', (result['handle'],))
    assert found == [(os.environ['session'], owner)], found
    if UNPAUSE_MODE == 'busy':
        unpause = subprocess.run([runner, 'mailbox', 'resume', '--session-id', os.environ['session'], '--json'],
                                 capture_output=True, text=True, timeout=15)
        assert unpause.returncode == 0, (unpause.stdout, unpause.stderr)
        response = json.loads(unpause.stdout)
        assert not response['paused'] and response['wake']['status'] == 'busy', response
        assert rows('SELECT delivered_at,delivery_attempts FROM mailbox WHERE handle=?', (result['handle'],)) == [(None, 0)]
        assert not (work / 'early-ack-verified.json').exists()
        (work / 'busy-unpause.json').write_text(unpause.stdout)
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
