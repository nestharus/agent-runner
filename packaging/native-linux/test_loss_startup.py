"""K1 finite startup-error controls: owned trees/pipes, mocked actor and
namespace transitions (no host proc/actor/knob effects). Successful physical
containment remains a separate owned-userns control, not a global guarantee.
K2 producer dependence is exercised by retained U316 evidence, not a validator.
"""
import contextlib
import ctypes
import json
import os
import shutil
from pathlib import Path
import subprocess
import types
import unittest
from unittest import mock

import frontdoor as fd
from test_frontdoor import Scratch, SITE, NOW, request, OWED


class StartupCorrection(Scratch):
    def exercise(self, live=False, wait=-9, kill_error=None, popen_error=None,
                 reader_error=None, emit_ok=True):
        run_id = 'started'
        run = Path(self.dir) / '4242' / run_id
        (run / 'private').mkdir(parents=True)
        (run / 'store').mkdir()
        (run / 'store' / 'source').write_text('owned synthetic source')
        (run / 'private' / fd.ROOT_TERMINAL).write_text(json.dumps(OWED))
        lock = os.open(run / 'private/lock', os.O_CREAT | os.O_RDWR, 0o600)
        self.addCleanup(lambda: os.close(lock) if live else None)
        checked = fd.check_request(request(), SITE, NOW)
        user = types.SimpleNamespace(pw_uid=4242, pw_name='fixture')
        entry = mock.Mock()
        entry.kill.side_effect = kill_error
        if isinstance(wait, Exception):
            entry.wait.side_effect = wait
        else:
            entry.wait.return_value = wait
        lines, captures = [], []
        account = {'schema': fd.STORE_ACCOUNT_SCHEMA, 'requester': 'uid:4242',
                   'inputs': [{'index': 0, 'state': 'owed-insertion-unresolved'}],
                   'inputs_total': 1, 'inputs_omitted': 0, 'complete': True}
        real_retire, real_open, real_pipe = fd.retire, os.open, os.pipe
        def retire(*args, **kwargs):
            captures.append(kwargs.get('capture'))
            return real_retire(*args, **kwargs)
        with contextlib.ExitStack() as stack:
            patches = [
                mock.patch.object(fd, 'admit', return_value=('/synthetic', SITE, user, checked, {}, b'')),
                mock.patch.object(fd, 'make_run', return_value=(run_id, str(run), lock, [])),
                mock.patch.object(fd, 'entry_request', return_value={'synthetic': True}),
                mock.patch.object(fd, 'emit', side_effect=lambda value: lines.append(value) or emit_ok),
                mock.patch.object(fd, 'retire', side_effect=retire),
                mock.patch.object(fd, 'check_owned'),
                mock.patch.object(fd, 'store_account', return_value=(None, 'fixture-unavailable') if reader_error else (account, None)),
                # Never read host proc: own /dev/null replaces namespace fd.
                mock.patch.object(fd.os, 'open', side_effect=lambda path, *a, **kw:
                    real_open('/dev/null' if path == '/proc/self/ns/pid' else path, *a, **kw)),
                mock.patch.object(fd.os, 'unshare', return_value=None),
                mock.patch.object(fd.os, 'CLONE_NEWPID', 0x20000000, create=True),
                mock.patch.object(fd, 'contained', return_value=lambda: None),
                mock.patch.object(fd.subprocess, 'Popen', side_effect=popen_error, return_value=entry),
                mock.patch.object(fd._libc, 'setns', return_value=-1),
                mock.patch.object(fd.signal, 'signal'),
            ]
            for patch in patches:
                stack.enter_context(patch)
            ctypes.set_errno(1)
            if live:
                read, write = real_pipe()
                listener = mock.Mock()
                daemon_code = fd.live_daemon('/synthetic', SITE, checked, run_id, str(run),
                                             '/request', user, listener, '/socket', 'fixture', os.dup(write))
                # Real failure bytes through an owned pipe, actual opening-
                # caller parser; fake fork is parent-only, no detached actor.
                stack.enter_context(mock.patch.object(fd, 'listen_live', return_value=(listener, '/socket')))
                stack.enter_context(mock.patch.object(fd.os, 'pipe', return_value=(read, write)))
                stack.enter_context(mock.patch.object(fd.os, 'fork', return_value=123))
                code = fd.open_live('/synthetic', SITE, checked, run_id, str(run), '/request', user)
                self.assertEqual(daemon_code, lines[-1]['exit'])
            else:
                code = fd.run_locked(['frontdoor', 'run'], {})
        return code, lines[-1], captures, entry, run

    def test_started_handle_and_closed_life_pipe_survive_failed_restore(self):
        entry = mock.Mock()
        entry.wait.return_value = -9
        real_open, real_pipe = os.open, os.pipe
        pipes = []
        def pipe():
            pair = real_pipe()
            pipes.extend(pair)
            return pair
        with mock.patch.object(fd.os, 'open', side_effect=lambda path, *a, **kw: real_open('/dev/null', *a, **kw)), \
                mock.patch.object(fd.os, 'pipe', side_effect=pipe), \
                mock.patch.object(fd.os, 'unshare'), \
                mock.patch.object(fd.os, 'CLONE_NEWPID', 0x20000000, create=True), \
                mock.patch.object(fd, 'contained', return_value=lambda: None), \
                mock.patch.object(fd.subprocess, 'Popen', return_value=entry), \
                mock.patch.object(fd._libc, 'setns', return_value=-1):
            ctypes.set_errno(1)
            with self.assertRaises(fd.StartedEntryFailed) as caught:
                fd.start_entry('/synthetic', '/request')
            self.assertIs(caught.exception.entry, entry)
            self.assertEqual(caught.exception.status, -9)
            self.assertTrue(caught.exception.killed)
            for descriptor in pipes:
                with self.assertRaises(OSError):
                    os.fstat(descriptor)

    def test_restore_failure_known_end_preserves_terminal_and_saved_capture_ordinary_live(self):
        for live in (False, True):
            with self.subTest(live=live):
                code, terminal, captures, entry, run = self.exercise(live=live)
                self.assertEqual(code, fd.EXIT_KILLED)
                self.assertTrue(terminal['entry_started'])
                self.assertEqual(terminal['entry_status'], -9)
                self.assertTrue(terminal['killed'])
                self.assertEqual(terminal['reason'], 'entry started; startup failed: PermissionError')
                self.assertTrue(terminal['retire']['run_removed'])
                self.assertFalse(run.exists())
                self.assertEqual(captures, [{'by': 'front-door-failed', 'entry_status': -9, 'killed': True}])
                record = fd.read_loss_account(str(run.parent / fd.LOSS_ACCOUNTS), run.name)
                self.assertEqual(record['ending_capture']['entry_status'], -9)
                self.assertTrue(record['ending_capture']['killed'])
                entry.kill.assert_called_once_with()
                entry.wait.assert_called_once_with(timeout=fd.COLLECTION_S)

    def test_restore_failure_unknown_end_keeps_source_without_any_disposition_ordinary_live(self):
        for live in (False, True):
            with self.subTest(live=live):
                code, terminal, captures, entry, run = self.exercise(live=live,
                    wait=subprocess.TimeoutExpired('synthetic-entry', fd.COLLECTION_S))
                self.assertEqual(code, fd.EXIT_UNKNOWN)
                self.assertTrue(terminal['entry_started'])
                self.assertIsNone(terminal['entry_status'])
                self.assertTrue(terminal['killed'])
                self.assertEqual(terminal['collection_errors'], ['entry-wait-failed:TimeoutExpired'])
                self.assertEqual(terminal['retire'], {'ok': False, 'stop': 'unknown', 'run_removed': False})
                self.assertEqual(captures, [])
                self.assertTrue((run / 'store/source').exists())
                self.assertTrue((run / 'private' / fd.ROOT_TERMINAL).exists())
                self.assertFalse((run.parent / fd.LOSS_ACCOUNTS).exists())
                entry.kill.assert_called_once_with()
                entry.wait.assert_called_once_with(timeout=fd.COLLECTION_S)
                # Remove only own fixture before exercising other mode.
                shutil.rmtree(run)

    def test_failed_kill_is_not_reported_as_success_and_cause_survives_wait_failure(self):
        code, terminal, captures, entry, run = self.exercise(
            kill_error=OSError('synthetic kill failure'),
            wait=subprocess.TimeoutExpired('synthetic-entry', fd.COLLECTION_S))
        self.assertEqual(code, fd.EXIT_UNKNOWN)
        self.assertFalse(terminal['killed'])
        self.assertEqual(terminal['reason'], 'entry started; startup failed: PermissionError')
        self.assertEqual(terminal['collection_errors'], ['entry-kill-failed:OSError', 'entry-wait-failed:TimeoutExpired'])
        self.assertEqual(captures, [])
        self.assertTrue((run / 'store/source').exists())

    def test_known_end_capture_unavailable_keeps_source_and_actual_ending(self):
        code, terminal, captures, entry, run = self.exercise(reader_error=True)
        self.assertEqual(code, fd.EXIT_KILLED)
        self.assertFalse(terminal['retire']['run_removed'])
        self.assertTrue(terminal['retire']['loss_account']['store_kept'])
        record = fd.read_loss_account(str(run.parent / fd.LOSS_ACCOUNTS), run.name)
        self.assertEqual(record['ending_capture']['entry_status'], -9)
        self.assertTrue(record['ending_capture']['killed'])
        self.assertTrue((run / 'store/source').exists())

    def test_not_started_still_reports_setup_failure_without_kill_ordinary_live(self):
        for live in (False, True):
            with self.subTest(live=live):
                code, terminal, captures, entry, run = self.exercise(live=live, popen_error=OSError('synthetic exec failure'))
                self.assertEqual(code, fd.EXIT_RUN_FAILED)
                self.assertIn('entry not started', terminal['reason'])
                self.assertIsNone(terminal['entry_status'])
                self.assertFalse(terminal['killed'])
                entry.kill.assert_not_called()
                entry.wait.assert_not_called()

    def test_unusable_live_readiness_is_unknown_not_evidence_entry_never_started(self):
        read, write = os.pipe()
        os.write(write, b'{invalid-readiness}\n')
        listener, lines = mock.Mock(), []
        with mock.patch.object(fd, 'listen_live', return_value=(listener, '/socket')), \
                mock.patch.object(fd.os, 'pipe', return_value=(read, write)), \
                mock.patch.object(fd.os, 'fork', return_value=123), \
                mock.patch.object(fd, 'emit', side_effect=lambda value: lines.append(value) or True):
            code = fd.open_live('/synthetic', SITE, {}, 'fixture', self.dir, '/request',
                                types.SimpleNamespace(pw_uid=4242))
        self.assertEqual(code, fd.EXIT_UNKNOWN)
        self.assertIn('readiness unavailable', lines[-1]['reason'])
        self.assertNotIn('not started', lines[-1]['reason'])

    def test_failed_terminal_delivery_stays_unknown(self):
        code, terminal, captures, entry, run = self.exercise(emit_ok=False)
        self.assertEqual(code, fd.EXIT_UNKNOWN)
        self.assertEqual(terminal['entry_status'], -9)
        self.assertTrue(terminal['killed'])


if __name__ == '__main__':
    unittest.main()
