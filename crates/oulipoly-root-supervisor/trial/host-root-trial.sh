#!/usr/bin/env bash
# One bounded host-root trial of the per-root custody controls.
#
# Run ONCE, as uid 0, by the root reviewer only; never by a model. It runs
# the already-built process-test binary of this crate (built unprivileged
# by its author; no build here) so that each owner starts root PID 1 in a
# new PID namespace in the host user namespace ("host-root-pidns") instead
# of the unprivileged user-namespace isolation.
#
# Usage: host-root-trial.sh <process-test-binary> <results-dir>
#   <process-test-binary>  target/debug/deps/process-<hash> from the build
#   <results-dir>          existing directory for the log and summary; kept
#
# Effects (and nothing else):
#   * creates one fresh directory /tmp/age319-root-trial.XXXXXX (mode 0700,
#     uid 0) used as TMPDIR: per-test scratch dirs, private root stores,
#     peer state files, root PID 1 receipts and sockets, and the stamps file;
#   * the tests start, as uid 0: owners (oulipoly-root-supervisor), root PID
#     1s and work PID 1s (oulipoly-root-pid1) in new PID namespaces, and
#     deterministic peers (oulipoly-acp-deterministic-peer) inside them. No
#     network, no installed file, service, socket outside the trial dir,
#     setuid, systemd unit or global state;
#   * signals only the trial's own processes: tests kill their own owner
#     children and, through pidfds verified against their own store, their
#     own root PID 1s (each takes its namespace with it);
#   * writes <results-dir>/trial.log and <results-dir>/summary.txt.
#
# Stops / labels:
#   STOP not-root           not uid 0; nothing done
#   STOP bad-binary         test binary missing or not executable
#   RESULT tests-exit=N     exit status of the test binary (0 = all passed)
#   LEAK pid start          a stamped root PID 1 still running (exact match)
#   CLEANUP removed dir     the trial directory was removed
#
# Exact cleanup manifest: the stamped root PID 1s (pid + start time in
# stamps) must all be gone; the trial directory is removed. Any LEAK line is
# left for the reviewer: kill it only after re-checking its start time.
set -u
bin=${1:-}
results=${2:-}
if [ "$(id -u)" != 0 ]; then echo "STOP not-root"; exit 2; fi
if [ ! -x "$bin" ] || [ ! -d "$results" ]; then echo "STOP bad-binary"; exit 2; fi
trial=$(mktemp -d /tmp/age319-root-trial.XXXXXX) || exit 2
chmod 0700 "$trial"
stamps="$trial/stamps"
: > "$stamps"
echo "trial dir: $trial" | tee "$results/summary.txt"
TMPDIR="$trial" ROOT_CUSTODY_STAMPS="$stamps" timeout 600 "$bin" --test-threads=1 \
    > "$results/trial.log" 2>&1
status=$?
echo "RESULT tests-exit=$status" | tee -a "$results/summary.txt"
leaks=0
while read -r pid start; do
    [ -n "$pid" ] || continue
    now=$(awk '{ sub(/.*\) /, ""); print $20 }' "/proc/$pid/stat" 2>/dev/null || true)
    if [ "$now" = "$start" ]; then
        echo "LEAK $pid $start" | tee -a "$results/summary.txt"
        leaks=$((leaks + 1))
    fi
done < "$stamps"
echo "stamped root pid1s: $(wc -l < "$stamps") leaks: $leaks" | tee -a "$results/summary.txt"
cp "$stamps" "$results/stamps.txt"
case "$trial" in
    /tmp/age319-root-trial.*) rm -rf -- "$trial" && echo "CLEANUP removed $trial" | tee -a "$results/summary.txt" ;;
esac
exit $(( status != 0 || leaks != 0 ))
