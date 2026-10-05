#!/bin/bash
# Offline end-to-end fixture: run as the unprivileged user, never as root.
#   run_e2e.sh STAGE_DIR OUT_DIR
# STAGE_DIR is a built package stage (build_package.py); OUT_DIR must not
# exist. Everything runs in a fresh user namespace (the caller mapped to
# root, its subordinate ids to 1..65536), with its own loopback-only
# network, mount and PID namespaces; the PID namespace dies with it.
set -u
[ "$(id -u)" != 0 ] || { echo "refused: run as the unprivileged user" >&2; exit 2; }
stage=$(realpath "$1")
out=$2
here=$(dirname "$(realpath "$0")")
exec timeout --kill-after=10 500 \
  unshare --user --map-root-user --map-users=1:100000:65536 --map-groups=1:100000:65536 \
    --net --mount --pid --fork --mount-proc --kill-child -- \
  env -i PATH=/usr/sbin:/usr/bin:/bin LANG=C.UTF-8 HOME=/nonexistent PYTHONDONTWRITEBYTECODE=1 \
    /usr/bin/python3 "$here/e2e.py" --stage "$stage" --out "$out"
