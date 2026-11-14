#!/bin/bash
# Base promotion/flattening and push/pull.
. "$(dirname "$0")/lib.sh"
setup 16G
stop_daemon
start_daemon --max-layers 4

W=$(mkws w)
export W
mkdir -p $W/tree/a $W/old && echo t >$W/tree/a/t && echo o >$W/old/o && ln -s README $W/link && echo x >$W/xat
setfattr -n user.k -v v $W/xat && chown -R 1000:1000 $W
afs ws register w --from $W >/dev/null
mainfp() { fp $W; } # main is the directory
viewfp() { # viewfp SNAP: fingerprint of a recorded state, through a read-only overlay of it
	local d=$STATE/snaps/$1 v=$T/view.$1 lower
	lower=$d/upper$(ls $d/l | sort -rn | sed "s|^|:$d/l/|" | tr -d '\n')
	mkdir -p $v && mount -t overlay overlay -o ro,lowerdir=$lower,redirect_dir=on,xino=auto $v && fp $v
	umount $v && rmdir $v
}
round() { # round N: one process changing main in many ways
	(cd / && arun --ws w -- bash -c "cd $W && echo r$1 >>README && echo n$1 >new$1 && rm -f new$(($1 - 1)) && mv tree tree$1 2>/dev/null || mv tree$(($1 - 1)) tree$1;
		rm -rf old && mkdir old && echo o$1 >old/o; chmod 6$(($1 % 8))0 src/f1.txt; ln -sf new$1 link; mkfifo fifo$1")
}
head_of() { afs ws list | j "[x['head_snap'] for x in d if x['name']=='w'][0]"; }
layers() { afs ws list | j "[x['layers'] for x in d if x['name']=='w'][0]"; }
export -f mainfp viewfp round head_of layers

round 1
mainfp >$T/fp1
H=$(head_of)
OLD_P=$(spawn --ws w)
ok 6.1 "promote main head" afs base promote $H
ok 6.1a "workspace has 2 layers" bash -c "[ \$(layers) = 2 ]"
mainfp >$T/fp1b
ok 6.1b "main content unchanged by promote" diff $T/fp1 $T/fp1b
bad 6.1c "promote of a non-head snapshot" afs base promote $H
ok 6.1d "promoted layer keeps whiteouts/opaque dirs (old/o is o1, tree1 renamed)" bash -c "grep -q \"^'tree1' \" $T/fp1b && ! grep -q \"^'tree' \" $T/fp1b"
x $OLD_P "echo from-old-process >oldsess"
ok 6.2 "a process forked before promote merges into main on the new base" bash -c "[ \$(quit $OLD_P) = 0 ] && [ \$(pstate $OLD_P) = merged ]"
ok 6.2a "its change is in main" bash -c "mainfp | grep -q \"^'oldsess' \""
for r in 2 3 4 5; do
	round $r
	afs base promote $(head_of) >/dev/null
done
ok 6.3 "promote beyond --max-layers 4 flattens to 1 layer" bash -c "[ \$(layers) -le 4 ]"
res 6.3a INFO "layers after 5 promotes with max 4: $(layers)"
mainfp >$T/fp5
ok 6.4 "explicit flatten" afs base flatten w
ok 6.4a "workspace has 1 layer" bash -c "[ \$(layers) = 1 ]"
mainfp >$T/fp5b
ok 6.4b "flattened content equals pre-flatten content (modes, xattrs, symlinks, fifos)" diff $T/fp5 $T/fp5b
FB=$(afs ws list | j "d[0]['base']")
ok 6.4c "flattened base has no overlay xattrs or whiteouts" bash -c "
	! getfattr -R -h -d -m '^trusted\.overlay' $STATE/bases/$FB 2>/dev/null | grep -q trusted && [ -z \"\$(find $STATE/bases/$FB -type c)\" ]"
ok 6.4d "a new process works on the flattened base" bash -c "round 6 && mainfp | grep -q \"^'new6' \""

# push / pull.
REMOTE=$T/remote
ok 6.5 "push to a remote directory" bash -c "afs push w --remote $REMOTE | j \"d['snaps_sent'] > 0\" | grep -qx True"
ok 6.5a "second push sends nothing" bash -c "afs push w --remote $REMOTE | j \"d['snaps_sent'] + d['bases_sent']\" | grep -qx 0"
round 7
ok 6.5b "incremental push sends only the new snapshots" bash -c "afs push w --remote $REMOTE | j \"0 < d['snaps_sent'] <= 3 and d['bases_sent'] == 0\" | grep -qx True"
mainfp >$T/fp7
H7=$(head_of)
# Lose everything locally and pull into a fresh daemon.
stop_daemon
umount -l $W
for m in $(findmnt -rn -o TARGET | grep -E "^$RUN(/|$)" | sort -r); do umount -l $m; done
umount $STATE && rm -f $T/state.img && mkstate && start_daemon --max-layers 4
ok 6.6 "pull into an empty state" bash -c "afs pull w --remote $REMOTE | j \"d['status']\" | grep -qx created"
ok 6.6a "pulled main head is the pushed head" bash -c "[ \$(head_of) = $H7 ]"
viewfp $H7 >$T/fp_pulled
ok 6.6b "pulled main is byte-identical" diff $T/fp7 $T/fp_pulled
ok 6.6c "the pulled workspace is detached: no process can branch it" bash -c "[ \$(afs ws list | j \"d[0]['live']\") = False ] && ! (cd / && arun --ws w -- true)"
ok 6.6e "pull again is up to date" bash -c "afs pull w --remote $REMOTE | j \"d['status']\" | grep -qx up-to-date"
# A second machine (fresh state) pulls, the first pushes more, the second fast-forwards.
cp -a $REMOTE $T/remote2
f=$(ls $T/remote2/w/streams | head -1)
printf 'X' | dd of=$T/remote2/w/streams/$f bs=1 seek=100 conv=notrunc status=none
stop_daemon
for m in $(findmnt -rn -o TARGET | grep -E "^$RUN(/|$)" | sort -r); do umount -l $m; done
umount $STATE && rm -f $T/state.img && mkstate && start_daemon --max-layers 4
bad 6.7 "corrupted stream is rejected by checksum" afs pull w --remote $T/remote2
ok 6.7a "nothing half-registered after the failed pull" bash -c "[ \$(afs ws list | j 'len(d)') = 0 ] && [ -z \"\$(find $STATE/bases $STATE/snaps -mindepth 1 -maxdepth 1)\" ]"
ok 6.8 "pull from the good remote" bash -c "afs pull w --remote $REMOTE | j \"d['status']\" | grep -qx created"
finish_test
