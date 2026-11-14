#!/bin/bash
# acfs run: the process's own branches, isolation, exit status and signals, failure cleanup.
. "$(dirname "$0")/lib.sh"
setup

W1=$(mkws w1)
W2=$(mkws w2)
afs ws register w1 --from "$W1" >/dev/null
afs ws register w2 --from "$W2" >/dev/null
W=$W1
export W W1 W2

cd $W1
out=$(arun -- sh -c 'pwd; echo hi >hello; id -u')
rc=$?
ok 2.1 "run as the caller, in the caller's directory, exit 0" bash -c "[ $rc = 0 ] && [ \"\$(echo '$out' | head -1)\" = '$W1' ] && [ \"\$(echo '$out' | tail -1)\" = $AGENT ]"
ok 2.1a "its write is in main once it exited" grep -qx hi $W1/hello
ok 2.1b "and owned by the caller" test "$(stat -c %u $W1/hello)" = $AGENT
ok 2.1c "the process is merged, its branches dropped" bash -c "
	afs proc list | j \"[p['state'] for p in d]\" | grep -qx \"\\['merged'\\]\" && [ \$(ls $STATE/branches | wc -l) = 2 ]"
cd /

A=$(spawn)
B=$(spawn)
x $A "echo from-a >a_only; echo mod >>README"
ok 2.2 "A sees its own write" bash -c "x $A 'cat a_only' | grep -qx from-a"
bad 2.2a "B does not see A's unmerged write" x $B "test -e a_only"
ok 2.2b "B's README is unchanged" bash -c "[ \"\$(x $B 'cat README')\" = readme ]"
ok 2.2c "main does not see it either" test ! -e $W1/a_only
ok 2.3 "every workspace of the caller is a branch of the process" bash -c "afs proc show $A | j \"sorted(b['ws'] for b in d['branches'])\" | grep -qx \"\\['w1', 'w2'\\]\""
ok 2.3a "the branch is mounted for the daemon, the write is in its upper" test -e $STATE/branches/$(branch $A w1)/upper/a_only
ok 2.3b "the process does not see the daemon's mounts" bash -c "[ -z \"\$(XDIR=/ x $A 'ls -A $RUN/mnt')\" ]"
ok 2.3c "ACFS_PROC names the process inside it" bash -c "(cd / && arun -- sh -c 'echo \$ACFS_PROC') | grep -qE '^[0-9a-z]{26}\$'"
C=$(spawn --ws w1)
ok 2.3d "--ws limits the branches" bash -c "afs proc show $C | j \"[b['ws'] for b in d['branches']]\" | grep -qx \"\\['w1'\\]\""
x $C "echo shared >$W2/direct"
ok 2.3e "a workspace left out of --ws is main itself: its writes land there at once" grep -qx shared $W2/direct
ok 2.4 "process changeset holds one snapshot per branch" bash -c "afs proc changeset $A --author t -m m | j 'len(d[\"snaps\"])' | grep -qx 2"

# Inode uniqueness with a git repository (.git directory included).
RG=$(mkrepo rg)
afs ws register rg --from $RG >/dev/null
G=$(spawn --ws rg)
XDIR=$RG x $G "mkdir -p newdir/a && echo x >newdir/a/f && echo y >>README.md && mv crates crates2"
python3 - "$RUN/mnt/$(branch $G rg)" >$T/ino <<'EOF'
import os, sys, collections
seen = collections.Counter()
for d, dirs, files in os.walk(sys.argv[1]):
    for n in dirs + files:
        st = os.lstat(os.path.join(d, n)); seen[(st.st_dev, st.st_ino)] += 1
print(len(seen), sum(1 for v in seen.values() if v > 1))
EOF
read n dups <$T/ino
ok 2.5 "no (st_dev, st_ino) collisions in the view ($n entries incl. .git)" test "$dups" = 0
ok 2.5b "cp -a of the view copies everything (incl. .git)" bash -c "
	XDIR=$RG x $G 'cp -a $RG $T/ag/copy && find $T/ag/copy | wc -l' >$T/n1 && XDIR=$RG x $G 'find $RG | wc -l' >$T/n2 && cmp $T/n1 $T/n2 && rm -rf $T/ag/copy"
ok 2.5c "git works in the view" bash -c "XDIR=$RG x $G 'git status --porcelain' | grep -q crates2"

