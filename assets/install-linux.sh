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
for dir in "$prefix" "$prefix/lib" "$base" "$prefix/bin"; do
    if [ -L "$dir" ]; then printf 'Refusing linked install directory: %s\n' "$dir" >&2; exit 1; fi
    mkdir -p -- "$dir"
    if [ "$(stat -c %u "$dir")" != "$(id -u)" ] || [ -n "$(find "$dir" -maxdepth 0 -perm /022 -print)" ]; then
        printf 'Unsafe install directory: %s\n' "$dir" >&2; exit 1
    fi
done
# Serialize installer processes without leaving a stale lock after interruption.
if ! command -v flock >/dev/null 2>&1; then printf '%s\n' 'This installer requires flock from util-linux.' >&2; exit 1; fi
if [ -L "$base/.install.lock" ] || { [ -e "$base/.install.lock" ] && { [ ! -f "$base/.install.lock" ] || [ "$(stat -c %u "$base/.install.lock")" != "$(id -u)" ]; }; }; then
    printf '%s\n' 'Refusing unsafe installation lock.' >&2; exit 1
fi
exec 9>>"$base/.install.lock"
if ! flock -n 9; then printf '%s\n' 'Another installation is already running; no installed files were changed.' >&2; exit 1; fi
stage=$(mktemp -d "$base/release.XXXXXXXX")
old=''
switched=false
cleanup() {
    code=$?
    trap - EXIT HUP INT TERM
    if [ "$code" -ne 0 ]; then
        if [ "$switched" = true ]; then
            if [ -n "$old" ]; then
                ln -s -- "$old" "$base/.rollback.$$"
                mv -Tf -- "$base/.rollback.$$" "$base/current"
            else
                rm -f -- "$base/current"
            fi
        fi
        if [ -z "$old" ]; then
            for name in animesh animesh-app animesh-desktop; do
                if [ -L "$prefix/bin/$name" ] && [ "$(readlink "$prefix/bin/$name")" = "$base/current/bin/$name" ]; then rm -f -- "$prefix/bin/$name"; fi
            done
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
# Adopt the old flat tarball layout before replacing any installed executable.
# During adoption every launcher still resolves to the complete old version.
if [ -z "$old" ]; then
    existing=0
    for name in animesh animesh-app animesh-desktop; do
        if [ -e "$prefix/bin/$name" ] || [ -L "$prefix/bin/$name" ]; then
            if [ ! -f "$prefix/bin/$name" ] || [ -L "$prefix/bin/$name" ] || [ "$(stat -c %u "$prefix/bin/$name")" != "$(id -u)" ]; then
                printf 'Refusing unmanaged executable: %s\n' "$prefix/bin/$name" >&2; exit 1
            fi
            existing=$((existing + 1))
        fi
    done
    if [ "$existing" -ne 0 ] && [ "$existing" -ne 3 ]; then
        printf '%s\n' 'The existing installation is incomplete; no installed files were changed.' >&2; exit 1
    fi
    if [ "$existing" -eq 3 ]; then
        legacy=$(mktemp -d "$base/release.XXXXXXXX")
        mkdir -- "$legacy/bin"
        for name in animesh animesh-app animesh-desktop; do cp -p -- "$prefix/bin/$name" "$legacy/bin/$name"; done
        ln -s -- "$legacy" "$base/.current.$$"
        mv -Tf -- "$base/.current.$$" "$base/current"
        old=$legacy
    fi
fi
for name in animesh animesh-app animesh-desktop; do
    ln -s -- "$base/current/bin/$name" "$prefix/bin/.$name.$$"
    mv -Tf -- "$prefix/bin/.$name.$$" "$prefix/bin/$name"
done
ln -s -- "$stage" "$base/.current.$$"
mv -Tf -- "$base/.current.$$" "$base/current"
switched=true
# The service registers the desktop launcher and embedded icons in XDG_DATA_HOME.
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
