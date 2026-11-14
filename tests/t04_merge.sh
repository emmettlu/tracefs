#!/bin/bash
# Merging processes into main (acfs proc merge, exit): concurrency, text merge, conflicts,
# resolutions, the resolver, updates of a running process, session labels.
. "$(dirname "$0")/lib.sh"
setup

W=$(mkws w)
seq 1 100 >$W/shared.txt
head -c 4096 /dev/urandom >$W/blob.bin
mkdir -p $W/keep $W/tree/sub $W/rtree/sub && echo k >$W/keep/k && echo t >$W/tree/sub/t && echo t >$W/rtree/sub/t && echo x >$W/xat
chown -R $AGENT:$AGENT $W
afs ws register w --from $W >/dev/null
OUT=$T/outside && mkdir -p $OUT && chmod 777 $OUT
fin() { afs proc merge "$@" >$T/fin.json 2>&1; echo $?; } # the task is done: merge now
conflicts() { j "[(c['path'], c['reason']) for r in d['results'] for c in r.get('conflicts', [])]" <$T/fin.json; }
export -f fin conflicts
export W OUT

# 4.1 eight processes, disjoint edits, all exiting at once.
NAT=$T/native && cp -a $W $NAT
for k in 1 2 3 4 5 6 7 8; do
	P[$k]=$(spawn)
	ops="echo p$k >>src/f$k.txt && echo new$k >new_p$k.txt && rm src/f$((k + 10)).txt && mkdir -p gen/p$k && seq 1 $k >gen/p$k/data"
	x ${P[$k]} "$ops"
	(cd $NAT && bash -c "$ops")
done
ok 4.0 "main is untouched while they run" bash -c "[ ! -e $W/new_p1.txt ]"
t0=$(ms)
for k in 1 2 3 4 5 6 7 8; do touch $(tag_of ${P[$k]}).stop; done
for k in 1 2 3 4 5 6 7 8; do quit ${P[$k]} >/dev/null; done
t1=$(ms)
ok 4.1 "8 processes exiting together all merge ($((t1 - t0)) ms)" bash -c "for k in ${P[*]}; do [ \$(pstate \$k) = merged ] || exit 1; done"
ok 4.1a "main equals the union of all edits" bash -c "diff <(cd $W && find . -type f -exec sha1sum {} + | sort) <(cd $NAT && find . -type f -exec sha1sum {} + | sort)"

A=$(spawn); B=$(spawn)
x $A "sed -i 's/^10\$/ten/' shared.txt"
x $B "sed -i 's/^90\$/ninety/' shared.txt"
afs proc merge $A >/dev/null
ok 4.2 "text edits on different lines merge" bash -c "[ \$(fin $B) = 0 ]"
ok 4.2a "both edits are in main" bash -c "grep -c '^ten\$\|^ninety\$' $W/shared.txt | grep -qx 2"
ok 4.2b "the process that merged first does not have the other's edit yet" bash -c "! x $A 'grep -qx ninety shared.txt'"
ok 4.2c "a later merge of the first process updates it" bash -c "fin $A >/dev/null; x $A 'grep -qx ninety shared.txt' && j \"d['updated'][0]['status']\" <$T/fin.json | grep -qx merged"

C=$(spawn); D=$(spawn)
x $C "sed -i 's/^50\$/fifty-c/' shared.txt"
x $D "sed -i 's/^50\$/fifty-d/' shared.txt"
afs proc merge $C >/dev/null
ok 4.3 "same-line edit: merge exits 2 with a text conflict" bash -c "[ \$(fin $D --no-resolver) = 2 ] && conflicts | grep -q \"'shared.txt', 'text'\""
ok 4.3a "the branch is conflicted, the process keeps running with its version" bash -c "
	[ \$(bstate $D w) = conflicted ] && [ \$(pstate $D) = running ] && x $D 'grep -qx fifty-d shared.txt' && grep -qx fifty-c $W/shared.txt"
echo '[{"ws":"w","path":"shared.txt","action":"ours"}]' >$T/r_ours.json
ok 4.3b "resolution ours keeps main's version" bash -c "[ \$(fin $D --resolutions $T/r_ours.json) = 0 ] && grep -qx fifty-c $W/shared.txt && [ \$(bstate $D w) = open ]"
ok 4.3b2 "and the process is updated to it" x $D "grep -qx fifty-c shared.txt"

