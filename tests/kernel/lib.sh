# Shared helpers for acfs verification scripts. Run as root via sudo.
set -u
V=${V:-$(getent passwd "${SUDO_USER:?run via sudo}" | cut -d: -f6)/agentfs-verify} # see vm/setup-guest.sh
OVL=$V/ovl
OVLOPTS=(-o redirect_dir=on -o metacopy=off -o index=off -o xino=auto)
IMGDIR=$V/img
MNTBASE=${MNTBASE:-/tmp/afsv} # mountpoints only; image data stays on disk
mkdir -p "$IMGDIR" "$V/logs"
chattr +C "$IMGDIR" 2>/dev/null || true

res() { printf 'RESULT|%s|%s|%s\n' "$1" "$2" "$3"; }
check() { # check ID "desc" cmd...
	local id=$1 desc=$2
	shift 2
	if "$@" >/dev/null 2>&1; then res "$id" PASS "$desc"; else res "$id" FAIL "$desc"; fi
}
fails() { ! "$@"; }

# mkimg NAME [SIZE] -> prints mountpoint; layout per AGENTS.md 4.
mkimg() {
	local n=$1 size=${2:-4G} img=$IMGDIR/$1.img mp=$MNTBASE/$1
	rmimg "$n" 2>/dev/null
	truncate -s "$size" "$img"
	mkfs.btrfs -q -f "$img" >/dev/null
	mkdir -p "$mp"
	mount -o loop,noatime "$img" "$mp"
	mkdir -p "$mp"/{bases,branches,snaps,mnt}
	echo "$mp"
}

rmimg() {
	local n=$1 mp=$MNTBASE/$1
	if mountpoint -q "$mp"; then
		# Unmount overlays and anything else under the image first.
		findmnt -rn -o TARGET | grep -F "$mp/" | sort -r | xargs -r -n1 umount -l
		umount -l "$mp"
	fi
	rm -f "$IMGDIR/$n.img"
}

# Mount an overlay with the plan's options. mnt_ovl TARGET UPPERBRANCH LOWER...
mnt_ovl() {
	local t=$1 b=$2
	shift 2
	local args=()
	for l in "$@"; do args+=(-l "$l"); done
	mkdir -p "$t"
	"$OVL" "${args[@]}" -u "$b/upper" -w "$b/work" "${OVLOPTS[@]}" -t "$t"
}

new_branch() { btrfs -q subvolume create "$1" && mkdir -p "$1/upper" "$1/work"; }
snap_ro() { btrfs -q subvolume snapshot -r "$1" "$2"; }

# Merged-view fingerprint: path, type, mode, symlink target, content hash, user xattrs.
# Uses os.walk instead of find(1): find skips dirs whose (dev, ino) repeats (probe 3.13).
fp() {
	python3 - "$1" <<'PYEOF'
import hashlib, os, stat, sys
root = sys.argv[1]
out = []
for d, dirs, files in os.walk(root):
    for n in dirs + files:
        p = os.path.join(d, n)
        st = os.lstat(p)
        rel = os.path.relpath(p, root)
        extra = ""
        if stat.S_ISREG(st.st_mode):
            h = hashlib.sha1()
            with open(p, "rb") as f:
                for b in iter(lambda: f.read(1 << 20), b""):
                    h.update(b)
            extra = h.hexdigest()
        elif stat.S_ISLNK(st.st_mode):
            extra = os.readlink(p)
        try:
            xs = sorted(x for x in os.listxattr(p, follow_symlinks=False) if x.startswith("user."))
            extra += " " + ",".join(x + "=" + os.getxattr(p, x, follow_symlinks=False).hex() for x in xs)
        except OSError:
            pass
        out.append("%r %o %s" % (rel, st.st_mode, extra))
print("\n".join(sorted(out)))
PYEOF
}
