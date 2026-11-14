#!/bin/bash
# Watchdog for the integration tests (started by run.sh as afst-guard.service).
# Every second it checks host headroom; when any check fails it records why in
# $OUT/GUARD, kills and removes all test state (reap.sh) and exits. Thresholds can be
# overridden with the AFST_* variables below.
#   guard.sh OUT_DIR RUNNER_PID
set -u
OUT=$1 RUNNER=$2
HERE=$(dirname "$(readlink -f "$0")")
TROOT=${TROOT:-/var/tmp/afst}
total=$(awk '/^MemTotal:/ {print $2}' /proc/meminfo)
# MemAvailable floor: 15% of RAM, at least 4 GiB.
pct15=$((total * 15 / 100))
MIN_AVAIL=${AFST_MIN_AVAIL_KB:-$((pct15 > 4194304 ? pct15 : 4194304))}
# Any host tmpfs may grow at most this much over its usage when the guard started,
# and a grown tmpfs may not pass 90% of its size. Tests should write nothing to tmpfs.
TMPFS_GROW=${AFST_TMPFS_GROW_KB:-1048576}
# Free space floor on the filesystem holding $TROOT.
MIN_DISK=${AFST_MIN_DISK_KB:-52428800}
# Memory pressure ceiling: PSI "full avg10" in percent.
MAX_PSI=${AFST_MAX_PSI_FULL:-20}

declare -A base
while read -r tgt used _; do base[$tgt]=$used; done < <(df -k --output=target,used,size -t tmpfs 2>/dev/null | tail -n +2)

trip() {
	{
		echo "GUARD tripped $(date -Is): $1"
		free -h
		df -h -t tmpfs
		df -h "$TROOT"
		ps -eo pid,rss,comm --sort=-rss | head -15
	} >>"$OUT/GUARD" 2>&1
	TROOT=$TROOT bash "$HERE/reap.sh" >>"$OUT/GUARD" 2>&1
	echo "reaped $(date -Is)" >>"$OUT/GUARD"
	exit 1
}

echo "guard: min_avail=${MIN_AVAIL}k tmpfs_grow=${TMPFS_GROW}k min_disk=${MIN_DISK}k max_psi=$MAX_PSI"
while kill -0 "$RUNNER" 2>/dev/null; do
	avail=$(awk '/^MemAvailable:/ {print $2}' /proc/meminfo)
	[ "$avail" -lt "$MIN_AVAIL" ] && trip "MemAvailable ${avail}k < ${MIN_AVAIL}k"
	while read -r tgt used size; do
		grow=$((used - ${base[$tgt]:-0}))
		[ "$grow" -gt "$TMPFS_GROW" ] && trip "tmpfs $tgt grew by ${grow}k"
		[ "$grow" -gt 0 ] && [ "$size" -gt 0 ] && [ $((used * 100 / size)) -ge 90 ] && trip "tmpfs $tgt at $((used * 100 / size))%"
	done < <(df -k --output=target,used,size -t tmpfs 2>/dev/null | tail -n +2)
	if [ -d "$TROOT" ]; then
		free=$(df -k --output=avail "$TROOT" | tail -1)
		[ "$free" -lt "$MIN_DISK" ] && trip "disk free ${free}k < ${MIN_DISK}k on $TROOT"
	fi
	psi=$(awk '/^full/ {split($2, a, "="); print int(a[2])}' /proc/pressure/memory)
	[ "$psi" -gt "$MAX_PSI" ] && trip "memory pressure full avg10=$psi%"
	sleep 1
done
echo "guard: runner $RUNNER gone, exiting"
