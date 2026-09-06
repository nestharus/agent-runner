"""Independent authorization, atomic replacement, and bounded rollback."""

from contextlib import contextmanager
from datetime import datetime, timezone
import fcntl
import os
import json
from pathlib import Path
import stat
import sys
import uuid

from .build import BUILD, ENVIRONMENT, PROFILE
from .common import (
    canonical_path,
    fingerprint,
    pinned_json,
    require,
    source_identity,
    write_json,
    write_new,
)


def expiry_valid(value):
    expires = datetime.fromisoformat(value)
    require(expires.tzinfo is not None, "authorization expiry needs timezone")
    require(expires > datetime.now(timezone.utc), "expired authorization")


def authorization(path, pin, target):
    auth = pinned_json(path, pin)
    require(auth["schema"] == 1, "unsupported authorization schema")
    require(auth["decision"] == "approved-for-install", "missing root review approval")
    require(bool(auth["review_evidence"]), "missing review evidence reference")
    expiry_valid(auth["expires_at"])
    require(auth["target"] == str(target), "wrong independently authorized target")
    require(
        type(auth["current_mode"]) is int and 0 <= auth["current_mode"] <= 0o777,
        "invalid authorized current mode",
    )
    require(auth["current"] != auth["candidate"], "current and candidate must differ")
    return auth


def require_sha256(value):
    require(
        isinstance(value, str)
        and len(value) == 64
        and all(c in "0123456789abcdef" for c in value),
        "invalid SHA-256 identity",
    )


def require_fingerprint(value):
    require(set(value) == {"sha256", "size"}, "invalid fingerprint fields")
    require_sha256(value["sha256"])
    require(
        type(value["size"]) is int and value["size"] >= 0, "invalid fingerprint size"
    )


def complete_input_identity(manifest):
    inputs = manifest["inputs"]
    for name in ("vendor_sha256", "source_snapshot_sha256", "generated_sha256"):
        require_sha256(inputs[name])
    for name in ("cargo_lock", "configuration"):
        require_fingerprint(inputs[name])
    host = inputs["host"]
    require(bool(host["system_trees"]), "missing system input identities")
    for value in host["system_trees"].values():
        require_sha256(value)
    for name in ("toolchain", "alternatives"):
        require_sha256(host[name])
    require_fingerprint(host["ld_cache"])
    for name in ("bin", "lib", "lib64", "kernel", "machine"):
        require(
            isinstance(host[name], str) and bool(host[name]), "missing host identity"
        )
    for name in ("rustc", "cargo"):
        require_fingerprint(manifest["toolchain"][name])
        require(
            manifest["toolchain"][name]
            == manifest["evidence"][
                {"rustc": "004-rustc.log", "cargo": "005-cargo.log"}[name]
            ],
            "toolchain evidence mismatch",
        )
    require_sha256(manifest["producer"]["script_sha256"])
    require_fingerprint(manifest["output"])
    require(bool(manifest["build"]["sandbox_argv"]), "missing sandbox configuration")


def validate_manifest(manifest, auth, source):
    complete_input_identity(manifest)
    require(
        manifest["schema"] == 1 and manifest["profile"] == PROFILE,
        "unsupported manifest/profile",
    )
    require(
        manifest["source"] == auth["source"], "manifest/source authorization mismatch"
    )
    require(manifest["source"]["clean"] is True, "unclean producer source")
    require(manifest["output"] == auth["candidate"], "candidate authorization mismatch")
    require(manifest["build"]["argv"] == BUILD, "unexpected build command")
    require(
        manifest["build"]["environment"] == ENVIRONMENT, "unexpected build environment"
    )
    require(manifest["build"]["exit_code"] == 0, "unsuccessful build")
    require(manifest["build"]["network"] is False, "networked build not admitted")
    require(
        manifest["producer"]["review_approval"] is False,
        "producer cannot approve review",
    )
    require(
        source_identity(source, auth["source"]["commit"]) == auth["source"],
        "selected source mismatch",
    )
    require(
        fingerprint(Path(source) / "Cargo.lock") == manifest["inputs"]["cargo_lock"],
        "locked dependency mismatch",
    )
    require(
        bool(manifest["inputs"]["vendor_sha256"]) and bool(manifest["inputs"]["host"]),
        "missing producer input identity",
    )


def bundle(manifest_path, custody_pin, auth, source):
    manifest_path = canonical_path(manifest_path)
    # A hash from the manifest itself is NOT producer custody. This pin must come
    # from the independently observed producer run, through the root's channel.
    require(
        custody_pin == auth["producer_manifest_sha256"],
        "custody/authorization mismatch",
    )
    manifest = pinned_json(manifest_path, custody_pin)
    validate_manifest(manifest, auth, source)
    root = manifest_path.parent
    require(
        fingerprint(root / "control/vendor.toml")
        == manifest["inputs"]["configuration"],
        "producer configuration mismatch",
    )
    for name in ("004-rustc.log", "005-cargo.log", "006-vendor.log", "007-build.log"):
        require(
            fingerprint(root / name) == manifest["evidence"][name],
            "build evidence mismatch",
        )
    candidate = canonical_path(root / "oulipoly-agent-runner")
    require(fingerprint(candidate) == auth["candidate"], "tampered candidate binary")
    return candidate


