"""Compare the whole utxo set the node holds against the follower's.

The node is the reference set, because the follower cannot be asked for a set it
has no index for. So this asks the follower for every one of the node's utxos by
key and checks that each one is there with the same lovelace. A utxo the
follower holds that the node does not is invisible to this comparison, and the
summary says so rather than leaving the direction unmentioned.

The run is guarded at both ends. The node's tip is read before and after the
dump, so the report names the window the dump belongs to, and every reply the
follower sends carries its own ledger tip, so the report names the window the
answers belong to.

A key the follower does not answer is not yet a divergence. The reads take
minutes, so by the time they finish the chain has moved and a key the node held
at the dump may have been spent since, which the follower is right to have
dropped. So a key that is missing is put through a second pass: the node is
dumped again once the follower is level, the keys the node no longer holds are
dropped as spent in between, and the follower is asked once more for what is
left. What survives both passes is held by the node and not by the follower with
the two within a few slots of each other. A run where the follower never came
level ends non-zero and says so, because such a run has compared two ledgers.

A reply item counts as held only when it carries the utxo itself. An item that
names the key and carries no body is not an answer, and a reply that names no
coin is not a matching coin, so both read as a divergence and the printed line
says which.

Every key that is missing or holds a different value is printed. The exit is
non-zero if anything differs or if the run was inconclusive.

Usage: compare-utxo-set.py

Reads the node with guarded-utxo.sh beside this file and the follower over
UtxoRPC with grpcurl. Read only.
"""

import base64
import json
import os
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))

CARDANO_CLI = os.environ.get("CARDANO_CLI", "/usr/local/bin/cardano-cli")
GRPCURL = os.environ.get("GRPCURL", "/opt/build/grpcurl")
GUARDED_DUMP = os.path.join(HERE, "guarded-utxo.sh")
DOLOS_GRPC = os.environ.get("DOLOS_GRPC", "localhost:50051")
READ_UTXOS = "utxorpc.v1alpha.query.QueryService/ReadUtxos"

# Keys per call. The follower answers a batch in one round trip, so this only
# bounds how large one request document gets.
BATCH = int(os.environ.get("BATCH", "300"))

# How far behind the node the follower may be and still count as level. The
# follower applies in batches and is a few slots behind between them.
LAG_SLOTS = int(os.environ.get("LAG_SLOTS", "200"))

# How long to give the follower to come level before the run is called
# inconclusive, and how often to look.
LEVEL_TIMEOUT = int(os.environ.get("LEVEL_TIMEOUT", "300"))
LEVEL_INTERVAL = int(os.environ.get("LEVEL_INTERVAL", "10"))

# How many missing keys to print in full.
SHOW = int(os.environ.get("SHOW", "40"))


def run(argv, stdin=None):
    done = subprocess.run(argv, input=stdin, capture_output=True, text=True)
    if done.returncode != 0:
        sys.exit(f"{argv[0]} failed with {done.returncode}: {done.stderr.strip()}")
    return done.stdout


def node_tip():
    tip = json.loads(run([CARDANO_CLI, "query", "tip"]))
    return int(tip["slot"]), int(tip["block"]), tip["hash"]


def node_utxos(path):
    done = subprocess.run(["sh", GUARDED_DUMP, path], capture_output=True, text=True)
    for line in done.stdout.splitlines():
        print(f"GUARD  {line}")
    if done.returncode != 0:
        sys.exit(f"the guarded dump ended {done.returncode}: {done.stderr.strip()}")

    with open(path) as handle:
        return json.load(handle)


def key_to_request(key):
    txid, index = key.split("#")
    raw = base64.b64encode(bytes.fromhex(txid)).decode()
    return {"hash": raw, "index": int(index)}


def reply_key(item):
    ref = item.get("txoRef") or {}
    raw = ref.get("hash")
    if raw is None:
        return None
    txid = base64.b64decode(raw).hex()
    return f"{txid}#{int(ref.get('index', 0))}"


def has_body(item):
    """Whether the item carries the utxo and not only the key that was asked."""
    return bool(item.get("nativeBytes")) or bool(item.get("cardano"))


def reply_lovelace(item):
    """The coin the item names, or None when it names none."""
    coin = (item.get("cardano") or {}).get("coin")
    if coin is None:
        return None
    if isinstance(coin, dict):
        return int(coin["int"]) if "int" in coin else None
    return int(coin)


