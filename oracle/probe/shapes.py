"""Counts child only inputs of ledger accepted batches by origin and output kind.

Usage: python3 -I shapes.py <corpus.jsonl>
"""
import json
import sys
from collections import Counter

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from cbor import loads, untag  # noqa: E402


def txin(h):
    v = untag(loads(h)[0])
    return (v[0].hex(), v[1])


def kinds(out_hex):
    o = untag(loads(out_hex)[0])
    if isinstance(o, list):
        addr, val, datum, sref = o[0], o[1], len(o) > 2, False
    else:
        addr, val, datum, sref = o.get(0), o.get(1), 2 in o, 3 in o
    hdr = addr[0] >> 4
    ks = []
    if datum:
        ks.append("datum")
    if sref:
        ks.append("refscript")
    if isinstance(untag(val), list):
        ks.append("token")
    if hdr in (4, 5):
        ks.append("pointer")
    if hdr < 8 and hdr & 1:
        ks.append("script")
    return ks or ["keycoin"]


def ins(body, key=0):
    return [(untag(x)[0].hex(), untag(x)[1]) for x in untag(body.get(key, []))]


def main():
    rs = [json.loads(l) for l in open(sys.argv[1])]
    origin, kind = Counter(), Counter()
    made = {}
    for r in rs:
        if not r["follows_prev"]:
            made = {}
        tx = untag(loads(r["tx"])[0])
        body = tx[0]
        top = set(ins(body))
        pre = {txin(i): o for i, o in r["pre_utxo"]}
        verdict = next(iter(r["outcome"]))
        for sub in untag(body.get(23, [])):
            for i in ins(untag(sub)[0]):
                if i in top:
                    continue
                o = "made in run" if i in made else ("seeded" if i in pre else "absent from pre")
                origin[(verdict, o)] += 1
                if i in pre and verdict == "accepted":
                    for k in kinds(pre[i]):
                        kind[k] += 1
        if verdict == "accepted":
            for i, _ in r["outcome"]["accepted"]["post_utxo"]:
                if txin(i) not in pre:
                    made[txin(i)] = r["n"]
    print("records", len(rs), Counter(next(iter(r["outcome"])) for r in rs))
    print("child only input origin", dict(sorted(origin.items())))
    print("accepted child only input kinds", dict(sorted(kind.items())))


main()
