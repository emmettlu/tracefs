#!/bin/bash
# Performance (AGENTS.md 8) on the test machine's NVMe btrfs root (compress=zstd:1).
# Envs: native = rw snapshot of base; ovl = kernel overlay (tracefs options) over ro base;
# fuse = fuse-overlayfs over the same base; loop-* = same on a loop btrfs image (fallback).
. "$(dirname "$0")/lib.sh"
P=$V/perf            # on the root btrfs, not /var/lib
M=/tmp/afsv-perf
L=$V/logs/t07
JL=/tmp/afsv-json   # hyperfine runs as the invoking user and must write its JSON here
mkdir -p $L $M $JL && chmod 1777 $JL
U="sudo -u $SUDO_USER"
HF="$U hyperfine --style none"
umount_all() { findmnt -rn -o TARGET | grep -E "^$M/" | sort -r | xargs -r -n1 umount -l; }
umount_all
cleanup_perf() {
	find $P -maxdepth 1 -mindepth 1 2>/dev/null | while read s; do
		btrfs property set "$s" ro false 2>/dev/null
		btrfs -q subvolume delete "$s" 2>/dev/null || rm -rf "$s"
	done
}
cleanup_perf
mkdir -p $P

setup_env() { # setup_env ROOT TAG : bases from data/, native snapshot, ovl and fuse mounts
	local root=$1 t=$2
	for w in linux ripgrep; do
		btrfs -q subvolume create $root/base-$w
		cp -a --reflink=auto $V/data/$w/. $root/base-$w/
		chown $SUDO_USER: $root/base-$w
	done
	head -c 1G /dev/urandom >$root/base-linux/fio-read.dat && chown $SUDO_USER: $root/base-linux/fio-read.dat
	$U git -C $root/base-linux status --porcelain >/dev/null   # make index stat info match
	for w in linux ripgrep; do btrfs property set $root/base-$w ro true; done
	for w in linux ripgrep; do
		btrfs -q subvolume snapshot $root/base-$w $root/native-$w
		mkdir -p $M/$t-native-$w && mount --bind $root/native-$w $M/$t-native-$w
		new_branch $root/br-$w && chown $SUDO_USER: $root/br-$w/upper
		mnt_ovl $M/$t-ovl-$w $root/br-$w $root/base-$w
		new_branch $root/fbr-$w && chown $SUDO_USER: $root/fbr-$w/upper
		mkdir -p $M/$t-fuse-$w
		fuse-overlayfs -o lowerdir=$root/base-$w,upperdir=$root/fbr-$w/upper,workdir=$root/fbr-$w/work,allow_other $M/$t-fuse-$w
	done
}

hf() { # hf NAME RUNS PREPARE CMD... -> mean ms
	local n=$1 runs=$2 prep=$3
	shift 3
	local a=(--runs "$runs" --export-json $JL/$n.json)
	[ -n "$prep" ] && a+=(--shell=sh --prepare "$prep") || a+=(--warmup 2)
	[ "${IGN:-0}" = 1 ] && a+=(-i)
	$HF "${a[@]}" "$*" >/dev/null 2>$JL/$n.err || { echo "FAIL($(head -c 150 $JL/$n.err))"; return; }
	python3 - $JL/$n.json <<'PY'
import json, statistics, sys
m = [x["time_wall_clock"]["value"] * 1000 for x in json.load(open(sys.argv[1]))["results"][0]["measurements"]]
print("%.0f±%.0f" % (statistics.mean(m), statistics.stdev(m) if len(m) > 1 else 0))
PY
}
fio_w() { # fio_w DIR DIRECT -> MiB/s and IOPS
	$U fio --name=w --filename=$1/fio-w.dat --rw=randwrite --bs=4k --size=1G --ioengine=psync --direct=$2 \
		--runtime=15 --time_based --end_fsync=1 --output-format=json >$L/fio.json 2>/dev/null
	rm -f $1/fio-w.dat
	python3 -c "import json;j=json.load(open('$L/fio.json'))['jobs'][0]['write'];print('%.0f IOPS' % j['iops'])"
}
fio_r() {
	$U fio --name=r --filename=$1/fio-read.dat --rw=randread --bs=4k --size=1G --ioengine=psync --direct=1 \
		--runtime=10 --time_based --output-format=json --readonly >$L/fio.json 2>/dev/null
	python3 -c "import json;j=json.load(open('$L/fio.json'))['jobs'][0]['read'];print('%.0f IOPS' % j['iops'])"
}
DROP="sync; echo 3 > /proc/sys/vm/drop_caches"

