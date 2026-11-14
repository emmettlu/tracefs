#!/bin/bash
# Callers are told apart by SO_PEERCRED: two non-root uids (no accounts needed) only
# see and act on their own workspaces, sessions and processes; root sees everything.
. "$(dirname "$0")/lib.sh"
setup
# The binary must be reachable by any uid (the build may sit in a private home).
mkdir -p $T/bin $T/ag && cp "$BIN/acfs" $T/bin/ && chmod 755 $T/bin $T/bin/acfs && chmod 777 $T/ag
stop_daemon
start_daemon --max-user-sessions 3

UA=60001 UB=60002
as() { local u=$1; shift; (cd / && setpriv --reuid=$u --regid=$u --clear-groups $T/bin/acfs --socket "$SOCK" "$@"); }
says() { local p=$1; shift; ! "$@" >$T/says.out 2>&1 && grep -q "$p" $T/says.out; } # fails, saying p
gone() { says 'not found' "$@"; }                                                     # refused as if absent
mine() { local d=$T/src/$1; mkdir -p "$d" && echo "$1" >"$d/README" && chown -R $2:$2 "$d" && echo "$d"; }

ok 9.1 "socket is world-connectable (0666)" test "$(stat -c %a $SOCK)" = 666
WA=$(mine wa $UA) WB=$(mine wb $UB) WL=$(mine wl $UB)
afs ws register wa --from $WA >/dev/null
afs ws register wb --from $WB >/dev/null
afs ws register wl --from $WL >/dev/null
ok 9.2 "ws list shows a user only its own workspaces" test "$(as $UA ws list | j "[w['name'] for w in d]")" = "['wa']"
ok 9.2a "root sees all of them" test "$(afs ws list | j 'len(d)')" = 3
ok 9.2b "a user registers a directory it owns" as $UA ws register wa2 --from "$(mine wa2 $UA)"
ok 9.2c "but not one owned by another uid" says 'owned by uid' as $UA ws register wb2 --from "$(mine wb2 $UB)"

HB=$(afs log --ws wb | j "d[0]['id']")
ok 9.3 "log of another user's workspace: not found" gone as $UA log --ws wb
ok 9.3a "diff of another user's changeset: not found" gone as $UA diff $HB
ok 9.3b "reset to another user's snapshot: not found" gone as $UA reset $HB
ok 9.3c "session over another user's workspace: not found" gone as $UA session create --ws wl
ok 9.3d "detach of another user's workspace: not found" gone as $UA ws detach wl
ok 9.3e "flatten of another user's workspace: not found" gone as $UA base flatten wb
ok 9.3f "a user may not pick a remote directory" says 'only root' as $UA push wa --remote $T/remote

SA=$(as $UA session create | j "d['id']")
ok 9.4 "session list shows a user only its own sessions" test "$(as $UB session list | j 'len(d)')" = 0
ok 9.4a "merge of another user's session: not found" gone as $UB session merge $SA
ok 9.4b "discard of another user's session: not found" gone as $UB session discard $SA
ok 9.4c "a process may not branch another user's workspace: not found" gone as $UB run --ws wa -- true

as $UA run -- sh -c "echo \$ACFS_PROC >$T/ag/pa.id; echo x >$WA/from-a; while [ ! -e $T/ag/pa.stop ]; do sleep 0.2; done" >/dev/null 2>&1 &
for _ in $(seq 1 100); do [ -s $T/ag/pa.id ] && break; sleep 0.05; done
PA=$(cat $T/ag/pa.id)
ok 9.5 "a user runs a process over its own workspaces only" test "$(afs proc show $PA | j "[b['ws'] for b in d['branches']]")" = "['wa', 'wa2']"
ok 9.5a "the process runs as that user" bash -c "[ \$(stat -c %u /proc/\$(afs proc show $PA | j \"d['pid']\")) = $UA ]"
ok 9.5b "show of another user's process: not found" gone as $UB proc show $PA
ok 9.5c "merge of another user's process: not found" gone as $UB proc merge $PA
ok 9.5c2 "changeset of another user's process: not found" gone as $UB proc changeset $PA --author x -m x
ok 9.5c3 "kill of another user's process: not found" gone as $UB proc kill $PA
ok 9.5d "proc list hides other users' processes" test "$(as $UB proc list | j 'len(d)')" = 0
ok 9.5e "root sees the process" test "$(afs proc show $PA | j "d['state']")" = running
ok 9.7 "the owner merges its process" test "$(as $UA proc merge $PA | j "(d['state'], d['results'][0]['status'])")" = "('running', 'merged')"
ok 9.7b "its write is in main" grep -qx x $WA/from-a
touch $T/ag/pa.stop
ok 9.7a "and its session merges" test "$(as $UA session merge $SA | j "d['state']")" = merged
for i in 1 2 3; do as $UA session create >/dev/null; done
ok 9.8 "per-user session limit" says 'limit 3' as $UA session create
ok 9.8a "other users are not affected" as $UB session create

finish_test
