#!/bin/bash
# Rule files (AGENTS.md 5.7, 5.13): .acfsignore keeps build output out of merges, diffs and
# automatic changesets; .acfsmerge decides paths both sides changed.
. "$(dirname "$0")/lib.sh"
setup
stop_daemon
start_daemon --auto-quiet-ms 300 --auto-max-ms 1500

W=$(mkws w)
printf 'target/\n*.o\n' >$W/.acfsignore
printf '*.bin newer\nCHANGELOG union\nCargo.toml unit=cargo\nCargo.lock unit=cargo\n.env ours\ndist/ theirs\n' >$W/.acfsmerge
head -c 2048 /dev/urandom >$W/blob.bin && echo base >$W/CHANGELOG && echo t0 >$W/Cargo.toml && echo l0 >$W/Cargo.lock
echo E0 >$W/.env && mkdir -p $W/dist && echo d0 >$W/dist/app.js
chown -R 1000:1000 $W
afs ws register w --from $W >/dev/null
fin() { afs proc merge "$@" --no-resolver >$T/fin.json 2>&1; echo $?; }
pair() { # the first process writes and merges first; the second writes last
	local p1 p2
	p1=$(spawn); p2=$(spawn)
	echo $p2 >$T/p2
	x $p1 "$1"; sleep 0.05; x $p2 "$2"
	afs proc merge $p1 >/dev/null
	fin $p2
}
late_first() { # the first process to merge wrote last
	local p1 p2
	p1=$(spawn); p2=$(spawn)
	echo $p2 >$T/p2
	x $p2 "$2"; sleep 0.05; x $p1 "$1"
	afs proc merge $p1 >/dev/null
	fin $p2
}
look() { bash -c "cd $W && $*"; } # main is the directory
export -f fin pair late_first look
export W

ok 12.1 "both build into target/ and *.o differently: no conflict" bash -c "
	[ \$(pair 'mkdir -p target && echo a >target/out && echo a >src/f1.o && echo a1 >>src/f1.txt' 'mkdir -p target && echo b >target/out && echo b >src/f1.o && echo b2 >>src/f2.txt') = 0 ]"
ok 12.1a "source edits merged, ignored files not" look "grep -qx a1 src/f1.txt && grep -qx b2 src/f2.txt && [ ! -e target ] && [ ! -e src/f1.o ]"
P=$(spawn)
x $P "mkdir -p target && echo c >target/x && echo more >>README"
CS=$(afs proc changeset $P --author t -m m | j "d['changeset']")
FORK=$(afs log --proc $P -n 1000 | j "d[-1]['snaps'][0]['parent']") # automatic snapshots may come in between
ok 12.1b "diff leaves ignored paths out" bash -c "afs diff $FORK $CS | j \"[c['path'] for c in d[0]['changes']]\" | grep -qx \"\\['README'\\]\""
ok 12.1c "diff --all lists them" bash -c "afs diff $FORK $CS --all | j \"sorted(c['path'] for c in d[0]['changes'])\" | grep -qx \"\\['README', 'target', 'target/x'\\]\""
afs proc merge $P --discard-ws w >/dev/null

ok 12.2 "*.bin newer: both modified, the later writer wins" bash -c "[ \$(pair 'printf x >>blob.bin' 'printf y >>blob.bin') = 0 ]"
ok 12.2a "blob.bin ends with y" look "[ \"\$(tail -c 1 blob.bin)\" = y ]"
ok 12.2b "the target wins when it wrote last" bash -c "[ \$(late_first 'printf r >>blob.bin' 'printf q >>blob.bin') = 0 ]"
ok 12.2c "blob.bin ends with r" look "[ \"\$(tail -c 1 blob.bin)\" = r ]"
ok 12.3 "CHANGELOG union: lines added at the same place on both sides" bash -c "[ \$(pair 'echo one >>CHANGELOG' 'echo two >>CHANGELOG') = 0 ]"
ok 12.3a "both lines are kept" look "grep -qx one CHANGELOG && grep -qx two CHANGELOG"
ok 12.4 "unit: Cargo.lock changed by one side, Cargo.toml later by the other" bash -c "[ \$(pair 'echo l1 >Cargo.lock' 'echo t2 >Cargo.toml') = 0 ]"
ok 12.4a "the later side provides the whole unit" look "[ \"\$(cat Cargo.toml)\" = t2 ] && [ \"\$(cat Cargo.lock)\" = l0 ]"
ok 12.5 ".env ours and dist/ theirs" bash -c "[ \$(pair 'echo E1 >.env && echo d1 >dist/app.js' 'echo E2 >.env && echo d2 >dist/app.js') = 0 ]"
ok 12.5a "main keeps its .env and takes the process's dist/" look "[ \"\$(cat .env)\" = E1 ] && [ \"\$(cat dist/app.js)\" = d2 ]"
ok 12.6 "without a rule the same edit still conflicts" bash -c "[ \$(pair 'echo r1 >README' 'echo r2 >README') = 2 ]"
afs proc merge $(cat $T/p2) --discard-ws w >/dev/null

# Writes below an ignored directory record nothing.
wsf() { afs ws list | j "[x['$2'] for x in d if x['name']=='$1'][0]"; }
ncs() { afs log --ws "$1" -n 100000 | j "len(d)"; }
settle() { for _ in $(seq 1 50); do sleep 0.2; [ "$(wsf "$1" pending)" = False ] && sleep 0.5 && return 0; done; return 1; }
export -f wsf ncs settle
L=$(mkws lv)
printf 'target/\n' >$L/.acfsignore && chown 1000:1000 $L/.acfsignore
afs ws register lv --from $L >/dev/null
N0=$(ncs lv)
mkdir -p $L/target && echo x >$L/target/a
settle lv
ok 12.7 "writes only below an ignored directory record no changeset" test "$(ncs lv)" = $N0
echo y >>$L/README
settle lv
ok 12.7a "a write elsewhere does, without the ignored file" bash -c "[ \$(ncs lv) = $((N0 + 1)) ] && afs diff \$(wsf lv head_snap) | j \"[c['path'] for c in d[0]['changes']]\" | grep -qx \"\\['README'\\]\""
ok 12.7b "the snapshot still holds the ignored file (diff --all)" bash -c "afs diff \$(wsf lv head_snap) --all | grep -q target/a"
finish_test
