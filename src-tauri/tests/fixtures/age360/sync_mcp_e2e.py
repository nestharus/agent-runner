#!/usr/bin/env python3
"""Permanent isolated regression: real MCP -> shared adapter -> Bash -> runner.

All executable/source inputs required. Stages exact copies before masking /home,
/root and /run in private mount/PID/network/user namespaces. Never invokes agents.
The runner launches only the repository's synthetic native-provider fixture.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import traceback

SESSION = 'ses_age360_native_wake'
MODEL = 'age360-native-model'
PROVIDER = 'age360-native-provider'


def digest(path):
    return hashlib.file_digest(open(path, 'rb'), 'sha256').hexdigest()


def wait(read, label):
    end = time.monotonic() + 120
    while time.monotonic() < end:
        try:
            result = read()
            if result:
                return result
        except (sqlite3.OperationalError, FileNotFoundError):
            pass
        time.sleep(.02)
    raise AssertionError('bounded observation expired: ' + label)


def rows(db, sql, params=()):
    with sqlite3.connect('file:' + str(db) + '?mode=ro', uri=True) as c:
        c.row_factory = sqlite3.Row
        return [dict(row) for row in c.execute(sql, params)]


def owner_evidence(db, expected):
    owner = rows(db, "SELECT * FROM completion_continuation_owner WHERE phase='running'")[0]
    actors = {}
    for role in ('guardian_identity', 'driver_identity'):
        identity = json.loads(owner[role])
        pid = identity['pid']
        assert pid > 1
        stat = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
        assert int(stat[19]) == identity['starttime_ticks']
        actual = digest(Path(f'/proc/{pid}/exe'))
        assert actual == digest(expected), (role, actual)
        actors[role] = dict(identity=identity, sha256=actual)
    return dict(owner=owner, actors=actors)


def repair_barrier(root, driver, event, start):
    # PROFILE is emitted after the SQL statement finishes. In this driver, the
    # listener activation statement belongs to triggered-event registration
    # repair. Pair it with successful COMMIT on the SAME SQLite connection.
    text = (root / 'sql-audit.tsv').read_text()[start:]
    pending = {}
    for line in text.splitlines():
        parts = line.split('\t', 6)
        if len(parts) != 7 or parts[1] != str(driver):
            continue
        _, _, connection, _, error, autocommit, sql = parts
        if 'UPDATE completion_event_listener SET active=1' in sql and event in sql:
            pending[connection] = line
        if sql == 'ROLLBACK':
            pending.pop(connection, None)
        if sql == 'COMMIT' and error == '0' and autocommit == '1' and connection in pending:
            return dict(update_profile=pending[connection], commit_profile=line,
                        interpretation='driver triggered-event repair SQL completed and same-connection commit succeeded')
    return None


def activation_output_obligations(root, terminal, expected):
    """Launcher output, provider outcome and physical wait are distinct from ACK."""
    state_db = root / 'data/state.db'
    attempts = [a for a in terminal['completion_continuation_attempt'] if a['operation'] == 'activation']
    evidence = []
    for attempt in attempts:
        # Resolve only this case's retained synthetic attempt, not arbitrary DB paths.
        directory = root / 'data/completion-continuation' / attempt['domain_id'] / 'attempts' / attempt['claim_token']
        stdout = (directory / 'launcher.stdout').read_bytes()
        stderr = (directory / 'launcher.stderr').read_bytes()
        invocation = rows(state_db, 'SELECT * FROM invocations WHERE invocation_uuid=?', (attempt['spawn_invocation_uuid'],))
        delivery = rows(state_db, 'SELECT * FROM invocation_output_deliveries WHERE invocation_uuid=?', (attempt['spawn_invocation_uuid'],))
        controls = [json.loads(line.split(b'=', 1)[1]) for line in stderr.splitlines() if line.startswith(b'OULIPOLY_RESULT=')]
        retained = root / 'data/invocations/output' / (attempt['spawn_invocation_uuid'] + '.stdout')
        evidence.append(dict(attempt=attempt, native_result=json.loads((directory / 'result.json').read_text()),
                             stdout_hex=stdout.hex(), stderr_hex=stderr.hex(), controls=controls,
                             invocation=invocation, delivery=delivery, retained_stdout_hex=retained.read_bytes().hex()))
    (root / 'activation-output-obligations.json').write_text(json.dumps(evidence, indent=2))
    assert len(attempts) == int(expected), ('activation count/replay', len(attempts), expected)
    for observed in evidence:
        receipt = observed['native_result']
        assert receipt['root_exit_code'] == 0 and receipt['root_wait_status'] == 0, ('activation terminal failure', receipt)
        assert receipt['owned_children'] == 'ECHILD' and receipt['spawn_error'] is None and receipt['accepted_cancellation'] is None
        assert observed['attempt']['phase'] == 'drained' and observed['attempt']['integrated'] == 1
        assert observed['stdout_hex'] == b'native resumed\n'.hex() == observed['retained_stdout_hex'], 'activation output not delivered'
        assert len(observed['invocation']) == 1 and len(observed['delivery']) == 1
        invocation, delivery = observed['invocation'][0], observed['delivery'][0]
        assert invocation['status'] == 'succeeded' and invocation['exit_code'] == 0
        assert delivery['provider_outcome_state'] == 'settled' and delivery['delivery_state'] == 'delivered'
        assert delivery['delivered_at'] and delivery['delivery_failure_stage'] is None and delivery['delivery_failure_kind'] is None
        assert delivery['stdout_bytes'] == 15 and delivery['stdout_sha256'] == hashlib.sha256(b'native resumed\n').hexdigest()
        assert len(observed['controls']) == 1, 'missing or repeated final control'
        control = observed['controls'][0]
        assert control['id'] == invocation['invocation_uuid'] and control['success'] and control['exit_code'] == 0
        assert 'stale_or_missing_provider_launch_owner_fence' not in bytes.fromhex(observed['stderr_hex']).decode()


def case(stage, mode, owner_kind=None):
    root = stage / (f'{owner_kind}-{mode}' if owner_kind else mode)
    root.mkdir()
    data = root / 'data'
    data.mkdir()
    config = root / 'config'
    app = config / 'oulipoly-agent-runner'
    models = app / 'models'
    models.mkdir(parents=True)
    (root / 'home').mkdir()
    for file in ('native-provider.py', 'mcp_client.py'):
        shutil.copyfile(stage / file, root / file)
    wrapper = root / 'provider.sh'
    wrapper.write_text('#!/bin/sh\nexec /usr/bin/python3 "' + str(root / 'native-provider.py') + '" "$@"\n')
    wrapper.chmod(0o755)
    (models / (MODEL + '.toml')).write_text('prompt_mode="arg"\n[[providers]]\nname="' + PROVIDER + '"\nargs=[]\n')
    (app / 'providers.toml').write_text(f'''[{PROVIDER}]
command="age360-native-fixture"
args=[]
prompt_mode="arg"
settings_id="{PROVIDER}"
[{PROVIDER}.implementation]
family="age360-native-wake"
executable="{wrapper}"
''')
    env = dict(PATH='/usr/bin:/bin', HOME=str(root / 'home'), XDG_CONFIG_HOME=str(config),
               OULIPOLY_CONFIG_HOME=str(config), XDG_STATE_HOME=str(root / 'spool'),
               XDG_DATA_HOME=str(data), OULIPOLY_DATA_DIR=str(data),
               AGE360_ROOT=str(root), AGE360_CASE=mode, AGE360_MCP_E2E='1',
               AGE360_E2E_ASSETS=str(stage), AGE360_AGENT_BASH_BIN=str(stage / 'agent-bash'),
               AGENT_BASH_AGENT_RUNNER_BIN=str(stage / 'runner'),
               LD_PRELOAD=str(stage / 'process-audit.so'), AGE360_PROCESS_AUDIT=str(root / 'process-audit.tsv'),
               AGE360_NATIVE_WAKE_MARKER=str(root / 'resume-prompts.jsonl'),
               AGE360_NATIVE_WAKE_GATE=str(root / 'release-resume'),
               AGE360_WORKLOAD_GATE=str(root / 'release-workload'))
    if mode == 'detach-lost-reply':
        env['AGE360_DROP_ACTIVATION_REPLY'] = str(root / 'activation-reply-lost')
    detached = mode.startswith("detach-")
    projection_fault = mode == 'async-projection-fault'
    if projection_fault:
        env.update(AGE360_SQLITE_SYMBOLS=str(stage / 'sqlite-symbols.txt'),
                   AGE360_SQL_AUDIT=str(root / 'sql-audit.tsv'),
                   AGE360_PROJECTION_FAULT=str(root / 'projection-fault-observed'))
    founder = None
    coordinated = (stage / "coordinated-idle-replacement").exists()
    if owner_kind:
        env.update(AGE360_SQLITE_SYMBOLS=str(stage / 'sqlite-symbols.txt'),
                   AGE360_SQL_AUDIT=str(root / 'sql-audit.tsv'))
        owner_binary = stage / ('prior-runner' if owner_kind == 'prior' else 'runner')
        founder_env = dict(env)  # no founder flags leak into the elected guardian's recipient environment
        founder = subprocess.Popen([owner_binary, '-m', MODEL, '--models-dir', models, 'synthetic independent owner founder'],
                                   env=founder_env, cwd=root, stdin=subprocess.DEVNULL,
                                   stdout=open(root / 'founder.stdout', 'wb'), stderr=open(root / 'founder.stderr', 'wb'))
        wait(lambda: (root / 'provider-initial-ready').exists(), 'founder native context alive')
        assert founder.poll() is None
        founded = owner_evidence(data / 'pid-identity.db', owner_binary)
        (root / 'owner-founded.json').write_text(json.dumps(founded, indent=2))
        if coordinated:
            # Candidate operational transition, NOT a product drain API. This
            # closed fixture has exactly one admission-capable entry and no
            # admitted work. Never extrapolate its inventory to a real domain.
            db = data / 'pid-identity.db'
            assert not rows(db, 'SELECT * FROM completion_continuation_source')
            assert not rows(db, 'SELECT * FROM completion_event_listener')
            assert not rows(db, 'SELECT * FROM completion_continuation_attempt')
            with socket.socket(socket.AF_UNIX) as probe:
                probe.settimeout(5)
                probe.connect(founded['owner']['endpoint'])
                probe.sendall(b'drain\n')
                response = probe.recv(8192)
                assert response == b'', 'existing owner unexpectedly supports drain'
            # Release the fake provider normally, do not signal an owner or
            # fabricate a DB phase. No other fixture entry can admit during it.
            (root / 'release-founder').touch()
            assert founder.wait(timeout=30) == 0
            guardian = founded['actors']['guardian_identity']['identity']['pid']
            waited = wait(lambda: (result if (result := os.waitpid(guardian, os.WNOHANG))[0] else None),
                          'actual original guardian terminal wait')
            assert waited == (guardian, 0), waited
            assert not Path(founded['owner']['endpoint']).exists()
            assert not rows(db, "SELECT * FROM completion_continuation_owner WHERE phase='running'")
            (root / 'coordinated-retirement.json').write_text(json.dumps(dict(
                operation='stop sole synthetic entry admissions; normal provider exit; wait original guardian; start fixed entry',
                drain_request_response_hex=response.hex(), founder_returncode=founder.returncode,
                guardian_wait=waited, original_owner=founded,
                owners=rows(db, 'SELECT * FROM completion_continuation_owner'),
                inventory_scope='empty completion inventory; no active-work transfer tested'), indent=2))
            founder = None
            owner_binary = stage / 'runner'
    # Hold fake provider before registration so passive private SQL audit triggers
    # are installed before any listener exists. They record, never change rows.
    (root / 'hold-mcp-admission').touch()
    proc = subprocess.Popen([stage / 'runner', '-m', MODEL, '--models-dir', models, 'synthetic MCP completion regression'],
                            env=env, cwd=root, stdin=subprocess.DEVNULL,
                            stdout=open(root / 'native.stdout', 'wb'), stderr=open(root / 'native.stderr', 'wb'))
    db = data / 'pid-identity.db'
    wait(lambda: rows(db, "SELECT name FROM sqlite_master WHERE name='completion_event_listener'"), 'schema ready')
    with sqlite3.connect(db) as c:
        c.executescript('''
CREATE TABLE e2e_listener_audit(seq INTEGER PRIMARY KEY, operation TEXT, event_id TEXT, listener_id TEXT, old_active INTEGER, new_active INTEGER, at TEXT);
CREATE TRIGGER e2e_listener_insert AFTER INSERT ON completion_event_listener BEGIN
 INSERT INTO e2e_listener_audit(operation,event_id,listener_id,old_active,new_active,at) VALUES('insert',NEW.event_id,NEW.listener_id,NULL,NEW.active,strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;
CREATE TRIGGER e2e_listener_update AFTER UPDATE OF active ON completion_event_listener BEGIN
 INSERT INTO e2e_listener_audit(operation,event_id,listener_id,old_active,new_active,at) VALUES('update',NEW.event_id,NEW.listener_id,OLD.active,NEW.active,strftime('%Y-%m-%dT%H:%M:%fZ','now')); END;
''')
    if projection_fault:
        with sqlite3.connect(db) as c:
            c.executescript("CREATE TRIGGER e2e_reject_projection BEFORE UPDATE OF state ON completion_event WHEN NEW.state='triggered' BEGIN SELECT e2e_projection_fault(); END;")
    if owner_kind:
        with sqlite3.connect(db) as c:
            c.executescript('''
ALTER TABLE e2e_listener_audit ADD COLUMN writer_pid INTEGER;
ALTER TABLE e2e_listener_audit ADD COLUMN owner_generation TEXT;
CREATE TRIGGER e2e_listener_writer AFTER INSERT ON e2e_listener_audit BEGIN
 UPDATE e2e_listener_audit SET writer_pid=e2e_writer_pid(),owner_generation=(SELECT generation FROM completion_continuation_owner WHERE phase='running') WHERE seq=NEW.seq; END;
''')
        if coordinated:
            wait(lambda: rows(db, "SELECT * FROM completion_continuation_owner WHERE phase='running'"),
                 'new owner published after preserved-schema startup')
        joined = owner_evidence(db, owner_binary)
        if coordinated:
            assert joined['owner']['domain_id'] == founded['owner']['domain_id'], 'domain was not preserved'
            assert joined['owner']['generation'] != founded['owner']['generation'], 'old generation survived'
        else:
            assert joined['owner']['generation'] == founded['owner']['generation'], 'foreground did not retain independent owner'
        (root / 'owner-joined.json').write_text(json.dumps(joined, indent=2))
        # Identical private schedule: fixed helper accepts before owner repairs.
        driver = joined['actors']['driver_identity']['identity']['pid']
        os.kill(driver, signal.SIGSTOP)
        wait(lambda: Path(f'/proc/{driver}/stat').read_text().rsplit(')', 1)[1].split()[0] == 'T', 'private driver stopped before admission')
    (root / 'hold-mcp-admission').unlink()
    source = wait(lambda: rows(db, 'SELECT * FROM completion_continuation_source'), 'source admission')[0]
    if mode not in ('detach-before', 'detach-race'):
        (root / 'release-workload').touch()
    wait(lambda: (root / 'mcp-response-received').exists(), 'actual MCP response')
    if projection_fault:
        wait(lambda: (root / 'projection-fault-observed').exists(), 'actual projection failure')
        handle_dir = root / 'spool/agent-bash' / source['event_id']
        assert (handle_dir / 'completion-snapshot-v2.json').exists()
        assert (handle_dir / 'source-outcome-v2.json').exists()
        failed_projection = dict(source=rows(db, 'SELECT * FROM completion_continuation_source'),
                                 listeners=rows(db, 'SELECT * FROM completion_event_listener'),
                                 events=rows(db, 'SELECT * FROM completion_event'), mailbox=rows(db, 'SELECT * FROM mailbox'))
        assert failed_projection['source'][0]['phase'] == 'registered'
        assert failed_projection['events'][0]['state'] == 'pending'
        assert not failed_projection['mailbox']
        assert failed_projection['listeners'][0]['acknowledged_at'] is None
        (root / 'failed-projection.json').write_text(json.dumps(failed_projection, default=str, indent=2))
        with sqlite3.connect(db) as c:
            c.execute('DROP TRIGGER e2e_reject_projection')
    wait(lambda: rows(db, "SELECT * FROM completion_continuation_source WHERE phase='accepted'"), 'accepted source')
    independent = mode == 'sync-independent'
    if independent:
        original_owner = owner_evidence(db, stage / 'runner')
        (root / 'release-initial-provider').touch()
        assert proc.wait(timeout=45) == 0
        guardian = original_owner['actors']['guardian_identity']['identity']['pid']
        waited = wait(lambda: (result if (result := os.waitpid(guardian, os.WNOHANG))[0] else None),
                      'original owner retirement before independent resume')
        assert waited == (guardian, 0)
        (root / 'independent-retirement.json').write_text(json.dumps(dict(owner=original_owner, guardian_wait=waited), indent=2))
        # A new native actor acquires its own live capability. The fixture does
        # not forge a listener or impersonate the immutable source owner.
        with open(root / 'listener.stdout', 'wb') as out, open(root / 'listener.stderr', 'wb') as err:
            listener_proc = subprocess.Popen([stage / 'runner', '--resume', SESSION, '--models-dir', models,
                'synthetic independent listener'], env=env, cwd=root, stdin=subprocess.DEVNULL, stdout=out, stderr=err)
            listener_rc = listener_proc.wait(timeout=45)
        if listener_rc != 0:
            rejected = json.loads((root / 'independent-listener-result.json').read_text())
            reply = json.loads(rejected['stdout'])
            assert rejected['rc'] == 1 and reply['status'] == 'unavailable'
            assert 'has no authoritative session binding for completion registration' in reply['message'], reply
            actor = json.loads((root / 'independent-admission-before.json').read_text())
            assert actor['runtime_session'] == SESSION
            assert actor['actor']['provider_session_id'] is None and actor['actor']['session_id'] is None
            assert actor['actor']['invocation_uuid'] == rejected['invocation']
            retained = rows(db, 'SELECT * FROM completion_event_listener')
            assert len(retained) == 1 and retained[0]['active'] == 0
            assert retained[0]['acknowledged_at'] is None and retained[0]['mailbox_seq'] is None
            assert len(rows(db, 'SELECT * FROM completion_continuation_source')) == 1
            assert (root / 'source-launches').read_bytes() == b'launch\n'
            assert json.loads((root / 'sync-byte-response.json').read_text())['output'] == 'paired-source-output'
            (root / 'independent-admission-red.json').write_text(json.dumps(dict(
                reply=reply, before=actor, retained_listeners=retained), indent=2))
            print(json.dumps(dict(mode=mode, result='INDEPENDENT_LISTENER_ADMISSION_RED',
                                  reason=reply['message'])), flush=True)
            return True
        wait(lambda: len(rows(db, 'SELECT * FROM completion_event_listener')) == 2, 'independent listener admitted')
    before = rows(db, 'SELECT * FROM completion_event_listener')
    if detached:
        assert len(before) == 1 and before[0]['active'] == 1
        assert before[0]['acknowledged_at'] is None, 'handoff must not fabricate ACK'
        assert len(rows(db, 'SELECT * FROM completion_continuation_source')) == 1
        assert len(rows(db, 'SELECT * FROM mailbox')) == 1
        request = rows(db, 'SELECT * FROM completion_continuation_notification')[0]
        assert request['requested_at'] and request['request_basis'] == 'explicit_listener_activation_unattributed'
        (root / 'accepted-handoff.json').write_text(json.dumps(dict(listeners=before, request=request), indent=2))
    (root / 'release-resume').touch()
    if owner_kind:
        # Require NEW observed post-response repair, not generation or silence.
        (root / 'fixed-acceptance-before-repair.json').write_text(json.dumps(dict(
            source=rows(db, 'SELECT * FROM completion_continuation_source'), listeners=before,
            audit=rows(db, 'SELECT * FROM e2e_listener_audit')), default=str, indent=2))
        if mode == 'sync':
            assert len(before) == 1 and before[0]['active'] == 0, 'sync must be inactive after fixed helper acceptance'
        start = len((root / 'sql-audit.tsv').read_text())
        os.kill(driver, signal.SIGCONT)
        barrier = wait(lambda: repair_barrier(root, driver, source['event_id'], start),
                       'actual independent driver repair commit')
        barrier['owner'] = owner_evidence(db, owner_binary)
        barrier['trace_start_offset'] = start
        (root / 'repair-barrier.json').write_text(json.dumps(barrier, indent=2))
    qualified = bool(rows(db, "SELECT name FROM sqlite_master WHERE name='completion_continuation_notification'"))
    if qualified and mode == 'sync' and not owner_kind:
        owner_before_release = owner_evidence(db, stage / 'runner')
        (root / 'owner-before.json').write_text(json.dumps(owner_before_release, indent=2))
    (root / 'release-initial-provider').touch()
    assert proc.wait(timeout=45) == 0, 'native initial failed'
    if mode == 'async' or projection_fault or detached or independent or (owner_kind and rows(db, 'SELECT * FROM completion_event_listener WHERE active=1')):
        wait(lambda: (root / 'recipient-byte-receipt.json').exists(), 'actual same-session notification receipt')
        wait(lambda: rows(db, 'SELECT * FROM completion_event_listener WHERE acknowledged_at IS NOT NULL'), 'listener ACK')
    # Explicit private owner-generation replacement forces durable inventory repair.
    # This is not a service/installation restart and cannot address deployed state.
    if qualified and mode == 'sync' and not owner_kind:
        # Exact original guardian wait, not missing PID or fabricated ACK.
        guardian = owner_before_release['actors']['guardian_identity']['identity']['pid']
        waited = wait(lambda: (result if (result := os.waitpid(guardian, os.WNOHANG))[0] else None),
                      'qualified original guardian normal retirement')
        assert waited == (guardian, 0), waited
        assert not rows(db, "SELECT * FROM completion_continuation_owner WHERE phase='running'")
        assert not Path(owner_before_release['owner']['endpoint']).exists()
        (root / 'qualified-retirement.json').write_text(json.dumps(dict(guardian_wait=waited, owner=owner_before_release), indent=2))
    elif mode == 'sync' and not owner_kind:
        owner = rows(db, "SELECT * FROM completion_continuation_owner WHERE phase='running'")[0]
        (root / 'owner-before.json').write_text(json.dumps(owner, default=str, indent=2))
        identity = json.loads(owner['driver_identity'])
        assert identity['pid'] > 1
        stat = Path(f"/proc/{identity['pid']}/stat").read_text().rsplit(')', 1)[1].split()
        assert int(stat[19]) == identity['starttime_ticks'], 'driver PID identity mismatch'
        os.kill(identity['pid'], 9)
        wait(lambda: [x for x in rows(db, "SELECT * FROM completion_continuation_owner WHERE phase='running'") if x['generation'] != owner['generation']], 'private guardian driver replacement')
    wait(lambda: not rows(db, "SELECT * FROM completion_continuation_attempt WHERE integrated=0"), 'attempt integration')
    # Read durable terminal states after accepted source and replacement, no short
    # silence oracle. Capture all columns including payload/reference retention.
    tables = ('completion_continuation_source', 'completion_event_listener', 'completion_continuation_attempt', 'completion_continuation_owner', 'e2e_listener_audit')
    terminal = {table: rows(db, 'SELECT * FROM ' + table) for table in tables}
    if qualified:
        terminal['completion_continuation_notification'] = rows(db, 'SELECT * FROM completion_continuation_notification')
    listed = subprocess.run([stage / 'runner', 'mailbox', 'list', '--session-id', SESSION, '--all', '--json'], env=env, cwd=root, capture_output=True, check=True, timeout=15)
    terminal['mailbox'] = json.loads(listed.stdout)
    terminal['before_provider_exit'] = before
    if owner_kind:
        writers = {}
        for line in (root / 'sql-audit.tsv.writers').read_text().splitlines():
            pid, sha, executable = line.split('\t', 2)
            writers[pid] = dict(sha256=sha, executable=executable)
        terminal['sql_writers'] = writers
        for write in terminal['e2e_listener_audit']:
            write['writer'] = writers[str(write['writer_pid'])]
        assert writers[str(driver)]['sha256'] == digest(owner_binary)
        admission = terminal['e2e_listener_audit'][0]
        assert admission['operation'] == 'insert' and admission['writer']['sha256'] == digest(stage / 'runner')
    (root / 'terminal.json').write_text(json.dumps(terminal, default=str, indent=2))
    listeners = terminal['completion_event_listener']
    assert len(listeners) == (2 if independent else 1)
    listener = next(l for l in listeners if l['owner_invocation_uuid'] == json.loads(
        (root / 'spool/agent-bash' / source['event_id'] / 'source-registration-v2.json').read_text())['owner_invocation_uuid'])
    assert listener['session_id'] == SESSION
    assert terminal['completion_continuation_source'][0]['phase'] == 'accepted'
    handle_dir = root / 'spool/agent-bash' / listener['event_id']
    registration = json.loads((handle_dir / 'source-registration-v2.json').read_text())
    outcome = json.loads((handle_dir / 'source-outcome-v2.json').read_text())
    assert registration['delivery_mode'] == ('sync' if detached or independent else 'async' if projection_fault else mode)
    if detached:
        assert registration == json.loads((root / 'registration-before-detach.json').read_text())
    assert registration['owner_session_id'] == listener['session_id']
    assert registration['owner_invocation_uuid'] == listener['owner_invocation_uuid']
    assert registration['helper']['sha256'] == digest(stage / 'runner'), 'wrong admitted helper executable'
    assert registration['recovery']['sha256'] == digest(stage / 'agent-bash'), 'wrong recovery executable'
    assert outcome['root_wait_status'] == 0 and outcome['output_closed']
    assert outcome['original_tree_drained'], 'original work not physically drained'
    assert outcome['registration_id'] == registration['registration_id']
    assert source['registration_id'] == registration['registration_id']
    audit = (root / 'process-audit.tsv').read_text()
    assert 'notify agent-bash-capability --json' in audit
    assert 'notify agent-bash-register --handle ' + listener['event_id'] in audit
    if detached:
        assert 'notify agent-bash-activate --handle ' + listener['event_id'] in audit
        assert len(terminal['completion_continuation_source']) == 1, 'helper acquired a workload source'
    else:
        assert 'notify agent-bash-activate' not in audit, 'unexpected explicit activation entry'
    (root / 'source-proof.json').write_text(json.dumps({'registration': registration, 'outcome': outcome}, indent=2))
    assert (root / 'source-launches').read_bytes() == b'launch\n', 'original workload replayed'
    duplicate = False
    handoff_red = mode == 'detach-lost-reply' and not json.loads((root / 'bash-reconciliation.json').read_text())['reconciled']
    if mode == 'sync' or independent:
        assert json.loads((root / 'sync-byte-response.json').read_text())['output'].encode() == b'paired-source-output'
        receipt = json.loads((handle_dir / 'output-receipt.json').read_text())
        (root / 'local-output-receipt.json').write_text(json.dumps(receipt, indent=2))
        assert 'accept-output ' + listener['event_id'] in audit
        assert 'status --tail-bytes 0 ' + listener['event_id'] in audit
        duplicate = bool(owner_kind and listener['active'])
        if duplicate:
            received = json.loads((root / 'recipient-byte-receipt.json').read_text())
            assert received['output_checked'] and received['output']['byte_len'] == len(b'paired-source-output')
            assert listener['event_id'] in (root / 'resume-prompts.jsonl').read_text()
            assert listener['acknowledged_at'] is not None
            activation = [x for x in terminal['e2e_listener_audit'] if x['old_active'] == 0 and x['new_active'] == 1]
            assert len(activation) == 1
            assert activation[0]['writer_pid'] == founded['actors']['driver_identity']['identity']['pid']
            assert activation[0]['owner_generation'] == founded['owner']['generation']
            (root / 'duplicate-proof.json').write_text(json.dumps(dict(listener=listener, activation=activation[0], recipient=received, owner=founded), indent=2))
        elif independent:
            assert listener['active'] == 0 and listener['mailbox_seq'] is None and listener['acknowledged_at'] is None
            other = next(l for l in listeners if l['listener_id'] != listener['listener_id'])
            assert other['session_id'] == listener['session_id'] and other['active'] == 1
            assert other['acknowledged_at'] is not None
            assert len(terminal['mailbox']['rows']) == 1
            request = next(n for n in terminal['completion_continuation_notification'] if n['listener_id'] == other['listener_id'])
            assert request['policy'] == 'notify' and request['requested_at'] is None
            assert json.loads((root / 'recipient-byte-receipt.json').read_text())['output_checked']
        else:
            assert listener['active'] == 0, 'SYNC REGRESSION: same-caller listener activated'
            assert listener['mailbox_seq'] is None and listener['acknowledged_at'] is None
            assert not terminal['mailbox']['rows'], 'SYNC REGRESSION: same-caller mailbox row'
            assert not (root / 'resume-prompts.jsonl').exists(), 'SYNC REGRESSION: actual notification'
            assert all(x['new_active'] == 0 for x in terminal['e2e_listener_audit']), 'transient sync activation'
    else:
        receipt = json.loads((root / 'recipient-byte-receipt.json').read_text())
        assert receipt['output_checked'] and receipt['output']['byte_len'] == len(b'paired-source-output')
        assert listener['active'] == 1 and listener['acknowledged_at'] is not None
        assert listener['event_id'] in (root / 'resume-prompts.jsonl').read_text()
    activation_output_obligations(root, terminal, bool(any(l['acknowledged_at'] for l in listeners)))
    if founder:
        assert founder.poll() is None, 'founding native context died during experiment'
        (root / 'owner-final.json').write_text(json.dumps(owner_evidence(db, owner_binary), indent=2))
        (root / 'release-founder').touch()
        assert founder.wait(timeout=30) == 0
    print(json.dumps({'owner_kind': owner_kind, 'mode': mode, 'result': 'BASH_RECONCILIATION_RED' if handoff_red else 'DUPLICATE_RED' if duplicate else 'PASS', 'listener': listener, 'audit_rows': len(terminal['e2e_listener_audit'])}), flush=True)
    return duplicate or handoff_red


def evidence_ignore(directory, names):
    import stat
    ignored = []
    for name in names:
        path = Path(directory) / name
        mode = path.lstat().st_mode
        if name.endswith(('.db', '.db-wal', '.db-shm', '.db-journal')):
            ignored.append(name)
        elif stat.S_ISSOCK(mode) or stat.S_ISFIFO(mode):
            ignored.append(name)
        elif path.is_file() and (path.stat().st_size > 32 * 1024 * 1024 or 'capability' in name or 'environment' in name):
            ignored.append(name)
    return ignored


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ('runner', 'agent-bash', 'bun', 'codex-source', 'bash-source', 'evidence'):
        parser.add_argument('--' + arg, required=True, type=Path)
    parser.add_argument('--handoff-cases', action='store_true', help='real paired detach before/after/race controls')
    parser.add_argument('--prior-runner', type=Path, help='enables fixed/prior independent-owner contrast')
    parser.add_argument('--coordinated-idle-replacement', action='store_true', help='test normal sole-context release and exact guardian wait BEFORE new admissions; requires prior runner')
    parser.add_argument('--prior-sha256', help='required exact expected prior artifact hash')
    args = parser.parse_args()
    assert not args.coordinated_idle_replacement or args.prior_runner, 'replacement requires contrast inputs'
    if args.prior_runner:
        assert args.prior_sha256 and digest(args.prior_runner) == args.prior_sha256, 'prior hash mismatch'
    assert not args.evidence.exists() or not any(args.evidence.iterdir()), 'use fresh evidence directory'
    args.evidence.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='age360-mcp-e2e-') as directory:
        stage = Path(directory)
        identities = {}
        if args.handoff_cases:
            assert not args.prior_runner, 'handoff cases require candidate-only owners'
            (stage / 'handoff-cases').touch()
        if args.coordinated_idle_replacement:
            (stage / 'coordinated-idle-replacement').touch()
        def copy(src, dest):
            src = src.resolve(strict=True)
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(src, dest)
            assert digest(dest) == digest(src), 'input changed during staging'
            identities[str(dest.relative_to(stage))] = {'source': str(src), 'sha256': digest(dest)}
        for name, path in [('runner', args.runner), ('agent-bash', args.agent_bash), ('bun', args.bun)]:
            copy(path, stage / name)
        if args.handoff_cases:
            nm = subprocess.run(['nm', str(stage / 'runner')], capture_output=True, text=True, check=True)
            symbols = {parts[2]: parts[0] for line in nm.stdout.splitlines() if len(parts := line.split()) == 3}
            (stage / 'sqlite-symbols.txt').write_text(symbols['sqlite3_open'] + ' ' + symbols['sqlite3_auto_extension'] + '\n')
        if args.prior_runner:
            copy(args.prior_runner, stage / 'prior-runner')
            assert digest(stage / 'prior-runner') == args.prior_sha256
            symbol_rows = []
            for binary in (stage / 'runner', stage / 'prior-runner'):
                nm = subprocess.run(['nm', str(binary)], capture_output=True, text=True, check=True)
                symbols = {parts[2]: parts[0] for line in nm.stdout.splitlines() if len(parts := line.split()) == 3}
                symbol_rows.append(symbols['sqlite3_open'] + ' ' + symbols['sqlite3_auto_extension'])
            (stage / 'sqlite-symbols.txt').write_text('\n'.join(symbol_rows) + '\n')
            (stage / 'owner-contrast').touch()
        for file in ('agent-bash-mcp.ts', 'opencode-tool-shim.ts', 'session-registration.ts'):
            copy(args.codex_source / 'integrations/codex' / file, stage / 'integrations/codex' / file)
        shared = Path('integrations/opencode/tools/bash.ts')
        assert digest(args.codex_source / shared) == digest(args.bash_source / shared), 'vendored shared adapter mismatch: disposition required'
        copy(args.bash_source / shared, stage / shared)
        for file in ('native-provider.py', 'mcp_client.py', 'sync_mcp_e2e.py', 'process_audit.c'):
            copy(Path(__file__).parent / file, stage / file)
        subprocess.run(['gcc', '-shared', '-fPIC', '-O2', '-Wall', '-o', str(stage / 'process-audit.so'), str(stage / 'process_audit.c'), '-ldl', '-lcrypto'], check=True)
        identities['process-audit.so'] = {'source': 'harness-local process_audit.c', 'sha256': digest(stage / 'process-audit.so')}
        (stage / 'identities.json').write_text(json.dumps(identities, indent=2))
        (stage / 'private.sh').write_text('''#!/bin/sh
set -eu
mount --make-rprivate /
mount -t tmpfs tmpfs /home
mount -t tmpfs tmpfs /root
mount -t tmpfs tmpfs /run
exec /usr/bin/python3 "$1/sync_mcp_e2e.py" --private "$1"
''')
        cmd = ['timeout', '--kill-after=5s', '600s' if args.prior_runner else '300s', 'unshare', '--user', '--map-root-user', '--net', '--pid', '--fork', '--mount-proc', '--kill-child=KILL', '--', '/bin/sh', str(stage / 'private.sh'), str(stage)]
        result = subprocess.run(cmd, cwd=stage, env={'PATH': '/usr/bin:/bin', 'HOME': str(stage), 'AGE360_PARENT_NET': str(os.readlink('/proc/self/ns/net'))}, capture_output=True)
        (args.evidence / 'run.stdout').write_bytes(result.stdout)
        (args.evidence / 'run.stderr').write_bytes(result.stderr)
        # Only synthetic evidence, omit binary copies. No production input copied.
        for entry in stage.iterdir():
            if entry.name in ('runner', 'prior-runner', 'agent-bash', 'bun', 'integrations'):
                continue
            destination = args.evidence / entry.name
            if entry.is_dir():
                shutil.copytree(entry, destination, dirs_exist_ok=True, symlinks=True, ignore=evidence_ignore)
            else:
                shutil.copy2(entry, destination)
        print(result.stdout.decode(errors='replace'), end='')
        print(result.stderr.decode(errors='replace'), file=sys.stderr, end='')
        print('isolated_e2e_rc=' + str(result.returncode))
        return result.returncode


if __name__ == '__main__':
    if len(sys.argv) == 3 and sys.argv[1] == '--private':
        stage = Path(sys.argv[2])
        assert os.readlink('/proc/self/ns/net') != os.environ['AGE360_PARENT_NET']
        assert Path.cwd() == stage, 'namespace controller must not retain a production cwd'
        assert not list(Path('/home').iterdir())
        (stage / 'isolation.json').write_text(json.dumps({
            'net': os.readlink('/proc/self/ns/net'), 'pid': os.readlink('/proc/self/ns/pid'),
            'mount': os.readlink('/proc/self/ns/mnt'), 'home_masked': True, 'controller_cwd': str(Path.cwd()),
            'mountinfo': Path('/proc/self/mountinfo').read_text()}))
        failed = False
        duplicate = False
        cases = [(owner, mode) for owner in ('fixed', 'prior') for mode in ('sync', 'async')] if (stage / 'owner-contrast').exists() else [(None, mode) for mode in ('sync', 'async')]
        if (stage / 'handoff-cases').exists():
            cases += [(None, mode) for mode in ('detach-before', 'detach-after', 'detach-race', 'detach-lost-reply', 'sync-independent', 'async-projection-fault')]
        for owner, mode in cases:
            try:
                duplicate = case(stage, mode, owner) or duplicate
            except Exception:
                failed = True
                traceback.print_exc()
                print(mode + ' FAIL', flush=True)
        sys.exit(1 if failed else 2 if duplicate else 0)
    sys.exit(main())
