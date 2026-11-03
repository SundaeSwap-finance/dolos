#!/bin/sh
# Checks the start a live leg names, when its sync loop waits, finishes or refuses a snapshot, and when a candidate store is held, kept or dropped, on stores of one file.
# Usage: snapshot_test.sh <empty work dir>
set -eu
W=$(cd "$1" && pwd)
HERE=$(cd "$(dirname "$0")" && pwd)
SHA=0123456789abcdef0123456789abcdef01234567

failed=0
expect() {
  case $2 in
    "$3") echo "SNAPSHOT-TEST $1 PASS" ;;
    *)
      echo "SNAPSHOT-TEST $1 FAIL got=\"$2\" want=\"$3\""
      failed=1
      ;;
  esac
}

# Runs one function of snapshot.sh against the build root $W/$1 with 20 gigabytes free, printing its exit and output.
run() {
  root=$W/$1
  shift
  (
    ROOT=$root
    . "$HERE/snapshot.sh"
    free_g() { echo "${FREE:-20}"; }
    status=0
    out=$("$@" 2>&1) || status=$?
    echo "exit=$status $out" | cut -d' ' -f1-6
  )
}

# The files under a build root's candidate dir with their contents, one per line.
tree() {
  (cd "$W/$1/candidate" 2>/dev/null && find . -type f | sort | while read -r f; do echo "$f:$(cat "$f")"; done) |
    paste -sd ' ' -
}

# A build root holding a kept snapshot, a held store, both, or neither.
root() {
  rm -rf "${W:?}/$1"
  mkdir -p "$W/$1/candidate"
  case $2 in
    *snapshot*)
      mkdir -p "$W/$1/candidate/snapshot/data"
      echo old >"$W/$1/candidate/snapshot/data/f"
      echo "commit=$SHA slot=100" >"$W/$1/candidate/snapshot/snapshot.txt"
      ;;
  esac
  case $2 in
    *held*)
      mkdir -p "$W/$1/candidate/held/data"
      echo new >"$W/$1/candidate/held/data/f"
      echo "commit=$SHA slot=200" >"$W/$1/candidate/held/snapshot.txt"
      ;;
  esac
}

# Must not fire: the two named starts, and snapshot when none is named.
expect genesis "$(LIVE_FROM=genesis run r live_from)" "exit=0 genesis"
expect snapshot "$(LIVE_FROM=snapshot run r live_from)" "exit=0 snapshot"
expect unset "$(unset LIVE_FROM; run r live_from)" "exit=0 snapshot"
# Must fire: any other start.
expect misspelt "$(LIVE_FROM=gensis run r live_from)" "exit=2 LIVE-CANDIDATE REFUSED live_from=gensis want=genesis,snapshot"

# Must not fire: a meta file naming a commit and a slot.
root meta snapshot
expect meta "$(run meta snapshot_meta "$W/meta/candidate/snapshot")" "exit=0 $SHA 100"
# Must fire: no meta file, and a meta file whose slot is not a number.
expect no_meta "$(run meta snapshot_meta "$W/meta/candidate/none")" "exit=1 "
echo "commit=$SHA slot=" >"$W/meta/candidate/snapshot/snapshot.txt"
expect bad_meta "$(run meta snapshot_meta "$W/meta/candidate/snapshot")" "exit=1 "

# Must not fire: a genesis start waits below the node and finishes near it whatever else holds.
expect genesis_wait "$(run s sync_step genesis 0 '' false 99999 5000)" "exit=0 wait"
expect genesis_synced "$(run s sync_step genesis 0 4950 false 99999 5000)" "exit=0 synced"
# Must not fire: a snapshot start waits while it opens or catches up, and finishes past its slot near the node.
expect opening "$(run s sync_step snapshot 1000 '' true 30 5000)" "exit=0 wait"
expect not_yet "$(run s sync_step snapshot 1000 1000 true 30 1050)" "exit=0 wait"
expect catching_up "$(run s sync_step snapshot 1000 1001 true 99999 5000)" "exit=0 wait"
expect advanced "$(run s sync_step snapshot 1000 4950 true 99999 5000)" "exit=0 synced"
# Must fire: a snapshot start whose candidate stopped, or that has not passed the snapshot's slot by the deadline.
expect exited "$(run s sync_step snapshot 1000 1500 false 30 5000)" "exit=0 unusable reason=exited"
expect stuck "$(run s sync_step snapshot 1000 1000 true 600 1050)" "exit=0 unusable reason=no_advance slot=1000"
expect silent "$(run s sync_step snapshot 1000 '' true 600 5000)" "exit=0 unusable reason=no_advance slot=none"
expect restarted "$(run s sync_step snapshot 1000 10 true 600 5000)" "exit=0 unusable reason=no_advance slot=10"

