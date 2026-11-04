#!/bin/sh
# Under the box lock, fetches the Sundae Dolos leios-musashi branch into the build root, puts the checkout $ROOT/nightly at REV, and runs that checkout's ship tier on REV into runs/nightly-<UTC date>.
# Usage: ROOT=dir [REV=ref] nightly.sh, REV leios-musashi by default, and once run the box crontab line
# 0 2 * * * ROOT=/opt/build/ledger-agreement-1028 sh /opt/build/ledger-agreement-1028/nightly/oracle/box/nightly.sh >>/opt/build/ledger-agreement-1028/runs/nightly.log 2>&1
set -eu
: "${ROOT:?}"
HERE=$(cd "$(dirname "$0")" && pwd)
URL=${NIGHTLY_URL:-https://github.com/SundaeSwap-finance/dolos.git}
REV=${REV:-leios-musashi}
. "$HERE/lib.sh"
take_lock
git -C "$ROOT/dolos.git" fetch -q "$URL" +refs/heads/leios-musashi:refs/heads/leios-musashi
REV=$(git -C "$ROOT/dolos.git" rev-parse --verify "$REV^{commit}")
CHECKOUT=$ROOT/nightly
if [ ! -d "$CHECKOUT/.git" ]; then
  git clone -q --no-checkout "$ROOT/dolos.git" "$CHECKOUT"
fi
git -C "$CHECKOUT" fetch -q "$ROOT/dolos.git" "$REV"
git -C "$CHECKOUT" checkout -q -f --detach "$REV"
OUT="$ROOT/runs/nightly-$(date -u +%Y%m%d)"
export ROOT REV OUT RUN_LOCK
exec sh "$CHECKOUT/oracle/box/run.sh" ship
