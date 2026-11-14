#!/bin/bash
# Kernel, btrfs and overlayfs behaviour that tracefs relies on or works around.
# Each check states the expected behaviour; a FAIL means the layer below tracefs changed and
# the part of AGENTS.md citing that id ("验证 x.y") needs another look.
# tracefs's own behaviour is tested by tests/t*.sh.
. "$(dirname "$0")/lib.sh"
R=$(mkimg probe)
cd "$R"
L=$V/logs/probe
mkdir -p $L

btrfs -q subvolume create bases/b1
mkdir -p bases/b1/d && echo base1 >bases/b1/f && echo x >bases/b1/d/x
btrfs -q subvolume create bases/b2
echo base2 >bases/b2/f && echo only2 >bases/b2/g
btrfs property set bases/b1 ro true
btrfs property set bases/b2 ro true
new_branch branches/p1

# Mount API
r=0
if mnt_ovl mnt/p1 branches/p1 bases/b2 bases/b1 2>$L/2.1; then
	[ "$(cat mnt/p1/f)" = base2 ] && r=1
	umount mnt/p1
fi
check 2.1 "fsopen/fsmount/move_mount with lowerdir+; the first lower is the top layer" test $r = 1

new_branch branches/odd
for name in 'a,b' 'a:b' 'a b' 'a=b' 'a\b' "$(printf 'a\nb')"; do
	d=bases/odd_$(echo -n "$name" | od -An -tx1 | tr -d ' \n')
	mkdir -p "$d/$name" && echo ok >"$d/$name/f"
	rm -rf branches/odd/upper/* branches/odd/work/*
	r=0
	if mnt_ovl mnt/odd branches/odd "$d/$name" 2>/dev/null; then
		[ "$(cat mnt/odd/f)" = ok ] && r=1
		umount mnt/odd
	fi
	check 2.2 "lowerdir+ takes $(printf %q "$name") verbatim" test $r = 1
done

LN=_with_a_reasonably_long_directory_name_to_mimic_real_paths
for i in $(seq 1 501); do mkdir -p layers/L$i$LN && echo $i >layers/L$i$LN/f$i; done
deep() { # deep N [ovl flags]: mount N layers read-only and see all of them
	local n=$1 a=() i r=1
	shift
	for i in $(seq 1 $n); do a+=(-l "$R/layers/L$i$LN"); done
	mkdir -p mnt/deep
	$OVL "$@" "${a[@]}" -r -o redirect_dir=on -t mnt/deep 2>>$L/2.3 || return 1
	[ "$(ls mnt/deep | wc -l)" = "$n" ] && r=0
	umount mnt/deep
	return $r
}
check 2.3 "lowerdir+ stacks 500 layers" deep 500
check 2.3a "layer 501 is rejected" fails deep 501
check 2.3b "legacy mount(2) options outgrow a page at 64 long layer paths" fails deep 64 -m

mkdir -p mnt/ro
check 2.4 "a single lower without upper is rejected" fails $OVL -l bases/b1 -r -t mnt/ro
btrfs -q subvolume create bases/empty
r=0
if $OVL -l bases/empty -l bases/b1 -o redirect_dir=on -t mnt/ro 2>/dev/null; then
	touch mnt/ro/new 2>/dev/null || r=1
	umount mnt/ro
fi
check 2.4b "two lowers (one empty) without upper mount read-only" test $r = 1

mnt_ovl mnt/p1 branches/p1 bases/b1
r=0
mnt_ovl mnt/p1b branches/p1 bases/b1 2>/dev/null && r=1 && umount mnt/p1b
umount mnt/p1
check 2.6 "index=off does not stop a second mount of the same upper (tracefs enforces it)" test $r = 1

snap_ro branches/p1 snaps/s0
r=0
if mnt_ovl mnt/s0 snaps/s0 bases/b1 2>/dev/null; then
	[ "$(findmnt -no OPTIONS mnt/s0 | cut -d, -f1)" = ro ] && r=1
	umount mnt/s0
fi
check 2.7 "an upper on a read-only snapshot mounts but silently comes up read-only (tracefs checks)" test $r = 1

r=0
mkdir -p mnt/fd
if $OVL -L bases/b2 -L bases/b1 -B branches/p1 "${OVLOPTS[@]}" -t mnt/fd 2>$L/2.10; then
	[ "$(cat mnt/fd/f)" = base2 ] && r=1
	umount mnt/fd
fi
check 2.10 "layers, upperdir and workdir can be passed as fds" test $r = 1

# Overlay semantics: every mutation is applied through the overlay and to a native copy.
LONG=$(printf 'n%.0s' $(seq 1 255))
mkbase() {
	local b=$1
	echo hello >$b/f
	printf 'l1\nl2\nl3\n' >$b/text
	head -c 64M /dev/urandom >$b/big
	echo hl >$b/hl1 && ln $b/hl1 $b/hl2
	mkdir -p $b/d1 $b/d2/sub $b/d3 $b/d4 $b/chain $b/tdir/x $b/dst $b/odd $b/far
	echo a >$b/d1/a && echo b >$b/d1/b && echo c >$b/d2/sub/c && echo e >$b/d3/e && echo g >$b/d4/g
	echo ch >$b/chain/k && echo m >$b/mode && echo t >$b/times && echo tf >$b/tfile && echo x >$b/tdir/x/y
	echo r1 >$b/ren_src && echo r2 >$b/ren_dst
	ln -s f $b/sym && mkfifo $b/fifo
	echo x >$b/xattr && setfattr -n user.k -v v1 $b/xattr
	echo l >$b/odd/"$LONG" && echo u >$b/odd/'中文 ü' && echo nl >$b/odd/"$(printf 'a\nb')"
	echo tr >$b/trunc
}
ops() { # run with cwd = view root
	echo more >>f
	chmod 600 mode
	touch -d '2001-01-01' times
	echo tail >>big
	: >trunc
	rm d1/a
	rm -rf d3
	rm -rf d1 && mkdir d1 && echo new >d1/n # opaque
	mv d2 d2r                                # redirect, same parent
	mv d4 dst/d4m                            # redirect, other parent
	mv chain chain1 && mv chain1 chain2 && mv chain2 chain
	mv ren_src ren_dst
	rm tfile && mkdir tfile && echo in >tfile/i
	rm -rf tdir && echo now-file >tdir
	ln -sf text sym
	setfattr -n user.k -v v2 xattr
	rm odd/"$LONG" && mv odd/'中文 ü' odd/'中文 ü2' && rm odd/"$(printf 'a\nb')"
	echo tmp >d2r/tmp && rm d2r/tmp
	echo new >newfile && mkdir -p newdir/a/b && echo z >newdir/a/b/z
}
btrfs -q subvolume create bases/s && mkbase bases/s && btrfs property set bases/s ro true
btrfs -q subvolume create native && cp -a --reflink=always bases/s/. native/
new_branch branches/s
mnt_ovl mnt/s branches/s bases/s
(cd mnt/s && echo tail >>big)
sync
excl=$(btrfs filesystem du --raw branches/s/upper/big | awk 'NR==2{print $2}')
check 3.1 "copy-up of a 64 MiB file is a reflink (upper exclusive bytes: $excl)" test "${excl:-0}" -gt 0 -a "${excl:-0}" -lt 1048576
(cd native && echo tail >>big)
(cd mnt/s && ops 2>$L/ops.err) && (cd native && ops)
fp mnt/s >$L/fp_ovl
fp native >$L/fp_nat
check 3.2 "every mutation works through the overlay and the view equals a native replay" bash -c "[ ! -s $L/ops.err ] && diff -q $L/fp_ovl $L/fp_nat"

(cd mnt/s && echo changed >>hl1)
check 3.12 "copy-up breaks a lower hard link (index=off): the other name keeps the old content" test "$(cat mnt/s/hl2)" = hl

n=$(python3 - mnt/s <<'EOF'
import collections, os, sys
seen = collections.Counter()
for d, dirs, files in os.walk(sys.argv[1]):
    for n in dirs + files:
        st = os.lstat(os.path.join(d, n))
        seen[(st.st_dev, st.st_ino)] += 1
print(sum(1 for v in seen.values() if v > 1))
EOF
)
check 3.13 "base and upper in different subvolumes give colliding (st_dev, st_ino) in the view ($n)" test "${n:-0}" -gt 0

check 4.7 "FICLONE from an overlay file to btrfs fails with EXDEV" python3 - <<'EOF'
import errno, fcntl, os, sys
s = os.open("mnt/s/big", os.O_RDONLY)
d = os.open("ficlone.dst", os.O_WRONLY | os.O_CREAT, 0o644)
try:
    fcntl.ioctl(d, 0x40049409, s)  # FICLONE
except OSError as e:
    sys.exit(e.errno != errno.EXDEV)
sys.exit(1)
EOF

snap_ro branches/s snaps/cfr
mkdir -p mnt/cfr && $OVL -l snaps/cfr/upper -l bases/s -r "${OVLOPTS[@]}" -t mnt/cfr
python3 - <<'EOF'
import os
s = os.open("mnt/cfr/big", os.O_RDONLY)
d = os.open("mnt/s/cfr.dst", os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o644)
n = os.fstat(s).st_size
while n > 0:
    k = os.copy_file_range(s, d, n)
    if k == 0:
        break
    n -= k
EOF
sync
ext() { filefrag -v "$1" | awk '$1 ~ /^[0-9]+:$/ {print $4 $5}'; }
a=$(ext snaps/cfr/upper/big) b=$(ext branches/s/upper/cfr.dst)
check 4.7b "copy_file_range from a read-only view to a branch overlay clones the underlying file (same extents)" bash -c "
	cmp -s mnt/cfr/big mnt/s/cfr.dst && [ -n '$a' ] && [ '$a' = '$b' ]"
rm mnt/s/cfr.dst
umount mnt/cfr
btrfs -q subvolume delete snaps/cfr >/dev/null

snap_ro branches/s snaps/sa
(cd mnt/s && mv dst/d4m d2r/ && mv d2r far/d2m) # far is copied up by the move
snap_ro branches/s snaps/sb
btrfs send -q --no-data -p snaps/sa snaps/sb | btrfs receive --dump >$L/nodata.dump 2>&1
res 4.9 INFO "send --no-data, dir moved into a dir created in the same increment: $(awk '{print $1}' $L/nodata.dump | sort | uniq -c | tr -s ' ' | tr '\n' ','); orphan names (diff v2 must resolve): $(grep -cE '/o[0-9]+-[0-9]+-[0-9]+' $L/nodata.dump)"
umount mnt/s

new_branch branches/off
$OVL -l bases/s -u branches/off/upper -w branches/off/work -o redirect_dir=off -o index=off -t mnt/s
check 3.18 "with redirect_dir=off renaming a lower directory fails with EXDEV" python3 - <<'EOF'
import errno, os, sys
try:
    os.rename("mnt/s/d2", "mnt/s/d2x")
except OSError as e:
    sys.exit(e.errno != errno.EXDEV)
sys.exit(1)
EOF
umount mnt/s

# Branch subvolumes
new_branch branches/victim && mnt_ovl mnt/victim branches/victim bases/b1
r=0
btrfs -q subvolume delete branches/victim 2>/dev/null && echo after >mnt/victim/after 2>/dev/null && r=1
umount -l mnt/victim
check 4.5 "a mounted branch subvolume can be deleted and the overlay keeps writing (tracefs unmounts first)" test $r = 1

# idmap
unshare --user --map-users=0:0:1 --map-users=1000:400000:1 --map-groups=0:0:1 --map-groups=1000:400000:1 sleep 600 &
HP=$!
sleep 0.3
new_branch branches/i && mkdir -p mnt/i
r=0
if $OVL -l bases/b1 -u branches/i/upper -w branches/i/work "${OVLOPTS[@]}" -i $HP -t mnt/i 2>$L/6.3; then
	umount mnt/i
else
	grep -q 'errno=22' $L/6.3 && r=1
fi
kill $HP
check 6.3 "an overlay mount cannot be idmapped (EINVAL)" test $r = 1

# File identity in the single-subvolume layout (all layers and the upper in one subvolume):
# merge follows renames by the merged view's st_ino.
btrfs -q subvolume create branches/one
mkdir -p branches/one/l/1/d/sub branches/one/upper branches/one/work
echo f >branches/one/l/1/f && echo c >branches/one/l/1/d/sub/c
mnt_ovl mnt/one branches/one branches/one/l/1
ino() { stat -c %i "$1"; }
f0=$(ino mnt/one/f) d0=$(ino mnt/one/d) c0=$(ino mnt/one/d/sub/c)
echo more >>mnt/one/f && chmod 600 mnt/one/f
f1=$(ino mnt/one/f)
mv mnt/one/f mnt/one/g && mv mnt/one/d mnt/one/d2
check 3.20 "a lower file keeps its st_ino through copy-up and rename" test "$f0" = "$f1" -a "$f0" = "$(ino mnt/one/g)"
check 3.20a "a renamed lower directory and the file below it keep their st_ino" test "$d0" = "$(ino mnt/one/d2)" -a "$c0" = "$(ino mnt/one/d2/sub/c)"
echo n >mnt/one/n && n0=$(ino mnt/one/n) && mv mnt/one/n mnt/one/d2/n2
check 3.20b "a new upper file keeps its st_ino through rename" test "$n0" = "$(ino mnt/one/d2/n2)"
umount mnt/one
snap_ro branches/one snaps/one
mkdir -p mnt/oneview
$OVL -l snaps/one/upper -l snaps/one/l/1 -r "${OVLOPTS[@]}" -t mnt/oneview
r=0
[ "$(ino mnt/oneview/d2/sub/c)" = "$c0" ] && [ "$(ino mnt/oneview/d2/n2)" = "$n0" ] && r=1
check 3.20c "the read-only view of a snapshot reports the same st_ino for lower files and new upper files" test $r = 1
# A copied-up entry is a lower layer of the view, which reports the upper inode's own st_ino;
# the original one is the objectid in its origin file handle (btrfs fid, u64 at byte 21).
origin_ino() { getfattr --only-values -n trusted.overlay.origin "$1" 2>/dev/null | od -An -j21 -N8 -tu8 | tr -d ' '; }
r=0
[ "$(ino mnt/oneview/g)" != "$f0" ] && [ "$(origin_ino snaps/one/upper/g)" = "$f0" ] && [ "$(origin_ino snaps/one/upper/d2)" = "$d0" ] && r=1
umount mnt/oneview
check 3.20f "in the read-only view a copied-up entry reports its upper st_ino; its origin handle holds the original" test $r = 1
for b in fa fb; do
	btrfs -q subvolume snapshot snaps/one branches/$b && rm -rf branches/$b/work && mkdir branches/$b/work
	mnt_ovl mnt/$b branches/$b branches/$b/l/1
	echo $b >mnt/$b/new_$b
done
r=0
[ "$(ino mnt/fa/g)" = "$f0" ] && [ "$(ino mnt/fb/g)" = "$f0" ] && r=1
check 3.20d "forks of a snapshot keep its st_ino" test $r = 1
check 3.20e "new files in two forks can get the same st_ino (identity only within one subvolume lineage)" test "$(ino mnt/fa/new_fa)" = "$(ino mnt/fb/new_fb)"
umount mnt/fa mnt/fb

cd /
rmimg probe
