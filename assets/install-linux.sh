#!/bin/sh
# User install from the release tarball. The daemon always runs as the owner.
set -eu
prefix=${1:-"$HOME/.local"}
source_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
for name in animesh animesh-app animesh-desktop; do
    install -Dm755 "$source_dir/bin/$name" "$prefix/bin/$name"
done
install -Dm644 "$source_dir/share/applications/animesh.desktop" "$prefix/share/applications/animesh.desktop"
for size in 16x16 24x24 32x32 48x48 256x256 512x512; do
    install -Dm644 "$source_dir/share/icons/hicolor/$size/apps/animesh.png" "$prefix/share/icons/hicolor/$size/apps/animesh.png"
done
# Also installs a user launcher with an absolute executable path, so it works
# before ~/.local/bin has been added to the graphical session's PATH.
if ! "$prefix/bin/animesh" service restart; then
    printf '\nThe files are installed, but the user service could not start.\n'
    printf 'Open Animesh in your applications menu to retry.\n'
fi
printf '\nInstalled Animesh in %s. Open Animesh in your applications menu.\n' "$prefix"
