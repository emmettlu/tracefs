#!/bin/bash
# Check a test host once: the tools the integration tests use.
#   sudo bash tests/setup-host.sh
set -eu
for c in btrfs mkfs.btrfs git python3 setfattr getfattr systemd-run nsenter setpriv findmnt; do
	command -v $c >/dev/null || { echo "missing $c" >&2; exit 1; }
done
echo "host ready"
