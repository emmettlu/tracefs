#!/bin/bash
# Build a stable kernel with acfs built in and run the acfs selftests in QEMU/KVM.
#   bash kernel/test.sh [WORKDIR]
# Environment: KVER (kernel version, default 7.3.4), JOBS (make -j, default nproc), TESTS (selftest names, default all),
# TIMEOUT/SMP/MEM/DISK/APPEND (passed to vm.sh). Needs gcc, make, flex, bison, bc, libelf, libssl, cpio, qemu-system-x86, busybox (static), /dev/kvm.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
work=${1:-${TMPDIR:-/tmp}/acfs-kernel}
kver=${KVER:-7.3.4}
jobs=${JOBS:-$(nproc)}
src=$work/linux-$kver

mkdir -p "$work"
if [ ! -d "$src" ]; then
	major=${kver%%.*}
	curl -fsSL "https://cdn.kernel.org/pub/linux/kernel/v$major.x/linux-$kver.tar.xz" | tar -xJ -C "$work"
fi

cp -R "$here/fs" "$here/include" "$here/Documentation" "$here/tools" "$src/"
grep -q 'fs/acfs/Kconfig' "$src/fs/Kconfig" || sed -i 's|^source "fs/btrfs/Kconfig"$|&\nsource "fs/acfs/Kconfig"|' "$src/fs/Kconfig"
grep -q 'fs/acfs/Kconfig' "$src/fs/Kconfig" || { echo "could not hook fs/acfs/Kconfig into fs/Kconfig" >&2; exit 1; }
grep -q 'CONFIG_AC_FS' "$src/fs/Makefile" || echo 'obj-$(CONFIG_AC_FS)		+= acfs/' >>"$src/fs/Makefile"

cd "$src"
make -s x86_64_defconfig kvm_guest.config
scripts/kconfig/merge_config.sh -m -O . .config tools/testing/selftests/ac/acfs/config >/dev/null
make -s olddefconfig
for c in $(grep -o '^CONFIG_[A-Z0-9_]*' tools/testing/selftests/ac/acfs/config); do
	grep -q "^$c=y" .config || { echo "$c is not set after olddefconfig" >&2; exit 1; }
done
make -s -j"$jobs" bzImage
make -s headers
make -s -C tools/testing/selftests/ac/acfs

# shellcheck disable=SC2086
exec bash tools/testing/selftests/ac/acfs/vm.sh arch/x86/boot/bzImage tools/testing/selftests/ac/acfs ${TESTS:-}
