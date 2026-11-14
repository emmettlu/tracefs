#!/bin/bash
# Workspaces: the host directory is mounted, every change there is recorded, sessions
# label stretches of history, reset/revert/discard write through the mount.
. "$(dirname "$0")/lib.sh"
setup
stop_daemon
DARGS=(--auto-quiet-ms 300 --auto-max-ms 1500)
start_daemon "${DARGS[@]}"

wsf() { afs ws list | j "[x['$2'] for x in d if x['name']=='$1'][0]"; } # wsf WS FIELD
ncs() { afs log --ws "$1" -n 100000 | j "len(d)"; }
settle() { # wait until WS has no unrecorded writes and its auto snapshot finished
	for _ in $(seq 1 50); do
		sleep 0.2
		[ "$(wsf "$1" pending)" = False ] && sleep 0.5 && return 0
	done
	return 1
}
state() { afs session list | j "[s['state'] for s in d if s['id']=='$1'][0]"; }
mounted() { [ "$(findmnt -n -o FSTYPE --mountpoint "$1")" = overlay ]; }
export -f wsf ncs settle state mounted

W=$(mkws lv)
echo 1 >$W/s.txt && chown 1000:1000 $W/s.txt
fp $W >$T/fp.before
afs ws register lv --from $W >/dev/null
ok 7.1 "register mounts an overlay over the directory" mounted $W
ok 7.1a "content unchanged through the mount" bash -c "fp $W | diff $T/fp.before -"
ok 7.1b "original content set aside and removed" bash -c "! ls -A $T/src | grep -q tracefs"
ok 7.1c "ws list reports it attached" bash -c "[ \$(wsf lv live) = True ]"
bad 7.1d "path inside a workspace rejected" afs ws register lvsub --from $W/src
bad 7.1e "same path twice rejected" afs ws register lv2 --from $W
ok 7.1f "rejections left one mount and one branch" bash -c "[ \$(findmnt -n --mountpoint $W | wc -l) = 1 ] && [ \$(ls $STATE/branches | wc -l) = 1 ]"

N0=$(ncs lv)
echo hi >$W/new.txt
settle lv
ok 7.2 "a write is snapshotted automatically" bash -c "[ \$(ncs lv) = $((N0 + 1)) ] && afs log --ws lv -n 1 | j \"d[0]['author']\" | grep -qx auto"
ok 7.2a "the auto changeset holds the write" bash -c "afs diff \$(wsf lv head_snap) | grep -q new.txt"
N1=$(ncs lv)
sleep 2
ok 7.2b "no writes, no changesets" bash -c "[ \$(ncs lv) = $N1 ]"
H1=$(wsf lv head_snap)
rm $W/README && mv $W/docs $W/docs2 && chmod 600 $W/src/f1.txt && mkdir $W/e && ln -s ../s.txt $W/e/link
settle lv
ok 7.2c "delete, rename, chmod, mkdir and symlink recorded" bash -c "afs diff $H1 \$(wsf lv head_snap) |
	j \"sorted(c['path'] for x in d for c in x['changes'])\" | grep -q \"'README'.*'docs2'.*'e/link'.*'src/f1.txt'\""

N2=$(ncs lv)
(for i in $(seq 1 40); do echo $i >>$W/stream.txt; sleep 0.1; done) &
WR=$!
sleep 3
ok 7.3 "continuous writes are snapshotted at least every --auto-max-ms" bash -c "[ \$(ncs lv) -gt $N2 ]"
wait $WR
settle lv

