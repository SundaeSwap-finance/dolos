#!/usr/bin/env python3
"""Verify immutable fixture dependencies and the exact regression source/logs."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys

here = Path(__file__).resolve().parent
root = here.parents[1]
subprocess.run([sys.executable, str(here.parent / 'musashi-native-submission/verify.py')], check=True)
p = json.loads((here / 'provenance.json').read_text())
for name, digest in p['tests'].items():
    assert hashlib.sha256((root / name).read_bytes()).hexdigest() == digest, name
for name, digest in p['evidence'].items():
    assert hashlib.sha256((here / name).read_bytes()).hexdigest() == digest, name
print('Verified estimation test sources and baseline/fixed evidence')
