#!/usr/bin/env python3
"""Run workspace verification, retaining each command's full output and exit status."""
import json
import os
from pathlib import Path
import subprocess
import time

root = Path(__file__).resolve().parents[3]
out = root / "target/musashi-native-submission-evidence"
out.mkdir(parents=True, exist_ok=True)
env = dict(os.environ, CARGO_TARGET_DIR=str(root / "target/musashi-native-submission-fixed"), CARGO_PROFILE_DEV_DEBUG="0", RUST_TEST_THREADS="1", CARGO_BUILD_JOBS="2")
checks = [
    ("clippy", ["clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings"]),
    ("build", ["build", "--workspace", "--all-targets", "--all-features"]),
    ("tests-default", ["test", "--workspace", "--all-targets", "--no-fail-fast"]),
    ("tests-all-features", ["test", "--workspace", "--all-features", "--exclude", "dolos-minibf", "--exclude", "dolos-minikupo", "--exclude", "dolos-trp", "--no-fail-fast"]),
]
results = []
for name, args in checks:
    command = ["cargo", "+1.97.0", *args]
    start = time.time()
    with (out / (name + ".log")).open("w") as log:
        result = subprocess.run(command, cwd=root, env=env, stdout=log, stderr=subprocess.STDOUT)
    results.append({"name":name,"command":command,"exit_code":result.returncode,"duration_seconds":round(time.time()-start,2),"log":name+".log"})
    (out / "checks.json").write_text(json.dumps({"environment":{"CARGO_TARGET_DIR":"target/musashi-native-submission-fixed","CARGO_PROFILE_DEV_DEBUG":"0","RUST_TEST_THREADS":"1","CARGO_BUILD_JOBS":"2"},"checks":results},indent=2)+"\n")
    print(name, result.returncode, flush=True)
