#!/bin/sh
# Reads a live node and the Dolos endpoint that follows it into one directory, through queries only.
# Usage: ENDPOINT=host:port NODE=container SERVE=container BATCH_SLOTS="slot..." live.sh <dir>
set -eu
DIR=$1
GRPCURL=${GRPCURL:-grpcurl}
: "${ENDPOINT:?}" "${NODE:?}" "${SERVE:?}" "${BATCH_SLOTS:?}"
mkdir -p "$DIR"

# Calls one endpoint method from the fixed list of reads.
read_endpoint() {
  case $1 in
    sync.SyncService/ReadTip | sync.SyncService/FetchBlock) ;;
    query.QueryService/ReadParams | query.QueryService/ReadEraSummary | query.QueryService/ReadTx) ;;
    *) echo "LIVE refused method $1" >&2 ; exit 2 ;;
  esac
  "$GRPCURL" -d "$2" "$ENDPOINT" "utxorpc.v1.$1" >"$DIR/$3"
}

# Runs one node query from the fixed list of reads.
query_node() {
  case $1 in
    tip | protocol-parameters) ;;
    *) echo "LIVE refused node query $1" >&2 ; exit 2 ;;
  esac
  docker exec "$NODE" cardano-cli latest query "$1" >"$DIR/$2"
}

docker inspect "$SERVE" --format '{{.Config.Image}}' >"$DIR/image.txt"
query_node tip node-tip-before.json
read_endpoint sync.SyncService/ReadTip '{}' tip.json
read_endpoint query.QueryService/ReadParams '{}' params.json
query_node protocol-parameters node-params.json
read_endpoint query.QueryService/ReadEraSummary '{}' era.json
query_node tip node-tip-after.json

for slot in $BATCH_SLOTS; do
  read_endpoint sync.SyncService/FetchBlock "{\"ref\":[{\"slot\":$slot}]}" "block-$slot.json"
  n=0
  for hash in $(jq -r '.block.cardano.body.tx[] | select(.subTransactions) | .common.hash' "$DIR/block-$slot.json"); do
    read_endpoint query.QueryService/ReadTx "{\"hash\":\"$hash\"}" "tx-$slot-$n.json"
    n=$((n + 1))
  done
done
echo "LIVE read dir=$DIR"
