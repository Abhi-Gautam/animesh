#!/bin/sh
# Stage complete releases and atomically switch one shared current pointer.
set -eu
umask 077
if [ "$(id -u)" -eq 0 ]; then
    printf '%s\n' 'Install as your own user. Use the .deb package for a system installation.' >&2
    exit 1
fi
prefix=${1:-"$HOME/.local"}
source_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
mkdir -p -- "$prefix"
prefix=$(CDPATH= cd -P -- "$prefix" && pwd)
base="$prefix/lib/animesh"
for dir in "$prefix/lib" "$base" "$prefix/bin"; do
    if [ -L "$dir" ]; then printf 'Refusing linked install directory: %s\n' "$dir" >&2; exit 1; fi
    mkdir -p -- "$dir"
    if [ "$(stat -c %u "$dir")" != "$(id -u)" ] || [ -n "$(find "$dir" -maxdepth 0 -perm /022 -print)" ]; then
        printf 'Unsafe install directory: %s\n' "$dir" >&2; exit 1
    fi
done
stage=$(mktemp -d "$base/release.XXXXXXXX")
old=''
switched=false
cleanup() {
    code=$?
    trap - EXIT HUP INT TERM
    if [ "$code" -ne 0 ]; then
        if [ "$switched" = true ]; then
            if [ -n "$old" ]; then ln -s -- "$old" "$base/.rollback.$$"; mv -Tf -- "$base/.rollback.$$" "$base/current"; else rm -f -- "$base/current"; fi
        fi
        rm -rf -- "$stage"
    fi
    rm -f -- "$base/.current.$$"
    exit "$code"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
for name in animesh animesh-app animesh-desktop; do
    if [ ! -f "$source_dir/bin/$name" ] || [ -L "$source_dir/bin/$name" ]; then printf 'Missing regular executable: %s\n' "$name" >&2; exit 1; fi
done
cp -R -- "$source_dir/bin" "$source_dir/share" "$stage/"
for name in animesh animesh-app animesh-desktop; do chmod 755 "$stage/bin/$name"; done
# Verify executable completeness before any installed pointer changes.
"$stage/bin/animesh" --version >/dev/null
"$stage/bin/animesh-desktop" --version >/dev/null
if [ -L "$base/current" ]; then old=$(readlink "$base/current"); elif [ -e "$base/current" ]; then printf '%s\n' 'Current release pointer is not a symbolic link.' >&2; exit 1; fi
ln -s -- "$stage" "$base/.current.$$"
mv -Tf -- "$base/.current.$$" "$base/current"
switched=true
for name in animesh animesh-app animesh-desktop; do
    ln -s -- "$base/current/bin/$name" "$prefix/bin/.$name.$$"
    mv -Tf -- "$prefix/bin/.$name.$$" "$prefix/bin/$name"
done
if ! "$prefix/bin/animesh" service restart; then
    # Put the previous release back before returning an unsuccessful installation.
    if [ -n "$old" ]; then
        ln -s -- "$old" "$base/.rollback.$$"
        mv -Tf -- "$base/.rollback.$$" "$base/current"
        switched=false
        "$prefix/bin/animesh" service restart || true
    fi
    printf '%s\n' 'Service startup failed; the previous release was preserved.' >&2
    exit 1
fi
# Keep the current and previous version for recovery; reclaim older app-owned stages.
for release in "$base"/release.*; do
    [ "$release" = "$stage" ] && continue
    [ "$release" = "$old" ] && continue
    [ -d "$release" ] && [ ! -L "$release" ] && rm -rf -- "$release"
done
printf '\nInstalled Animesh in %s. Open Animesh in your applications menu.\n' "$prefix"