# Must not fire: a store whose sync failed, or whose stop slot is unknown, is removed and not held.
root hold none
mkdir -p "$W/hold/candidate/run/data" && echo s >"$W/hold/candidate/run/data/f"
expect hold_failed "$(run hold hold_store 1 "$W/hold/candidate/run" $SHA 300 | cut -d' ' -f1-3)" \
  "exit=0 LIVE-CANDIDATE removed"
expect hold_failed_tree "$(tree hold)" ""
mkdir -p "$W/hold/candidate/run/data" && echo s >"$W/hold/candidate/run/data/f"
expect hold_no_slot "$(run hold hold_store 0 "$W/hold/candidate/run" $SHA '' | cut -d' ' -f1-3)" \
  "exit=0 LIVE-CANDIDATE removed"
expect hold_no_slot_tree "$(tree hold)" ""
# Must fire: a store whose sync finished is held with its commit and stop slot.
mkdir -p "$W/hold/candidate/run/data" && echo s >"$W/hold/candidate/run/data/f"
expect hold "$(run hold hold_store 0 "$W/hold/candidate/run" $SHA 300 | cut -d' ' -f1-2)" "exit=0 LIVE-CANDIDATE"
expect hold_tree "$(tree hold)" "./held/data/f:s ./held/snapshot.txt:commit=$SHA slot=300"

# Must not fire: a genesis start drops the kept and held stores, and a snapshot start drops only the held one.
root drop_g snapshot,held
run drop_g drop_stale genesis >/dev/null
expect drop_genesis "$(tree drop_g)" ""
root drop_s snapshot,held
run drop_s drop_stale snapshot >/dev/null
expect drop_snapshot "$(tree drop_s)" "./snapshot/data/f:old ./snapshot/snapshot.txt:commit=$SHA slot=100"
# Must fire: a snapshot start takes the kept store, which is then no longer kept.
root take snapshot
expect take "$(run take start_store snapshot "$W/take/candidate/run")" "exit=0 "
expect take_tree "$(tree take)" "./run/data/f:old ./run/snapshot.txt:commit=$SHA slot=100"
root take_g none
expect take_genesis "$(run take_g start_store genesis "$W/take_g/candidate/run")" "exit=0 "
expect take_genesis_dir "$(test -d "$W/take_g/candidate/run" && echo made)" made

# Must not fire: a failed live leg, too little room, or nothing held, keeps no new snapshot.
root fail snapshot,held
expect fail "$(run fail settle_held FAIL)" "exit=0 LIVE-SNAPSHOT dropped live=FAIL free=20G min=10G"
expect fail_tree "$(tree fail)" "./snapshot/data/f:old ./snapshot/snapshot.txt:commit=$SHA slot=100"
root room held
expect room "$(FREE=9 run room settle_held PASS)" "exit=0 LIVE-SNAPSHOT dropped live=PASS free=9G min=10G"
expect room_tree "$(tree room)" ""
root nothing none
expect nothing "$(run nothing settle_held PASS)" "exit=0 LIVE-SNAPSHOT none live=PASS"
# Must fire: a passing live leg replaces the kept snapshot with the held store.
root keep snapshot,held
expect keep "$(run keep settle_held PASS | cut -d' ' -f1-5)" "exit=0 LIVE-SNAPSHOT kept commit=$SHA slot=200"
expect keep_tree "$(tree keep)" "./snapshot/data/f:new ./snapshot/snapshot.txt:commit=$SHA slot=200"

if [ "$failed" -eq 0 ]; then
  echo "SNAPSHOT-TEST RESULT PASS"
else
  echo "SNAPSHOT-TEST RESULT FAIL"
  exit 1
fi
