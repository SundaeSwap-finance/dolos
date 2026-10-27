"""Verify captured bytes and the identical baseline/fixed regression sources."""
import hashlib
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
provenance = json.loads((HERE / "provenance.json").read_text())
for fixture in provenance["captured_files"]:
    raw = bytes.fromhex((HERE / fixture["path"]).read_text())
    assert len(raw) == fixture["decoded_bytes"], fixture["path"]
    assert hashlib.sha256(raw).hexdigest() == fixture["sha256"], fixture["path"]

sources = json.loads((HERE / "evidence/test-source-sha256.json").read_text())
for label, expected in sources.items():
    source = (ROOT / label.split("#")[0]).read_text()
    if label.endswith("#tests"):
        source = source[source.index("#[cfg(test)]"):]
    assert hashlib.sha256(source.encode()).hexdigest() == expected, label
print("Verified 3 unchanged captures and baseline/fixed regression source identity")
