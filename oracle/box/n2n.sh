# The node to node leg: one session of the head with a live Leios peer, judged against the blocks the serving Dolos holds, run by the done tier alone.
# Sourced by run.sh after lib.sh. Reads ROOT, OUT, HERE, IN_IMAGE, GRPCURL, LAST_N2N, N2N_PEER, N2N_ENDPOINT and N2N_BLOCKS.

# The record of the inputs of the last passing node to node leg.
LAST_N2N=${LAST_N2N:-$ROOT/runs/last-n2n}
# The address and network magic of the Leios peer the leg talks to.
N2N_PEER=${N2N_PEER:-leios-node.play.dev.cardano.org:3001 164}
# The utxorpc endpoint whose blocks the leg judges the peer's endorser blocks against.
N2N_ENDPOINT=${N2N_ENDPOINT:-u5c.musashi.sundae.fi:443}
# The grpcurl the leg reads the endpoint with.
GRPCURL=${GRPCURL:-grpcurl}
# The number of blocks below the endpoint's tip the leg reads.
N2N_BLOCKS=${N2N_BLOCKS:-20}

# The inputs of the node to node leg at the commit $1: the commit, the peer, the endpoint and the hash of this file.
n2n_inputs() {
  echo "commit=$1 peer=$(echo "$N2N_PEER" | tr ' ' /) endpoint=$N2N_ENDPOINT files=$(sha256sum <"$HERE/n2n.sh" | cut -c1-12)"
}

# Calls one endpoint method with the request $2 into the file $3 of the reads dir, trying again after a pause while the endpoint's rate limit refuses it.
n2n_read() {
  for pause in 1 2 4 8; do
    if printf '%s' "$2" | "$GRPCURL" -max-msg-sz 200000000 -d @ "$N2N_ENDPOINT" "utxorpc.v1.$1" >"$OUT/n2n/$3" 2>"$OUT/n2n.err"; then
      return 0
    fi
    grep -q 'Too Many Requests' "$OUT/n2n.err" || return 1
    sleep "$pause"
  done
  return 1
}

# Reads the endpoint's tip block and the N2N_BLOCKS blocks below it, then runs the head's node to node test against N2N_PEER, printing its lines and the leg's verdict line.
n2n_leg() {
  mkdir -p "$OUT/n2n" "$OUT/bin"
  echo "$N2N_PEER" >"$OUT/n2n/peer"
  if ! n2n_read sync.SyncService/ReadTip '{}' tip.json; then
    echo "N2N n2n-head=FAIL reason=tip-unread"
    return 1
  fi
  slot=$(jq -er .tip.slot "$OUT/n2n/tip.json")
  if ! n2n_read sync.SyncService/FetchBlock "{\"ref\":[{\"slot\":$slot}]}" "block-$slot.json"; then
    echo "N2N n2n-head=FAIL reason=block-unread slot=$slot"
    return 1
  fi
  height=$(jq -er .block.cardano.header.height "$OUT/n2n/block-$slot.json")
  below=1
  while [ "$below" -le "$N2N_BLOCKS" ]; do
    at=$((height - below))
    if ! n2n_read sync.SyncService/FetchBlock "{\"ref\":[{\"height\":$at}]}" "block-h$at.json"; then
      echo "N2N n2n-head=FAIL reason=block-unread height=$at"
      return 1
    fi
    below=$((below + 1))
  done
  if ! built=$(build_judge "$OUT/bin/n2n.build" --locked); then
    echo "N2N n2n-head=FAIL reason=judge-build-failed $built"
    return 1
  fi
  judge=$(executable "$OUT/bin/n2n.build")
  cp "$ROOT${judge#/w}" "$OUT/bin/n2n-judge"
  # The judge runs in the run's own dir, since the arms check other commits out in the scratch tree meanwhile.
  run_into_log "$OUT/n2n.log" env LIVE_DIR="/w/${OUT#"$ROOT"/}/n2n" sh "$IN_IMAGE" "${OUT#"$ROOT"/}" \
    "/w/${OUT#"$ROOT"/}/bin/n2n-judge" "$ORACLE_TESTS::n2n::n2n" --exact --ignored --nocapture || true
  grep -E '^N2N ' "$OUT/n2n.log" | cut -c1-400 || true
  if grep -q '^N2N RESULT PASS ' "$OUT/n2n.log"; then
    echo "N2N n2n-head=PASS peer=$(echo "$N2N_PEER" | tr ' ' /) tip=$slot blocks=$((N2N_BLOCKS + 1))"
  else
    echo "N2N n2n-head=FAIL peer=$(echo "$N2N_PEER" | tr ' ' /) tip=$slot log=$OUT/n2n.log"
    return 1
  fi
}
