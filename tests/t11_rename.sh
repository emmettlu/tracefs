#!/bin/bash
# Merges follow renames (AGENTS.md 5.7): edits, additions and deletions on one side land
# where the other side moved the file or directory.
. "$(dirname "$0")/lib.sh"
setup

W=$(mkws w)
mkdir -p $W/rtree/sub $W/d && echo t >$W/rtree/sub/t && echo n >$W/d/keep
seq 1 50 >$W/a.txt && seq 1 50 >$W/x.txt && seq 1 50 >$W/sw1 && echo two >$W/sw2 && echo f >$W/f && echo g >$W/g
chown -R 1000:1000 $W
afs ws register w --from $W >/dev/null
fin() { afs proc merge "$@" >$T/fin.json 2>&1; echo $?; }
conflicts() { j "[(c['path'], c['reason']) for r in d['results'] for c in r.get('conflicts', [])]" <$T/fin.json; }
pair() { # pair OPS1 OPS2: two processes, the first merges first; prints the second's merge code (its id in $T/p2)
	local p1 p2
	p1=$(spawn); p2=$(spawn)
	echo $p2 >$T/p2
	x $p1 "$1"; x $p2 "$2"
	afs proc merge $p1 >/dev/null
	fin $p2 --no-resolver
}
look() { bash -c "cd $W && $*"; } # main is the directory
drop2() { afs proc merge $(cat $T/p2) --discard-ws w >/dev/null; }
export -f fin conflicts pair look drop2
export W

ok 11.1 "dir renamed, then an edit inside merges into the new path" bash -c "[ \$(pair 'mv rtree rtree2' 'echo e >>rtree/sub/t') = 0 ]"
ok 11.1a "the edit is at rtree2/sub/t, rtree is gone" look "[ \"\$(cat rtree2/sub/t)\" = \"\$(printf 't\ne')\" ] && [ ! -e rtree ]"
ok 11.2 "edit inside merged first, then the dir rename" bash -c "[ \$(pair 'echo e2 >>rtree2/sub/t' 'mv rtree2 rtree3') = 0 ]"
ok 11.2a "the rename moved the edited file along" look "grep -qx e2 rtree3/sub/t && [ ! -e rtree2 ]"
ok 11.3 "file renamed vs edited" bash -c "[ \$(pair 'mv a.txt b.txt' 'sed -i s/^5\$/five/ a.txt') = 0 ]"
ok 11.3a "the edit is in b.txt, a.txt is gone" look "grep -qx five b.txt && [ ! -e a.txt ]"
ok 11.4 "renamed and edited in place vs edited: three-way merge at the new path" bash -c "
	[ \$(pair 'mv b.txt c.txt && echo tail >>c.txt' 'sed -i s/^40\$/forty/ b.txt') = 0 ]"
ok 11.4a "c.txt has both edits" look "grep -c '^tail\$\|^forty\$\|^five\$' c.txt | grep -qx 3 && [ ! -e b.txt ]"
# sed -i saves a new file: renamed and saved that way in one step, the file is not
# recognised (neither identity nor content match), so it is merged path by path.
ok 11.4b "renamed and replaced by a save vs edited: delete/modify" bash -c "
	[ \$(pair 'mv c.txt e.txt && sed -i s/^20\$/twenty/ e.txt' 'sed -i s/^30\$/thirty/ c.txt') = 2 ] && conflicts | grep -q \"'c.txt', 'delete/modify'\""
drop2
ok 11.5 "file added to a directory the other side renamed" bash -c "[ \$(pair 'mv d d2' 'echo new >d/new') = 0 ]"
ok 11.5a "d2 has the new file" look "[ \"\$(cat d2/new)\" = new ] && [ -e d2/keep ] && [ ! -e d ]"
ok 11.6 "renamed differently on both sides: rename/rename" bash -c "[ \$(pair 'mv f f1' 'mv f f2') = 2 ] && conflicts | grep -q \"'f2', 'rename/rename'\""
P6=$(cat $T/p2)
echo '[{"ws":"w","path":"f2","action":"theirs"}]' >$T/r6.json
ok 11.6a "resolution theirs takes the second process's name" bash -c "[ \$(fin $P6 --resolutions $T/r6.json) = 0 ]"
ok 11.6b "only f2 is left" look "[ -e f2 ] && [ ! -e f1 ] && [ ! -e f ]"
ok 11.7 "renamed vs deleted: rename/delete" bash -c "[ \$(pair 'rm g' 'mv g g2') = 2 ] && conflicts | grep -q \"'g2', 'rename/delete'\""
drop2
ok 11.8 "files swapped vs one of them edited" bash -c "[ \$(pair 'mv sw1 tmp && mv sw2 sw1 && mv tmp sw2' 'sed -i s/^3\$/three/ sw1') = 0 ]"
ok 11.8a "the edit followed the file to sw2" look "grep -qx three sw2 && [ \"\$(cat sw1)\" = two ]"
ok 11.9 "editor-style save (new file renamed over) vs edit: still a text merge" bash -c "
	[ \$(pair 'cp x.txt x.tmp && sed -i s/^1\$/one/ x.tmp && mv x.tmp x.txt' 'sed -i s/^50\$/fifty/ x.txt') = 0 ]"
ok 11.9a "x.txt has both edits" look "grep -c '^one\$\|^fifty\$' x.txt | grep -qx 2"

# A merge brings a rename into a running process, which keeps its own edit.
U=$(spawn); V=$(spawn)
x $U "echo u >>d2/keep"
x $V "mv d2 d3"
afs proc merge $V >/dev/null
afs proc merge $U >$T/upd.json
ok 11.10 "the edit lands at the renamed path in main" bash -c "grep -qx u $W/d3/keep && [ ! -e $W/d2 ]"
ok 11.10a "and the update brings the rename into the process" bash -c "
	j \"d['updated'][0]['status']\" <$T/upd.json | grep -qx merged && x $U 'grep -qx u d3/keep' && ! x $U 'test -e d2'"

# Undo a rename after the renamed file was edited.
L=$(mkws lv)
seq 1 20 >$L/note && chown 1000:1000 $L/note
afs ws register lv --from $L >/dev/null
mv $L/note $L/note2
C1=$(afs changeset lv -m rename | j "d['changeset']")
echo later >>$L/note2
afs changeset lv -m edit >/dev/null
ok 11.11 "diff reports the file rename" bash -c "afs diff $C1 | j \"[(c['kind'], c.get('from')) for c in d[0]['changes'] if c['path']=='note2']\" | grep -qx \"\\[('rename', 'note')\\]\""
ok 11.11a "revert of the rename keeps the later edit" bash -c "afs revert $C1 | j \"d['conflict']\" | grep -qx False"
ok 11.11b "note is back with the edit, note2 is gone" bash -c "tail -1 $L/note | grep -qx later && [ ! -e $L/note2 ]"
mkdir -p $L/dd && echo z >$L/dd/z && chown -R 1000:1000 $L/dd
afs changeset lv -m dd >/dev/null
mv $L/dd $L/ee
C2=$(afs changeset lv -m mvdir | j "d['changeset']")
echo w >$L/ee/w && chown 1000:1000 $L/ee/w
afs changeset lv -m add >/dev/null
ok 11.12 "revert of a directory rename keeps a file added inside later" bash -c "afs revert $C2 | j \"d['conflict']\" | grep -qx False && [ -e $L/dd/w ] && [ -e $L/dd/z ] && [ ! -e $L/ee ]"
finish_test
