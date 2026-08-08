#!/usr/bin/env bash
# Prepares a Raspberry Pi to build, run and trace animesh.
#
# Safe to re-run. Reports the environment first, because the glibc version is
# what decides whether a binary built elsewhere will run here at all.
set -euo pipefail

echo "== this machine =="
echo "  arch:    $(uname -m)"
echo "  kernel:  $(uname -r)"
. /etc/os-release && echo "  distro:  $PRETTY_NAME"
echo "  glibc:   $(ldd --version | head -1 | awk '{print $NF}')"
echo "  cores:   $(nproc)"
echo "  ram:     $(free -h | awk '/Mem:/{print $2}')"
echo "  systemd: $(systemctl --user is-system-running 2>&1 | head -1)"
echo "  dbus:    ${DBUS_SESSION_BUS_ADDRESS:-none}"
echo "  storage: $(findmnt -no SOURCE,FSTYPE / 2>/dev/null || echo unknown)"

echo
echo "== toolchain =="
sudo apt-get update -qq
# build-essential and pkg-config for linking; the rest is the reason we are here.
sudo apt-get install -y -qq \
  build-essential pkg-config git curl \
  gdb strace ltrace linux-perf \
  sqlite3 jq

if ! command -v rustup >/dev/null; then
  curl -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.96.1 --profile minimal -c clippy,rustfmt
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
fi
rustup toolchain install 1.96.1 -c clippy,rustfmt >/dev/null 2>&1 || true

echo
echo "== versions =="
echo "  rustc: $(rustc --version 2>/dev/null || echo 'restart your shell')"
echo "  gdb:   $(gdb --version | head -1)"
echo "  perf:  $(perf --version 2>&1 | head -1)"

echo
echo "== perf permissions =="
p=$(cat /proc/sys/kernel/perf_event_paranoid 2>/dev/null || echo '?')
echo "  perf_event_paranoid = $p"
[ "$p" != "?" ] && [ "$p" -gt 1 ] && echo "  → for user-space profiling: sudo sysctl kernel.perf_event_paranoid=1"

echo
echo "ready."
