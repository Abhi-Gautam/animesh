#!/usr/bin/env bash
# Opens a walk session in gdb with its breakpoints already set.
#
#   ./scripts/walk.sh 1          interactive — you drive
#   ./scripts/walk.sh 1 --batch  run it through once and print
#
# Sessions are listed in plans/002-walking-the-code.md. Nothing here is
# required: it only removes the two frictions — the test binary has a content
# hash in its name, and libtest --exact wants the fully-qualified test path.
set -euo pipefail
cd "$(dirname "$0")/.."

SESSION="${1:?usage: walk.sh <session> [--batch]}"
BATCH="${2:-}"

# kind: lib | test:<name>   test: fully-qualified   breaks: space-separated
case "$SESSION" in
  1)  KIND=lib
      TEST=domain::ids::tests::ids_reject_out_of_range_during_deserialization
      BREAKS="animesh::domain::ids::AniListId::new"
      ASK="Before you continue: what is \`value\` the first time, and the second?"
      ;;
  2)  KIND=lib
      TEST=library::reducers::tests::a_later_episode_before_airtime_supersedes_the_old_one
      BREAKS="animesh::library::reducers::reduce_release"
      ASK="Before you step out: which ReleaseTransition, and why that retire_state?"
      ;;
  *)  echo "session $SESSION is not wired up yet; add it to scripts/walk.sh" >&2
      exit 1 ;;
esac

case "$KIND" in
  lib)     LOOKUP=(cargo test --lib --no-run --message-format=json) ;;
  test:*)  LOOKUP=(cargo test --test "${KIND#test:}" --no-run --message-format=json) ;;
esac

BIN=$("${LOOKUP[@]}" 2>/dev/null \
      | grep -o '"executable":"[^"]*"' | grep -v null | tail -1 | cut -d'"' -f4)
[ -n "$BIN" ] || { echo "could not find the test binary" >&2; exit 1; }

ARGS=(-q)
for b in $BREAKS; do ARGS+=(-ex "break $b"); done

if [ "$BATCH" = "--batch" ]; then
  ARGS+=(-batch -ex "run $TEST --exact --nocapture" -ex "info args" -ex "bt 8")
else
  echo "session $SESSION"
  echo "  binary: $BIN"
  echo "  test:   $TEST"
  echo "  breaks: $BREAKS"
  echo
  echo "  type:  run $TEST --exact --nocapture"
  echo "  then:  info args | bt | p <var> | continue | finish"
  echo
  echo "  $ASK"
  echo
fi

exec rust-gdb "${ARGS[@]}" "$BIN"
