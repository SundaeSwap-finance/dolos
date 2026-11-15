#!/bin/sh
# Writes the fixture of the epoch boundary replay judge from a copy of a Musashi node's database: the blocks up to the first block of the last epoch given, and the state the node holds at the first block of each epoch given.
# Usage: SOURCE=dir CONFIG=dir [IMAGE=ref] replay-fixture.sh <out dir> <epoch>...
# SOURCE holds protocolMagicId and immutable/ of a node database copied read only, never a running node's own dirs, and CONFIG the node's config.json and genesis files.
set -eu
: "${SOURCE:?}" "${CONFIG:?}"
IMAGE=${IMAGE:-ghcr.io/input-output-hk/ouroboros-leios/cardano-node-testnet:rc-2026w45}
OUT=$1
shift
HERE=$(cd "$(dirname "$0")" && pwd)
# A Musashi epoch is 21600 slots and an immutable chunk 1080, so epoch E starts at chunk 20E.
CHUNKS_PER_EPOCH=20
WORK=$OUT/work
NAME=replay-fixture-node
mkdir -p "$OUT"

# Runs cardano-cli of the image against the socket of the node run on $WORK.
cli() {
  docker run --rm -v "$WORK:/data" --entrypoint cardano-cli "$IMAGE" "$@" --testnet-magic 164 --socket-path /data/node.socket
}

# Writes the ledger state and the tip of a node run with no peers over chunks 0 to $1 and the first block of the next chunk.
node_state() {
  last=$1
  next=$(printf %05d $((last + 1)))
  # The node writes its database as root, so the previous run's copy is removed from inside a container.
  docker run --rm -v "$OUT:/o" --entrypoint rm "$IMAGE" -rf /o/work
  mkdir -p "$WORK/db/immutable" "$WORK/config"
  cp "$SOURCE/protocolMagicId" "$WORK/db/"
  n=0
  while [ "$n" -le "$last" ]; do
    for ext in chunk primary secondary; do
      cp "$SOURCE/immutable/$(printf %05d "$n").$ext" "$WORK/db/immutable/"
    done
    n=$((n + 1))
  done
  # The second entry of a secondary index starts with the offset of the second block, which is the size of the first.
  first=$(od -An -t u1 -j 56 -N 8 "$SOURCE/immutable/$next.secondary" | awk '{ v = 0; for (i = 1; i <= NF; i++) v = v * 256 + $i; print v }')
  head -c "$first" "$SOURCE/immutable/$next.chunk" >"$WORK/db/immutable/$next.chunk"
  # A secondary index entry holds the block's header hash at bytes 16 to 48.
  target=$(od -An -t x1 -j 16 -N 32 "$SOURCE/immutable/$next.secondary" | tr -d ' \n')
  cp "$CONFIG"/*.json "$WORK/config/"
  echo '{"localRoots":[],"publicRoots":[],"useLedgerAfterSlot":-1}' >"$WORK/config/topology.json"
  docker rm -f "$NAME" >/dev/null 2>&1 || true
  docker run -d --name "$NAME" -v "$WORK:/data" --entrypoint cardano-node "$IMAGE" run \
    --config /data/config/config.json --topology /data/config/topology.json \
    --database-path /data/db --socket-path /data/node.socket --port 3001 >/dev/null
  # The node answers once it has replayed the copy, and its tip names the target block once it has applied it.
  until cli query tip 2>/dev/null | grep -q "\"hash\": \"$target\""; do
    if ! docker ps -q -f name="$NAME" | grep -q .; then
      docker logs --tail 40 "$NAME"
      exit 1
    fi
    sleep 2
  done
  cli query ledger-state --out-file /data/ledger-state.json
  cli query tip >"$2"
  docker run --rm -v "$WORK:/data" --entrypoint chmod "$IMAGE" 644 /data/ledger-state.json
  docker rm -f "$NAME" >/dev/null
}

pairs=
for epoch in "$@"; do
  last=$((CHUNKS_PER_EPOCH * epoch - 1))
  node_state "$last" "$OUT/tip-e$epoch.json"
  cp "$WORK/ledger-state.json" "$OUT/ledger-state-e$epoch.json"
  pairs="$pairs $OUT/ledger-state-e$epoch.json $OUT/tip-e$epoch.json"
done
docker run --rm -v "$OUT:/o" --entrypoint rm "$IMAGE" -rf /o/work
python3 -I "$HERE/replay_fixture.py" blocks "$SOURCE/immutable" "$last" | gzip -9 -n >"$OUT/replay-blocks.cbor.gz"
python3 -I "$HERE/replay_fixture.py" checkpoints $pairs >"$OUT/replay-checkpoints.json"
echo "FIXTURE epochs=\"$*\" last_chunk=$last blocks_bytes=$(wc -c <"$OUT/replay-blocks.cbor.gz" | tr -dc 0-9) checkpoints_bytes=$(wc -c <"$OUT/replay-checkpoints.json" | tr -dc 0-9) out=$OUT"
