#!/bin/sh
# Checks the start a live leg names, which commits may start from a snapshot, when its sync loop waits, finishes or refuses a snapshot, and when a candidate store is held, kept or dropped, on stores of one file.
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
    . "$HERE/lib.sh"
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

# The free gibibytes of a filesystem holding DF_BYTES bytes free, with a df that answers -B1 in bytes and -BG in gibibytes rounded up.
free_of() {
  (
    ROOT=$W/free
    . "$HERE/lib.sh"
    . "$HERE/snapshot.sh"
    df() {
      echo Avail
      case $1 in
        -B1) echo "$DF_BYTES" ;;
        -BG) echo "$(((DF_BYTES + 1073741823) / 1073741824))G" ;;
      esac
    }
    DF_BYTES=$1 free_g /
  )
}

# Must not fire: exactly 10 gibibytes free is 10.
expect free_whole "$(free_of 10737418240)" 10
# Must fire: one byte short of 10 gibibytes is 9, so a 10 gibibyte floor refuses it.
expect free_short "$(free_of 10737418239)" 9

# Must not fire: the two named starts, and snapshot when none is named.
expect genesis "$(LIVE_FROM=genesis run r live_from)" "exit=0 genesis"
expect snapshot "$(LIVE_FROM=snapshot run r live_from)" "exit=0 snapshot"
expect unset "$(unset LIVE_FROM; run r live_from)" "exit=0 snapshot"
# Must fire: any other start.
expect misspelt "$(LIVE_FROM=gensis run r live_from)" "exit=2 LIVE-CANDIDATE REFUSED live_from=gensis want=genesis,snapshot"

# A repository whose base commit pins pallas in Cargo.toml and Cargo.lock beside serde, with a file in each of crates/cardano, crates/core, crates/fjall and oracle/box.
R=$W/repo
rm -rf "$R"
mkdir -p "$R/crates/cardano/src" "$R/crates/core/src" "$R/crates/fjall/src" "$R/oracle/box"
git -C "$R" init -q
TOML='[workspace.dependencies]
pallas = { git = "https://github.com/SundaeSwap-finance/pallas", rev = "aaaa" }
serde = "1.0.0"'
LOCK='[[package]]
name = "pallas-validate"
version = "1.0.0"
source = "git+https://github.com/SundaeSwap-finance/pallas?rev=aaaa#aaaa"

[[package]]
name = "serde"
version = "1.0.0"
dependencies = [
 "pallas-validate",
]'
printf '%s\n' "$TOML" >"$R/Cargo.toml"
printf '%s\n' "$LOCK" >"$R/Cargo.lock"
echo 1 >"$R/crates/cardano/src/lib.rs"
echo 1 >"$R/crates/core/src/lib.rs"
echo 1 >"$R/crates/fjall/src/lib.rs"
echo 1 >"$R/oracle/box/run.sh"
# Commits the repository's files and prints the commit.
fixture_commit() {
  git -C "$R" add -A
  git -C "$R" -c user.name=t -c user.email=t@t commit -q -m t
  git -C "$R" rev-parse HEAD
}
BASE=$(fixture_commit)
# A commit on the base that sets the file $1 to the text $2.
derive() {
  git -C "$R" checkout -q --detach "$BASE"
  printf '%s\n' "$2" >"$R/$1"
  fixture_commit
}
# The start a candidate at commit $1 takes from a snapshot written by the commit $2, the base when none is given.
start_of() {
  run r snapshot_start "$R" "${2:-$BASE}" "$1"
}
CHECKED="checked=Cargo.toml,Cargo.lock,crates/cardano/,crates/core/,crates/fjall/"

# Must not fire: a change to the oracle's scripts only, and a change to a dependency that is not pallas, start from the snapshot.
expect oracle_only "$(start_of "$(derive oracle/box/run.sh 2)")" "exit=0 snapshot $CHECKED"
git -C "$R" checkout -q --detach "$BASE"
printf '%s\n' "$TOML" | sed 's/^serde = "1.0.0"$/serde = "1.0.1"/' >"$R/Cargo.toml"
printf '%s\n' "$LOCK" | sed '8s/1.0.0/1.0.1/' >"$R/Cargo.lock"
other_dep=$(fixture_commit)
expect other_dep "$(start_of "$other_dep")" "exit=0 snapshot $CHECKED"
# Must not fire: the quick tier lists a pin file only when its pallas entries differ.
expect other_dep_files "$(run r differing_files "$R" "$BASE" "$other_dep" | paste -sd, -)" "exit=0 "
# Must fire: a new pallas rev or version in Cargo.lock, a new pallas entry in Cargo.toml, a change under crates/cardano, crates/core or crates/fjall, and a snapshot commit the repository does not hold.
pallas_lock=$(derive Cargo.lock "$(printf '%s\n' "$LOCK" | sed 's/aaaa#aaaa/bbbb#bbbb/')")
expect pallas_lock "$(start_of "$pallas_lock")" "exit=0 genesis reason=reads_old_blocks paths=Cargo.lock"
expect pallas_lock_files "$(run r differing_files "$R" "$BASE" "$pallas_lock" | paste -sd, -)" "exit=0 Cargo.lock"
expect pallas_version "$(start_of "$(derive Cargo.lock "$(printf '%s\n' "$LOCK" | sed '3s/1.0.0/1.0.1/')")")" \
  "exit=0 genesis reason=reads_old_blocks paths=Cargo.lock"
expect pallas_toml "$(start_of "$(derive Cargo.toml "$(printf '%s\n' "$TOML" | sed 's/rev = "aaaa"/rev = "aaaa", features = ["relaxed"]/')")")" \
  "exit=0 genesis reason=reads_old_blocks paths=Cargo.toml"
expect cardano "$(start_of "$(derive crates/cardano/src/lib.rs 2)")" \
  "exit=0 genesis reason=reads_old_blocks paths=crates/cardano/src/lib.rs"
expect core "$(start_of "$(derive crates/core/src/lib.rs 2)")" \
  "exit=0 genesis reason=reads_old_blocks paths=crates/core/src/lib.rs"
expect fjall "$(start_of "$(derive crates/fjall/src/lib.rs 2)")" \
  "exit=0 genesis reason=reads_old_blocks paths=crates/fjall/src/lib.rs"
expect unknown_commit "$(start_of "$BASE" "$SHA")" "exit=0 genesis reason=unknown_commit commit=$SHA"

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
