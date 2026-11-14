#!/bin/bash
# Changesets and diff correctness against an independent ground truth.
. "$(dirname "$0")/lib.sh"
setup
CMP="python3 $(dirname "$0")/cmpdiff.py"

# A richer base tree.
W=$T/src/rich
mkdir -p $W/d1 $W/d2/sub $W/d3 $W/d4 $W/chain $W/tdir/x $W/dst $W/odd $W/deep/$(printf 'q%.0s' $(seq 1 120))/mv
echo hello >$W/f; printf 'l1\nl2\nl3\n' >$W/text; head -c 64M /dev/urandom >$W/big
truncate -s 256M $W/sparse; echo a >$W/d1/a; echo b >$W/d1/b; echo c >$W/d2/sub/c; echo e >$W/d3/e; echo g >$W/d4/g
echo ch >$W/chain/k; echo m >$W/mode; echo t >$W/times; echo tf >$W/tfile; echo y >$W/tdir/x/y
echo r1 >$W/ren_src; echo r2 >$W/ren_dst; ln -s f $W/sym; mkfifo $W/fifo; echo x >$W/xattr; setfattr -n user.k -v v1 $W/xattr
echo l >$W/odd/$(printf 'n%.0s' $(seq 1 255)); echo u >"$W/odd/中文 ü"; echo tr >$W/trunc; echo hl >$W/hl1; ln $W/hl1 $W/hl2
echo dm >$W/deep/$(printf 'q%.0s' $(seq 1 120))/mv/f
chown -R 1000:1000 $W
afs ws register rich --from $W >/dev/null
export W
P=$(spawn --ws rich)
B=$(branch $P rich)
V=$RUN/mnt/$B
fp $V >$T/fp0
LONG=$(printf 'n%.0s' $(seq 1 255))
x $P "
set -e
echo more >>f; chmod 600 mode; touch -d 2001-01-01 times; echo tail >>big
printf x | dd of=sparse bs=1 seek=100 conv=notrunc status=none
: >trunc; rm d1/a; rm -rf d3; rm -rf d1 && mkdir d1 && echo new >d1/n
mv d2 d2r; mv d4 dst/d4m; mv chain chain1 && mv chain1 chain2 && mv chain2 chain
mv ren_src ren_dst; rm tfile && mkdir tfile && echo in >tfile/i; rm -rf tdir && echo now-file >tdir
ln -sf text sym; rm odd/$LONG && mv 'odd/中文 ü' 'odd/中文 ü2'
echo tmp >d2r/tmp && rm d2r/tmp; echo new >newfile && mkdir -p newdir/a/b && echo z >newdir/a/b/z
ln newfile newhl; mkfifo newfifo; touch \"\$(printf 'bad\377')\"; echo x >>hl1
"
xr $P "touch u5 && chown 5:5 u5"
python3 -c "import os; os.setxattr('$V/xattr', 'user.k', b'v2')"
fp $V >$T/fp1
C1=$(afs proc changeset $P --author tester -m "ops 1" --tool-call call-1 | j "d['changeset']")
afs diff $C1 >$T/d1.json
ok 3.1 "diff of changeset 1 equals ground truth ($($CMP $T/fp0 $T/fp1 $T/d1.json | head -1))" $CMP $T/fp0 $T/fp1 $T/d1.json
afs diff $C1 --full >$T/d1f.json
ok 3.1a "fast diff equals --full diff" python3 -c "
import json; a=json.load(open('$T/d1.json')); b=json.load(open('$T/d1f.json')); assert a[0]['changes']==b[0]['changes']"
ok 3.1b "directory renames reported with their source" python3 -c "
import json; c={x['path']: x.get('from') for x in json.load(open('$T/d1.json'))[0]['changes'] if x['kind']=='rename'}
assert c.get('d2r')=='d2' and c.get('dst/d4m')=='d4', c"
ok 3.1c "touch-only and create+delete leave no change" python3 -c "
import json; p={x['path'] for x in json.load(open('$T/d1.json'))[0]['changes'] if isinstance(x['path'], str)}
assert 'times' not in p and 'd2r/tmp' not in p and 'chain' not in p, p"
ok 3.1d "non-UTF-8 name returned as base64" python3 -c "
import json; c=[x['path'] for x in json.load(open('$T/d1.json'))[0]['changes'] if isinstance(x['path'], dict)]
assert c==[{'base64':'YmFk/w=='}], c"

