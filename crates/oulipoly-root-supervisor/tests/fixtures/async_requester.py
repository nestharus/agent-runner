#!/usr/bin/python3
"""Test-owned requester: asks this root's Bash ingress for one background
(`delivery` `async`) run that waits for GATE, then writes MARK and exits 0.
Saves the ingress replies (through `detached` or a refusal) to OUT."""
import json, os, socket, sys

out, gate, mark = sys.argv[1:4]
script = 'while [ ! -e "$1" ]; do sleep 0.02; done; echo ran; echo ended > "$2"'
request = {"v": 1, "op": "run", "argv": ["/bin/sh", "-c", script, "run", gate, mark],
           "cwd": "/", "delivery": "async"}
s = socket.socket(socket.AF_UNIX)
s.connect(os.environ["OULIPOLY_ROOT_BASH_V1"])
s.sendall((json.dumps(request) + "\n").encode())
replies = []
for line in s.makefile():
    replies.append(json.loads(line))
    if replies[-1].get("event") in ("detached", "refused", "end", "end-unknown"):
        break
s.close()
with open(out + ".tmp", "w") as f:
    json.dump(replies, f)
os.rename(out + ".tmp", out)
