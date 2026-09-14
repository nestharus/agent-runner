"""Fake provider's real MCP client. No provider/model or Bash protocol mocks."""
import json
import os
import re
import select
import subprocess
import time
from types import SimpleNamespace


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
            reply = call(2, 'tools/call', {'name': 'bash', 'arguments': {'command': workload, 'delivery': mode}})
            assert not reply.get('isError'), reply
            text = reply['content'][0]['text']
            handle = re.search(r'\bhandle=(ab_[a-zA-Z0-9_]+)', text).group(1)
            (root / 'mcp-result.txt').write_text(text)
            if mode == 'sync':
                # Framing can evolve; output is exact, not a substring assertion.
                body = text.split('--- output ---\n', 1)[1]
                assert body.encode() == b'paired-source-output', repr(body)
                (root / 'sync-byte-response.json').write_text(json.dumps({'output': body, 'transport': 'actual MCP JSON-RPC response'}))
            (root / 'mcp-response-received').touch()
            if mode == 'async':
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
