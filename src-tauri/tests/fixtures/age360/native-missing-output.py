"""Runner-only public-source fixture, NOT a Bash producer or private staging adapter.
Owns one real child/wait, deliberately shortens its selected inode, then submits
public revision5 through native State admission and CLI validation.
"""
import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import time
import uuid


def encode(value):
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def identity(pid):
    fields = pathlib.Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    return {"pid": pid, "boot_id": pathlib.Path("/proc/sys/kernel/random/boot_id").read_text().strip(), "starttime_ticks": int(fields[19])}


def publish():
    root = pathlib.Path(os.environ["AGE360_ROOT"])
    runner = os.environ["AGENT_BASH_AGENT_RUNNER_BIN"]
    parent = json.loads(os.environ["OULIPOLY_PARENT_INVOCATION"])["id"]
    session = "ses_age360_native_wake"
    canonical = json.loads(root.joinpath("missing-output-wire.json").read_text())
    source = json.loads(canonical["registration_bytes_utf8"])
    handle = "ab_native_missing_fixture"
    directory = root / "source-spool" / handle
    directory.mkdir(parents=True)
    recovery = directory / "recovery"
    shutil.copyfile("/bin/false", recovery)
    recovery.chmod(0o700)
    helper = directory / "runner"
    # Source-owned private copy; /tmp can be a different filesystem.
    shutil.copyfile(runner, helper)
    helper.chmod(0o700)
    environment = encode(dict(os.environ))
    directory.joinpath("delivery-helper-environment.json").write_bytes(environment)
    domain = json.loads(subprocess.check_output([runner,"notify","agent-bash-capability","--json"]))["domain_id"]
    observer = identity(os.getpid())
    source.update(domain_id=domain, source_id=str(uuid.uuid4()), registration_id=str(uuid.uuid4()), handle=handle,
        spool_root=str(directory.parent), handle_dir=str(directory), owner_session_id=session, owner_invocation_uuid=parent,
        registering_caller=observer, delivery_mode="async", completion_kind="exit", completion_scope="root",
        listeners=[{"listener_id":parent,"session_id":session,"owner_invocation_uuid":parent}])
    for key, path in [("helper",helper),("recovery",recovery)]:
        source[key] = {"path":str(path),"sha256":hashlib.sha256(path.read_bytes()).hexdigest(),"environment_sha256":hashlib.sha256(environment).hexdigest()}
    registration = encode(source)
    directory.joinpath(source["registration_relative"]).write_bytes(registration)
    callers = []
    pid = os.getpid()
    while pid > 0:
        callers.append(identity(pid))
        pid = int(pathlib.Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[1])
    directory.joinpath("meta.json").write_bytes(encode({"owner_session_id":session,"owner_invocation_uuid":parent,"caller_chain":callers}))
    common_args = ["--handle",handle,"--state-dir",str(directory),"--meta",str(directory/"meta.json"),"--log",str(directory/"log"),"--rc",str(directory/"rc"),"--completion-protocol",source["protocol"],"--registration-file",str(directory/source["registration_relative"]),"--json"]
    registered = subprocess.run([runner,"notify","agent-bash-register","--delivery-mode","async",*common_args],capture_output=True)
    root.joinpath("native-source-registration.json").write_bytes(registered.stdout)
    assert registered.returncode == 0, registered.stderr
    assert json.loads(registered.stdout)["status"] == "registered", registered.stdout
    # Genuine original process outcome and selected storage; no synthetic wait.
    selected = directory / "selected-original.bin"
    fd = os.open(selected, os.O_WRONLY|os.O_CREAT|os.O_EXCL, 0o600)
    child = os.fork()
    if child == 0:
        os.dup2(fd, 1)
        os.close(fd)
        os.execl("/bin/sh", "sh", "-c", "printf original-private-output; exit 37")
    os.close(fd)
    waited, raw_wait = os.waitpid(child, 0)
    assert waited == child and os.waitstatus_to_exitcode(raw_wait) == 37
    original_stat = selected.stat()
    assert original_stat.st_size > 0
    # Fault injection into this fixture's OWN sole original store, not a live handle.
    with selected.open("wb") as body:
        body.flush()
        os.fsync(body.fileno())
    shortened_stat = selected.stat()
    assert (original_stat.st_dev, original_stat.st_ino) == (shortened_stat.st_dev, shortened_stat.st_ino)
    common = {k:source[k] for k in ["protocol","domain_id","source_id","handle","registration_id"]}
    common["registration_digest"] = hashlib.sha256(registration).hexdigest()
    outcome = dict(common, completion_revision=1, kind="exit_root", observer=observer, root_wait_status=raw_wait,
        output_closed=True, original_tree_drained=False, cancellation_id=None, launch_fence_revision=1, ready_sentinel=None)
    outcome_bytes = encode(outcome)
    digest = hashlib.sha256(outcome_bytes).hexdigest()
    output = dict(representation="missing-original-output-v1", capture_state="irrecoverable", reason="selected_storage_short",
        producer=observer, original_observer=observer, completion_revision=1, outcome_sha256=digest,
        selection={"device":original_stat.st_dev,"inode":original_stat.st_ino,"byte_len":original_stat.st_size}, observed_byte_len=shortened_stat.st_size,
        detail="Runner-only fixture observed actual sole selected inode shortened after original wait; no surviving copy or producer buffer.")
    snapshot = dict(common, completion_revision=1,outcome_sha256=digest,outcome_byte_len=len(outcome_bytes),rc=37,status="original_output_unavailable",output=output)
    directory.joinpath(source["outcome_relative"]).write_bytes(outcome_bytes)
    path = directory / source["snapshot_relative"]
    # A transient observation is deliberately invalid public wire. It must not
    # trigger even with a valid original wait/outcome and real admission.
    output["capture_state"] = "pending"
    path.write_bytes(encode(snapshot))
    rejected = subprocess.run([runner,"notify","agent-bash-complete","--caller-ppid",str(os.getpid()),*common_args,"--snapshot",str(path)],capture_output=True)
    root.joinpath("native-source-transient-rejection.json").write_bytes(rejected.stdout)
    assert rejected.returncode != 0, rejected.stdout
    root.joinpath("native-source-invalid-ready").touch()
    deadline = time.monotonic()+30
    while not root.joinpath("release-source-proof").exists():
        if time.monotonic()>deadline: raise RuntimeError("native fixture proof gate expired")
        time.sleep(.02)
    output["capture_state"] = "irrecoverable"
    temp = path.with_suffix(".tmp")
    temp.write_bytes(encode(snapshot))
    with temp.open("rb") as stream: os.fsync(stream.fileno())
    temp.replace(path)
    # No direct complete call: independent native owner must consume validated
    # public files and materialize ordinary delivery from the existing admission.
    root.joinpath("native-source-original-observation.json").write_bytes(encode({"waited_pid":waited,"root_wait_status":raw_wait,"output":output}))
