"""Prints the blocks and the checkpoints of the epoch boundary replay judge.

Usage:
  replay_fixture.py blocks <immutable dir> <last chunk>
  replay_fixture.py checkpoints <ledger state json> <tip json> [<ledger state json> <tip json> ...]
"""
import json
import sys

# The size of one entry of a secondary index, whose first eight bytes are the block's offset in its chunk.
SECONDARY_ENTRY = 56


def blocks(immutable, last):
    """Writes the blocks of chunks 0 to last and the first block of the next chunk, as the chunks hold them."""
    out = sys.stdout.buffer
    for n in range(last + 1):
        with open(f"{immutable}/{n:05d}.chunk", "rb") as chunk:
            out.write(chunk.read())
    with open(f"{immutable}/{last + 1:05d}.secondary", "rb") as index:
        entries = index.read(2 * SECONDARY_ENTRY)
    if len(entries) < 2 * SECONDARY_ENTRY:
        sys.exit(f"refused: chunk {last + 1} holds fewer than two blocks")
    first = int.from_bytes(entries[SECONDARY_ENTRY : SECONDARY_ENTRY + 8], "big")
    with open(f"{immutable}/{last + 1:05d}.chunk", "rb") as chunk:
        out.write(chunk.read(first))


def checkpoint(state_path, tip_path):
    """The compared state the node holds at its tip, with the tip's epoch, slot and hash."""
    with open(tip_path) as f:
        tip = json.load(f)
    with open(state_path) as f:
        state = json.load(f)["stateBefore"]
    ledger = state["esLState"]
    delegation = ledger["delegationState"]
    pools = delegation["pstate"]
    retiring = pools["retiring"]
    return {
        "epoch": tip["epoch"],
        "slot": tip["slot"],
        "hash": tip["hash"],
        "state": {
            "pots": {
                "reserves": state["esChainAccountState"]["reserves"],
                "treasury": state["esChainAccountState"]["treasury"],
                "deposited": ledger["utxoState"]["deposited"],
                "fees": ledger["utxoState"]["fees"],
            },
            "accounts": {
                credential: {key: account[key] for key in ("balance", "deposit", "drep", "spool")}
                for credential, account in delegation["dstate"]["accounts"].items()
            },
            "pools": {
                pool: {"deposit": params["spsDeposit"], "retiring": retiring.get(pool)}
                for pool, params in pools["stakePools"].items()
            },
            "dreps": delegation["vstate"]["dreps"],
        },
    }


def checkpoints(paths):
    """Prints the checkpoint of each pair of a ledger state and its tip, in order."""
    if not paths or len(paths) % 2:
        sys.exit("refused: checkpoints takes pairs of a ledger state and its tip")
    pairs = zip(paths[0::2], paths[1::2])
    print(json.dumps([checkpoint(state, tip) for state, tip in pairs], indent=1, sort_keys=True))


def main(args):
    if args[:1] == ["blocks"] and len(args) == 3:
        blocks(args[1], int(args[2]))
    elif args[:1] == ["checkpoints"]:
        checkpoints(args[1:])
    else:
        sys.exit(__doc__)


main(sys.argv[1:])
