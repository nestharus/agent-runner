"""U16 focused offline controls. Python peers are stand-ins, never models.
Scratch uses TMPDIR. CORRECTION_SOURCE selects preserved source for red
controls. Unprivileged tests do not establish host-root semantics."""
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

if os.environ.get('CORRECTION_SOURCE'):
    for name in ('frontdoor', 'native_call', 'install_package'):
        spec = importlib.util.spec_from_file_location(name, os.path.join(os.environ['CORRECTION_SOURCE'], name+'.py'))
        module = importlib.util.module_from_spec(spec)
        sys.modules[name] = module
        spec.loader.exec_module(module)
import frontdoor as fd
import native_call as caller
import install_package as installer
from test_frontdoor import SITE, NOW, request
from test_build_install import Scratch as InstallScratch, PACKAGE_ID

class Scratch(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix='correction-'))
        self.addCleanup(shutil.rmtree, self.root, True)

    def run_tree(self, name='run'):
        root = self.root/(name+'-'+str(time.monotonic_ns()))
        for sub in ('private', 'launch/provider', 'store'):
            (root/sub).mkdir(parents=True)
        for rel in ('launch/provider/adapter-state',):
            (root/rel).write_text('fake-secret')
        (root/'private/retention').write_text('discard')
        (root/'private/lock').touch()
        return root

    def actor(self, code):
        proc = subprocess.Popen([sys.executable, '-c', code], stdin=subprocess.PIPE, stdout=subprocess.PIPE, bufsize=0)
        def stop():
            if proc.poll() is None:
                proc.kill()
            proc.wait(timeout=2)
            proc.stdin.close()
            proc.stdout.close()
        self.addCleanup(stop)
        return proc

    def pipes(self):
        r,w = os.pipe()
        for n in (r,w):
            self.addCleanup(os.close,n)
        return r,w

class OwnedCleanup(Scratch):
    def test_replaced_ancestor_preserves_outside_for_keep_discard_sweep(self):
        # nes-owned surrogate for a root-private outside file. The traversal
        # fault reproduces without privileges; no real root test is claimed.
        outside = self.root/'outside'
        outside.mkdir()
        (outside/'auth.json').write_text('must survive')
        for mode in ('keep','discard','sweep'):
            run = self.run_tree(mode)
            ancestor = run/'launch/provider'
            shutil.rmtree(ancestor)
            ancestor.symlink_to(outside, target_is_directory=True)
            if mode == 'sweep':
                with mock.patch.object(fd,'check_owned'):
                    results = fd.sweep(str(self.root))
                result = next(r for r in results if r['run']==str(run))
            else:
                result = fd.retire(str(run),mode)
            self.assertTrue((outside/'auth.json').exists(),mode)
            self.assertTrue(result['ok'],mode)
            self.assertEqual(run.exists(),mode=='keep')

    def test_final_symlink_unlinked_without_target_effect(self):
        run = self.run_tree()
        outside = self.root/'outside'
        outside.write_text('keep')
        auth = run/'launch/provider/adapter-state'
        auth.unlink()
        auth.symlink_to(outside)
        self.assertTrue(fd.retire(str(run),'discard')['ok'])
        self.assertTrue(outside.exists())

    def test_discard_failure_changes_class(self):
        run=self.run_tree()
        with mock.patch.object(fd.shutil,'rmtree',side_effect=PermissionError):
            result=fd.retire(str(run),'discard')
        cls=caller.classify(87,{'stdout_eof':True,'errors':[]},[{'frontdoor':'terminal','retire':result}],{})[0]
        self.assertEqual(cls,'cleanup-failed')
        self.assertFalse(result['ok'])

class Bounds(Scratch):
    def drive(self, code, deadline=.1, control=b'', fill=False, full_control=False):
        proc=self.actor(code)
        ir,iw=self.pipes()
        outr,outw=self.pipes()
        if fill or full_control:
            target=outw if fill else proc.stdin.fileno()
            os.set_blocking(target,False)
            try:
                while True:
                    os.write(target,b'x'*4096)
            except BlockingIOError:
                pass
            os.set_blocking(target,True)
        if control:
            os.write(iw,control)
        start=time.monotonic()
        with mock.patch.object(fd,'OUT_FD',outw):
            relay=fd.Relay(proc,str(self.run_tree()),deadline,.1)
            status=relay.run_to_end(ir)
        self.assertLess(time.monotonic()-start,1.5)
        self.assertTrue(relay.killed)
        self.assertIsNotNone(status)

    def test_nondraining_output_deadline(self):
        self.drive('import time;print("x",flush=True);time.sleep(30)',fill=True)

    def test_full_native_control_pipe_deadline(self):
        self.drive('import time;time.sleep(30)',full_control=True)

    def test_early_cancel_close_ignoring_entry(self):
        for cmd in ('cancel','close'):
            with self.subTest(cmd=cmd):
                self.drive('import time;time.sleep(30)',deadline=3600,control=json.dumps({'cmd':cmd}).encode()+b'\n')

    def test_unterminated_admission_bound(self):
        r,w=self.pipes()
        os.write(w,b'{"v":1')
        with self.assertRaises(fd.Refused):
            import inspect
            if "timeout" in inspect.signature(fd.read_first_line).parameters:
                fd.read_first_line(r,4096,timeout=.1)
            else:
                fd.read_first_line(r,4096)

    def test_unterminated_output_bound(self):
        self.drive('import sys,time;sys.stdout.write("x"*100000);sys.stdout.flush();time.sleep(30)')

    def test_unknown_native_stop_does_not_wait_forever(self):
        proc=self.actor('import time;time.sleep(30)')
        ir,iw=self.pipes()
        outr,outw=self.pipes()
        with mock.patch.object(fd,'OUT_FD',outw),mock.patch.object(fd,'COLLECTION_S',.1,create=True):
            relay=fd.Relay(proc,str(self.run_tree()),.1,.1)
            with mock.patch.object(relay,'kill',side_effect=lambda:setattr(relay,'killed',True)):
                start=time.monotonic()
                status=relay.run_to_end(ir)
        self.assertIsNone(status)
        self.assertIn('stop-or-eof-not-observed',relay.collection_errors)
        self.assertLess(time.monotonic()-start,1)

