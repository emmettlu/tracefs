#!/bin/bash
# A process over several workspaces, and going back in its history: reset and revert of
# its own changesets apply to its branches while it runs, to main after it is done.
. "$(dirname "$0")/lib.sh"
setup

WA=$T/src/a WB=$T/src/b WC=$T/src/c
for n in a b c; do W_=$(mkws $n); afs ws register $n --from $W_ >/dev/null; done
W=$WA
export W WA WB WC

P=$(spawn)
ok 5.1 "every workspace is visible at its host path" bash -c "XDIR=/ x $P 'cat $WB/README' | grep -qx readme"
XDIR=/ x $P "echo added >$WB/added"
ok 5.1b "writes land in the workspace's branch" test -e $STATE/branches/$(branch $P b)/upper/added
ok 5.1c "a changeset covers every branch" bash -c "afs proc changeset $P --author t -m three | j 'len(d[\"snaps\"])' | grep -qx 3"
ok 5.1d "main does not see the write" test ! -e $WB/added

# Reset of the process's own changesets: on its branch, the process keeps running.
x $P "echo v1 >state"
C1=$(afs proc changeset $P --author t -m v1 | j "d['changeset']")
x $P "echo v2 >state; echo later >later"
C2=$(afs proc changeset $P --author t -m v2 | j "d['changeset']")
x $P "echo uncommitted >state"
PID=$(pid_of $P)
ok 5.6 "reset to changeset 1" bash -c "afs reset $C1 | j \"d['conflict']\" | grep -qx False"
ok 5.6a "the process's view equals changeset 1" bash -c "[ \"\$(x $P 'cat state')\" = v1 ] && ! x $P 'test -e later'"
ok 5.6b "the process kept running, its other branches untouched, main unchanged" bash -c "
	[ \$(pid_of $P) = $PID ] && XDIR=/ x $P 'test -e $WB/added' && [ \"\$(cat $WA/README)\" = readme ] && [ ! -e $WA/state ]"
ok 5.6c "the edits made before the reset are in its history" bash -c "afs log --proc $P -n 2 | j \"[c['message'] for c in d]\" | grep -q 'before reset to $C1'"
ok 5.6d "reset forward to changeset 2" bash -c "afs reset $C2 >/dev/null && [ \"\$(x $P 'cat state')\" = v2 ] && x $P 'test -e later'"

# Revert of one changeset, and of a range (a tool call: before .. after).
x $P "echo t1 >t1"
CT1=$(afs proc changeset $P --author t -m t1 | j "d['changeset']")
x $P "echo t2 >t2"
CT2=$(afs proc changeset $P --author t -m t2 | j "d['changeset']")
ok 5.7 "revert of one of its changesets undoes only that one" bash -c "afs revert $CT1 >/dev/null && ! x $P 'test -e t1' && x $P 'test -e t2'"
ok 5.7b "nothing changed since the head: no changeset, no snapshot" bash -c "
	n=\$(ls $STATE/snaps | wc -l); afs proc changeset $P --author pi -m none | j \"d['unchanged'] and d['changeset'] is None\" | grep -qx True &&
	[ \$(ls $STATE/snaps | wc -l) = \$n ] && ! afs log --proc $P -n 1 | j \"d[0]['message']\" | grep -qx none"
BEFORE=$(afs proc changeset $P --author pi -m 'before tool calls' --since $CT2 | j "d['changeset']")
x $P "echo call >call.txt && echo call >>state"
AFTER=$(afs proc changeset $P --author pi -m 'tool call' | j "d['changeset']")
x $P "echo next >next.txt"
afs proc changeset $P --author pi -m next >/dev/null
ok 5.7a "revert --from --to undoes the tool call, keeps what came after" bash -c "
	afs revert --from $BEFORE --to $AFTER >/dev/null && ! x $P 'test -e call.txt' && [ \"\$(x $P 'cat state')\" = v2 ] && x $P 'test -e next.txt'"
x $P "echo since >since.txt"
afs proc changeset $P --author t -m head >/dev/null
ok 5.7c "--since an older changeset: recorded though the head already holds it" bash -c "
	afs proc changeset $P --author pi -m 'since before' --since $AFTER | j \"d['changeset'] is not None\" | grep -qx True"
ok 5.7d "--since the head itself: nothing" bash -c "
	H=\$(afs log --proc $P -n 1 | j \"d[0]['id']\"); afs proc changeset $P --author pi -m same --since \$H | j \"d['unchanged']\" | grep -qx True"

# Reset to before an update: the update brought another process's work from main; a reset
# undoes only the process's own work, so merging again keeps the other's work in main.
Q=$(spawn --ws a)
x $Q "echo from-q >q.txt"
afs proc merge $Q >/dev/null
R=$(spawn --ws a)
x $R "echo r1 >r.txt"
RC1=$(afs proc changeset $R --author t -m r1 | j "d['changeset']")
Q2=$(spawn --ws a)
x $Q2 "echo from-q2 >q2.txt"
afs proc merge $Q2 >/dev/null
x $R "echo r2 >r2.txt"
afs proc merge $R >/dev/null
ok 5.8 "R's merge brought q2.txt into R" x $R "test -e q2.txt"
afs reset $RC1 >/dev/null
ok 5.8a "reset to before that merge undoes R's own r2.txt, keeps q2.txt from main" bash -c "x $R 'test -e q2.txt' && x $R 'test -e r.txt' && ! x $R 'test -e r2.txt'"
afs proc merge $R >/dev/null
ok 5.8b "merging R again keeps main's q2.txt and drops r2.txt" bash -c "[ -e $WA/q2.txt ] && [ -e $WA/r.txt ] && [ -e $WA/q.txt ] && [ ! -e $WA/r2.txt ]"

# After the process is done its changesets apply to main.
quit $P >/dev/null
ok 5.9 "the exited process's work is in main" bash -c "[ \"\$(cat $WA/state)\" = v2 ] && [ -e $WA/next.txt ] && [ -e $WB/added ]"
ok 5.9a "reset to one of its changesets now applies to main" bash -c "afs reset $C1 >/dev/null && [ \"\$(cat $WA/state)\" = v1 ] && [ ! -e $WA/later ]"
ok 5.9b "and is recorded as a workspace changeset" bash -c "afs log --ws a -n 1 | j \"(d[0]['owner_kind'], d[0]['message'])\" | grep -qx \"('workspace', 'reset to $C1')\""

# Whole files a reset brings back share the snapshot's blocks: copy_file_range from the
# snapshot's view to the branch's overlay is a btrfs clone of the underlying files.
ext() { filefrag -v "$1" | awk '$1 ~ /^[0-9]+:$/ {print $4 $5}'; }
export -f ext
snap_of() { afs log "$@" -n 5 | j "[s['snap'] for c in d if c['id']=='$CS' for s in c['snaps']][0]"; }
head -c 32M /dev/urandom >$WC/big
CS=$(afs changeset c -m big | j "d['changeset']")
SNAP=$(snap_of --ws c)
head -c 32M /dev/urandom >$WC/big
afs changeset c -m big2 >/dev/null
afs reset $CS >/dev/null
MAIN_C=$(afs ws list | j "[w['main_branch'] for w in d if w['name']=='c'][0]")
ok 5.10 "reset on main: the restored file has the snapshot's extents" bash -c "
	cmp -s $WC/big $STATE/snaps/$SNAP/upper/big && [ -n \"\$(ext $STATE/snaps/$SNAP/upper/big)\" ] &&
	[ \"\$(ext $STATE/branches/$MAIN_C/upper/big)\" = \"\$(ext $STATE/snaps/$SNAP/upper/big)\" ]"
P2=$(spawn --ws c)
XDIR=$WC x $P2 "head -c 32M /dev/urandom >big2"
CS=$(afs proc changeset $P2 --author t -m big2 | j "d['changeset']")
SNAP=$(snap_of --proc $P2)
XDIR=$WC x $P2 "head -c 32M /dev/urandom >big2"
afs proc changeset $P2 --author t -m big2-again >/dev/null
afs reset $CS >/dev/null
ok 5.10a "reset on a running process's branch: same" bash -c "
	[ -n \"\$(ext $STATE/snaps/$SNAP/upper/big2)\" ] &&
	[ \"\$(ext $STATE/branches/$(branch $P2 c)/upper/big2)\" = \"\$(ext $STATE/snaps/$SNAP/upper/big2)\" ]"
quit $P2 >/dev/null
finish_test
