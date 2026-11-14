#!/bin/bash
# session.create with end_on_exit: the session ends (merges) once the creating process
# exits, also across daemon restarts; other sessions are unaffected.
. "$(dirname "$0")/lib.sh"
setup
stop_daemon
DARGS=(--auto-quiet-ms 300 --auto-max-ms 1500)
start_daemon "${DARGS[@]}"

D=$(cd "$(dirname "$0")" && pwd)
export TRACEFS_SOCKET=$SOCK PYTHONDONTWRITEBYTECODE=1
state() { afs session list | j "[s['state'] for s in d if s['id']=='$1'][0]"; }
until_state() { for _ in $(seq 1 50); do [ "$(state "$1")" = "$2" ] && return 0; sleep 0.2; done; return 1; }
export -f state until_state

# holder N OUT: a process that creates N bound sessions on lv one after another (merging
# the previous one first), writes their ids to OUT, then waits to be killed.
holder() {
	python3 - "$1" >"$2" <<PY &
import sys, time
sys.path.insert(0, "$D/../sdk/python/src")
from tracefs_sdk import Client
c, prev = Client(), None
for _ in range(int(sys.argv[1])):
    if prev:
        c.call("session.merge", {"session": prev})
    prev = c.call("session.create", {"ws": ["lv"], "end_on_exit": True})["id"]
    print(prev, flush=True)
time.sleep(3600)
PY
	HP=$!
	for _ in $(seq 1 50); do [ "$(wc -l <"$2")" -ge "$1" ] && return 0; sleep 0.1; done
	return 1
}

W=$(mkws lv)
afs ws register lv --from "$W" >/dev/null

holder 2 $T/h1
S1=$(head -1 $T/h1) S2=$(tail -1 $T/h1)
sleep 2
ok 10.1 "a bound session stays active while its process runs" bash -c "[ \$(state $S2) = active ]"
ok 10.1a "the same process can merge one session and bind the next" bash -c "[ \$(state $S1) = merged ]"
echo bound >$W/bound.txt
kill -9 $HP
wait $HP 2>/dev/null
ok 10.2 "the session merges once its process exits" until_state $S2 merged
ok 10.2a "writes before the exit are in the session" bash -c "afs log --session $S2 | j \"[c['id'] for c in d]\" | grep -q ."

holder 1 $T/h2
S3=$(cat $T/h2)
stop_daemon
start_daemon "${DARGS[@]}"
sleep 2
ok 10.3 "after a daemon restart the session is still bound and active" bash -c "[ \$(state $S3) = active ]"
kill -9 $HP
wait $HP 2>/dev/null
ok 10.3a "and merges when its process exits" until_state $S3 merged

holder 1 $T/h3
S4=$(cat $T/h3)
stop_daemon
kill -9 $HP
wait $HP 2>/dev/null
start_daemon "${DARGS[@]}"
ok 10.4 "a process that exited while the daemon was down ends its session at start" until_state $S4 merged

S5=$(afs session create --ws lv | j "d['id']")
sleep 2
ok 10.5 "an unbound session outlives the connection that created it" bash -c "[ \$(state $S5) = active ]"
afs session merge $S5 >/dev/null

finish_test
