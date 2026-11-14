#!/bin/sh
# Copy acfs-portable into the plugin: Codex installs a plugin by copying its directory, so it cannot import ../portable.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
rm -rf "$here/lib/portable"
mkdir -p "$here/lib/portable/src"
cp "$here/../portable/index.js" "$here/../portable/index.d.ts" "$here/../portable/package.json" "$here/lib/portable/"
cp "$here/../portable/src/"*.js "$here/lib/portable/src/"
