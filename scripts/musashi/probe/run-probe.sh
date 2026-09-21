#!/bin/sh
# Drives both comparison scripts against a node and a follower that answer from
# fixtures, so each script is run once over a ledger that agrees and once over
# the same ledger with a known fault in it. A script that cannot report a fault
# and a script that reports one where there is none both fail here.
#
# Usage: run-probe.sh
#
# Expected verdict line: PROBE 6 arms, 6 as expected
#
# Reads nothing outside this directory and talks to no node.
set -u

HERE=$(cd "$(dirname "$0")" && pwd)
SCRIPTS=$(dirname "$HERE")

CARDANO_CLI="$HERE/fake-node"
GRPCURL="$HERE/fake-grpcurl"
export CARDANO_CLI GRPCURL
export DOLOS_GRPC=probe
export LEVEL_TIMEOUT=0
export EXTRA_KEYS="$HERE/extra-keys.txt"

chmod +x "$CARDANO_CLI" "$GRPCURL"

expected=0
met=0

# An arm names the exit it wants and the line it wants, because an exit alone
# says a script objected and not what it objected to, and a script that reaches
# the right exit by the wrong reading has not been tested.
arm() {
  name=$1
  script=$2
  want=$3
  line=$4
  expected=$((expected + 1))

  out=$(PROBE_ARM="$name" python3 "$SCRIPTS/$script" 2>&1)
  got=$?

  case "$out" in
    *"$line"*) said=yes ;;
    *) said=no ;;
  esac

  if [ "$got" -eq "$want" ] && [ "$said" = yes ]; then
    met=$((met + 1))
    echo "ARM    $script $name exited $got and said $line"
  else
    echo "ARM    $script $name exited $got wanting $want, said the line $said"
    printf '%s\n' "$out" | sed 's/^/       /'
  fi
}

HOLLOW_KEY=3333333333333333333333333333333333333333333333333333333333333333#0
EXTRA_KEY=6666666666666666666666666666666666666666666666666666666666666666#0

arm agree compare-params.py 0 "PARAMS 33 compared, 0 differing"
arm diverge compare-params.py 1 "DIFFERS poolRetirementEpochBound costModels.plutusV4"
arm agree compare-utxo-set.py 0 "only on node 0, value diffs 0, only on follower 0"
arm diverge compare-utxo-set.py 1 "only on node 1, value diffs 1"
arm hollow compare-utxo-set.py 1 "ONLYNODE $HOLLOW_KEY 3000000"
arm extra compare-utxo-set.py 1 "ONLYFOLLOWER $EXTRA_KEY"

echo "PROBE $expected arms, $met as expected"
[ "$expected" -eq "$met" ] || exit 1
