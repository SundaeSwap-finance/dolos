#!/bin/sh
# Runs one tier of the oracle at REV, and oracle/README.md says what each tier checks and when to run it.
# Usage: ROOT=dir [REV=ref] [BASE=ref] run.sh quick|done
# The quick tier judges the arms whose plants change a file that differs between BASE and REV, and BASE defaults to the commit of the last done run that passed. The done tier records its commit in runs/last-done when it passes.
# A tier fails when a caught or held arm served other lines than its row of served.txt records, and a change that alters what an arm serves on purpose copies the arm's row from the run's served.txt into served.txt.
set -eu
: "${ROOT:?}"
BASE=${BASE:-}
LAST_DONE="$ROOT/runs/last-done"
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/lib.sh"
. "$HERE/value.sh"
. "$HERE/amaru.sh"
trap finish EXIT
case ${1:-} in
  quick | done) ;;
  *) echo "usage: run.sh quick|done" ; exit 2 ;;
esac
start_run "$0" "$@"

# Judges the head, the control and the arms the change from BASE touches, every arm when no BASE is known.
quick_run() {
  mkdir -p "$OUT"
  require_free
  require_lib_test
  require_build_image
  reset_scratch
  commit=$(git -C "$ROOT/$SCRATCH" rev-parse HEAD)
  if [ -z "$BASE" ] && [ -f "$LAST_DONE" ]; then
    BASE=$(cat "$LAST_DONE")
  fi
  base=none
  arms=$PLANTED_ARMS
  if [ -n "$BASE" ]; then
    git -C "$ROOT/$SCRATCH" fetch -q "$ROOT/dolos.git" "$BASE"
    base=$(git -C "$ROOT/$SCRATCH" rev-parse FETCH_HEAD)
    touched_arms "$base" "$commit" >"$OUT/arms"
    arms=$(paste -sd ' ' - <"$OUT/arms")
  fi
  echo "QUICK base=$base arms=\"$arms\""
  ARMS="head control${arms:+ $arms}" sh "$HERE/breaks.sh" | tee "$OUT/breaks.out"
  verdicts=$(sed -n 's/^ARMS \(.*\) logs=.*/\1/p' "$OUT/breaks.out")
  verdicts="$verdicts $(value_verdicts)"
  echo "DONE tier=quick $verdicts base=$base commit=$commit logs=$OUT"
  passed "$verdicts"
}

# Sets checked to the verdicts of the leg $1 whose run wrote $OUT/$1.out and whose verdict line the sed script $2 prints, its head failing when none does, records the inputs $4 in the file $3 when every verdict passes, and prints the leg's lines.
leg_verdicts() {
  cat "$OUT/$1.out"
  served_check "$(sed -n "$2" "$OUT/$1.out" | grep . || echo "$1-head=FAIL")"
  if passed "head=PASS $checked"; then
    echo "$4" >"$3"
  fi
}

# Runs over a reset scratch tree the value leg and the amaru leg, each when its inputs differ from those of its last passing run, printing their lines and returning their verdicts on the last line, a leg not run as its name and passed-on-these-inputs.
value_verdicts() {
  reset_scratch >&2
  pin=$(pallas_pin)
  inputs=$(value_inputs "$pin")
  if leg_due "$LAST_VALUE" "$inputs"; then
    value_leg >"$OUT/value.out" 2>&1 || true
    leg_verdicts value 's/^VALUE \(value-head=.*\) pin=.*/\1/p' "$LAST_VALUE" "$inputs" >&2
    legs=$checked
  else
    echo "VALUE due=no $inputs" >&2
    legs=value=passed-on-these-inputs
  fi
  pallas_history "$pin" >&2 || true
  inputs=$(amaru_inputs "$(amaru_pin "$pin")" "$(image_id "$BUILD_IMAGE")")
  if leg_due "$LAST_AMARU" "$inputs"; then
    amaru_leg "$pin" >"$OUT/amaru.out" 2>&1 || true
    leg_verdicts amaru 's/^AMARU \(amaru-head=[A-Z]*\( amaru-[a-z0-9-]*=[A-Z]*\)*\).*/\1/p' "$LAST_AMARU" "$inputs" >&2
    legs="$legs $checked"
  else
    echo "AMARU due=no $inputs" >&2
    legs="$legs amaru=passed-on-these-inputs"
  fi
  reset_scratch >&2
  echo "$legs"
}

# Judges the fix arms, the head, the control and every arm, prints their lines and sets verdicts to their verdicts and those of the value and amaru legs.
judge_arms() {
  fix_arms=fix-arms=PASS
  fix_arms_both >"$OUT/fix-arms.out" 2>&1 || fix_arms=fix-arms=FAIL
  cat "$OUT/fix-arms.out"
  sh "$HERE/breaks.sh" | tee "$OUT/breaks.out"
  verdicts=$(sed -n 's/^ARMS \(.*\) logs=.*/\1/p' "$OUT/breaks.out")
  verdicts="$verdicts $fix_arms $(value_verdicts)"
}

# Judges the head, the control and every arm, and records the commit when every verdict passes.
done_run() {
  mkdir -p "$OUT"
  require_free
  require_lib_test
  require_build_image
  reset_scratch
  commit=$(git -C "$ROOT/$SCRATCH" rev-parse HEAD)
  judge_arms
  echo "DONE tier=done $verdicts commit=$commit logs=$OUT"
  record_pass "$commit" "$verdicts"
}

"${1}_run"