echo t >$W/tool.txt
out=$(afs changeset lv --author claude -m "tool call" --tool-call tc1)
ok 7.4 "manual changeset records pending writes with author and tool call" bash -c "
	[ \"\$(echo '$out' | j \"d['changeset']\")\" != None ] && afs log --ws lv -n 1 | j \"(d[0]['author'], d[0]['tool_call'])\" | grep -qx \"('claude', 'tc1')\""
ok 7.4a "nothing pending: unchanged" bash -c "afs changeset lv | j \"d['unchanged']\" | grep -qx True"
bad 7.4b "changeset of an unknown workspace rejected" afs changeset nope

S=$(afs session create --ws lv --name s1 | j "d['id']")
START=$(afs session list | j "[s['live'][0]['start_snap'] for s in d if s['id']=='$S'][0]")
ok 7.5 "session starts at the current head" bash -c "[ $START = \$(wsf lv head_snap) ] && [ \$(wsf lv session) = $S ]"
bad 7.5a "a workspace carries one active session" afs session create --ws lv
bad 7.5b "an unknown workspace cannot be labelled" afs session create --ws nope
echo 2 >$W/s.txt && echo b >$W/b.txt
afs changeset --session $S -m step >/dev/null
echo b2 >>$W/b.txt
settle lv
ok 7.5c "manual and auto changesets carry the session" bash -c "
	afs log --session $S | j \"sorted(set((x['author'], x['session']) for x in d))\" | grep -q \"('auto', '$S').*('user', '$S')\""
ok 7.6 "session merge ends the label" bash -c "afs session merge $S | j \"(d['state'], d['live'][0]['end_snap'] is not None)\" | grep -qx \"('merged', True)\""
echo after >$W/c.txt
settle lv
ok 7.6a "changes after the session are unlabelled" bash -c "afs log --ws lv -n 1 | j \"d[0]['session']\" | grep -qx None"
ok 7.7 "revert --session undoes only the session's changes" bash -c "
	afs revert --session $S >/dev/null && [ \"\$(cat $W/s.txt)\" = 1 ] && [ ! -e $W/b.txt ] && [ \"\$(cat $W/c.txt)\" = after ]"

printf 'x\ny\n' >$W/conf.txt
settle lv
sed -i 's/^x$/x1/' $W/conf.txt
settle lv
C=$(afs log --ws lv -n 1 | j "d[0]['id']")
sed -i 's/^x1$/x2/' $W/conf.txt
settle lv
afs revert $C >$T/rv.json 2>&1
rc=$?
ok 7.8 "revert conflicting with a later edit: exit 2, nothing written" bash -c "[ $rc = 2 ] && grep -q conf.txt $T/rv.json && grep -qx x2 $W/conf.txt"
ok 7.8a "revert of a changeset without conflicts" bash -c "afs revert \$(afs log --ws lv -n 1 | j \"d[0]['id']\") >/dev/null && grep -qx x1 $W/conf.txt"
F=$(wsf lv head_snap)
echo r1 >$W/r1.txt
settle lv
echo r2 >$W/r2.txt
settle lv
TO=$(wsf lv head_snap)
echo later >$W/later.txt
settle lv
ok 7.8b "revert --from --to undoes every changeset in between, not later ones" bash -c "
	afs revert --from $F --to $TO >/dev/null && [ ! -e $W/r1.txt ] && [ ! -e $W/r2.txt ] && grep -qx later $W/later.txt"
bad 7.8c "revert --from needs --to" afs revert --from $F

bash -c "cd $W/src && sleep 3 && ls >/dev/null && echo alive >$T/alive" &
SH=$!
sleep 0.5
ok 7.9 "reset brings the content back" bash -c "afs reset $START >/dev/null && [ \"\$(cat $W/s.txt)\" = 1 ] && [ ! -e $W/c.txt ] && [ ! -e $W/conf.txt ]"
wait $SH
ok 7.9a "a shell inside the directory survives a reset" grep -qx alive $T/alive
ok 7.9b "reset is recorded as a new changeset" bash -c "afs log --ws lv -n 1 | j \"d[0]['message']\" | grep -qx 'reset to $START'"

S2=$(afs session create --ws lv | j "d['id']")
echo junk >$W/junk.txt
settle lv
ok 7.10 "session discard brings the workspace back to the session start" bash -c "afs session discard $S2 >/dev/null && [ ! -e $W/junk.txt ]"
ok 7.10a "the discarded work stays in history" bash -c "afs log --session $S2 | j \"len(d)\" | grep -qv '^0$'"

S3=$(afs session create --ws lv --idle-timeout 2 | j "d['id']")
echo idle >$W/idle.txt
ok 7.11 "an idle session ends by itself" bash -c "for i in \$(seq 1 60); do [ \$(state $S3) = merged ] && exit 0; sleep 0.2; done; exit 1"

(cd / && arun --ws lv -- sh -c "sleep 1; echo process >$W/ctr.txt") &
RP=$!
sleep 0.3
echo host >$W/host.txt
wait $RP
ok 7.12 "a process merges into the directory, host edits stay" bash -c "[ \"\$(cat $W/ctr.txt)\" = process ] && [ \"\$(cat $W/host.txt)\" = host ]"
ok 7.12a "the merge is the process's changeset, the host edit was recorded before it" bash -c "
	afs log --ws lv -n 1 | j \"(d[0]['owner_kind'], d[0]['message'].startswith('merge process'))\" | grep -qx \"('process', True)\" &&
	! afs diff \$(afs log --ws lv -n 1 | j \"d[0]['id']\") | grep -q host.txt"

settle lv
fp $W >$T/fp.pre
ok 7.13 "promote the head of main" bash -c "afs base promote \$(wsf lv head_snap) >/dev/null && [ \$(wsf lv layers) = 2 ]"
ok 7.13a "still mounted, content unchanged" bash -c "mounted $W && fp $W | diff $T/fp.pre -"
echo post >$W/post.txt
settle lv
ok 7.13b "writes after promote are still recorded" bash -c "afs diff \$(wsf lv head_snap) | grep -q post.txt"
bash -c "cd $W && exec sleep 10" &
BUSY=$!
sleep 0.3
bad 7.13c "promote refused while the directory is in use" bash -c "afs base promote \$(wsf lv head_snap)"
ok 7.13d "and the directory stays mounted" mounted $W
kill $BUSY
wait $BUSY 2>/dev/null

stop_daemon
ok 7.14 "the mount survives a daemon restart" mounted $W
echo down >$W/down.txt
start_daemon "${DARGS[@]}"
settle lv
ok 7.14a "writes made while the daemon was down are recorded after restart" bash -c "afs diff \$(wsf lv head_snap) | grep -q down.txt"
N=$(ncs lv)
echo r >$W/restart.txt
settle lv
ok 7.14b "watching resumes after restart" bash -c "[ \$(ncs lv) -gt $N ]"

S4=$(afs session create --ws lv | j "d['id']")
stop_daemon
umount $W && umount -R $RUN # what a reboot leaves behind
start_daemon "${DARGS[@]}"
ok 7.15 "after a reboot the workspace is mounted again" mounted $W
ok 7.15a "and sessions are ended" bash -c "[ \$(state $S4) = merged ]"

settle lv
fp $W >$T/fp.head
bash -c "cd $W && exec sleep 10" &
BUSY=$!
sleep 0.3
bad 7.16 "detach refused while the directory is in use" afs ws detach lv
ok 7.16a "and it stays mounted and attached" bash -c "mounted $W && [ \$(wsf lv live) = True ]"
kill $BUSY
wait $BUSY 2>/dev/null
S5=$(afs session create --ws lv | j "d['id']")
bad 7.16b "detach refused while a session labels the workspace" afs ws detach lv
afs session merge $S5 >/dev/null
ok 7.16c "detach" afs ws detach lv
ok 7.16d "the directory is plain again and holds the head" bash -c "! findmnt --mountpoint $W >/dev/null && fp $W | diff $T/fp.head -"
ok 7.16e "no staging leftovers" bash -c "! ls -A $T/src | grep -q tracefs"
N=$(ncs lv)
echo gone >$W/gone.txt
sleep 1.5
ok 7.16f "writes after detach are not recorded" bash -c "[ \$(ncs lv) = $N ] && [ \$(wsf lv live) = False ]"

W2=$(mkws lv2)
bash -c "cd $W2 && exec sleep 30" &
ST=$!
sleep 0.3
out=$(afs ws register lv2 --from $W2)
ok 7.17 "processes left in the old directory are reported" bash -c "echo '$out' | j \"d['stale_processes']\" | grep -q sleep"
ok 7.17a "their cwd is gone instead of silently stale" bash -c "readlink /proc/$ST/cwd | grep -q deleted"
kill $ST
wait $ST 2>/dev/null

stop_daemon
export TRACEFS_FAILPOINT=merge-before-snapshot
start_daemon "${DARGS[@]}"
unset TRACEFS_FAILPOINT
H=$(wsf lv2 head_snap)
echo x >$W2/fp.txt
settle lv2
afs reset $H >/dev/null 2>&1 # the daemon aborts after writing, before the snapshot
sleep 0.5
start_daemon "${DARGS[@]}"
ok 7.18 "an interrupted reset is recorded on restart with its undo hint" bash -c "
	afs log --ws lv2 -n 1 | j \"d[0]['message']\" | grep -q \"interrupted merge; tracefs reset\" && [ ! -e $W2/fp.txt ]"

finish_test
