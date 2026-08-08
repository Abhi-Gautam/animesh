#!/usr/bin/env bash
# Builds animesh on the Pi and proves the parts that have only ever run
# elsewhere. Run after pi-bootstrap.sh.
#
#   ./scripts/pi-verify.sh [checkout-dir]     default: ~/animesh
set -euo pipefail

DIR="${1:-$HOME/animesh}"
REPO="https://github.com/Abhi-Gautam/animesh.git"

if [ ! -d "$DIR/.git" ]; then
  echo "== cloning into $DIR =="
  git clone "$REPO" "$DIR"
fi
cd "$DIR"
echo "== $DIR @ $(git rev-parse --short HEAD) ($(git branch --show-current)) =="

echo
echo "== build =="
cargo build --bins --locked

echo
echo "== test suite =="
cargo test --workspace --locked 2>&1 | grep -E "^test result:" \
  | awk -F'[ ;]' '{p+=$4; f+=$6} END {print "  " p " passed, " f " failed"}'

echo
echo "== does this box have a notification server? =="
if [ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
  if busctl --user status org.freedesktop.Notifications >/dev/null 2>&1; then
    echo "  yes — the D-Bus adapter can be exercised for real"
  else
    echo "  session bus present, no notification server (headless-ish)"
  fi
else
  echo "  no session bus — the adapter should degrade, not fail"
fi

echo
echo "== systemd user manager =="
systemctl --user is-system-running 2>&1 | sed 's/^/  /'
echo "  (animesh service start is exercised separately, on purpose:"
echo "   it registers a real unit and should be watched, not scripted)"
