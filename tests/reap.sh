#!/bin/bash
# Kill and remove everything the integration tests may have left behind: processes in
# the test slices, mounts and loop devices under $TROOT and /run/afst-*, and the files.
# Safe to run any time as root; touches nothing outside the test slices and paths.
TROOT=${TROOT:-/var/tmp/afst}
case $TROOT in /var/tmp/* | /srv/* | /home/*/*) ;; *) echo "refusing TROOT=$TROOT" >&2; exit 1 ;; esac
systemctl kill -s KILL afstrun.slice afstest.slice >/dev/null 2>&1
pkill -KILL -f "tracefsd --state-dir $TROOT/" 2>/dev/null
sleep 1
for m in $(findmnt -rn -o TARGET | grep -E "^($TROOT/|/run/afst-)" | sort -r); do umount -l "$m"; done
for l in $(losetup -nO NAME,BACK-FILE | awk -v r="$TROOT/" 'index($2, r) == 1 {print $1}'); do losetup -d "$l"; done
rm -rf --one-file-system "${TROOT:?}"/* /run/afst-* 2>/dev/null
systemctl stop afstrun.slice afstest.slice >/dev/null 2>&1
true
