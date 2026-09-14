"""Fake provider's real MCP client. No provider/model or Bash protocol mocks."""
import json
import os
import re
import select
import sqlite3
import subprocess
import time
from types import SimpleNamespace
from concurrent.futures import ThreadPoolExecutor


def dispatch(root, mode, workload, env):
    assets = os.environ['AGE360_E2E_ASSETS']
    env = dict(env, AGENT_BASH_BIN=os.environ['AGE360_AGENT_BASH_BIN'],
               AGENT_RUNNER_CODEX_SESSION_ID='ses_age360_native_wake',
               AGENT_RUNNER_CODEX_INTERACTIVE='1', AGENT_BASH_TOOL_POLL_MS='20')
    # This fixture uses the runner's real native session-marker binding, not the
    # Codex host-specific metadata handshake. No registration capability forged.
    with (root / 'mcp.stderr').open('wb') as err:
        bridge = subprocess.Popen([assets + '/bun', assets + '/integrations/codex/agent-bash-mcp.ts'],
                                  stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=err, env=env)
        try:
            def call(ident, method, params):
                request = dict(jsonrpc='2.0', id=ident, method=method, params=params)
                with (root / 'mcp-wire.jsonl').open('a') as log:
                    log.write(json.dumps({'request': request}) + '\n')
                bridge.stdin.write((json.dumps(request) + '\n').encode())
                bridge.stdin.flush()
                assert select.select([bridge.stdout], [], [], 120)[0], 'MCP response deadline'
                line = bridge.stdout.readline()
                assert line, 'MCP closed before response'
                reply = json.loads(line)
                with (root / 'mcp-wire.jsonl').open('a') as log:
                    log.write(json.dumps({'response': reply}) + '\n')
                assert reply['id'] == ident and 'error' not in reply, reply
                return reply['result']
            call(1, 'initialize', {})
            case = os.environ['AGE360_CASE']
            detached = case.startswith('detach-')
            def detach(handle):
                result = subprocess.run([env['AGENT_BASH_BIN'], 'detach', handle], env=env,
                                        capture_output=True, timeout=45)
                with (root / 'detach-results.jsonl').open('a') as log:
                    log.write(json.dumps(dict(rc=result.returncode, stdout=result.stdout.decode(),
                                             stderr=result.stderr.decode(), caller_pid=os.getpid(), handle=handle)) + '\n')
                if case == 'detach-lost-reply':
                    return result
                assert result.returncode == 0, result.stderr
                return result
            # Real MCP request stays outstanding while a different executor,
            # descended from the original provider, requests delivery handoff.
            with ThreadPoolExecutor(max_workers=1) as pool:
                pending = pool.submit(call, 2, 'tools/call', {'name': 'bash', 'arguments': {'command': workload, 'delivery': mode}})
                if detached and case not in ('detach-after', 'detach-lost-reply'):
                    deadline = time.monotonic() + 60
                    registrations = []
                    while not registrations:
                        assert time.monotonic() < deadline, 'registration deadline'
                        registrations = list((root / 'spool/agent-bash').glob('*/source-registration-v2.json'))
                        time.sleep(.02)
                    assert len(registrations) == 1
                    registration = json.loads(registrations[0].read_text())
                    handle = registration['handle']
                    (root / 'registration-before-detach.json').write_text(json.dumps(registration))
                    while True:
                        with sqlite3.connect('file:' + str(root / 'data/pid-identity.db') + '?mode=ro', uri=True) as db:
                            admitted = db.execute('SELECT 1 FROM completion_event_listener WHERE event_id=?', (handle,)).fetchone()
                        if admitted: break
                        assert time.monotonic() < deadline, 'listener admission deadline'
                        time.sleep(.02)
                    if case == 'detach-race':
                        (root / 'release-workload').touch()
                    detach(handle)
                    if case == 'detach-before':
                        with sqlite3.connect('file:' + str(root / 'data/pid-identity.db') + '?mode=ro', uri=True) as db:
                            state = db.execute('SELECT state FROM completion_event WHERE event_id=?', (handle,)).fetchone()[0]
                            listener = db.execute('SELECT active,mailbox_seq,acknowledged_at FROM completion_event_listener WHERE event_id=?', (handle,)).fetchone()
                        assert state == 'pending' and listener == (1, None, None), (state, listener)
                        (root / 'helper-is-not-completion.json').write_text(json.dumps(dict(event_state=state, listener=listener)))
                    (root / 'detach-accepted').touch()
                    (root / 'release-workload').touch()
                reply = pending.result(timeout=120)
            assert not reply.get('isError'), reply
            text = reply['content'][0]['text']
            handle = re.search(r'\bhandle=(ab_[a-zA-Z0-9_]+)', text).group(1)
            (root / 'mcp-result.txt').write_text(text)
            if mode == 'sync' and (not detached or case in ('detach-after', 'detach-lost-reply')):
                # Framing can evolve; output is exact, not a substring assertion.
                body = text.split('--- output ---\n', 1)[1]
                assert body.encode() == b'paired-source-output', repr(body)
                (root / 'sync-byte-response.json').write_text(json.dumps({'output': body, 'transport': 'actual MCP JSON-RPC response'}))
            if detached:
                if case in ('detach-after', 'detach-lost-reply'):
                    registration = json.loads((root / 'spool/agent-bash' / handle / 'source-registration-v2.json').read_text())
                    (root / 'registration-before-detach.json').write_text(json.dumps(registration))
                    deadline = time.monotonic() + 60
                    while True:
                        with sqlite3.connect('file:' + str(root / 'data/pid-identity.db') + '?mode=ro', uri=True) as db:
                            accepted = db.execute("SELECT phase FROM completion_continuation_source WHERE event_id=?", (handle,)).fetchone()
                        if accepted == ('accepted',): break
                        assert time.monotonic() < deadline, 'acceptance before detach deadline'
                        time.sleep(.02)
                    first = detach(handle)
                    if case == 'detach-lost-reply':
                        assert first.returncode != 0 and (root / 'activation-reply-lost').exists(), 'reply-loss intervention did not discriminate'
                        with sqlite3.connect('file:' + str(root / 'data/pid-identity.db') + '?mode=ro', uri=True) as db:
                            db.row_factory = sqlite3.Row
                            first_request = dict(db.execute('SELECT * FROM completion_continuation_notification WHERE event_id=?', (handle,)).fetchone())
                        assert first_request['requested_at'], 'owner had not accepted activation'
                        (root / 'request-before-retry.json').write_text(json.dumps(first_request))
                # A real repeat control request must reconcile the same handoff,
                # never launch another workload or invent a listener ACK.
                repeated = detach(handle)
                if case == 'detach-lost-reply':
                    # Keep the unfavorable Bash outcome, then exercise Runner's
                    # same-target API retry separately; this does not turn Bash
                    # reconciliation into a pass.
                    (root / 'bash-reconciliation.json').write_text(json.dumps(dict(
                        reconciled=repeated.returncode == 0, rc=repeated.returncode)))
                    recovered = subprocess.run([env['AGENT_BASH_AGENT_RUNNER_BIN'], 'notify',
                        'agent-bash-activate', '--handle', handle], env=env, capture_output=True, timeout=45)
                    (root / 'runner-activation-retry.json').write_text(json.dumps(dict(rc=recovered.returncode,
                        stdout=recovered.stdout.decode(), stderr=recovered.stderr.decode())))
                    assert recovered.returncode == 0, recovered.stderr
                    with sqlite3.connect('file:' + str(root / 'data/pid-identity.db') + '?mode=ro', uri=True) as db:
                        db.row_factory = sqlite3.Row
                        after = dict(db.execute('SELECT * FROM completion_continuation_notification WHERE event_id=?', (handle,)).fetchone())
                    assert first_request == after, 'retry replaced first effective provenance or invented ACK'
                    (root / 'request-after-retry.json').write_text(json.dumps(after))
                (root / 'detach-accepted').touch()
            (root / 'mcp-response-received').touch()
            if mode == 'async' or detached:
                # Keep the synthetic MCP host alive until original completion;
                # immediate host shutdown can explicitly cancel running work.
                deadline = time.monotonic() + 120
                snapshot = root / 'spool/agent-bash' / handle / 'completion-snapshot-v2.json'
                while not snapshot.exists():
                    assert time.monotonic() < deadline, 'async original completion deadline'
                    time.sleep(.02)
            return SimpleNamespace(stdout=json.dumps({'handle': handle}).encode(), stderr=b'', returncode=0)
        finally:
            bridge.stdin.close()
            bridge.wait(timeout=8)
            (root / 'mcp-closed.json').write_text(json.dumps({'pid': bridge.pid, 'rc': bridge.returncode, 'time_ns': time.time_ns()}))
