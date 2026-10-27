#!/bin/sh
# Judges the oracle branch head and each named arm, one oracle run each.
# Usage: [REV=ref] [ARMS="head colret coin rodrigo-reverted rob-reverted"] breaks.sh
# The branch is read from the bare repository $ROOT/dolos.git.
set -eu
ROOT=${ROOT:-/opt/build/ledger-oracle-1027}
REV=${REV:-feat/ledger-oracle}
ARMS=${ARMS:-head colret coin rodrigo-reverted rob-reverted}
MIN_FREE_G=${MIN_FREE_G:-6}
HERE=$(cd "$(dirname "$0")" && pwd)
IMAGE="$HERE/in-image.sh"
SCRATCH=scratch/dolos
PALLAS=scratch/pallas
# The pallas commit the rob arm reverts f3419a3a on, one where no later change touches the lines of that merge.
ROB_BASE=${ROB_BASE:-362f5e0e2610686382752b1392fde02c84eb3b50}
TEST=serve::grpc::v1::ledger_oracle_tests::ledger_oracle
OUT=${OUT:-"$ROOT/runs/$(date -u +%Y%m%dT%H%M%SZ)"}
mkdir -p "$OUT"
. "$HERE/lib.sh"
require_free

# The source cargo resolves pallas-validate from, as the scratch tree's build sees it.
pallas_source() {
  sh "$IMAGE" "$SCRATCH" cargo tree -q -i pallas-validate --depth 0 -e normal 2>/dev/null |
    head -1 | sed -n 's/.*(\(.*\)).*/\1/p'
}

plant_rodrigo() {
  git -C "$ROOT/$SCRATCH" diff 5192df05^ 5192df05 -- src/serve/grpc/block_context.rs |
    git -C "$ROOT/$SCRATCH" apply -R
}

plant_colret() {
  if [ -n "${COLRET_FIX:-}" ]; then
    git -C "$ROOT/$SCRATCH" diff "$COLRET_FIX^" "$COLRET_FIX" -- . ':(exclude)**/*tests*' |
      git -C "$ROOT/$SCRATCH" apply -R
  else
    git -C "$ROOT/$SCRATCH" apply "$HERE/plants/colret.diff"
  fi
}

plant_coin() {
  git -C "$ROOT/$SCRATCH" apply "$HERE/plants/coin.diff"
}

plant_rob() {
  rm -rf "${ROOT:?}/$PALLAS"
  for db in "$ROOT"/cargo-home/git/db/pallas-*; do
    if git -C "$db" cat-file -e "$ROB_BASE^{commit}" 2>/dev/null; then
      git clone -q --no-checkout "$db" "$ROOT/$PALLAS"
      break
    fi
  done
  git -C "$ROOT/$PALLAS" checkout -q --detach "$ROB_BASE"
  git -C "$ROOT/$PALLAS" revert -m 1 --no-commit f3419a3a
  sed -E -i 's#^(pallas[a-z-]*) = \{ git = "https://github.com/[^/]+/pallas", rev = "[0-9a-f]+"#\1 = { path = "/w/'"$PALLAS"'/\1"#' \
    "$ROOT/$SCRATCH/Cargo.toml"
}

# Whether an arm's log holds the line its plant must produce and the head log lacks.
caught() {
  arm=$1
  case $arm in
    head) return 1 ;;
    colret) pattern='^(DIFF utxo .* ledger=none dolos=|DISAGREE supp#9 |KNOWN_STALE supp#9 row=S3 )' ;;
    coin) pattern='^DIFF utxo .* fields=([a-z_]+,)*coin(,|$)' ;;
    rodrigo-reverted) pattern='^STATE as_output disagree=' ;;
    rob-reverted) pattern='^(UNSUPPORTED_UNEXPECTED|DISAGREE|REASON_MISMATCH|STALE_MANIFEST) ic#(851|861|1215|1233) ' ;;
  esac
  if [ ! -f "$OUT/head.log" ]; then
    echo "ARM $arm NO_HEAD_LOG"
    return 1
  fi
  grep -E "$pattern" "$OUT/$arm.log" >"$OUT/$arm.expected" || true
  grep -E "$pattern" "$OUT/head.log" | grep -v -x -F -f - "$OUT/$arm.expected" >"$OUT/$arm.new" || true
  head -4 "$OUT/$arm.new"
  [ -s "$OUT/$arm.new" ]
}

head_result=SKIPPED
verdicts=""
for arm in $ARMS; do
  reset_scratch
  pin=$(sed -n 's/^pallas-validate = { git = "[^"]*", rev = "\([0-9a-f]*\)".*/\1/p' "$ROOT/$SCRATCH/Cargo.toml" | head -1)
  pin_short=$(echo "$pin" | cut -c1-7)
  locked=--locked
  case $arm in
    head) ;;
    colret) plant_colret ;;
    coin) plant_coin ;;
    rodrigo-reverted) plant_rodrigo ;;
    rob-reverted) plant_rob ; locked= ;;
    *) echo "unknown arm $arm" ; exit 2 ;;
  esac
  source=$(pallas_source)
  if [ "$arm" = rob-reverted ]; then
    pallas_rev="$(echo "$ROB_BASE" | cut -c1-7)-reverted-f3419a3a"
    if [ "$source" != "/w/$PALLAS/pallas-validate" ]; then
      echo "ARM $arm REFUSED pallas_source=$source"
      verdicts="$verdicts $arm=MISSED"
      continue
    fi
  else
    pallas_rev=$pin_short
  fi
  dolos_rev=$(git -C "$ROOT/$SCRATCH" rev-parse --short HEAD)
  echo "ARM $arm dolos=$dolos_rev pallas=$pallas_rev pallas_source=$source"
  if ! sh "$IMAGE" "$SCRATCH" cargo check -q --release $locked -p dolos --lib --tests >"$OUT/$arm.check" 2>&1; then
    echo "ARM $arm CHECK_FAILED log=$OUT/$arm.check"
    verdicts="$verdicts $arm=MISSED"
    continue
  fi
  RUST_BACKTRACE=0 sh "$IMAGE" "$SCRATCH" cargo test -q --release $locked -p dolos --lib "$TEST" \
    -- --exact --ignored --nocapture >"$OUT/$arm.log" 2>&1 || true
  if [ "$arm" = head ]; then
    grep -E '^(CORPUS|VALIDATION|DEPENDENT|KNOWN|STATE|ROLL_FAIL|RESULT|DISAGREE|REASON_MISMATCH|CASCADE|UNSUPPORTED_UNEXPECTED|STALE_MANIFEST|KNOWN_STALE|UNKNOWN_REFUSAL) ' "$OUT/$arm.log" |
      cut -c1-400 || true
    if grep -q '^RESULT PASS' "$OUT/$arm.log" && ! grep -q '^RESULT FAIL' "$OUT/$arm.log"; then
      head_result=PASS
    else
      head_result=FAIL
    fi
    verdicts="$verdicts head=$head_result"
  elif caught "$arm"; then
    echo "$arm=CAUGHT"
    verdicts="$verdicts $arm=CAUGHT"
  else
    echo "$arm=MISSED"
    verdicts="$verdicts $arm=MISSED"
  fi
done
reset_scratch

echo "ARMS$verdicts logs=$OUT"
case "$verdicts" in
  *MISSED* | *FAIL* | *SKIPPED*) exit 1 ;;
esac
[ "$head_result" = PASS ]
