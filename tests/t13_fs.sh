#!/bin/bash
# File access without a mount (AGENTS.md 5.16): fs.read/list/stat of main or a recorded state;
# fs.edit records one changeset per call.
. "$(dirname "$0")/lib.sh"
setup

W=$(mkws w)
afs ws register w --from $W >/dev/null
head_of() { afs ws list | j "[x['head_snap'] for x in d if x['name']=='$1'][0]"; }
ncs() { afs log --ws "$1" -n 100000 | j "len(d)"; }
st() { afs fs stat w "$1" | j "$2"; }
export -f head_of ncs st

ok 13.1 "cat reads main" test "$(afs fs cat w README)" = readme
ok 13.1a "ls lists a directory" bash -c "afs fs ls w | j \"sorted(e['path'] for e in d['entries'])\" | grep -qx \"\\['README', 'docs', 'src'\\]\""
ok 13.1b "stat reports kind, mode and owner" test "$(st src/f1.txt "(d['kind'], oct(d['mode']), d['uid'])")" = "('file', '0o644', 1000)"
H0=$(head_of w) N0=$(ncs w)
echo hello | afs fs write w notes/new.txt -m "add notes" --author agent >$T/w.json
ok 13.2 "write creates parents and records one changeset" bash -c "
	j \"d['status']\" <$T/w.json | grep -qx applied && [ \$(ncs w) = $((N0 + 1)) ] &&
	afs log --ws w -n 1 | j \"(d[0]['author'], d[0]['message'])\" | grep -qx \"('agent', 'add notes')\""
ok 13.2a "the file reads back, owned by the workspace owner" bash -c "[ \"\$(afs fs cat w notes/new.txt)\" = hello ] && [ \$(st notes \"d['uid']\") = 1000 ] && [ \$(st notes/new.txt \"d['uid']\") = 1000 ]"
ok 13.2b "the write is in the directory, owned by the workspace owner" bash -c "grep -qx hello $W/notes/new.txt && [ \$(stat -c %u $W/notes/new.txt) = 1000 ]"
ok 13.2c "diff of the changeset" bash -c "afs diff \$(afs log --ws w -n 1 | j \"d[0]['id']\") | j \"sorted(c['path'] for c in d[0]['changes'])\" | grep -qx \"\\['notes', 'notes/new.txt'\\]\""

head -c 3000 /dev/urandom >$T/a.bin
cat >$T/ops.json <<EOF
[{"op": "mkdir", "path": "m"},
 {"op": "write", "path": "m/a", "content_path": "$T/a.bin", "mode": 384},
 {"op": "rename", "from": "README", "to": "README.md"},
 {"op": "symlink", "path": "m/link", "target": "a"},
 {"op": "chmod", "path": "src/f2.txt", "mode": 384},
 {"op": "delete", "path": "docs"}]
EOF
N1=$(ncs w)
ok 13.3 "edit applies a batch as one changeset" bash -c "afs fs edit w --ops $T/ops.json | j \"d['status']\" | grep -qx applied && [ \$(ncs w) = $((N1 + 1)) ]"
ok 13.3a "every op took effect" bash -c "
	cmp <(afs fs cat w m/a) $T/a.bin && [ \$(st m/a \"oct(d['mode'])\") = 0o600 ] && [ \"\$(afs fs cat w README.md)\" = readme ] &&
	[ \"\$(afs fs cat w m/link)\" = a ] && [ \$(st m/link \"d['kind']\") = symlink ] && [ \$(st src/f2.txt \"oct(d['mode'])\") = 0o600 ] &&
	! afs fs stat w docs >/dev/null 2>&1"
ok 13.3b "diff reports the rename" bash -c "afs diff \$(afs log --ws w -n 1 | j \"d[0]['id']\") | j \"[c.get('from') for c in d[0]['changes'] if c['path']=='README.md']\" | grep -qx \"\\['README'\\]\""
ok 13.4 "an edit that changes nothing records nothing" bash -c "echo readme | afs fs write w README.md | j \"(d['status'], d['changeset'])\" | grep -qx \"('unchanged', None)\""
ok 13.5 "--at reads a recorded state" bash -c "[ \"\$(afs fs cat w README --at $H0)\" = readme ] && ! afs fs cat w README >/dev/null 2>&1"
B=$(head_of w)
echo x1 | afs fs write w src/f3.txt >/dev/null
printf y | afs fs write w src/f3.txt --base $B >$T/c.json; rc=$?
ok 13.6 "--base: a path changed since is refused (exit 2), nothing written" bash -c "
	[ $rc = 2 ] && j \"(d['status'], d['conflicts'])\" <$T/c.json | grep -qx \"('conflict', \\['src/f3.txt'\\])\" && [ \"\$(afs fs cat w src/f3.txt)\" = x1 ]"
ok 13.6a "--base: other paths go through" bash -c "printf z | afs fs write w src/f4.txt --base $B | j \"d['status']\" | grep -qx applied"
bad 13.7 "a path leaving the workspace is refused" afs fs cat w ../etc/passwd
afs fs ln w /etc esc >/dev/null
bad 13.7a "reads do not follow a symlink below the root" afs fs cat w esc/passwd
bad 13.7b "writes do not either" bash -c "printf x | afs fs write w esc/tracefs-evil"
ok 13.7c "nothing was written outside" test ! -e /etc/tracefs-evil
bad 13.7d "deleting a missing path fails" afs fs rm w nope
bad 13.7e "the root cannot be removed" afs fs rm w /

export W
P=$(spawn --ws w)
printf s | afs fs write w sess.txt >/dev/null
ok 13.8 "a running process does not see a later write to main" bash -c "! x $P 'test -e sess.txt'"
ok 13.8a "until its next merge updates it" bash -c "afs proc merge $P >/dev/null && x $P 'grep -qx s sess.txt'"

echo direct >$W/direct.txt
ok 13.9 "cat reads the directory as it is now, before the write is recorded" test "$(afs fs cat w direct.txt)" = direct
I0=$(stat -c %i $W/src/f1.txt)
printf new | afs fs write w src/f1.txt >/dev/null
ok 13.9a "rewriting a file keeps its inode (later merges still follow it)" bash -c "[ \$(stat -c %i $W/src/f1.txt) = $I0 ] && [ \"\$(cat $W/src/f1.txt)\" = new ]"

mkdir -p $T/bin && cp "$BIN/tracefs" $T/bin/ && chmod 755 $T/bin $T/bin/tracefs
as() {
	local u=$1
	shift
	setpriv --reuid=$u --regid=$u --clear-groups $T/bin/tracefs --socket "$SOCK" "$@"
}
export -f as
ok 13.10 "another uid cannot read the workspace: not found" bash -c "! as 60001 fs cat w README.md >$T/as.out 2>&1 && grep -q 'not found' $T/as.out"
ok 13.10a "the owner can" bash -c "[ \"\$(as 1000 fs cat w README.md)\" = readme ]"
finish_test