E=$(spawn); F=$(spawn)
x $E "sed -i 's/^60\$/sixty-e/' shared.txt"; x $F "sed -i 's/^60\$/sixty-f/' shared.txt"
afs proc merge $E >/dev/null
echo '[{"ws":"w","path":"shared.txt","action":"theirs"}]' >$T/r_theirs.json
ok 4.3c "resolution theirs takes the process's version" bash -c "[ \$(fin $F --resolutions $T/r_theirs.json) = 0 ] && grep -qx sixty-f $W/shared.txt"
G=$(spawn); H=$(spawn)
x $G "echo g >>keep/k"; x $H "echo h >>keep/k"
afs proc merge $G >/dev/null
printf 'resolved\n' >$T/content.txt
echo "[{\"ws\":\"w\",\"path\":\"keep/k\",\"action\":\"content\",\"content_path\":\"$T/content.txt\"}]" >$T/r_content.json
ok 4.3e "resolution content writes the given file" bash -c "[ \$(fin $H --resolutions $T/r_content.json) = 0 ] && [ \"\$(cat $W/keep/k)\" = resolved ]"
I=$(spawn)
echo '[{"ws":"w","path":"no/such","action":"ours"}]' >$T/r_unused.json
x $I "echo i >new_i"
ok 4.3g "unused resolutions are reported, merge still succeeds" bash -c "[ \$(fin $I --resolutions $T/r_unused.json) = 0 ] && grep -q 'no/such' $T/fin.json"

# Conflict kinds.
pair() { # pair "ops for first (merged)" "ops for second" -> second's merge exit code
	local p1 p2
	p1=$(spawn); p2=$(spawn)
	x $p1 "$1"; x $p2 "$2"
	afs proc merge $p1 >/dev/null
	fin $p2 --no-resolver
	afs proc merge $p2 --discard-ws w >/dev/null 2>&1
	touch $(tag_of $p1).stop $(tag_of $p2).stop
}
export -f pair
ok 4.4 "binary both modified -> binary conflict" bash -c "[ \$(pair 'echo a >>blob.bin' 'echo b >>blob.bin') = 2 ] && conflicts | grep -q binary"
ok 4.4a "delete vs modify -> delete/modify" bash -c "[ \$(pair 'rm xat' 'echo m >>xat') = 2 ] && conflicts | grep -q 'delete/modify'"
ok 4.4b "file vs directory -> type" bash -c "[ \$(pair 'rm keep/k && mkdir keep/k' 'echo kk >>keep/k') = 2 ] && conflicts | grep -q type"
ok 4.4c "both add same path differently -> add/add" bash -c "[ \$(pair 'echo 1 >aa' 'echo 2 >aa') = 2 ] && conflicts | grep -q add/add"
ok 4.4d "both add identical file -> no conflict" bash -c "[ \$(pair 'echo same >bb' 'echo same >bb') = 0 ]"
ok 4.4e "symlink retargeted differently -> symlink conflict" bash -c "[ \$(pair 'ln -s a sl' 'ln -s b sl') = 2 ] && conflicts | grep -qE 'add/add|symlink'"
ok 4.4f "delete dir vs add file inside -> subtree conflict" bash -c "[ \$(pair 'rm -rf tree' 'echo n >tree/sub/new') = 2 ] && conflicts | grep -q 'subtree\|parent deleted'"
ok 4.4g "rename dir vs edit inside -> merged below the new name (t11)" bash -c "[ \$(pair 'mv rtree rtree2' 'echo e >>rtree/sub/t') = 0 ]"
ok 4.4h "chmod vs content edit merge" bash -c "[ \$(pair 'chmod 600 src/f1.txt' 'echo z >>src/f1.txt') = 0 ]"
ok 4.4i "chmod differently on both sides -> metadata conflict" bash -c "[ \$(pair 'chmod 600 src/f2.txt' 'chmod 640 src/f2.txt') = 2 ] && conflicts | grep -q metadata"

# Symlink escape: main replaces a directory by a symlink to a host path; a process adds a
# file below the old directory.
ok 4.5 "symlink parent in target -> conflict, nothing written outside" bash -c "
	[ \$(pair 'rm -rf src && ln -s $OUT src' 'echo evil >src/evil') = 2 ] && conflicts | grep -q 'parent' && [ ! -e $OUT/evil ]"

J=$(spawn)
x $J "echo dropped >dropped"
ok 4.6 "merge --discard-ws drops the work, the process goes on" bash -c "
	afs proc merge $J --discard-ws w | j \"(d['state'], d['results'][0]['status'])\" | grep -qx \"('running', 'discarded')\""
ok 4.6a "the discarded change is neither in main nor in the process any more" bash -c "[ ! -e $W/dropped ] && ! x $J 'test -e dropped'"
quit $J >/dev/null
ok 4.6b "an exited process with nothing left merges" test "$(pstate $J)" = merged

