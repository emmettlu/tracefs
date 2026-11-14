#!/bin/bash
# Install or upgrade tracefsd on a host; safe to rerun. Run as root from a built tracefs tree:
#   sudo bash deploy/install.sh
# Installs the binaries, the systemd unit and tracefs.slice limits, and creates the state dir
# /var/lib/tracefs as its own btrfs subvolume (kept out of root snapshots). Everything is
# root:root. The socket /run/tracefs/tracefs-daemon.sock is 0666; tracefsd authorizes every
# request by the caller's uid.
set -euo pipefail
cd "$(dirname "$0")/.."
[ "$(id -u)" = 0 ] || { echo "run as root" >&2; exit 1; }
BIN=${BIN:-target/release}
STATE=/var/lib/tracefs

install -o root -g root -m755 "$BIN/tracefsd" "$BIN/tracefs" /usr/local/bin/

if [ ! -e $STATE ]; then
	if [ "$(stat -f -c %T "$(dirname $STATE)")" = btrfs ]; then
		btrfs -q subvolume create $STATE
	else
		mkdir $STATE # tracefsd falls back to a loop btrfs image next to it
	fi
fi

install -Dm644 deploy/tracefsd.service /etc/systemd/system/tracefsd.service
install -Dm644 deploy/tracefs-slice-limits.conf /etc/systemd/system/tracefs.slice.d/limits.conf
systemctl daemon-reload
systemctl enable tracefsd.service
systemctl restart tracefsd.service
systemctl is-active tracefsd.service
