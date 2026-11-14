#!/bin/bash
# Run integration tests detached: sudo nohup bash tests/run.sh [test...] &
# Results: tests/out/<name>.out, summary in tests/out/summary, guard trips in tests/out/GUARD.
#
# Host safety: test state is on disk ($TROOT), each test runs in a scope capped at
# AFST_MEM_MAX, tracefs run processes run in afstest.slice with the same cap (tmpfs pages count
# against these caps too), and guard.sh reaps everything when host RAM, tmpfs, disk
# or memory pressure crosses its thresholds.
set -u
cd "$(dirname "$(readlink -f "$0")")"
OUT=$PWD/out
export TROOT=${TROOT:-/var/tmp/afst}
MEM_MAX=${AFST_MEM_MAX:-8G}
LIMITS=(-p MemoryMax="$MEM_MAX" -p MemorySwapMax=0 -p TasksMax=8192 -p CPUWeight=50 -p IOWeight=50)
mkdir -p "$OUT"
tests=${*:-$(ls t[0-9]*.sh)}
: >"$OUT/summary"
rm -f "$OUT/GUARD"

# Cap for the processes tracefsd starts under --slice afstest.
mkdir -p /run/systemd/system
printf '[Slice]\nMemoryMax=%s\nMemorySwapMax=0\nTasksMax=8192\nCPUWeight=50\nIOWeight=50\n' "$MEM_MAX" >/run/systemd/system/afstest.slice
systemctl daemon-reload

systemctl stop afst-guard.service >/dev/null 2>&1
systemctl reset-failed afst-guard.service >/dev/null 2>&1
systemd-run --quiet --unit=afst-guard --collect -p OOMScoreAdjust=-1000 -p Nice=-5 -p RuntimeMaxSec=8h \
	--setenv=TROOT="$TROOT" bash "$PWD/guard.sh" "$OUT" $$ || { echo "cannot start guard" >&2; exit 1; }
sleep 1
systemctl is-active -q afst-guard.service || { echo "guard is not running" >&2; exit 1; }

for t in $tests; do
	n=$(basename "$t" .sh)
	if [ -e "$OUT/GUARD" ]; then
		echo "SUMMARY|$n|skipped: guard tripped" >>"$OUT/summary"
		continue
	fi
	systemd-run --scope --quiet --slice=afstrun --unit="afst-$n" "${LIMITS[@]}" \
		timeout -k 10 1800 bash "$PWD/$n.sh" >"$OUT/$n.out" 2>&1
	rc=$?
	grep -h '^SUMMARY' "$OUT/$n.out" >>"$OUT/summary" || echo "SUMMARY|$n|aborted rc=$rc" >>"$OUT/summary"
	# Clean up whatever an aborted test left behind.
	NAME=$n bash -c '. ./lib.sh; cleanup; rm -rf "$T"' >/dev/null 2>&1
done

systemctl stop afst-guard.service >/dev/null 2>&1
bash ./reap.sh
rm -f /run/systemd/system/afstest.slice
systemctl daemon-reload
[ -e "$OUT/GUARD" ] && echo "GUARD tripped, see out/GUARD" >>"$OUT/summary"
echo DONE >>"$OUT/summary"
