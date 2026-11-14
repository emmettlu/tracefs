#!/bin/bash
# Install or upgrade acfsd on a host; safe to rerun. Run as root from a built acfs tree:
#   sudo bash deploy/install.sh
# Installs the binaries, the systemd unit and acfs.slice limits, and creates the state dir
# /var/lib/acfs as its own btrfs subvolume (kept out of root snapshots). Everything is
# root:root. The socket /run/acfs/acfs-daemon.sock is 0666; acfsd authorizes every
# request by the caller's uid.
set -euo pipefail
cd "$(dirname "$0")/.."
[ "$(id -u)" = 0 ] || { echo "run as root" >&2; exit 1; }
BIN=${BIN:-target/release}
STATE=/var/lib/acfs

install -o root -g root -m755 "$BIN/acfsd" "$BIN/acfs" /usr/local/bin/

if [ ! -e $STATE ]; then
	if [ "$(stat -f -c %T "$(dirname $STATE)")" = btrfs ]; then
		btrfs -q subvolume create $STATE
	else
		mkdir $STATE # acfsd falls back to a loop btrfs image next to it
	fi
fi

install -Dm644 deploy/acfsd.service /etc/systemd/system/acfsd.service
install -Dm644 deploy/acfs-slice-limits.conf /etc/systemd/system/acfs.slice.d/limits.conf
systemctl daemon-reload
systemctl enable acfsd.service
systemctl restart acfsd.service
systemctl is-active acfsd.service