class CallerBounds(Scratch):
    def call(self, code, prompt='look',deadline=1):
        actor=self.root/'actor.py';actor.write_text(code)
        path=self.root/'prompt';path.write_text(prompt)
        out=self.root/'out'
        args=['--route','fixture','--prompt-file',str(path),'--cwd',str(self.root),'--out',str(out),'--trusted-task','--deadline',str(deadline),'--frontdoor',str(actor),'--direct-requester-uid','1000']
        with mock.patch.object(caller,'STOP_GRACE_S',.2,create=True),mock.patch.object(caller,'EOF_GRACE_S',.2):
            start=time.monotonic();code=caller.main(args)
        self.assertLess(time.monotonic()-start,2)
        result=json.loads((out/'result.json').read_text())
        self.assertEqual((code,result['class']),(6,'incomplete'))
        self.assertIn('unknown',json.dumps(result['collection']))

    def test_peer_never_reads_request_no_terminal(self):
        self.call('import time;time.sleep(30)',prompt='x'*1000000)

    def test_early_close_collection_relative_to_close(self):
        self.call('import sys,json,time;sys.stdin.readline();print(json.dumps({"event":"turn-end","input":0}),flush=True);time.sleep(30)',deadline=3600)

    def test_terminal_without_eof_exit(self):
        self.call('import sys,json,time;sys.stdin.readline();print(json.dumps({"frontdoor":"terminal","exit":87}),flush=True);time.sleep(30)',deadline=3600)

    def test_write_failure_type_only_class(self):
        path=self.root/'prompt';path.write_text('look')
        args=['--route','x','--prompt-file',str(path),'--cwd',str(self.root),'--out',str(self.root/'out'),'--trusted-task']
        with mock.patch.object(caller,'write_json',side_effect=PermissionError('fake-secret')),mock.patch.object(caller.sys,'stderr',io.StringIO()) as stream:
            code=caller.main(args)
        result=json.loads(stream.getvalue())
        self.assertEqual((code,result['class'],result['reason']),(6,'incomplete','PermissionError'))
        self.assertNotIn('fake-secret',stream.getvalue())

class InputClasses(unittest.TestCase):
    def test_surrogate_refused(self):
        with self.assertRaises(fd.Refused):
            fd.check_request(request(message='\ud800'),SITE,NOW)



    def test_custody_resolved_ancestry(self):
        good=mock.Mock(st_uid=0,st_mode=0o100644)
        bad=mock.Mock(st_uid=1000,st_mode=0o40755)
        with mock.patch.object(fd.os.path,'realpath',return_value='/writable/owned/config'),mock.patch.object(fd.os,'lstat',side_effect=lambda p:bad if p=='/writable' else good):
            with self.assertRaises(fd.Refused):
                fd.check_owned('/root-alias/owned/config')