run_suite() { # run_suite TAG ENVS...
	local t=$1
	shift
	for e in "$@"; do
		local d=$M/$t-$e-linux r=$M/$t-$e-ripgrep
		$U git -C $d status --porcelain >/dev/null 2>&1 # first refresh (writes index -> copy-up)
		res 7.1 INFO "[$t/$e] git status warm (96k files): $(hf $t-$e-gs 10 '' git -C $d status --porcelain) ms"
		res 7.2 INFO "[$t/$e] rg full tree warm: $(IGN=1 hf $t-$e-rg 10 '' rg -c --no-messages zzqqxx_nomatch $d) ms"
		res 7.3 INFO "[$t/$e] git status cold: $(HF="hyperfine --style none" hf $t-$e-gsc 3 "sh -c '$DROP'" $U git -C $d status --porcelain) ms"
		res 7.4 INFO "[$t/$e] fio 4k randwrite buffered+end_fsync: $(fio_w $d 0)"
		res 7.5 INFO "[$t/$e] fio 4k randwrite O_DIRECT: $(fio_w $d 1)"
		res 7.6 INFO "[$t/$e] fio 4k randread O_DIRECT lower file: $(fio_r $d)"
		if [ "$t" = nvme ]; then
			res 7.7 INFO "[$t/$e] cargo build ripgrep (debug, -j4, offline): $(hf $t-$e-cargo 2 "rm -rf $r/target" "cd $r && cargo build --offline -q") ms"
		fi
	done
}

refine() { # interleaved repetitions to separate env effects from run-to-run noise
	local t=nvme r
	for e in native ovl fuse; do $U git -C $M/$t-$e-linux status --porcelain >/dev/null 2>&1; done
	for round in 1 2 3; do
		for e in native ovl fuse; do
			echo "gs $e $(hf r-gs-$e 30 '' git -C $M/$t-$e-linux status --porcelain)"
			echo "rg $e $(IGN=1 hf r-rg-$e 30 '' rg -c --no-messages zzqqxx_nomatch $M/$t-$e-linux)"
		done
	done >$L/refine-read
	for round in 1 2 3 4 5; do
		for e in native ovl fuse; do echo "fiobuf $e $(fio_w $M/$t-$e-linux 0)"; echo "fiodirect $e $(fio_w $M/$t-$e-linux 1)"; done
	done >$L/refine-fio
	for round in 1 2 3; do
		for e in native ovl fuse; do
			r=$M/$t-$e-ripgrep
			echo "cargo $e $(hf r-cargo-$e 1 "rm -rf $r/target" "cd $r && CARGO_TARGET_DIR=$r/target cargo build --release --offline -q -j4")"
		done
	done >$L/refine-cargo
	cat $L/refine-read $L/refine-fio $L/refine-cargo | while read k e v; do res 7.R INFO "$k $e $v"; done
}

if [ "${1:-}" = refine ]; then
	setup_env $P nvme
	refine
	umount_all
	cleanup_perf
	exit 0
fi

setup_env $P nvme
run_suite nvme native ovl fuse

# loop btrfs image fallback (non-btrfs hosts)
LR=$(mkimg t07 32G)
setup_env $LR loop
run_suite loop native ovl

# diff timing: linux base, A = 5000 changed files, B = A + 1000 changed + 50 del + 50 add
D=$P/diff
mkdir -p $D
new_branch $P/br-diff && mnt_ovl $M/diff $P/br-diff $P/base-linux
files=$(cd $P/base-linux && git ls-files -- '*.c' | head -6100)
echo "$files" | head -5000 | (cd $M/diff && while read f; do echo "// a" >>"$f"; done)
snap_ro $P/br-diff $P/snapA
echo "$files" | tail -1100 | head -1000 | (cd $M/diff && while read f; do echo "// b" >>"$f"; done)
echo "$files" | tail -100 | head -50 | (cd $M/diff && xargs rm)
(cd $M/diff && for i in $(seq 1 50); do echo n >new_$i.c; done)
snap_ro $P/br-diff $P/snapB
res 7.9 INFO "diff v2 (btrfs send --no-data -p | receive --dump): $(HF="hyperfine --style none --shell=sh" hf diffv2 5 '' "btrfs send -q --no-data -p $P/snapA $P/snapB | btrfs receive --dump >/dev/null") ms; ops $(btrfs send -q --no-data -p $P/snapA $P/snapB | btrfs receive --dump | wc -l)"

# changeset cost on NVMe: 3 branches, 10k dirty files total, then 3 snapshots
for k in 1 2 3; do new_branch $P/cs$k && mnt_ovl $M/cs$k $P/cs$k $P/base-linux; done
for k in 1 2 3; do (cd $M/cs$k && mkdir g && for i in $(seq 1 3334); do echo $i >g/$i; done); done
t0=$(date +%s%N); for k in 1 2 3; do snap_ro $P/cs$k $P/cs$k-s1; done; t1=$(date +%s%N)
res 7.10 INFO "NVMe: 3 snapshots with 10k dirty new files (no syncfs): $(((t1 - t0) / 1000000)) ms"
for k in 1 2 3; do (cd $M/cs$k && for i in $(seq 1 3334); do echo x >>g/$i; done); done
t0=$(date +%s%N); sync -f $M/cs1; t1=$(date +%s%N); for k in 1 2 3; do snap_ro $P/cs$k $P/cs$k-s2; done; t2=$(date +%s%N)
res 7.10a INFO "NVMe: syncfs (fs-wide) $(((t1 - t0) / 1000000)) ms + 3 snapshots $(((t2 - t1) / 1000000)) ms"

umount_all
rmimg t07
cleanup_perf
