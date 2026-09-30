#!/usr/bin/env python3
"""Verify retained fixture bytes without re-encoding any CBOR."""
import hashlib
import json
from pathlib import Path

root = Path(__file__).resolve().parent
provenance = json.loads((root / "provenance.json").read_text())
for name, entry in provenance["files"].items():
    raw = (root / name).read_bytes()
    if "sha256_decoded" in entry:
        raw = bytes.fromhex(raw.decode().strip())
    expected = entry.get("sha256_decoded", entry.get("sha256"))
    assert hashlib.sha256(raw).hexdigest() == expected, name
print(f"Verified {len(provenance['files'])} immutable fixture files")
