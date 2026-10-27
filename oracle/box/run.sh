#!/bin/sh
# Runs the oracle on the build box, its done run or the regeneration of its committed manifests.
# Usage: [REV=ref] run.sh done|regen
# The live leg reads ENDPOINT, NODE, SERVE, GRPCURL and BATCH_SLOTS from $LIVE_ENV.
set -eu
ROOT=${ROOT:-/opt/build/ledger-oracle-1027}
REV=${REV:-feat/ledger-oracle}
MIN_FREE_G=${MIN_FREE_G:-6}
HERE=$(cd "$(dirname "$0")" && pwd)
LIVE_ENV=${LIVE_ENV:-$ROOT/live.env}
REGEN_DIR=${REGEN_DIR:-$ROOT/regen}
SCRATCH=scratch/dolos
TEST=serve::grpc::v1::ledger_oracle_tests
OUT=${OUT:-"$ROOT/runs/$(date -u +%Y%m%dT%H%M%SZ)"}
case $OUT in
  "$ROOT"/*) ;;
  *) echo "REFUSED OUT=$OUT is outside $ROOT" ; exit 2 ;;
esac
export ROOT REV MIN_FREE_G OUT
. "$HERE/lib.sh"

# Reads the endpoint and the node into $OUT/live and judges the reads in the scratch tree, logging to $OUT/live.log.
live_leg() {
  set -a
  . "$LIVE_ENV"
  set +a
  sh "$HERE/live.sh" "$OUT/live" &&
    LIVE_DIR="/w/${OUT#"$ROOT"/}/live" sh "$HERE/in-image.sh" "$SCRATCH" \
      cargo test -q --release --locked -p dolos --lib "$TEST::live::live" \
      -- --exact --ignored --nocapture >"$OUT/live.log" 2>&1
}

done_run() {
  mkdir -p "$OUT"
  sh "$HERE/breaks.sh" | tee "$OUT/breaks.out"
  arms=$(sed -n 's/^ARMS \(.*\) logs=.*/\1/p' "$OUT/breaks.out")
  live=FAIL
  if live_leg; then
    live=PASS
  fi
  grep -E '^(LIVE|KNOWN|KNOWN_STALE|LIVE-DIFF|LIVE-GAP) ' "$OUT/live.log" | cut -c1-400 || true
  echo "DONE $arms live=$live logs=$OUT"
  case " $arms " in
    *" head=PASS "*) ;;
    *) exit 1 ;;
  esac
  case "$arms live=$live" in
    *MISSED* | *FAIL* | *SKIPPED*) exit 1 ;;
  esac
}

regen() {
  mkdir -p "$OUT"
  require_free
  reset_scratch
  sh "$HERE/in-image.sh" "$SCRATCH" cargo test -q --release --locked -p dolos --lib \
    "$TEST::ledger_oracle" -- --exact --ignored --nocapture >"$OUT/head.log" 2>&1 || true
  if ! grep -q '^RESULT ' "$OUT/head.log"; then
    echo "REGEN REFUSED the head run printed no RESULT log=$OUT/head.log"
    exit 1
  fi
  live_leg || true
  if ! grep -q '^LIVE RESULT ' "$OUT/live.log"; then
    echo "REGEN REFUSED the live leg printed no LIVE RESULT log=$OUT/live.log"
    exit 1
  fi
  mkdir -p "$REGEN_DIR"
  committed=test_data/ledger-oracle
  for file in expected-unsupported.txt known.txt; do
    cat "$OUT/head.log" "$OUT/live.log" | sed -n "s/^CANDIDATE $file //p" >"$REGEN_DIR/$file"
  done
  changed=$(git -C "$ROOT/$SCRATCH" status --porcelain -- \
    "$committed/expected-unsupported.txt" "$committed/known.txt")
  if [ -n "$changed" ]; then
    echo "REGEN FAIL the run changed a committed manifest: $changed"
    exit 1
  fi
  for file in expected-unsupported.txt known.txt; do
    lines=$(wc -l <"$REGEN_DIR/$file" | tr -dc 0-9)
    if git diff --no-index -- "$ROOT/$SCRATCH/$committed/$file" "$REGEN_DIR/$file"; then
      echo "REGEN $file lines=$lines same"
    else
      echo "REGEN $file lines=$lines differs"
    fi
  done
  echo "REGEN tree=unchanged candidates=$REGEN_DIR logs=$OUT"
}

case ${1:-} in
  done) done_run ;;
  regen) regen ;;
  *) echo "usage: run.sh done|regen" ; exit 2 ;;
esac