def target_identity(path):
    info = path.lstat()
    require(
        stat.S_ISREG(info.st_mode) and info.st_nlink == 1,
        "target must be a regular single-link file",
    )
    require(
        info.st_uid == os.geteuid() and info.st_gid == os.getegid(),
        "target ownership must match installing effective uid/gid",
    )
    require(not os.listxattr(path), "target extended metadata is not supported")
    return fingerprint(path)


@contextmanager
def target_lock(target):
    # Directory inode lock leaves verify-only completely read-only. All users of
    # this installer cooperate; hostile same-UID/root writers are out of model.
    fd = os.open(target.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        yield fd
    finally:
        os.close(fd)


def selected_inputs(target, authorization_path, authorization_pin):
    require(sys.platform == "linux", "installation is Linux-only")
    target = canonical_path(target)
    return target, authorization(authorization_path, authorization_pin, target)


def install(
    source,
    manifest,
    custody_pin,
    authorization_path,
    authorization_pin,
    target,
    verify_only,
):
    target, auth = selected_inputs(target, authorization_path, authorization_pin)
    with target_lock(target) as directory_fd:
        candidate = bundle(manifest, custody_pin, auth, source)
        require(
            target_identity(target) == auth["current"], "wrong current target bytes"
        )
        require(
            stat.S_IMODE(target.stat().st_mode) == auth["current_mode"],
            "wrong current target mode",
        )
        if verify_only:
            return {"verified": True, "changed": False, "target": str(target)}
        transaction = target.parent / (".runner-install-" + uuid.uuid4().hex)
        transaction.mkdir(mode=0o700)
        old_mode = auth["current_mode"]
        require(old_mode & 0o7000 == 0, "special target mode not supported")
        write_new(transaction / "previous", target.read_bytes(), old_mode)
        write_new(transaction / "candidate", candidate.read_bytes(), 0o755)
        require(
            fingerprint(transaction / "previous") == auth["current"],
            "current changed during staging",
        )
        require(
            fingerprint(transaction / "candidate") == auth["candidate"],
            "candidate changed during staging",
        )
        receipt = {
            "schema": 1,
            "authorization_sha256": authorization_pin,
            "producer_manifest_sha256": custody_pin,
            "target": str(target),
            "previous": auth["current"],
            "candidate": auth["candidate"],
            "previous_mode": old_mode,
        }
        write_json(transaction / "receipt.json", receipt)
        sync_directory(transaction)
        os.fsync(directory_fd)
        expiry_valid(auth["expires_at"])
        require(
            target_identity(target) == auth["current"],
            "target changed before replacement",
        )
        os.replace(transaction / "candidate", target)
        os.fsync(directory_fd)
        return {
            "changed": True,
            "target": str(target),
            "transaction": str(transaction),
            "installed": fingerprint(target),
        }


def sync_directory(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def rollback(authorization_path, authorization_pin, target, transaction):
    target, auth = selected_inputs(target, authorization_path, authorization_pin)
    transaction = canonical_path(transaction)
    require(
        transaction.parent == target.parent
        and transaction.name.startswith(".runner-install-"),
        "transaction must be adjacent to target",
    )
    with target_lock(target) as directory_fd:
        receipt = json.loads((transaction / "receipt.json").read_bytes())
        require(
            receipt
            == {
                "schema": 1,
                "authorization_sha256": authorization_pin,
                "producer_manifest_sha256": auth["producer_manifest_sha256"],
                "target": str(target),
                "previous": auth["current"],
                "candidate": auth["candidate"],
                "previous_mode": auth["current_mode"],
            },
            "wrong rollback receipt",
        )
        previous = canonical_path(transaction / "previous")
        require(fingerprint(previous) == auth["current"], "tampered rollback bytes")
        require(
            target_identity(target) == auth["candidate"], "rollback current mismatch"
        )
        mode = receipt["previous_mode"]
        require(type(mode) is int and 0 <= mode <= 0o777, "invalid rollback mode")
        staged = transaction / ("rollback-" + uuid.uuid4().hex)
        write_new(staged, previous.read_bytes(), mode)
        require(
            fingerprint(staged) == auth["current"], "rollback changed during staging"
        )
        sync_directory(transaction)
        require(target_identity(target) == auth["candidate"], "rollback target changed")
        expiry_valid(auth["expires_at"])
        os.replace(staged, target)
        os.fsync(directory_fd)
        return {
            "rolled_back": True,
            "target": str(target),
            "restored": fingerprint(target),
        }
