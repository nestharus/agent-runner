#!/usr/bin/python3
"""Executable provider used only by the feature-gated private native fixture."""
import ctypes
import json
import os
import signal
import sys
import time

libc = ctypes.CDLL(None, use_errno=True)
assert libc.prctl(39, 0, 0, 0, 0) == 0, "provider inherited NNP"
assert libc.prctl(21, 0, 0, 0, 0) == 0, "provider inherited seccomp"
request = json.load(sys.stdin)
marker = os.environ["PRIVATE_PROVIDER_MARKER"]


def status_field(name):
    with open("/proc/self/status") as status:
        return next(line.split()[1] for line in status if line.startswith(name + ":"))


provider_host_pid = status_field("Pid")
os.setsid()
assert libc.unshare(0x20000000) == 0, "provider could not unshare PID namespace"
child = os.fork()
if child == 0:
    # This nested PID1 is still inside the broker work. Its parent exits below;
    # the original provider custodian adopts it before the observation is sent.
    while status_field("PPid") == provider_host_pid:
        time.sleep(0.005)
    assert os.getpid() == 1
    host_pid = status_field("Pid")
    with open(marker, "a") as output:
        output.write("nnp=0 seccomp=0 setsid=ok unshare=ok adopted=1 host_pid=" + host_pid + "\n")
    os.environ.clear()
    for fd in os.listdir("/proc/self/fd"):
        try:
            os.close(int(fd))
        except OSError:
            pass
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    while not os.path.exists(marker + ".release"):
        time.sleep(0.02)
    os._exit(0)

print(json.dumps({
    "contract": request["contract"], "request_id": request["request_id"], "ok": True,
    "result": {"provider_id": "private", "display_name": "Private",
               "contract_versions": [request["contract"]],
               "preferred_contract": request["contract"],
               "capabilities": {"launch": True, "policy": False, "quota": False,
                                "session": False, "terminal": False, "rotation": False,
                                "discovery": False, "settings": False, "setup_brain": False,
                                "setup": False, "migration": False},
               "concurrency": {"safe_for_parallel_invocation": True, "state_locking": "none"}}
}))
