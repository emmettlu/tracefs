#!/bin/bash
# Thinning of workspace history (AGENTS.md 5.13): old automatic snapshots are kept one
# per hour, then one per day; pinned ones and hand-made changesets stay.
. "$(dirname "$0")/lib.sh"
setup
stop_daemon
start_daemon --retain-interval-secs 0

L=$(mkws lv)
afs ws register lv --from $L >/dev/null
auto() { echo "$1" >>$L/log.txt && afs changeset lv --author auto -m auto | j "d['changeset']"; }
snap_of() { afs log --ws lv -n 100000 | j "[s['snap'] for c in d if c['id']=='$1' for s in c['snaps']][0]"; }
present() { afs log --ws lv -n 100000 | j "[s['present'] for c in d if c['id']=='$1' for s in c['snaps']][0]"; }
export -f snap_of present
A=() B=() C=()
for i in 1 2 3; do A+=($(auto a$i)); done
S=$(afs session create --ws lv | j "d['id']")
for i in 4 5 6 7 8 9 10; do A+=($(auto a$i)); done
echo m >>$L/log.txt
M=$(afs changeset lv -m "by hand" | j "d['changeset']")
afs session merge $S >/dev/null
for i in $(seq 1 10); do B+=($(auto b$i)); done
for i in $(seq 1 10); do C+=($(auto c$i)); done
# Backdate: A in one day a month ago (one per hour), B in two hours three days ago.
python3 - "$STATE/meta.db" "${A[*]}" "$M" "${B[*]}" <<'PY'
import sqlite3, sys, time
db, a, m, b = sys.argv[1], sys.argv[2].split(), sys.argv[3], sys.argv[4].split()
H, D = 3600_000, 86400_000
now = int(time.time() * 1000)
day = now // D * D - 30 * D
hour = (now - 3 * D) // H * H
when = {cs: day + k * H for k, cs in enumerate(a)}
when[m] = day + 10 * H
when.update({cs: hour + k * 10 * 60_000 for k, cs in enumerate(b)})
c = sqlite3.connect(db, timeout=30)
for cs, t in when.items():
    c.execute("UPDATE changesets SET created_at = ? WHERE id = ?", (t, cs))
    c.execute("UPDATE snaps SET created_at = ? WHERE changeset = ?", (t, cs))
c.commit()
PY
# A: keep the last of the day (a10) and the session start (a3). B: the last of each hour (b6, b10).
EXP=$(for k in 0 1 3 4 5 6 7 8; do snap_of ${A[$k]}; done; for k in 0 1 2 3 4 6 7 8; do snap_of ${B[$k]}; done)
EXP=$(echo "$EXP" | sort | tr '\n' ' ')
afs ws prune lv --dry-run >$T/dry.json
ok 15.1 "dry run lists the expected 16 snapshots" bash -c "[ \"\$(j \"' '.join(sorted(d['dropped'])) + ' '\" <$T/dry.json)\" = '$EXP' ]"
ok 15.1a "and drops nothing" test "$(present ${A[0]})" = True
afs ws prune lv >/dev/null
ok 15.2 "prune drops them" bash -c "[ \$(present ${A[0]}) = False ] && [ \$(present ${B[0]}) = False ]"
ok 15.2a "kept: the last of each bucket, the session start, the hand-made changeset, recent ones" bash -c "
	for cs in ${A[2]} ${A[9]} $M ${B[5]} ${B[9]} ${C[*]}; do [ \$(present \$cs) = True ] || exit 1; done"
ok 15.3 "diff of a dropped changeset returns its summary" bash -c "afs diff ${A[4]} | j \"(d[0].get('summary_only'), [c['path'] for c in d[0]['changes']])\" | grep -qx \"(True, \\['log.txt'\\])\""
ok 15.3a "diff of a kept changeset whose parent was dropped too" bash -c "afs diff ${A[9]} | j \"d[0].get('summary_only')\" | grep -qx True"
bad 15.4 "revert of a dropped changeset is refused" afs revert ${A[4]}
ok 15.4a "revert of a kept one still works" bash -c "afs revert ${C[9]} | j \"d['conflict']\" | grep -qx False && ! grep -qx c10 $L/log.txt"
ok 15.5 "a second prune finds nothing" bash -c "afs ws prune lv | j \"d['dropped']\" | grep -qx '\\[\\]'"
# The daemon thins by itself.
for i in 1 2 3; do auto d$i >/dev/null; done
python3 - "$STATE/meta.db" <<'PY'
import sqlite3, sys, time
c = sqlite3.connect(sys.argv[1], timeout=30)
old = int(time.time() * 1000) - 40 * 86400_000
c.execute("UPDATE snaps SET created_at = ? WHERE id IN (SELECT s.id FROM snaps s JOIN changesets cs ON cs.id = s.changeset WHERE cs.message = 'auto' AND s.present = 1 ORDER BY s.created_at DESC LIMIT 3 OFFSET 1)", (old,))
c.commit()
PY
stop_daemon
start_daemon --retain-interval-secs 1
sleep 3
ok 15.6 "the periodic thinning drops old automatic snapshots" bash -c "afs log --ws lv -n 3 | j \"[s['present'] for c in d for s in c['snaps']]\" | grep -qx '\\[True, True, False\\]'"
finish_test
