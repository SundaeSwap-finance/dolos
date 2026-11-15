#!/bin/sh
# Writes the fixtures of the node to node tests, the peer's reply to each recorded handshake proposal and the endorser block certified at a slot as the peer delivers it beside the transactions the endpoint serves for it.
# Usage: ROOT=dir TREE=dir ENDPOINT=host:port GRPCURL=path PEER="host:port magic" n2n-fixture.sh <out dir> <certifying slot>
set -eu
: "${ROOT:?}" "${TREE:?}" "${ENDPOINT:?}" "${GRPCURL:?}" "${PEER:?}"
OUT=$1
SLOT=$2
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/lib.sh"
enter_slice "$0" "$OUT" "$SLOT"
BUILD_IMAGE=$(build_image dolos-build)
export BUILD_IMAGE
WORK=fixture-work/n2n
DIR=$ROOT/$WORK
rm -rf "$DIR"
mkdir -p "$DIR"

# Writes one FetchBlock answer of the endpoint for a block reference.
fetch() {
  "$GRPCURL" -max-msg-sz 200000000 -d "{\"ref\":[$1]}" "$ENDPOINT" utxorpc.v1.sync.SyncService/FetchBlock >"$DIR/$2"
}

echo "$PEER" >"$DIR/peer"
echo "$SLOT" >"$DIR/slot"
fetch "{\"slot\":$SLOT}" "block-$SLOT.json"
height=$(jq -er .block.cardano.header.height "$DIR/block-$SLOT.json")
fetch "{\"height\":$((height - 1))}" "parent-$SLOT.json"
FIXTURE_DIR=/w/$WORK sh "$HERE/in-image.sh" "$TREE" cargo test --release --locked -p dolos --lib -- \
  serve::grpc::v1::ledger_oracle_tests::n2n::record_n2n --exact --ignored --nocapture
mkdir -p "$OUT"
cp "$DIR/out/handshake.txt" "$DIR/out/fetch.txt" "$OUT/"
echo "FIXTURE handshake=$(wc -l <"$OUT/handshake.txt" | tr -dc 0-9) fetch=$(wc -l <"$OUT/fetch.txt" | tr -dc 0-9) out=$OUT"
