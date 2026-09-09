#!/usr/bin/python3
"""Bounded stdin observer; FIFO controls lifetime, never replaces input reads."""
import os
import select
import sys
import time

received, authority, control_path = sys.argv[1:]
assert all(os.isatty(fd) for fd in (0, 1, 2))
with open(authority, "w", encoding="utf-8") as output:
    output.write(os.environ.get("OULIPOLY_COMPLETION_REGISTRATION_AUTHORITY", ""))
control = os.open(control_path, os.O_RDWR | os.O_NONBLOCK)
os.set_blocking(0, False)
deadline = time.monotonic() + 30
seen = bytearray()
commands = bytearray()
notified = False
# Optional fixture-owned sink. Four numeric records maximum, no input payload.
try:
    stages = os.open(os.path.join(os.path.dirname(control_path), "stream-stages"),
                     os.O_WRONLY | os.O_APPEND | os.O_NONBLOCK)
except OSError:
    stages = None
started = time.monotonic()


def stage(code):
    if stages is not None:
        try:
            record = f"{code} {min(65536, int((time.monotonic() - started) * 1000))} {len(seen)}\n"
            os.write(stages, record.encode("ascii"))
        except OSError:
            pass

print("\033[?2004hREADY_FOR_NOTIFY", flush=True)


def observe(output):
    global notified
    data = os.read(0, 4096)
    if not data:
        raise RuntimeError("provider stdin closed before owned release")
    if len(seen) + len(data) > 65536:
        raise RuntimeError("provider observation byte limit exceeded")
    output.write(data)
    seen.extend(data)
    if not notified and b"[END OULIPOLY NOTIFICATIONS]" in seen:
        notified = True
        print("GOT_NOTIFY", flush=True)


with open(received, "wb", buffering=0) as output:
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise RuntimeError("provider lifetime deadline")
        readable, _, _ = select.select([0, control], [], [], remaining)
        if 0 in readable:
            observe(output)
        if control not in readable:
            continue
        commands.extend(os.read(control, 4096))
        if len(commands) > 4096:
            raise RuntimeError("provider control byte limit exceeded")
        while b"\n" in commands:
            command, _, rest = commands.partition(b"\n")
            commands[:] = rest
            if command == b"probe":
                print("PROVIDER_LIFETIME_HELD", flush=True)
            elif command == b"release":
                stage(1)
                # Caller sends a broker input fence after retry and waits until
                # it is observed. Also retain any queued input before exiting.
                while select.select([0], [], [], 0)[0]:
                    observe(output)
                stage(2)
                print("PROVIDER_INPUT_RELEASED", flush=True)
                stage(3)
                os.close(control)
                stage(4)
                sys.exit(0)
            else:
                raise RuntimeError("unknown provider lifetime command")
