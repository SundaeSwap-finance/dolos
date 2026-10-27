# Functions the oracle box scripts share, reading ROOT, SCRATCH, REV and MIN_FREE_G.

# Exits when the build root has less than MIN_FREE_G gigabytes free.
require_free() {
  free=$(df -BG --output=avail "$ROOT" | tail -1 | tr -dc 0-9)
  if [ "$free" -lt "$MIN_FREE_G" ]; then
    echo "REFUSED free=${free}G below ${MIN_FREE_G}G at $ROOT"
    exit 2
  fi
}

# Puts the scratch tree at REV of the bare repository $ROOT/dolos.git with no planted change.
reset_scratch() {
  if [ ! -d "$ROOT/$SCRATCH/.git" ]; then
    git clone -q --no-checkout "$ROOT/dolos.git" "$ROOT/$SCRATCH"
  fi
  git -C "$ROOT/$SCRATCH" fetch -q "$ROOT/dolos.git" "$REV"
  git -C "$ROOT/$SCRATCH" checkout -q --detach FETCH_HEAD
  git -C "$ROOT/$SCRATCH" reset -q --hard
  git -C "$ROOT/$SCRATCH" clean -q -fdx
}