class InstallRecovery(InstallScratch):
    def record(self,dest):
        files=list(Path(dest).rglob('*.install.json'))
        self.assertEqual(len(files),1)
        return files[0]

    def recover(self,record,dest):
        return installer.main(['uninstall','--record','/'+str(record.relative_to(dest)),'--dest-root',dest,'--unprivileged-test','--purge-site'])

    def test_failure_every_created_effect_recoverable_record(self):
        archive,digest=self.archive(self.stage())
        done=installer.Record.done
        for nth in range(1,15):
            calls=0
            def fail(journal,effect,host):
                nonlocal calls
                done(journal,effect,host);calls+=1
                if calls==nth:
                    raise OSError('injected effect failure')
            with self.subTest(nth=nth),mock.patch.object(installer.Record,'done',fail):
                code,dest=self.install(archive,digest)
            record=self.record(dest)
            if os.environ.get('CORRECTION_EVIDENCE'):
                evidence=Path(os.environ['CORRECTION_EVIDENCE'])
                evidence.mkdir(parents=True,exist_ok=True)
                (evidence/('effect-'+str(nth)+'-'+str(time.monotonic_ns())+'.json')).write_text(record.read_text())
            self.assertIn(json.loads(record.read_text())['status'],('installed','failed-or-interrupted'))
            self.assertEqual(self.recover(record,dest),0)
            self.assertFalse(list(Path(dest).rglob('*.install.json')))
            self.assertFalse((Path(dest)/'etc/sudoers.d/oulipoly-native').exists())
            self.assertEqual(code,3 if calls>=nth else 0)

    def test_failure_after_config_has_cleanup_record(self):
        archive,digest=self.archive(self.stage())
        write=installer.write_new
        def fail(path,*args,**kwargs):
            if str(path).endswith('sudoers.d/oulipoly-native.partial'):
                raise OSError('failure after site creation')
            return write(path,*args,**kwargs)
        with mock.patch.object(installer,'write_new',side_effect=fail):
            try:
                code,dest=self.install(archive,digest)
            except OSError:
                dest=os.path.join(self.dir,'root')
        record=self.record(dest)
        self.assertEqual(self.recover(record,dest),0)

    def test_rule_enable_durably_recorded_before_interruption(self):
        archive,digest=self.archive(self.stage())
        rename=installer.os.rename
        def fail(src,dst):
            if str(dst).endswith('sudoers.d/oulipoly-native'):
                data=json.loads(self.record(str(Path(dst).parents[2])).read_text())
                self.assertEqual(data['status'],'ready-to-enable')
                self.assertTrue(any(e.get('alternate')=='/etc/sudoers.d/oulipoly-native' for e in data['effects']))
                rename(src,dst);raise KeyboardInterrupt
            return rename(src,dst)
        with mock.patch.object(installer.os,'rename',side_effect=fail):
            code,dest=self.install(archive,digest)
        self.assertEqual(code,3)
        self.assertEqual(self.recover(self.record(dest),dest),0)

    def test_changed_package_preserved_with_record(self):
        archive,digest=self.archive(self.stage());code,dest=self.install(archive,digest)
        package=Path(dest)/'opt/oulipoly-native'/PACKAGE_ID
        (package/'bin/oulipoly-agent-runner').write_text('changed by administrator')
        record=self.record(dest)
        self.assertEqual(self.recover(record,dest),3)
        self.assertTrue(record.exists());self.assertTrue(package.exists())

    def test_changed_site_rule_preserved(self):
        archive,digest=self.archive(self.stage());code,dest=self.install(archive,digest)
        record=self.record(dest)
        for p in ('etc/sudoers.d/oulipoly-native','etc/oulipoly-native/frontdoor.json'):
            os.chmod(Path(dest)/p,0o600)
            (Path(dest)/p).write_text('changed')
        self.assertEqual(self.recover(record,dest),3)
        self.assertTrue(record.exists())
        self.assertEqual((Path(dest)/'etc/sudoers.d/oulipoly-native').read_text(),'changed')
        self.assertTrue((Path(dest)/'opt/oulipoly-native'/PACKAGE_ID).exists())

    def test_package_admission_lock_stops_uninstall(self):
        archive,digest=self.archive(self.stage());code,dest=self.install(archive,digest)
        package=Path(dest)/'opt/oulipoly-native'/PACKAGE_ID
        lock=os.open(package,os.O_RDONLY|os.O_DIRECTORY);self.addCleanup(os.close,lock)
        import fcntl
        fcntl.flock(lock,fcntl.LOCK_SH|fcntl.LOCK_NB)
        record=self.record(dest)
        self.assertEqual(self.recover(record,dest),3)
        self.assertTrue(record.exists());self.assertTrue((Path(dest)/'etc/sudoers.d/oulipoly-native').exists())

    def test_live_runs_stop_uninstall(self):
        archive,digest=self.archive(self.stage());code,dest=self.install(archive,digest)
        private=Path(dest)/'var/lib/oulipoly-native/runs/1000/run/private';private.mkdir(parents=True)
        lock=os.open(private/'lock',os.O_CREAT|os.O_RDWR,0o600);self.addCleanup(os.close,lock)
        import fcntl
        fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
        record=self.record(dest)
        self.assertEqual(self.recover(record,dest),3)
        self.assertTrue(record.exists());self.assertTrue((Path(dest)/'etc/sudoers.d/oulipoly-native').exists())

    def test_failed_uninstall_retains_record_then_retry_finishes(self):
        archive,digest=self.archive(self.stage());code,dest=self.install(archive,digest)
        record=self.record(dest);unlink=installer.os.unlink
        def fail(path,*args,**kwargs):
            if str(path).endswith('frontdoor.json'):
                raise PermissionError
            return unlink(path,*args,**kwargs)
        with mock.patch.object(installer.os,'unlink',side_effect=fail):
            self.assertEqual(self.recover(record,dest),3)
        self.assertTrue(record.exists())
        self.assertEqual(self.recover(record,dest),0)

if __name__=='__main__':
    unittest.main()
