"""Read only this synthetic fixture; preserve errors, complete rows and file bytes.

Each database is a separate read transaction, not a cross-database atomic claim.
No installed runner or host state is consulted.
"""
import hashlib
import json
import pathlib
import sqlite3
import sys


def capture(root):
    root = pathlib.Path(root)
    directories = [root / 'xdg-data', root / 'work']
    sidecar = root / 'xdg-data' / 'oulipoly-agent-runner' / 'pid-identity.db'
    if sidecar.exists():
        try:
            with sqlite3.connect('file:' + str(sidecar) + '?mode=ro', uri=True) as db:
                for domain, endpoint in db.execute(
                        'SELECT domain_id, endpoint FROM completion_continuation_owner'):
                    directory = pathlib.Path(endpoint).parent
                    # Only this fixture's recorded private service directory.
                    if directory.parent == pathlib.Path('/tmp') and directory.name.endswith(domain):
                        directories.append(directory)
        except Exception as error:
            print(json.dumps(dict(service_capture_error=str(error))))
    for directory in directories:
        for path in sorted(directory.rglob('*')):
            if not path.is_file() or path.is_symlink():
                continue
            try:
                if path.suffix == '.db':
                    with sqlite3.connect('file:' + str(path) + '?mode=ro', uri=True) as db:
                        db.execute('BEGIN')
                        # Complete schema/rows includes activation, queue, runtime,
                        # submission, receipt, claim and invocation identities.
                        print(json.dumps(dict(path=str(path), sql=list(db.iterdump()))))
                elif not str(path).endswith(('-wal', '-shm')):
                    with path.open("rb") as stream:
                        executable = stream.read(4) == b"\x7fELF"
                    if executable:
                        # Real Bash ingress retains copied source executables in
                        # its spool. Identify those bytes without hex-expanding
                        # every binary repeatedly in assertion diagnostics.
                        with path.open("rb") as stream:
                            digest = hashlib.file_digest(stream, "sha256").hexdigest()
                        print(json.dumps(dict(path=str(path), executable_sha256=digest,
                                              byte_len=path.stat().st_size)))
                    else:
                        print(json.dumps(dict(path=str(path), hex=path.read_bytes().hex())))
            except Exception as error:
                print(json.dumps(dict(path=str(path), capture_error=str(error))))


if __name__ == '__main__':
    capture(sys.argv[1])
