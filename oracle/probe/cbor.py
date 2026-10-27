"""Minimal CBOR reader for corpus probes."""


class Tag:
    def __init__(self, n, v):
        self.n = n
        self.v = v

    def __repr__(self):
        return f"Tag({self.n},{self.v!r})"


def _arg(b, i, ai):
    if ai < 24:
        return ai, i
    if ai == 24:
        return b[i], i + 1
    if ai == 25:
        return int.from_bytes(b[i:i + 2], "big"), i + 2
    if ai == 26:
        return int.from_bytes(b[i:i + 4], "big"), i + 4
    if ai == 27:
        return int.from_bytes(b[i:i + 8], "big"), i + 8
    if ai == 31:
        return None, i
    raise ValueError(f"bad ai {ai}")


def decode(b, i=0):
    ib = b[i]
    i += 1
    mt, ai = ib >> 5, ib & 31
    if mt == 7:
        if ai == 20:
            return False, i
        if ai == 21:
            return True, i
        if ai == 22:
            return None, i
        if ai == 25:
            return ("f16", b[i:i + 2]), i + 2
        if ai == 26:
            return ("f32", b[i:i + 4]), i + 4
        if ai == 27:
            return ("f64", b[i:i + 8]), i + 8
        raise ValueError("simple")
    n, i = _arg(b, i, ai)
    if mt == 0:
        return n, i
    if mt == 1:
        return -1 - n, i
    if mt in (2, 3):
        if n is None:
            out = b""
            while b[i] != 0xFF:
                c, i = decode(b, i)
                out += c if isinstance(c, bytes) else c.encode()
            return out, i + 1
        v = b[i:i + n]
        return (v if mt == 2 else v.decode("utf8", "replace")), i + n
    if mt == 4:
        out = []
        if n is None:
            while b[i] != 0xFF:
                x, i = decode(b, i)
                out.append(x)
            return out, i + 1
        for _ in range(n):
            x, i = decode(b, i)
            out.append(x)
        return out, i
    if mt == 5:
        out = []
        if n is None:
            while b[i] != 0xFF:
                k, i = decode(b, i)
                v, i = decode(b, i)
                out.append((k, v))
            return dict(out) if all(isinstance(k, (int, str, bytes)) for k, _ in out) else out, i + 1
        for _ in range(n):
            k, i = decode(b, i)
            v, i = decode(b, i)
            out.append((k, v))
        try:
            return dict(out), i
        except TypeError:
            return out, i
    if mt == 6:
        v, i = decode(b, i)
        return Tag(n, v), i
    raise ValueError("mt")


def loads(h):
    b = bytes.fromhex(h) if isinstance(h, str) else h
    v, i = decode(b, 0)
    return v, len(b) - i


def untag(x):
    while isinstance(x, Tag):
        x = x.v
    return x
