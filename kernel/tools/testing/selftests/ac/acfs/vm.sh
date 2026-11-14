#!/bin/sh
# SPDX-License-Identifier: GPL-2.0
#
# Boot KERNEL in QEMU/KVM with a busybox initramfs and two empty disks
# (/dev/vda and a small /dev/vdb, fresh sparse images per test), run the
# acfs selftests of TESTDIR (the static binaries this directory's Makefile
# builds) as root and power off. A test named *crash* boots twice on the
# same disks, run with argument 1 (it is expected to crash the VM) and
# then 2. Prints "actest: PASS name" or "actest: FAIL name" per
# test, then whether the kernel log grew a warning after boot:
# "actest: PASS no-splat". Exits 0 when everything passed.
#
# Usage: vm.sh KERNEL TESTDIR [TEST...]   (default: every acfs_* test)
# Environment: BUSYBOX (default: busybox in PATH), TIMEOUT (seconds, 1200),
# QEMU (qemu-system-x86_64), SMP (4), MEM (2G), DISK (size, 1G),
# LOGLEVEL (4), APPEND (more kernel parameters; NAME=VALUE ones reach the
# tests' environment, e.g. "ACFS_SEEDS=10 ACFS_OPS=20000").
set -eu

[ $# -ge 2 ] || { echo "usage: $0 KERNEL TESTDIR [TEST...]" >&2; exit 2; }
kernel=$1 tests=$2
shift 2
busybox=${BUSYBOX:-$(command -v busybox || true)}
[ -x "$busybox" ] || { echo "no busybox (set BUSYBOX)" >&2; exit 2; }

root=$(mktemp -d)
trap 'rm -rf "$root" "$root.cpio.gz" "$root.log" "$root".disk*' EXIT
mkdir -p "$root/bin" "$root/dev" "$root/proc" "$root/sys" "$root/tmp" "$root/t" "$root/mnt"
cp "$busybox" "$root/bin/busybox"
for a in sh mount umount poweroff cat ls echo sleep kill ps dmesg grep tr wc tail mkdir; do
	ln -s busybox "$root/bin/$a"
done
names=${*:-$(cd "$tests" && for f in acfs_*; do [ -x "$f" ] && [ -f "$f" ] && echo "$f"; done | tr '\n' ' ')}
for t in $names; do cp "$tests/$t" "$root/t/"; done
cp "$tests/mkfs.acfs" "$tests/fsck.acfs" "$root/t/"

boot() { # TEST ARG
	t=$1 arg=$2
	cat >"$root/init" <<INIT
#!/bin/sh
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev
mount -t tmpfs tmpfs /tmp
boot=\$(dmesg | wc -l)
echo "actest: run $t $arg"
if /t/$t $arg; then echo "actest: PASS $t"; else echo "actest: FAIL $t (\$?)"; fi
if dmesg | tail -n +\$((boot + 1)) | grep -E 'WARNING:|BUG:|KASAN|lockdep|possible recursive locking|circular locking|suspicious RCU|Oops|general protection|refcount_t:|list_(add|del) corruption|acfs.*(aborted|damaged|wrong|out of range)' >/tmp/splat; then
	echo "actest: FAIL splat ($t)"; cat /tmp/splat
else
	echo "actest: PASS no-splat ($t)"
fi
echo "actest: done"
poweroff -f
INIT
	chmod +x "$root/init"
	(cd "$root" && find . | cpio -o -H newc --quiet | gzip -1) >"$root.cpio.gz"
	timeout "${TIMEOUT:-1200}" "${QEMU:-qemu-system-x86_64}" -enable-kvm -cpu host \
		-smp "${SMP:-4}" -m "${MEM:-2G}" -nographic -no-reboot \
		-drive "file=$disk,if=virtio,format=raw,cache=unsafe" \
		-drive "file=$disk.log,if=virtio,format=raw,cache=unsafe" \
		-kernel "$kernel" -initrd "$root.cpio.gz" \
		-append "console=ttyS0 panic=-1 oops=panic quiet loglevel=${LOGLEVEL:-4} ${APPEND:-}" </dev/null | tee "$root.log"
}

result=0
for t in $names; do
	disk=$root.disk.$t
	truncate -s "${DISK:-1G}" "$disk"
	truncate -s 1M "$disk.log"
	case $t in
	*crash*)
		boot "$t" 1
		! grep -aq "actest: FAIL\|actest: done" "$root.log" || { echo "actest: FAIL $t: did not crash"; result=1; }
		boot "$t" 2;;
	*)
		boot "$t" "";;
	esac
	grep -aq "actest: done" "$root.log" || { echo "actest: FAIL $t: no result (timeout or crash)"; result=1; }
	! grep -aq "actest: FAIL" "$root.log" || result=1
	rm -f "$disk" "$disk.log"
done
exit $result
