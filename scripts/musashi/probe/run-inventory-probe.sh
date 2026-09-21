#!/bin/sh
# Drives ignored-inventory.py over six logs, so the check is run once over a
# workspace it must accept and once over each shape it must refuse. A check that
# accepts a log holding a silent binary and a check that refuses a log where
# every binary asserted something both fail here.
#
# Usage: run-inventory-probe.sh [tree root]
#
# Expected verdict line: INVENTORY PROBE 6 arms, 6 as expected
#
# Reads only the logs under test_data/inventory-arms and the recorded list, and
# talks to no node.
set -u

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=${1:-$(cd "$HERE/../../.." && pwd)}

ARMS="$ROOT/test_data/inventory-arms"
LIST="$ROOT/test_data/all-ignored-binaries.toml"
CHECK="$ROOT/scripts/musashi/ignored-inventory.py"

expected=0
met=0

# An arm names the exit it wants and the line it wants, because an exit alone
# says the check objected and not what it objected to, and a check that reaches
# the right exit by the wrong reading has not been tested.
arm() {
  log=$1
  want=$2
  line=$3
  expected=$((expected + 1))

  out=$(python3 "$CHECK" "$ARMS/$log" "$LIST" 2>&1)
  got=$?

  case "$out" in
    *"$line"*) said=yes ;;
    *) said=no ;;
  esac

  if [ "$got" -eq "$want" ] && [ "$said" = yes ]; then
    met=$((met + 1))
    echo "ARM    $log exited $got and said $line"
  else
    echo "ARM    $log exited $got wanting $want, said the line $said"
    printf '%s\n' "$out" | sed 's/^/       /'
  fi
}

arm agree.log 0 "INVENTORY 3 binaries, 0 all ignored, 0 empty, 0 unrecorded, 0 stale"
arm unrecorded.log 1 "UNRECORDED orphan tests/orphan.rs ignored 2 and passed nothing"
arm stale.log 1 "STALE smoke tests/e2e/smoke.rs passes a test now"
arm no-binary.log 1 "names no test binary"
arm failure.log 1 "REFUSED the log holds 1 test failures"
arm ambiguous.log 1 "AMBIGUOUS dolos unittests src/lib.rs appears more than once"

echo "INVENTORY PROBE $expected arms, $met as expected"
[ "$expected" -eq "$met" ] || exit 1
