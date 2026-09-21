#!/bin/sh
# Take a fixed library of restorable Dolos store copies, one just below each
# named slot.
#
# checkpoint.sh keeps a rolling four of the live sync's store for recovery.
# This keeps a permanent set, one per slot a test or a bisection wants to start
# from, so a developer reaches an interesting block in seconds instead of
# resyncing from origin. The two cannot share one script: checkpoint.sh names
# the live sync's directory, container, config and binary as constants, and
# that directory is not writable by this work.
#
# Usage:
#   checkpoint-library.sh <slot> [<slot> ...]
#
# Each slot is a TARGET the checkpoint must sit BELOW, because a store that has
# already applied the target block cannot be rewound to before it. The script
# starts the follower, watches its applied slot, stops it while it is still
# under the target, copies the store and starts it again. Targets are taken in
# the order given and a target already passed is refused by name rather than
# skipped silently.
#
# Environment, each with the value this program used:
#   RUN        /opt/build/run-audit-lib   the follower's directory
#   CONTAINER  dolos-audit-lib            the follower's container
#   CONFIG     dolos-audit-lib.toml       its config, relative to RUN
#   BINARY     /bins-audit/dolos.audit    the binary inside the image
#   BINDIR     /opt/build/dolos-b5        the host directory BINARY comes from
#   IMAGE      dolos-build:1
#   FLOOR_KB   5242880                    free space that must remain after a copy
#   POLL       1                          seconds between slot reads
#   MARGIN     0                          slots below the target to stop at,
#                                         0 means three polls at the measured rate
#   RESTART_SAFETY 6000                    a follower within this many slots of
#                                         the target is checkpointed where it
#                                         stands rather than watched
set -eu

RUN=${RUN:-/opt/build/run-audit-lib}
CONTAINER=${CONTAINER:-dolos-audit-lib}
CONFIG_NAME=${CONFIG:-dolos-audit-lib.toml}
BINARY=${BINARY:-/bins-audit/dolos.audit}
BINDIR=${BINDIR:-/opt/build/dolos-b5}
IMAGE=${IMAGE:-dolos-build:1}
FLOOR_KB=${FLOOR_KB:-5242880}
POLL=${POLL:-1}
MARGIN=${MARGIN:-0}
# Three times the fastest replay this chain was measured at, 1763 slots per
# second over the empty stretch below the first endorser announcement, times
# one poll. Two of the targets are one epoch apart, 21600 slots, so a safety
# wider than that would answer the second target with a copy taken for the
# first.
RESTART_SAFETY=${RESTART_SAFETY:-6000}

DATA=$RUN/data
CKPT=$RUN/checkpoints
CONFIG=$RUN/$CONFIG_NAME
LOG=$RUN/checkpoint-library.log
LOCK=$RUN/checkpoint-library.lock
STOP_TIMEOUT=120
RESUME_TIMEOUT=600
# A target this far above the follower's current slot is not waited for, it is
# refused, because waiting for a slot the chain has not produced yet never ends.
WAIT_TIMEOUT=${WAIT_TIMEOUT:-18000}

say() { echo "$(date -u '+%Y-%m-%dT%H:%M:%SZ') $*"; }
logline() { say "$*" | tee -a "$LOG"; }
die() { say "REFUSED $*" >&2; exit 2; }

[ $# -gt 0 ] || die "no target slot given"
for t in "$@"; do
  case "$t" in
    ''|*[!0-9]*) die "target $t is not a slot number" ;;
  esac
done

[ -d "$RUN" ] || die "$RUN is not a directory"
[ -r "$CONFIG" ] || die "$CONFIG is not readable"
grep -q '^path = "data"$' "$CONFIG" || die "$CONFIG does not point storage at data"
[ -d "$BINDIR" ] || die "$BINDIR is not a directory"
docker inspect "$CONTAINER" >/dev/null 2>&1 || die "container $CONTAINER does not exist"
docker image inspect "$IMAGE" >/dev/null 2>&1 || die "image $IMAGE does not exist"
mkdir -p "$CKPT" || die "cannot create $CKPT"

if ! mkdir "$LOCK" 2>/dev/null; then
  die "$LOCK exists, another run is in progress"
fi
# The signal handlers exit. A handler that only released the lock would let a
# signalled run carry on holding no lock, which is how one stale run of this
# script kept copying after it had been told to stop.
trap 'rmdir "$LOCK" 2>/dev/null || true' EXIT
trap 'rmdir "$LOCK" 2>/dev/null || true; exit 130' INT
trap 'rmdir "$LOCK" 2>/dev/null || true; exit 143' TERM

