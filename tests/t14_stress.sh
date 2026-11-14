#!/bin/bash
# fsstress-style consistency (stress.py): random operations through acfs leave the same
# tree as a native replay, and diff, revert, reset, flatten and merges keep it so.
. "$(dirname "$0")/lib.sh"
setup 24G
stop_daemon
start_daemon --auto-quiet-ms 300 --auto-max-ms 1500
ST() { python3 "$SRC/tests/stress.py" "$@" 2>>$T/stress.err; }
export -f ST
changes() { afs diff "$@" | j "sorted((c['path'], c['kind']) for x in d for c in x['changes'])"; }

# A workspace against a native copy.
L=$(mkws lv)
NAT=$T/native && cp -a $L $NAT
fp $L >$T/fp.orig
afs ws register lv --from $L >/dev/null
REG=$(afs log --ws lv -n 1 | j "d[0]['id']")
ST $L 11 1500 && ST $NAT 11 1500
ok 14.1 "1500 random operations through a workspace equal a native replay" bash -c "diff <(fp $L) <(fp $NAT)"
C=$(afs changeset lv -m stress | j "d['changeset'] or ''")
[ -n "$C" ] || C=$(afs log --ws lv -n 1 | j "d[0]['id']")
ok 14.2 "diff and diff --full agree on the whole stress run" bash -c "[ \"\$(changes $REG $C)\" = \"\$(changes $REG $C --full)\" ] && [ \"\$(changes $REG $C)\" != '[]' ]"
ok 14.3 "revert of everything since register restores the original tree" bash -c "
	afs revert --from $REG --to $C | j \"d['conflict']\" | grep -qx False && diff <(fp $L) $T/fp.orig"
ok 14.4 "reset to the stress result brings it back" bash -c "afs reset $C | j \"d['conflict']\" | grep -qx False && diff <(fp $L) <(fp $NAT)"
ok 14.5 "flatten keeps the tree" bash -c "afs base flatten lv >/dev/null && diff <(fp $L) <(fp $NAT)"
ST $L 12 800 && ST $NAT 12 800
ok 14.6 "another 800 operations on the flattened base" bash -c "diff <(fp $L) <(fp $NAT)"
C2=$(afs changeset lv -m stress2 | j "d['changeset'] or ''")
[ -n "$C2" ] || C2=$(afs log --ws lv -n 1 | j "d[0]['id']")
ok 14.6a "diff and diff --full agree after flatten" bash -c "[ \"\$(changes $C $C2)\" = \"\$(changes $C $C2 --full)\" ]"
afs ws detach lv >/dev/null

# Merges: one process renames and moves at random (stress.py), another edits 30 files
# in place. Main must equal the native run with each edit at its file's new path.
W=$(mkws w)
PROT=""
for i in $(seq 1 30); do
	mkdir -p $W/p$((i % 5))
	echo "base $i" >$W/p$((i % 5))/f$i.txt
	PROT="$PROT,p$((i % 5))/f$i.txt"
done
chown -R 1000:1000 $W
afs ws register w --from $W >/dev/null
REGW=$(afs log --ws w -n 1 | j "d[0]['id']")
edit_all() { for f in ${PROT//,/ }; do echo "edit by $2" >>$1/$f; done; } # in place
expected() { # expected DIR MAP TAG: the native result with the edits at the mapped paths
	python3 - "$1" "$2" "$3" <<'PY'
import json, sys
root, where, tag = sys.argv[1], json.load(open(sys.argv[2])), sys.argv[3]
for orig, now in where.items():
    with open(root + "/" + now, "a") as f:
        f.write("edit by %s\n" % tag)
PY
}
for order in renames-first edits-first; do
	P1=$(spawn --ws w) P2=$(spawn --ws w)
	EXP=$T/exp-$order && rm -rf $EXP && cp -a $W $EXP
	seed=$([ $order = renames-first ] && echo 21 || echo 22)
	ST $RUN/mnt/$(branch $P1 w) $seed 1000 --protect "${PROT#,}" --map $T/map-$order.json
	ST $EXP $seed 1000 --protect "${PROT#,}" --map $T/mapn-$order.json
	edit_all $RUN/mnt/$(branch $P2 w) p2
	expected $EXP $T/mapn-$order.json p2
	if [ $order = renames-first ]; then first=$P1 second=$P2; else first=$P2 second=$P1; fi
	afs proc merge $first >/dev/null
	afs proc merge $second --no-resolver >$T/fin-$order.json
	ok 14.7-$order "renames and in-place edits merge without conflicts" bash -c "j \"d['conflict']\" <$T/fin-$order.json | grep -qx False"
	ok 14.7a-$order "the protected files went to the same places as natively" cmp $T/map-$order.json $T/mapn-$order.json
	ok 14.7b-$order "main equals the native result with the edits applied" bash -c "diff <(fp $W) <(fp $EXP)"
	quit $P1 >/dev/null && quit $P2 >/dev/null
	afs reset $REGW >/dev/null # back to the original tree for the next order
done
res 14.8 INFO "stress.py errors (same on both sides): $(sort $T/stress.err | cut -d: -f1 | uniq -c | tr -s ' ' | tr '\n' ',')"
finish_test
