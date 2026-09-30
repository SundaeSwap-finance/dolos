#!/usr/bin/env python3
"""Replay the unchanged positive assertions on the recorded source before the submission fix."""
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import io

root = Path(__file__).resolve().parents[3]
baseline = root / "target/musashi-native-submission-baseline-source"
baseline.mkdir(parents=True, exist_ok=True)
archive = subprocess.check_output(["git", "archive", "abe2642391006e7e5439a00493d41db5da280a02"], cwd=root)
with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
    tar.extractall(baseline, filter="data")
shutil.copytree(root / "test_data/musashi-native-submission", baseline / "test_data/musashi-native-submission", dirs_exist_ok=True, ignore=shutil.ignore_patterns("evidence"))
(baseline / "tests/support").mkdir(exist_ok=True)
shutil.copyfile(root / "tests/support/musashi_submission.rs", baseline / "tests/support/musashi_submission.rs")
source = (root / "tests/musashi_submission.rs").read_text()
# Baseline receives the same positive tests and fixture adapter. New negative
# tests call the new evaluation signature and are not part of baseline replay.
source = source[:source.index("use dolos_cardano")]
(baseline / "tests/musashi_submission.rs").write_text(source)
env = dict(os.environ, CARGO_TARGET_DIR=str(root / "target/musashi-native-submission-baseline"), CARGO_PROFILE_DEV_DEBUG="0")
out = root / "target/musashi-native-submission-evidence"
out.mkdir(parents=True, exist_ok=True)
with (out / "baseline.log").open("w") as log:
    result = subprocess.run(["cargo", "+1.97.0", "test", "--offline", "--test", "musashi_submission"],cwd=baseline,env=env,stdout=log,stderr=subprocess.STDOUT)
print("baseline exit", result.returncode, "log:", out / "baseline.log")
raise SystemExit(result.returncode)