# Ownership, modes, xattrs survive the merge.
K=$(spawn)
xr $K "touch u5 && chown 5:6 u5 && chmod 4750 u5 && mkfifo ff && ln -s target lnk"
python3 -c "import os; os.setxattr('$RUN/mnt/$(branch $K w)/u5', 'user.tag', b'v')"
afs proc merge $K >/dev/null
ok 4.7 "owner 5:6, mode 4750 and user xattr preserved through the merge" bash -c "
	f=$W/u5 && [ \"\$(stat -c %u:%g:%a \$f)\" = 5:6:4750 ] && getfattr --only-values -n user.tag \$f | grep -qx v &&
	[ -p $W/ff ] && [ \"\$(readlink $W/lnk)\" = target ]"

# Update: main -> a running process at its merge, keeping the process's own edits.
U=$(spawn); V=$(spawn)
x $U "echo mine >mine.txt && sed -i 's/^20\$/twenty-u/' shared.txt"
x $V "echo theirs >theirs.txt && sed -i 's/^30\$/thirty-v/' shared.txt"
afs proc merge $V >/dev/null
afs proc merge $U >$T/upd.json
ok 4.8 "merge brings main's other changes into the running process" bash -c "
	j \"(d['results'][0]['status'], d['updated'][0]['status'])\" <$T/upd.json | grep -qx \"('merged', 'merged')\" &&
	x $U 'grep -qx theirs theirs.txt && grep -qx mine mine.txt && grep -c \"^twenty-u\$\|^thirty-v\$\" shared.txt' | grep -qx 2"
ok 4.8a "a second merge with nothing new is a no-op" bash -c "afs proc merge $U | j \"(d['results'][0]['status'], d['updated'])\" | grep -qx \"('up-to-date', \\[\\])\""
x $U "echo more >>mine.txt"
ok 4.8b "a merge after the update does not re-apply main's changes" bash -c "[ \$(fin $U) = 0 ] && j \"d['results'][0]['status']\" <$T/fin.json | grep -qx merged &&
	[ \$(grep -c thirty-v $W/shared.txt) = 1 ] && [ \"\$(cat $W/mine.txt)\" = \"\$(printf 'mine\\nmore')\" ]"
echo host >$W/host.txt
ok 4.8c "host edits of main are recorded before a merge writes" bash -c "fin $V >/dev/null; afs log --ws w -n 3 | j \"[x['message'] for x in d]\" | grep -q 'before merging process $V' && x $V 'grep -qx host host.txt'"

# Exit with a conflict: the process stays conflicted with its branch until settled.
U2=$(spawn); V2=$(spawn)
x $U2 "sed -i 's/^70\$/u/' shared.txt"; x $V2 "sed -i 's/^70\$/v/' shared.txt"
afs proc merge $V2 >/dev/null
rc=$(quit $U2)
ok 4.9 "acfs run of a process that exits with a conflict still returns its status" test "$rc" = 0
ok 4.9a "it is conflicted and keeps its branch, main is unchanged" bash -c "
	[ \$(pstate $U2) = conflicted ] && [ -d $STATE/branches/\$(branch $U2 w) ] && grep -qx v $W/shared.txt"
ok 4.9b "merge again: still a conflict (exit 2)" bash -c "[ \$(fin $U2 --no-resolver) = 2 ]"
ok 4.9c "with resolution theirs it merges and is done" bash -c "[ \$(fin $U2 --resolutions $T/r_theirs.json) = 0 ] && grep -qx u $W/shared.txt && [ \$(pstate $U2) = merged ]"
bad 4.9d "a merged process cannot be merged again" afs proc merge $U2

# The resolver: a third agent that settles conflicts at merge time.
cat >$T/ag/resolver.sh <<EOF
#!/bin/sh
cp conflicts.json $T/ag/conflicts.json
id -u >$T/ag/resolver.uid
python3 - <<'PY'
import json
out = []
for c in json.load(open("conflicts.json")):
    if c["reason"] == "text":
        ours, theirs = open(c["ours"]).read(), open(c["theirs"]).read()
        open(f"{c['id']}/result", "w").write(ours.replace("eighty-o\n", "eighty-o+t\n"))
        out.append({"id": c["id"], "action": "content"})
json.dump(out, open("resolutions.json", "w"))
PY
EOF
chmod 755 $T/ag/resolver.sh
O=$(spawn); R=$(spawn --resolver $T/ag/resolver.sh)
x $O "sed -i 's/^80\$/eighty-o/' shared.txt"; x $R "sed -i 's/^80\$/eighty-r/' shared.txt"
afs proc merge $O >/dev/null
ok 4.10 "a conflict goes to the resolver, which settles it" bash -c "[ \$(fin $R) = 0 ] && grep -qx eighty-o+t $W/shared.txt &&
	j \"d['resolver']\" <$T/fin.json | grep -q 'resolved 1 of 1'"
ok 4.10a "it ran as the caller with base, ours and theirs" bash -c "[ \$(cat $T/ag/resolver.uid) = $AGENT ] &&
	j \"[(c['path'], c['reason'], c['base'] is not None, c['ours'] is not None, c['theirs'] is not None, c['ws']) for c in d]\" <$T/ag/conflicts.json |
	grep -qx \"\\[('shared.txt', 'text', True, True, True, 'w')\\]\""
ok 4.10b "the process was updated to the resolved file" x $R "grep -qx eighty-o+t shared.txt"
O2=$(spawn); R2=$(spawn --resolver $T/ag/resolver.sh)
x $O2 "echo o >>blob.bin"; x $R2 "echo r >>blob.bin"
afs proc merge $O2 >/dev/null
ok 4.10c "what the resolver leaves out stays a conflict" bash -c "[ \$(fin $R2) = 2 ] && j \"d['resolver']\" <$T/fin.json | grep -q 'resolved 0 of 1'"
ok 4.10d "--no-resolver reports without running it" bash -c "rm -f $T/ag/resolver.uid; [ \$(fin $R2 --no-resolver) = 2 ] && [ ! -e $T/ag/resolver.uid ] && j \"d.get('resolver')\" <$T/fin.json | grep -qx None"
afs proc merge $R2 --discard-ws w >/dev/null
O3=$(spawn); R3=$(spawn --resolver $T/ag/resolver.sh)
x $O3 "sed -i 's/^81\$/eighty-o/' shared.txt"; x $R3 "sed -i 's/^81\$/eighty-one-r/' shared.txt"
afs proc merge $O3 >/dev/null
quit $R3 >/dev/null
ok 4.10e "the resolver also runs when the process exits" bash -c "[ \$(pstate $R3) = merged ] && [ \$(grep -c '^eighty-o+t\$' $W/shared.txt) = 2 ]"

# Session labels: a process's work in a labelled workspace belongs to that session, and one
# process can change workspaces of different sessions.
W2=$(mkws w2)
afs ws register w2 --from $W2 >/dev/null
S1=$(afs session create --ws w | j "d['id']")
S2=$(afs session create --ws w2 | j "d['id']")
L=$(spawn --ws w --ws w2)
XDIR=/ x $L "echo l >$W/l1.txt && echo l >$W2/l2.txt"
LC=$(afs proc changeset $L --author t -m both | j "d['changeset']")
afs proc merge $L >/dev/null
ok 4.11 "one changeset of the process is in both sessions' logs" bash -c "
	afs log --session $S1 | j \"[c['id'] for c in d]\" | grep -q $LC && afs log --session $S2 | j \"[c['id'] for c in d]\" | grep -q $LC"
ok 4.11a "the merges into main are labelled by each workspace's session" bash -c "
	afs log --session $S1 | j \"[c['message'] for c in d if c['owner'] == '$L']\" | grep -q 'merge process' &&
	afs log --session $S2 | j \"[c['message'] for c in d if c['owner'] == '$L']\" | grep -q 'merge process'"
ok 4.11b "revert --session undoes the process's work in that session's workspace only" bash -c "
	afs revert --session $S2 >/dev/null && [ ! -e $W2/l2.txt ] && [ -e $W/l1.txt ]"
ok 4.11c "session merge ends the labels" bash -c "afs session merge $S1 | j \"d['state']\" | grep -qx merged && afs session merge $S2 | j \"d['state']\" | grep -qx merged"

# Large file merge through the overlay.
Z=$(spawn --ws w)
x $Z "head -c 1G /dev/urandom >big1g"
t0=$(ms); afs proc merge $Z >/dev/null; t1=$(ms)
res 4.12 INFO "merge of a new 1 GiB file into main: $((t1 - t0)) ms"
MB=$(afs ws list | j "[x['main_branch'] for x in d if x['name']=='w'][0]")
excl=$(btrfs filesystem du --raw $STATE/branches/$MB/upper/big1g 2>/dev/null | awk 'NR==2{print $2}')
res 4.12a INFO "exclusive bytes of the merged 1 GiB file in main: $excl (0 = reflinked)"
finish_test
