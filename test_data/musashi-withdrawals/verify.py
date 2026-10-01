#!/usr/bin/env python3
"""Check committed capture bytes and reused offline fixture dependencies."""
import hashlib
import json
from pathlib import Path

root = Path(__file__).resolve().parent
manifest = json.loads((root / "provenance.json").read_text())
for name, expected in manifest["files"].items():
    data = (root / name).read_bytes()
    if expected["domain"] == "decoded CBOR":
        data = bytes.fromhex(data.decode().strip())
    assert len(data) == expected["bytes"], name
    assert hashlib.sha256(data).hexdigest() == expected["sha256"], name
print(f"verified {len(manifest['files'])} fixture/dependency checksums")