# Failure paths leave nothing behind.
X=$T/src/other && mkdir -p $X && echo x >$X/f && chown -R 1001:1001 $X
afs ws register other --from $X >/dev/null
before=$(ls $STATE/branches | wc -l)
# Read-only views (run/views) are unmounted in the background; count the other mounts.
mounts() { findmnt -rn -o TARGET | grep "^$RUN" | grep -vc "^$RUN/views/"; }
export -f mounts
mounts_before=$(mounts)
cd /
arun --ws nope -- true 2>$T/e; rc=$?
ok 2.7 "unknown workspace: exit 125, not found" bash -c "[ $rc = 125 ] && grep -q 'not found' $T/e"
arun --ws other -- true 2>$T/e; rc=$?
ok 2.7a "another uid's workspace: exit 125, not found" bash -c "[ $rc = 125 ] && grep -q 'not found' $T/e"
arun -- /no/such/binary 2>$T/e; rc=$?
ok 2.7b "missing command: exit 127 with the reason" bash -c "[ $rc = 127 ] && grep -q 'no/such/binary' $T/e"
mkdir -p $T/priv && chmod 700 $T/priv
(cd $T/priv && arun -- true 2>$T/e); rc=$?
ok 2.7c "a working directory the caller may not enter: exit 127" bash -c "[ $rc = 127 ] && grep -q 'cd $T/priv' $T/e"
ok 2.7g "failed starts left no branches or mounts" bash -c "
	[ \$(ls $STATE/branches | wc -l) = $before ] && [ \$(mounts) = $mounts_before ]"

# Exit status and signals.
ok 2.8 "exit status is passed through" bash -c "arun -- sh -c 'exit 3'; [ \$? = 3 ]"
ok 2.8a "killed by a signal: 128 + signal" bash -c "arun -- sh -c 'kill -USR1 \$\$'; [ \$? = 138 ]"
cd $W2
(arunx -- sh -c "trap 'echo bye >term; exit 7' TERM; while :; do sleep 0.1; done") &
CLI=$!
sleep 1
kill -TERM $CLI
wait $CLI
rc=$?
ok 2.8b "TERM to acfs run reaches the process; its exit status comes back" test $rc = 7
ok 2.8c "and what it wrote while stopping is merged" grep -qx bye $W2/term
(arunx -- sh -c "echo x >killed; exec sleep 600") &
CLI=$!
sleep 1
K=$(afs proc list | j "[p['id'] for p in d if p['state']=='running' and p['cmd'][-1].endswith('sleep 600')][0]")
KP=$(pid_of $K)
kill -KILL $CLI
wait $CLI 2>/dev/null
ok 2.8d "SIGKILL of acfs run stops the process group" bash -c "for i in \$(seq 1 50); do kill -0 $KP 2>/dev/null || exit 0; sleep 0.1; done; exit 1"
ok 2.8e "and its work is merged" bash -c "for i in \$(seq 1 50); do [ \$(pstate $K) = merged ] && grep -qx x $W2/killed && exit 0; sleep 0.1; done; exit 1"
cd /
P=$(spawn)
afs proc kill $P >$T/kill.json
ok 2.8f "proc kill signals the process" bash -c "j \"d['sent']\" <$T/kill.json | grep -qx True &&
	for i in \$(seq 1 50); do [ -e \$(tag_of $P).rc ] && break; sleep 0.1; done; [ \$(cat \$(tag_of $P).rc) = 143 ]"
ok 2.8g "proc kill of a finished process sends nothing" bash -c "afs proc kill $P | j \"d['sent']\" | grep -qx False"

# A caller without workspaces runs without branches.
mkdir -p $T/bin && cp "$BIN/acfs" $T/bin/ && chmod 755 $T/bin $T/bin/acfs
ok 2.9 "no workspaces: the command still runs" bash -c "setpriv --reuid=60001 --regid=60001 --clear-groups $T/bin/acfs --socket $SOCK run -- true"

# Daemon restart while a process runs.
cd $W1
(arunx -- sh -c "sleep 3; echo after >restart" >$T/rs.out 2>&1) &
CLI=$!
sleep 1
R=$(afs proc list | j "[p['id'] for p in d if p['state']=='running' and 'restart' in p['cmd'][-1]][0]")
stop_daemon
start_daemon
wait $CLI
rc=$?
cd /
ok 2.10 "acfs run waits for the process across a daemon restart (status unknown: 1)" bash -c "[ $rc = 1 ] && grep -q 'lost the daemon' $T/rs.out"
ok 2.10a "the restarted daemon merges its work" bash -c "for i in \$(seq 1 50); do [ \$(pstate $R) = merged ] && grep -qx after $W1/restart && exit 0; sleep 0.1; done; exit 1"

for p in $(afs proc list | j "' '.join(p['id'] for p in d if p['state']=='running')"); do quit $p >/dev/null; done
# Processes that outlived the restart are settled by the daemon after acfs run returned.
# (While it settles an exited process shows as exiting.)
for _ in $(seq 1 100); do [ "$(afs proc list | j "sum(p['state'] != 'merged' for p in d)")" = 0 ] && break; sleep 0.1; done
ok 2.11 "every process merged, no process branches or mounts left" bash -c "
	afs proc list | j \"set(p['state'] for p in d)\" | grep -qx \"{'merged'}\" && [ \$(ls $STATE/branches | wc -l) = \$(afs ws list | j 'len(d)') ] &&
	[ \$(findmnt -rn -o TARGET | grep -c '^$RUN/mnt/') = 0 ]"
ok 2.11a "no scopes left" bash -c "[ -z \"\$(systemctl list-units --plain --no-legend '$SLICE-p*.scope')\" ]"

finish_test