def ask(keys):
    """The follower's answer for one batch, as a map and the reply's ledger tip."""
    request = json.dumps({"keys": [key_to_request(k) for k in keys]})
    raw = run([GRPCURL, "-plaintext", "-d", "@", DOLOS_GRPC, READ_UTXOS], stdin=request)
    reply = json.loads(raw) if raw.strip() else {}

    held = {}
    for item in reply.get("items") or []:
        key = reply_key(item)
        if key is not None and has_body(item):
            held[key] = reply_lovelace(item)

    tip = reply.get("ledgerTip") or {}
    return held, int(tip.get("slot", 0))


def ask_all(keys):
    held = {}
    tips = []
    for start in range(0, len(keys), BATCH):
        batch = keys[start : start + BATCH]
        answered, tip = ask(batch)
        held.update(answered)
        tips.append(tip)
    return held, tips


def wait_until_level(after_slot):
    """Wait for the follower to reach the dump's tip, and report what happened."""
    waited = 0
    while True:
        node_slot, _, _ = node_tip()
        _, follower_slot = ask([f"{'00' * 32}#0"])
        gap = node_slot - follower_slot
        if follower_slot >= after_slot and gap <= LAG_SLOTS:
            return True, node_slot, follower_slot, gap, waited
        if waited >= LEVEL_TIMEOUT:
            return False, node_slot, follower_slot, gap, waited
        time.sleep(LEVEL_INTERVAL)
        waited += LEVEL_INTERVAL


def main():
    before_slot, before_block, _ = node_tip()

    with tempfile.TemporaryDirectory() as work:
        utxos = node_utxos(os.path.join(work, "node-utxos.json"))

    after_slot, after_block, _ = node_tip()

    keys = sorted(utxos)
    wanted = {key: int(utxos[key]["value"]["lovelace"]) for key in keys}
    print(
        f"DUMP   {len(keys)} utxos, node tip {before_slot} block {before_block} "
        f"before and {after_slot} block {after_block} after"
    )

    held, tips = ask_all(keys)
    print(
        f"READ   {len(held)} answered in {len(tips)} batches of at most {BATCH}, "
        f"follower ledger tip {min(tips)} to {max(tips)}"
    )

    missing = [key for key in keys if key not in held]

    level, node_slot, follower_slot, gap, waited = wait_until_level(after_slot)
    print(
        f"LEVEL  follower at {follower_slot}, node at {node_slot}, gap {gap} slots, "
        f"waited {waited}s, dump tip {after_slot}  "
        + ("REACHED" if level else "NOT REACHED")
    )

    answered = len(held)
    spent = []

    if missing:
        with tempfile.TemporaryDirectory() as work:
            again = node_utxos(os.path.join(work, "node-utxos-again.json"))

        spent = [key for key in missing if key not in again]
        missing = [key for key in missing if key in again]
        print(
            f"SPENT  {len(spent)} of the absent keys the node no longer holds either, "
            f"so they were spent between the two dumps"
        )

        rechecked, _ = ask_all(missing)
        held.update(rechecked)
        missing = [key for key in missing if key not in held]
        print(f"RETRY  {len(missing)} still absent from the follower on the second pass")

    value_diffs = [key for key in sorted(held) if held[key] != wanted.get(key)]

    for key in missing[:SHOW]:
        print(f"ONLYNODE {key} {wanted[key]}")
    if len(missing) > SHOW:
        print(f"ONLYNODE {len(missing) - SHOW} further keys not printed")

    for key in value_diffs[:SHOW]:
        shown = "no coin in the reply" if held[key] is None else held[key]
        print(f"VALUE    {key} node {wanted.get(key)} follower {shown}")
    if len(value_diffs) > SHOW:
        print(f"VALUE    {len(value_diffs) - SHOW} further keys not printed")

    print(
        f"UTXO   node {len(keys)}, follower {answered} on the first pass, "
        f"spent between the dumps {len(spent)}, only on node {len(missing)}, "
        f"value diffs {len(value_diffs)}, matched slot {follower_slot}"
    )
    print("DIRECTION a utxo the follower holds and the node does not is not visible here  REPORTED")

    if not level:
        print("INCONCLUSIVE the follower never came level with the node")
        return 2
    if missing or value_diffs:
        print("DIFFERS")
        return 1

    print("IDENTICAL")
    return 0


if __name__ == "__main__":
    sys.exit(main())
