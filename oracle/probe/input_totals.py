"""Counts the inputs a gzipped corpus names, in the order the judge's input totals test asserts them.

Usage: python3 -I input_totals.py <corpus.jsonl.gz>
"""
import gzip
import json
import sys

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from cbor import loads, untag  # noqa: E402


def ins(body, key):
    return [(untag(x)[0], untag(x)[1]) for x in untag(body.get(key, []))]


def main():
    t = [0] * 7
    with gzip.open(sys.argv[1], "rt") as f:
        for line in f:
            if not line.strip():
                continue
            body = untag(loads(json.loads(line)["tx"])[0])[0]
            spend = ins(body, 0)
            t[0] += len(spend)
            t[1] += len(ins(body, 13))
            t[2] += len(ins(body, 18))
            subs = untag(body.get(23, []))
            t[3] += len(subs)
            for sub in subs:
                sb = untag(sub)[0]
                s = ins(sb, 0)
                t[4] += len(s)
                t[5] += len(ins(sb, 18))
                t[6] += sum(1 for i in s if i not in spend)
    print(t)


main()
