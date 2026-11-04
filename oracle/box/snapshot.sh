# Functions for the candidate store a passing live leg keeps for the next sync to start from, reading ROOT and the functions of lib.sh.

# The store the last passing live leg kept, with a snapshot.txt naming its commit and slot.
SNAPSHOT=$ROOT/candidate/snapshot
# The store of a finished sync, until its live leg's verdict keeps or drops it.
HELD=$ROOT/candidate/held
ADVANCE_MINUTES=${ADVANCE_MINUTES:-10}
SNAPSHOT_MIN_FREE_G=${SNAPSHOT_MIN_FREE_G:-10}
# Whether the path $1 is one the candidate's release build of the dolos binary does not read: oracle tooling, documentation, and the judge's crate, corpus and test module.
unbuilt_path() {
  case $1 in
    oracle/* | docs/* | adrs/* | *.md) ;;
    crates/ledger-oracle-judge/* | test_data/ledger-oracle/* | src/serve/grpc/v1/ledger_oracle_tests/*) ;;
    *) return 1 ;;
  esac
}

# The names of a candidate's config files in its store.
CONFIG_FILES="dolos.toml byron-genesis.json shelley-genesis.json alonzo-genesis.json conway-genesis.json dijkstra-genesis.json"

# Writes the candidate's config into the dir $1: the serving genesis files, and the serving dolos.toml without its serve sections followed by the candidate's grpc listener.
write_config() {
  for file in $CONFIG_FILES; do
    case $file in
      dolos.toml)
        awk '/^\[/ { skip = ($0 ~ /^\[serve/) } !skip' "$SERVE_CONFIG/dolos.toml" >"$1/dolos.toml"
        printf '[serve.grpc]\nlisten_address = "%s"\n' "$CANDIDATE_GRPC" >>"$1/dolos.toml"
        ;;
      *) cat "$SERVE_CONFIG/$file" >"$1/$file" ;;
    esac
  done
}

# The start the config in the dir $2 allows from the store $1, snapshot only when the store holds every config file and each equals the new one.
config_start() {
  all=
  same=
  differ=
  unknown=
  for file in $CONFIG_FILES; do
    all="$all,$file"
    status=0
    cmp -s "$1/$file" "$2/$file" || status=$?
    case $status in
      0) same="$same,$file" ;;
      1) differ="$differ,$file" ;;
      *) unknown="$unknown,$file" ;;
    esac
  done
  if [ -n "$same" ] && [ "$same" = "$all" ]; then
    echo "snapshot same_config=${same#,}"
  elif [ -n "$unknown" ]; then
    echo "genesis reason=config_unknown files=${unknown#,}"
  else
    echo "genesis reason=config_differs files=${differ#,}"
  fi
}

# The start a candidate at commit $3 with the config in the dir $5 takes from the store $4 written by the commit $2 of the repository $1, snapshot only when the diff between the commits ran, every path it lists is unbuilt, and config_start allows it.
snapshot_start() {
  for known in "$2" "$3"; do
    if ! git -C "$1" cat-file -e "$known^{commit}" 2>/dev/null; then
      echo "genesis reason=unknown_commit commit=$known"
      return
    fi
  done
  if ! paths=$(git -C "$1" diff --no-renames --name-only "$2" "$3"); then
    echo "genesis reason=diff_failed"
    return
  fi
  unbuilt=$(echo "$paths" | while read -r path; do
    if [ -n "$path" ] && unbuilt_path "$path"; then
      echo "$path"
    fi
  done)
  if [ "$unbuilt" != "$paths" ]; then
    built=$(echo "$paths" | while read -r path; do
      if ! unbuilt_path "$path"; then
        echo "$path"
      fi
    done)
    echo "genesis reason=built_paths_differ paths=$(echo "$built" | paste -sd, -)"
    return
  fi
  config=$(config_start "$4" "$5")
  case $config in
    snapshot\ *) echo "snapshot unbuilt_paths=$(echo "$paths" | paste -sd, -) ${config#snapshot }" ;;
    *) echo "$config" ;;
  esac
}

# The whole gibibytes free on the filesystem holding $1, rounded down.
free_g() {
  echo $(($(df -B1 --output=avail "$1" | tail -1 | tr -dc 0-9) / 1073741824))
}

# The start a candidate syncs from, snapshot when LIVE_FROM is unset, refusing any value but genesis or snapshot.
live_from() {
  case ${LIVE_FROM:-snapshot} in
    genesis | snapshot) echo "${LIVE_FROM:-snapshot}" ;;
    *)
      echo "LIVE-CANDIDATE REFUSED live_from=$LIVE_FROM want=genesis,snapshot" >&2
      return 2
      ;;
  esac
}

# The commit and slot a store's snapshot.txt names, failing when it names no commit or no slot.
snapshot_meta() {
  sed -n 's/^commit=\([0-9a-f]\{40\}\) slot=\([0-9][0-9]*\)$/\1 \2/p' "$1/snapshot.txt" 2>/dev/null | grep .
}

# Whether a sync from the start $1 at slot $2 waits, is synced, or cannot use its snapshot, given the candidate's tip slot (empty when it answers none), whether it runs, the seconds since it started and the node's slot.
sync_step() {
  case $1 in
    genesis)
      if [ "${3:-0}" -ge $(($6 - 100)) ]; then echo synced; else echo wait; fi
      ;;
    snapshot)
      if [ "$4" != true ]; then
        echo "unusable reason=exited"
      elif [ -n "$3" ] && [ "$3" -gt "$2" ] && [ "$3" -ge $(($6 - 100)) ]; then
        echo synced
      elif [ "$5" -ge $((ADVANCE_MINUTES * 60)) ] && { [ -z "$3" ] || [ "$3" -le "$2" ]; }; then
        echo "unusable reason=no_advance slot=${3:-none}"
      else
        echo wait
      fi
      ;;
  esac
}

# Removes the held store, and for a genesis start the kept snapshot too.
drop_stale() {
  rm -rf "$HELD"
  if [ "$1" = genesis ] && [ -d "$SNAPSHOT" ]; then
    rm -rf "$SNAPSHOT"
    echo "LIVE-CANDIDATE snapshot_dropped start=genesis"
  fi
}

# Puts the store a sync starts from at $2, the kept snapshot for a snapshot start and an empty dir for a genesis one.
start_store() {
  case $1 in
    snapshot) mv "$SNAPSHOT" "$2" ;;
    genesis) mkdir -p "$2" ;;
  esac
}

# Holds a store whose sync exited 0 with the commit and slot it stopped at, and removes any other.
hold_store() {
  if [ "$1" -eq 0 ] && [ -n "$4" ]; then
    rm -rf "$HELD"
    mv "$2" "$HELD"
    echo "commit=$3 slot=$4" >"$HELD/snapshot.txt"
    echo "LIVE-CANDIDATE held store=$HELD commit=$3 slot=$4"
  else
    rm -rf "$2"
    echo "LIVE-CANDIDATE removed store=$2 status=$1 slot=${4:-none}"
  fi
}

# Makes the held store the kept snapshot when the live verdict $1 is PASS and enough room stays free, and removes it otherwise.
settle_held() {
  if [ ! -d "$HELD" ]; then
    echo "LIVE-SNAPSHOT none live=$1"
    return
  fi
  room=$(free_g "$ROOT")
  if [ "$1" != PASS ] || [ "$room" -lt "$SNAPSHOT_MIN_FREE_G" ]; then
    rm -rf "$HELD"
    echo "LIVE-SNAPSHOT dropped live=$1 free=${room}G min=${SNAPSHOT_MIN_FREE_G}G"
    return
  fi
  rm -rf "$SNAPSHOT"
  mv "$HELD" "$SNAPSHOT"
  kept=$(snapshot_meta "$SNAPSHOT")
  echo "LIVE-SNAPSHOT kept commit=${kept% *} slot=${kept#* } bytes=$(du -sb "$SNAPSHOT" | cut -f1) free=${room}G"
}
