#!/bin/bash
# Workspace registration edge cases.
. "$(dirname "$0")/lib.sh"
setup

W=$(mkws w1)
out=$(afs ws register w1 --from "$W")
ok 1.1 "register returns id, owner 1000:1000 and host path" python3 -c "
import json,sys; d=json.loads(sys.argv[1]); assert d['uid']==1000 and d['gid']==1000 and d['host_path']=='$W', d" "$out"
BASE=$(echo "$out" | j "d['base']")
ok 1.1a "base subvolume is read-only" bash -c "btrfs property get $STATE/bases/$BASE ro | grep -q true"

bad 1.2 "duplicate name rejected" afs ws register w1 --from "$W"
for n in -x a/b '' "$(printf 'a%.0s' $(seq 1 65))" 'a b' '.hidden'; do
	bad 1.3 "invalid name $(printf %q "$n")" afs ws register "$n" --from "$W"
done
bad 1.4 "nonexistent path" afs ws register nope --from $TROOT/does-not-exist
touch $T/file
bad 1.4a "regular file as workspace" afs ws register file --from $T/file
bad 1.4b "state dir overlap" afs ws register st --from "$STATE/bases"
bad 1.4c "parent of state dir" afs ws register st --from $TROOT

bad 1.2a "the same directory twice rejected (it is taken over)" afs ws register w1b --from "$W"
V=$(mkws viasym)
ln -s "$V" $T/link
out=$(afs ws register viasym --from $T/link)
ok 1.5 "symlinked dir registers under its canonical path" bash -c "echo '$out' | j \"d['host_path']\" | grep -qx '$V'"

# Special content preserved byte for byte.
S=$T/src/special
mkdir -p "$S/deep/$(printf 'd%.0s' $(seq 1 200))/x" "$S/empty" "$S/mnt"
echo hl >"$S/hl1" && ln "$S/hl1" "$S/hl2"
echo x >"$S/xat" && setfattr -n user.k -v v1 "$S/xat"
ln -s /etc/passwd "$S/abs-link" && ln -s missing "$S/dangling"
mkfifo "$S/fifo"
mknod "$S/chardev" c 1 3
head -c 1M /dev/urandom >"$S/bin"
truncate -s 1G "$S/sparse"
echo s >"$S/suid" && chmod 4755 "$S/suid"
echo n >"$S/$(printf 'bad\xffname')"
echo l >"$S/$(printf 'n%.0s' $(seq 1 255))"
echo u >"$S/中文 名字"
chown -R 1000:1000 "$S"
chown 1000:1000 "$S/suid" && chmod 4755 "$S/suid"
mount -t tmpfs tmpfs "$S/mnt" && echo inside >"$S/mnt/hidden"
out=$(afs ws register special --from "$S")
SB=$(echo "$out" | j "d['base']")
umount "$S/mnt"
fp "$S" | grep -v "^'mnt' " >$T/fp.src
fp "$STATE/bases/$SB/l/1" | grep -v "^'mnt' " >$T/fp.base
ok 1.6 "base copy equals source (modes, owners, xattrs, symlinks, fifo, device, non-UTF-8 and 255-byte names, setuid)" diff $T/fp.src $T/fp.base
ok 1.6a "hard links preserved inside the base" bash -c "[ \$(stat -c %i '$STATE/bases/$SB/l/1/hl1') = \$(stat -c %i '$STATE/bases/$SB/l/1/hl2') ]"
ok 1.6b "sparse file stays sparse" bash -c "[ \$(du -k '$STATE/bases/$SB/l/1/sparse' | cut -f1) -lt 1024 ]"
ok 1.7 "nested mount inside the workspace is not copied (cp -x; the mountpoint dir stays, empty)" bash -c "[ -d '$STATE/bases/$SB/l/1/mnt' ] && [ -z \"\$(ls -A '$STATE/bases/$SB/l/1/mnt')\" ]"

R=$T/src/rootowned
mkdir -p "$R" && echo r >"$R/f"
ok 1.8 "root-owned workspace can be registered" afs ws register rootws --from "$R"
ok 1.8a "but another uid cannot branch it (not found)" bash -c "! (cd / && arun --ws rootws -- true) 2>$T/r.err && grep -q 'not found' $T/r.err"

U=$T/src/$(printf 'u\xff')
mkdir -p "$U"
bad 1.9 "non-UTF-8 workspace path rejected" afs ws register utf --from "$U"

ok 1.11 "no stray subvolumes after failed registrations" bash -c "[ \$(ls $STATE/bases | wc -l) = 4 ] && [ \$(ls $STATE/branches | wc -l) = 4 ] && [ \$(ls $STATE/snaps | wc -l) = 4 ]"

# Workspaces must lie below the caller's home (here --workspace-root, see lib.sh).
below() { ! "$@" >$T/below.out 2>&1 && grep -q 'is not below' $T/below.out; } # rejected by the path check
ok 1.12 "path outside the workspace root rejected" below afs ws register etc --from /etc
ln -s /etc $T/esc
ok 1.12a "symlink leading out of the workspace root rejected" below afs ws register esc --from $T/esc
ok 1.12b "/proc rejected" below afs ws register pr --from /proc

# Without --workspace-root the caller's home decides, by SO_PEERCRED.
U1=${SUDO_USER:?run via sudo}
H1=$(getent passwd "$U1" | cut -d: -f6)
stop_daemon
WSROOT= start_daemon
asuser() { runuser -u "$U1" -- "$BIN/tracefs" --socket "$SOCK" "$@"; }
mkdir -p "$H1/.afst-t01/w" && chown -R "$U1:" "$H1/.afst-t01"
ok 1.13 "a user registers a directory below its home" asuser ws register home1 --from "$H1/.afst-t01/w"
ok 1.13a "but not outside it" below asuser ws register out1 --from "$(mkws out1)"
ok 1.13b "root is held to /root" below afs ws register out2 --from "$(mkws out2)"
afs ws detach home1 >/dev/null
rm -rf "$H1/.afst-t01"

finish_test
