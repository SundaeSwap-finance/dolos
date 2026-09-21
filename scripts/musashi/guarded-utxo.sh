#!/bin/sh
# Whole utxo dump from the node, under a guard, so a dump that would press the
# box is killed rather than allowed to finish. Resident size above the cap,
# available memory below the floor, free space on /opt below the floor, or the
# time cap, each stop it. A guard whose reading cannot be taken is named on
# stdout and is not armed, so a run never counts an unreadable guard as passing.
#
# Usage: guarded-utxo.sh <output path>
#
# Exit 0 with the dump written, 9 when a guard stopped it, or whatever the node
# exited with.
set -u

OUT=$1
CARDANO_CLI=${CARDANO_CLI:-/usr/local/bin/cardano-cli}
RSS_CAP_KB=${RSS_CAP_KB:-3000000}
MEM_FLOOR_KB=${MEM_FLOOR_KB:-1200000}
DISK_FLOOR_KB=${DISK_FLOOR_KB:-6000000}
DISK_PATH=${DISK_PATH:-/opt}
TIME_CAP=${TIME_CAP:-900}

if [ ! -r /proc/meminfo ]; then
  echo "GUARD UNARMED no /proc/meminfo, available memory is not watched"
fi

"$CARDANO_CLI" query utxo --whole-utxo --output-json --out-file "$OUT" &
PID=$!

i=0
while kill -0 "$PID" 2>/dev/null; do
  i=$((i + 1))
  rss=$(awk '/VmRSS/{print $2}' "/proc/$PID/status" 2>/dev/null || echo 0)
  avail=$(awk '/MemAvailable/{print $2}' /proc/meminfo 2>/dev/null || echo "")
  dfree=$(df -k "$DISK_PATH" | awk 'NR==2{print $4}')

  stop=""
  [ "${rss:-0}" -gt "$RSS_CAP_KB" ] && stop="resident size ${rss}kB over ${RSS_CAP_KB}kB"
  [ -n "$avail" ] && [ "$avail" -lt "$MEM_FLOOR_KB" ] && stop="available memory ${avail}kB under ${MEM_FLOOR_KB}kB"
  [ "$dfree" -lt "$DISK_FLOOR_KB" ] && stop="free space ${dfree}kB under ${DISK_FLOOR_KB}kB"
  [ "$i" -gt "$TIME_CAP" ] && stop="ran ${i}s over the ${TIME_CAP}s cap"

  if [ -n "$stop" ]; then
    echo "GUARD STOP $stop"
    kill "$PID" 2>/dev/null
    wait "$PID" 2>/dev/null
    exit 9
  fi

  [ $((i % 15)) -eq 0 ] && echo "at ${i}s rss=${rss:-0}kB avail=${avail:-unread}kB dfree=${dfree}kB out=$(stat -c %s "$OUT" 2>/dev/null || echo 0)"
  sleep 1
done

wait "$PID"
rc=$?
echo "DUMPED exit $rc after ${i}s, $(stat -c %s "$OUT" 2>/dev/null || echo 0) bytes"
exit $rc