# Second round of edits on top of renamed/opaque directories.
x $P "set -e; mv d2r d2again; mv dst/d4m d4back; rm -rf newdir; echo z >>d1/n; rm -rf tfile; mkdir -p d2again/sub/deeper && echo k >d2again/sub/deeper/k; echo 2 >>text"
fp $V >$T/fp2
C2=$(afs proc changeset $P --author tester -m "ops 2" | j "d['changeset']")
afs diff $C2 >$T/d2.json
ok 3.2 "diff of changeset 2 equals ground truth" $CMP $T/fp1 $T/fp2 $T/d2.json
afs diff $C1 $C2 >$T/d12.json
ok 3.2a "two-changeset diff equals ground truth" $CMP $T/fp1 $T/fp2 $T/d12.json
afs diff $C2 $C1 >$T/d21.json
ok 3.2b "reverse diff equals ground truth" $CMP $T/fp2 $T/fp1 $T/d21.json
FORK=$(afs log --proc $P | j "d[-1]['snaps'][0]['parent']")
afs diff $FORK $C2 >$T/d02.json
ok 3.2c "diff from fork snapshot to changeset 2 equals ground truth" $CMP $T/fp0 $T/fp2 $T/d02.json

afs diff $C2 --patch >$T/p.json
ok 3.3 "--patch has a unified diff for text" bash -c "j \"d[0]['patch']\" <$T/p.json | grep -q '^+2$'"
x $P "printf '\0\1' >>big2; echo x >>big"
C3=$(afs proc changeset $P --author t -m bin | j "d['changeset']")
ok 3.3a "--patch marks binary and large files" bash -c "afs diff $C3 --patch | j \"d[0]['patch']\" | grep -q 'Binary file big2\|Large file big'"


# Atomic changeset across three workspaces.
for n in a b c; do W_=$(mkws m$n); afs ws register m$n --from $W_ >/dev/null; done
M=$(spawn --ws ma --ws mb --ws mc)
XDIR=/ x $M "for n in a b c; do echo marker >$T/src/m\$n/marker; done"
CM=$(afs proc changeset $M --author t -m multi | j "d['changeset']")
bad 3.5a "diff of two unrelated workspaces" bash -c "afs diff $C1 \$(afs ws list | j \"[w['head_snap'] for w in d if w['name']=='ma'][0]\")"
ok 3.6 "one changeset holds one snapshot per branch, each with the marker" bash -c "
	afs diff $CM | j \"len(d)==3 and all(any(c['path']=='marker' for c in x['changes']) for x in d)\" | grep -qx True"

# Pause length with 10k dirty files over 3 workspaces; another process keeps running.
(cd / && arunx --ws mb -- bash -c "while [ ! -e $T/ag/qstop ]; do echo \$EPOCHREALTIME; done >$T/ag/ts") &
Q=$!
sleep 0.5
for k in 1 2 3; do
	XDIR=/ x $M "for n in a b c; do mkdir -p $T/src/m\$n/g$k; for i in \$(seq 1 3334); do echo \$i >$T/src/m\$n/g$k/\$i; done; done"
	afs proc changeset $M --author t -m "bulk $k" | j "round(d['pause_ms'])"
done >$T/pause
touch $T/ag/qstop
wait $Q
gap=$(python3 -c "
ts=[float(l) for l in open('$T/ag/ts') if l.strip()]
print('%.1f' % (max(b-a for a,b in zip(ts,ts[1:]))*1000))")
ok 3.7 "changeset pause with 10002 new files over 3 workspaces < 100 ms (got: $(tr '\n' ' ' <$T/pause)ms)" bash -c "[ \$(sort -n $T/pause | tail -1) -lt 100 ]"
ok 3.7a "other process never paused (max loop gap ${gap} ms)" python3 -c "assert $gap < 20"
ok 3.7b "all 10002 files of the last round are in the snapshots" bash -c "
	n=0; for s in \$(afs log --proc $M -n 1 | j \"' '.join(x['snap'] for x in d[0]['snaps'])\"); do n=\$((n + \$(ls $STATE/snaps/\$s/upper/g3 | wc -l))); done; [ \$n = 10002 ]"

# Dirty mmap pages without msync are captured.
MB=$(branch $M ma)
python3 - $RUN/mnt/$MB/mm $T/mm-ready <<'PY' &
import mmap, os, sys, time
fd = os.open(sys.argv[1], os.O_RDWR | os.O_CREAT, 0o644)
os.ftruncate(fd, 1 << 20)
m = mmap.mmap(fd, 1 << 20)
m[:9] = b"mmap-data"
open(sys.argv[2], "w").close()
time.sleep(3)
PY
MMPID=$!
while [ ! -e $T/mm-ready ]; do sleep 0.05; done
SM=$(afs proc changeset $M --author t -m mmap | j "[s['snap'] for s in d['snaps'] if s['ws']=='ma'][0]")
ok 3.8 "mmap write without msync is in the snapshot" bash -c "[ \"\$(head -c 9 $STATE/snaps/$SM/upper/mm)\" = mmap-data ]"
rm -f $T/mm-ready; wait $MMPID

ok 3.9 "snapshots are read-only subvolumes" bash -c "btrfs property get $STATE/snaps/$SM ro | grep -q true"
finish_test
