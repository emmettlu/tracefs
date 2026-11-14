#!/bin/bash
# Build this host's architecture's package for the GitHub release of a v* tag: run it once on an x86_64 Arch host and
# once on an Arch Linux ARM host. HEAD must be the tag. With `gh` logged in here it creates the release if needed and
# uploads the package and its .sha256; otherwise it prints the commands to run where gh is.
#   packaging/arch/release.sh v0.1.0
set -euo pipefail
tag=${1:?usage: release.sh vX.Y.Z}
cd "$(dirname "$0")"
arch=$(uname -m)
[[ $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "tag must be vX.Y.Z" >&2; exit 1; }
[ "$(git describe --tags --exact-match HEAD 2>/dev/null)" = "$tag" ] || { echo "HEAD is not $tag" >&2; exit 1; }
repo=$(git remote get-url origin | sed -E 's#^(git@github\.com:|https://github\.com/)##; s#\.git$##')
rm -f -- ./*-"$arch".pkg.tar.zst ./*-"$arch".pkg.tar.zst.sha256
PKGEXT=.pkg.tar.zst makepkg -sf --noconfirm
pkg=$(ls ./*-"$arch".pkg.tar.zst)
sha256sum "${pkg#./}" > "$pkg.sha256"
if gh auth status >/dev/null 2>&1; then
	gh release view "$tag" -R "$repo" >/dev/null 2>&1 || gh release create "$tag" -R "$repo" --verify-tag --title "$tag" --notes ''
	gh release upload "$tag" -R "$repo" "$pkg" "$pkg.sha256" --clobber
else
	echo "gh is not logged in here; copy the files to where it is and run:"
	echo "  gh release view $tag -R $repo >/dev/null 2>&1 || gh release create $tag -R $repo --verify-tag --title $tag --notes ''"
	echo "  gh release upload $tag -R $repo ${pkg#./} ${pkg#./}.sha256 --clobber"
	echo "files: $(realpath "$pkg") $(realpath "$pkg.sha256")"
fi
