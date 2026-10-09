"""Owned userns/PIDns: real fork/private proc/known fixture actor signals.
Synthetic entry and account reader; custody mocked at retention, not G2."""
import json
import os
from pathlib import Path
import select
import signal
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
import frontdoor as fd


def fixture():
    with tempfile.TemporaryDirectory(prefix="loss-namespace-") as tmp:
        base = Path(tmp)
        package = base / "package"
        (package / "bin").mkdir(parents=True)
        runner = package / fd.RUNNER
        runner.write_text('#!/usr/bin/python3 -I\n' + '''import json,os,sys,time
request=json.load(open(sys.argv[3]))
print(json.dumps({"entry_pid":os.getpid(),"proc_pid":os.readlink("/proc/self"),"parent":os.getppid()}),flush=True)
if request.get("block"): time.sleep(60)
''')
        runner.chmod(0o755)
        reader = package / fd.SUPERVISOR
        reader.write_text('#!/usr/bin/python3 -I\n' + '''import json,sys
assert sys.argv[1]=="--account"
print(json.dumps({"kind":"root_store_account","schema":"oulipoly.root_store_account/v1",
"requester":sys.argv[4],"root":"fixture-root","inputs_total":1,"inputs":[{"index":0,"state":"owed-insertion-unresolved"}],"complete":True}))
''')
        reader.chmod(0o755)
        request = base / "request.json"
        request.write_text('{}')
        original = os.readlink('/proc/self/ns/pid_for_children')
        entry, alive = fd.start_entry(str(package), str(request))
        output, _ = entry.communicate(timeout=5)
        os.close(alive)
        identity = json.loads(output)
        assert identity['entry_pid'] == 1 and identity['proc_pid'] == '1', identity
        assert entry.returncode == 0, entry.returncode
        assert os.readlink('/proc/self/ns/pid_for_children') == original
        assert subprocess.run(['/bin/true'], timeout=3).returncode == 0
        user = base / '4242'
        run = user / 'lost'
        (run / 'private').mkdir(parents=True)
        (run / 'store').mkdir()
        # Actual subprocess reader and retire AFTER init death, not a mock.
        with mock.patch.object(fd, 'check_owned'):
            result = fd.retire(str(run), 'discard', package=str(package),
                               capture={'by':'run-end','entry_status':0,'killed':False})
        assert result['run_removed'] is True, result
        record = fd.read_loss_account(str(user / fd.LOSS_ACCOUNTS), 'lost')
        assert record['store_account']['inputs'][0]['state'] == 'owed-insertion-unresolved'
        assert record['ending_capture']['entry_status'] == 0
        request.write_text('{"block":true}')
        read, write = os.pipe()
        parent = os.fork()
        if parent == 0:
            os.close(read)
            child, alive = fd.start_entry(str(package), str(request))
            assert select.select([child.stdout], [], [], 5)[0]
            assert json.loads(child.stdout.readline())['entry_pid'] == 1
            os.write(write, str(child.pid).encode() + b'\n')
            os.close(write)
            child.wait(timeout=15)
            os.close(alive)
            os._exit(0)
        os.close(write)
        watch = None
        try:
            assert select.select([read], [], [], 8)[0], 'fixture startup'
            pid = int(os.read(read, 128))
            watch = os.pidfd_open(pid)
            os.kill(parent, signal.SIGKILL)
            assert os.waitpid(parent, 0)[0] == parent
            assert select.select([watch], [], [], 5)[0], 'entry survived caller death'
        finally:
            os.close(read)
            if watch is not None: os.close(watch)
        print(json.dumps({'entry_pid':1,'private_proc':True,'restored_children_namespace':True,
                          'post_init_retire_captured_inputs':1,'store_removed_after_readable_capture':True,
                          'direct_entry_died_with_fixture_frontdoor':True}))


class NamespaceCorrection(unittest.TestCase):
    @unittest.skipUnless(sys.platform == 'linux' and hasattr(os, 'unshare') and hasattr(os, 'pidfd_open'),
                         'needs Linux Python namespace/pidfd support')
    def test_real_namespace_post_entry_capture_and_direct_parent_death(self):
        result = subprocess.run(['/usr/bin/unshare', '--user', '--map-root-user', '--pid', '--fork',
                                 '--mount', '--mount-proc', sys.executable, __file__, '--fixture'],
                                capture_output=True, timeout=25)
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        value = json.loads(result.stdout)
        self.assertTrue(value['restored_children_namespace'])
        self.assertTrue(value['direct_entry_died_with_fixture_frontdoor'])
        print(result.stdout.decode().strip())


if __name__ == '__main__':
    if sys.argv[1:] == ['--fixture']: fixture()
    else: unittest.main()
