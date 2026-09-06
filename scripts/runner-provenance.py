#!/usr/bin/env python3
"""Explicit Linux raw-runner build, verify/install, and rollback boundary."""

# Bytecode must not dirty the source before its clean-state check.
# ruff: noqa: E402
import argparse
import json
import sys

sys.dont_write_bytecode = True

from runner_provenance.build import build
from runner_provenance.common import Rejected
from runner_provenance.install import install, rollback


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="operation", required=True)
    producer = sub.add_parser("build", help="produce unapproved custody evidence")
    producer.add_argument("--source", required=True)
    producer.add_argument("--commit", required=True)
    producer.add_argument(
        "--destination", required=True, help="new external directory; never reused"
    )
    producer.add_argument(
        "--toolchain", required=True, help="actual Rust toolchain directory"
    )
    verifier = sub.add_parser("install", help="verify only unless --apply is explicit")
    verifier.add_argument("--source", required=True)
    verifier.add_argument("--manifest", required=True)
    verifier.add_argument("--producer-manifest-sha256", required=True)
    verifier.add_argument("--apply", action="store_true")
    undo = sub.add_parser("rollback")
    undo.add_argument("--transaction", required=True)
    for command in (verifier, undo):
        command.add_argument("--authorization", required=True)
        command.add_argument("--authorization-sha256", required=True)
        command.add_argument("--target", required=True)
    args = parser.parse_args()
    if args.operation == "build":
        build(args.source, args.commit, args.destination, args.toolchain)
        return
    if args.operation == "install":
        result = install(
            args.source,
            args.manifest,
            args.producer_manifest_sha256,
            args.authorization,
            args.authorization_sha256,
            args.target,
            not args.apply,
        )
    else:
        result = rollback(
            args.authorization, args.authorization_sha256, args.target, args.transaction
        )
    print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (Rejected, OSError, ValueError, KeyError, TypeError) as error:
        print(f"REJECTED: {error}", file=sys.stderr)
        sys.exit(1)
