#!/bin/sh
# Copy tracefs-portable into the plugins that are installed on their own (Codex copies a plugin directory, dsh installs sdk/dsh from Git
# or npm), so none of them imports ../portable. Run after changing sdk/portable; the plugins' tests fail while a copy differs.
set -eu
sdk=$(cd "$(dirname "$0")" && pwd)
for p in codex dsh; do
	dst=$sdk/$p/lib/portable
	rm -rf "$dst"
	mkdir -p "$dst/src"
	cp "$sdk/portable/index.js" "$sdk/portable/index.d.ts" "$sdk/portable/package.json" "$dst/"
	cp "$sdk/portable/src/"*.js "$dst/src/"
done