# Sum of every regular file's size and the count of them. Two directories that
# agree on both held the same files, which is what makes a copy complete rather
# than merely finished.
measure() { find "$1" -type f -printf '%s\n' | awk '{n++; s+=$1} END {printf "%d %d\n", n+0, s+0}'; }

# The apply stage's last forwarded slot, read from the container's own log.
# A file beside the container is not used: the log dies with the container and
# a stale file has already misled one diagnosis on this box.
current_slot() {
  docker logs --tail 4000 "$CONTAINER" 2>&1 |
    tr '\r' '\n' | sed 's/\x1b\[[0-9;]*[A-Za-z]//g' |
    grep -o 'rolling forward point=[0-9]*' | tail -1 | cut -d= -f2
}

wait_for_resume() {
  since=$1
  waited=0
  while [ "$waited" -lt "$RESUME_TIMEOUT" ]; do
    if docker logs --since "$since" "$CONTAINER" 2>&1 |
        sed 's/\x1b\[[0-9;]*[A-Za-z]//g' |
        grep -qF 'stage{stage="apply"}: gasket::runtime: stage bootstrap ok'; then
      return 0
    fi
    sleep 5
    waited=$((waited + 5))
  done
  return 1
}

running() { [ "$(docker inspect -f '{{.State.Running}}' "$CONTAINER")" = true ]; }

# Reads the copied store's own tip, which is the number the checkpoint is named
# by. The copy is read rather than the live store, because opening a Dolos store
# writes to it, so reading the live one would make a read of the follower's
# position a write to the follower's data.
copy_tip() {
  incoming_rel=$1
  tipconf=$CKPT/tip.$$.toml
  sed "s|^path = \"data\"\$|path = \"$incoming_rel\"|" "$CONFIG" > "$tipconf"
  grep -q "^path = \"$incoming_rel\"\$" "$tipconf" || { rm -f "$tipconf"; return 1; }
  summary=$(docker run --rm --network none \
    -v "$BINDIR":/bins-audit:ro -v "$RUN":/run-dolos -w /run-dolos \
    --entrypoint "$BINARY" "$IMAGE" \
    -c "checkpoints/tip.$$.toml" data summary 2>&1) || true
  rm -f "$tipconf"
  echo "$summary" | tr -d ' ,' | grep -A1 '"state"' | grep '"tip_slot"' | cut -d: -f2
}

take_checkpoint() {
  target=$1
  incoming=$CKPT/incoming.$$
  rm -rf "$incoming"

  data_kb=$(du -sk "$DATA" | cut -f1)
  free_kb=$(df -k --output=avail "$RUN" | tail -1)
  after_kb=$((free_kb - data_kb))
  if [ "$after_kb" -lt "$FLOOR_KB" ]; then
    logline "no checkpoint below $target: about $data_kb K copied would leave about $after_kb K free, below the floor of $FLOOR_KB K"
    return 4
  fi

  stopped_at=$(date +%s)
  if running; then
    docker stop -t "$STOP_TIMEOUT" "$CONTAINER" >/dev/null
    is_running=$(docker inspect -f '{{.State.Running}}' "$CONTAINER")
    [ "$is_running" = false ] || die "$CONTAINER is still running after docker stop"
  fi
  say "$CONTAINER stopped, exit code $(docker inspect -f '{{.State.ExitCode}}' "$CONTAINER")"

  read -r files bytes <<EOF
$(measure "$DATA")
EOF
  [ "$files" -gt 0 ] || die "$DATA holds no files"
  cp -a "$DATA" "$incoming" || die "cp -a of $DATA failed"
  read -r cfiles cbytes <<EOF
$(measure "$incoming")
EOF
  [ "$cfiles" = "$files" ] && [ "$cbytes" = "$bytes" ] ||
    die "copy holds $cfiles files and $cbytes bytes, source held $files and $bytes"

  # Started again before the copy is named, so the follower is down only for
  # the copy itself and not for the tip read as well.
  since=$(date -u '+%Y-%m-%dT%H:%M:%S')
  docker start "$CONTAINER" >/dev/null
  running || die "$CONTAINER did not start again"
  stop_seconds=$(($(date +%s) - stopped_at))

  tip=$(copy_tip "checkpoints/incoming.$$") || tip=""
  case "$tip" in
    ''|*[!0-9]*)
      kept=$CKPT/unnamed-$(date -u '+%Y%m%dT%H%M%SZ')
      mv "$incoming" "$kept"
      logline "checkpoint below $target UNNAMED, the copy's tip slot could not be read, copy kept at $kept"
      return 6
      ;;
  esac
  if [ "$tip" -ge "$target" ]; then
    kept=$CKPT/overshot-$tip
    rm -rf "$kept"
    mv "$incoming" "$kept"
    logline "checkpoint below $target OVERSHOT at tip $tip, copy kept at $kept"
    return 7
  fi

  # Reading the tip opened the copy, and opening a Dolos store replays its
  # write ahead log journals into tables and rewrites them, so the copy is no
  # longer the bytes that were checked against the source. Both sizes are
  # logged, because reporting only the checked one would describe something
  # that is not there.
  read -r rfiles rbytes <<EOF
