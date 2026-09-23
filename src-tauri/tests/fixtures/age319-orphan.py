import ctypes
import json
import os
import pathlib
import subprocess
import time
import uuid

root = pathlib.Path(os.environ['AGE360_ROOT'])
libc = ctypes.CDLL(None, use_errno=True)
def keyctl(command, a, b=0, c=0, d=0):
    result = libc.syscall(ctypes.c_long(250), *(ctypes.c_long(x) for x in (command, a, b, c, d)))
    if result < 0:
        raise OSError(ctypes.get_errno(), 'keyctl')
    return result

def cmdline(pid):
    return pathlib.Path(f'/proc/{pid}/cmdline').read_bytes().replace(b'\0', b' ').decode(errors='replace')

def ppid(pid):
    return int(pathlib.Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()[1])

worker = None
ancestors = []
ancestor = os.getppid()
while ancestor > 1:
    command = cmdline(ancestor)
    ancestors.append({'pid': ancestor, 'cmdline': command[:160]})
    if worker is None and ('__completion-root-worker-v1' in command or '__root-original-work-v1' in command):
        worker = ancestor
    ancestor = ppid(ancestor)
ring = keyctl(0, -3, 0)
buffer = ctypes.create_string_buffer(256)
length = keyctl(6, ring, ctypes.addressof(buffer), 256)
assert 0 < length <= 256 and buffer.raw[length - 1] == 0
name = buffer.raw[:length - 1].decode().split(';')[-1]
prefix = 'oulipoly-paired-original-work-v1:'
paired = name.startswith(prefix)
if paired:
    assert str(uuid.UUID(name[len(prefix):])) == name[len(prefix):]
    assert uuid.UUID(name[len(prefix):]).version == 4
old = int(os.environ['AGE319_OLD_RING'])
key_type = ctypes.create_string_buffer(b'user')
key_name = ctypes.create_string_buffer(os.environ['AGE319_KEY_NAME'].encode())
found = keyctl(10, ring, ctypes.addressof(key_type), ctypes.addressof(key_name), 0)
assert found == int(os.environ['AGE319_KEY_SERIAL']), (found, os.environ['AGE319_KEY_SERIAL'])
guardian = ppid(worker) if worker is not None else None
root.joinpath('ring-report.json').write_text(json.dumps({'guardian': guardian, 'worker': worker, 'observer': os.getpid(), 'old_ring': old, 'new_ring': ring, 'switched': ring != old, 'name': name, 'old_key_found': found, 'paired': paired, 'ancestors': ancestors, 'had_required': 'OULIPOLY_ORIGINAL_WORK_REQUIRED_V1' in os.environ, 'had_authority': 'OULIPOLY_ROOT_AUTHORITY_V1' in os.environ, 'had_endpoint': 'OULIPOLY_COMPLETION_ENDPOINT' in os.environ}))

# A Bash workload has no provider-session attestation of its own. Start the
# ordinary descendant Runner/provider path; that provider submits nested Bash
# work with an exact live owner binding and records nested-report.json.
nested = subprocess.run([os.environ['AGE360_RUNNER_BIN'], '-m', 'age360-native-model', '--models-dir', str(root / 'config/oulipoly-agent-runner/models'), 'age319 authorized nested'], env=os.environ, capture_output=True, text=True, timeout=40)
root.joinpath('nested-runner-report.json').write_text(json.dumps({'rc': nested.returncode, 'stderr': nested.stderr, 'stdout': nested.stdout}))

ready_read, ready_write = os.pipe()
nearer = os.fork()
if nearer == 0:
    os.close(ready_write)
    nearer_pid = os.getpid()
    orphan = os.fork()
    if orphan == 0:
        os.setsid()
        os.read(ready_read, 1)
        os.close(ready_read)
        start = time.monotonic()
        while os.getppid() == nearer_pid and time.monotonic() - start < 5:
            time.sleep(.01)
        first_adopter = os.getppid()
        if first_adopter == worker:
            os.kill(worker, 9)
        start = time.monotonic()
        while os.getppid() == worker and time.monotonic() - start < 5:
            time.sleep(.01)
        adopted = os.getppid()
        env = dict(os.environ, AGENT_BASH_AGENT_RUNNER_BIN=os.environ['AGE360_RUNNER_BIN'])
        for key in ('AGENT_BASH_OWNER_SESSION_ID', 'AGENT_BASH_OWNER_INVOCATION_UUID'):
            env.pop(key, None)
        for key in ('OULIPOLY_ORIGINAL_WORK_REQUIRED_V1', 'OULIPOLY_ROOT_AUTHORITY_V1', 'OULIPOLY_COMPLETION_ENDPOINT'):
            env.pop(key, None)
        result = subprocess.run([os.environ['AGE360_AGENT_BASH_BIN'], 'run', '--delivery', 'async', '--', '/bin/sh', '-c', 'printf effect > "$AGE319_EFFECT"'], env=env, capture_output=True, text=True, timeout=15)
        if result.returncode == 0:
            end = time.monotonic() + 10
            while not root.joinpath('unauthorized-effect').exists() and time.monotonic() < end:
                time.sleep(.02)
        root.joinpath('orphan-report.json').write_text(json.dumps({'effect_exists': root.joinpath('unauthorized-effect').exists(), 'first_adopter': first_adopter, 'rc': result.returncode, 'stderr': result.stderr, 'stdout': result.stdout, 'adopted_by': adopted, 'adopted_cmdline': cmdline(adopted), 'nearer_pid': nearer_pid, 'expected_worker': worker, 'ring': keyctl(0, -3, 0), 'sid': os.getsid(0), 'pid': os.getpid()}))
        os._exit(0)
    os._exit(0)
os.close(ready_read)
os.waitpid(nearer, 0)
os.write(ready_write, b'1')
os.close(ready_write)
end = time.monotonic() + 20
while not root.joinpath('orphan-report.json').exists():
    assert time.monotonic() < end, 'orphan report timeout'
    time.sleep(.02)
