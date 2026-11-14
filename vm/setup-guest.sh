#!/bin/bash
# Prepare the Lima guest (agentos.yaml) for tracefs: packages, the kernel probes of tests/kernel
# and ovl, test data from the read-only host cache. Run as root via sudo from the repo:
#   limactl shell agentos -- sudo bash vm/setup-guest.sh
set -eux
SRC=${SRC:-$(cd "$(dirname "$0")/../tests/kernel" && pwd)}
DATA=${DATA:-$HOME/.cache/afsdata} # agentos.yaml mounts it at the host path
U=${SUDO_USER:?run via sudo}
V=${V:-$(getent passwd "$U" | cut -d: -f6)/agentfs-verify}

pacman -S --noconfirm --needed btrfs-progs git python gcc hyperfine fio fuse-overlayfs rust rsync ripgrep nodejs npm uv
mkdir -p "$V"
rsync -a "$SRC/" "$V/"
gcc -O2 -o "$V/ovl" "$V/ovl.c"
mkdir -p "$V/data" && chown "$U:" "$V" "$V/data"
# check out inside the guest: the linux tree has case-colliding paths (host is APFS)
[ -d "$V/data/linux" ] || sudo -u "$U" git clone -q --depth 1 "file://$DATA/linux.git" "$V/data/linux"
[ -d "$V/data/ripgrep" ] || sudo -u "$U" git clone -q --depth 1 "file://$DATA/ripgrep.git" "$V/data/ripgrep"
sudo -u "$U" sh -c "cd '$V/data/ripgrep' && cargo fetch -q"
echo "V=$V files=$(git -C "$V/data/linux" ls-files | wc -l)"