$(measure "$incoming")
EOF
  dest=$CKPT/$tip
  rm -rf "$dest"
  mv "$incoming" "$dest"
  echo "$target" > "$dest.target"
  sync

  if wait_for_resume "$since"; then
    resume="applying again $(($(date +%s) - stopped_at)) s after the stop"
  else
    resume="the apply stage did not report bootstrap ok within $RESUME_TIMEOUT s"
  fi
  logline "checkpoint $tip below target $target  $rbytes bytes in $rfiles files after the tip read recovered it, $bytes in $files copied and checked  stopped $stop_seconds s  $resume"
  return 0
}

for target in "$@"; do
  slot=$(current_slot)
  case "$slot" in
    ''|*[!0-9]*) die "could not read $CONTAINER's slot from its log" ;;
  esac

  if [ "$slot" -ge "$target" ]; then
    logline "target $target REFUSED, $CONTAINER is at slot $slot and a store cannot be rewound"
    continue
  fi

  # A follower this close to the target is checkpointed where it stands,
  # whether it is running or not, because its rate has not been measured yet
  # and the watch below cannot measure one without first sleeping. Replay of
  # the write ahead log covers thousands of slots per second on the empty
  # stretch below the first endorser announcement, so one sleep is enough to
  # land past the target. That is not a hypothetical: target 371614 was lost
  # that way on the first run of this script, from a follower that was already
  # running, 1229 slots below the target, moving 1763 slots per second.
  if [ $((target - slot)) -lt "$RESTART_SAFETY" ]; then
    say "target $target, $CONTAINER at slot $slot, $((target - slot)) below it and under the safety of $RESTART_SAFETY, checkpointing where it stands"
    take_checkpoint "$target" || say "target $target did not produce a checkpoint"
    continue
  fi

  if ! running; then
    since=$(date -u '+%Y-%m-%dT%H:%M:%S')
    docker start "$CONTAINER" >/dev/null
    wait_for_resume "$since" || say "$CONTAINER started but did not report bootstrap ok"
  fi

  say "target $target, $CONTAINER at slot $slot, watching"
  waited=0
  prev=$slot
  while :; do
    sleep "$POLL"
    waited=$((waited + 1))
    slot=$(current_slot)
    case "$slot" in ''|*[!0-9]*) slot=$prev ;; esac
    delta=$((slot - prev))
    [ "$delta" -lt 0 ] && delta=0
    margin=$MARGIN
    if [ "$margin" -eq 0 ]; then
      margin=$((delta * 3))
      [ "$margin" -lt 200 ] && margin=200
    fi
    prev=$slot
    if [ $((slot + margin)) -ge "$target" ]; then
      say "slot $slot is within $margin of $target, last poll moved $delta, stopping"
      break
    fi
    if [ "$waited" -ge "$WAIT_TIMEOUT" ]; then
      logline "target $target ABANDONED after $waited polls, $CONTAINER reached only slot $slot"
      break
    fi
    if [ $((waited % 300)) -eq 0 ]; then
      say "target $target, slot $slot, $((target - slot)) to go, last poll moved $delta"
    fi
  done

  if [ "$slot" -lt "$target" ]; then
    take_checkpoint "$target" || say "target $target did not produce a checkpoint"
  else
    logline "target $target LOST, the watch left $CONTAINER at slot $slot which is at or past it"
  fi
done

logline "library run done, checkpoints now: $(ls -1 "$CKPT" | grep -E '^[0-9]+$' | sort -n | tr '\n' ' ')"
