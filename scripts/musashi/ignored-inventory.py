#!/usr/bin/env python3
"""Refuses a test binary whose green asserted nothing unless it is recorded.

A binary that passes no test and ignores every test it holds still prints ok,
and a summed count line over the whole workspace hides it. This reads a cargo
test log, names every such binary and every binary that holds no test at all,
and refuses one the recorded list does not hold, a recorded entry that now
passes a test, a log that names no binary, and a log that holds a failure.

Usage: ignored-inventory.py <cargo test log> [recorded list]

Expected verdict line, with the list current:

    INVENTORY 65 binaries, 8 all ignored, 9 empty, 0 unrecorded, 0 stale

Exits 0 only when nothing was unrecorded and no recorded entry was stale.
"""

import re
import sys
import tomllib

RUNNING = re.compile(r"^\s+Running (?P<source>.+?) \(.*?/deps/(?P<name>[^)]+)\)\s*$")
DOCTESTS = re.compile(r"^\s+Doc-tests (?P<name>\S+)\s*$")
RESULT = re.compile(
    r"^test result: (?P<verdict>\w+)\. (?P<passed>\d+) passed; "
    r"(?P<failed>\d+) failed; (?P<ignored>\d+) ignored;"
)
HASHED = re.compile(r"-[0-9a-f]{8,}$")


def identity(name, source):
    """The pair that names one target, because neither half is unique alone.

    Thirteen targets in this workspace print the source `unittests src/lib.rs`
    and two print the binary name `dolos`, so a list keyed on either one would
    hold entries that match more than one target.
    """
    return f"{name} {source}" if source else f"doctest {name}"


def read_log(path):
    """The targets a log names, each with the counts its result line printed."""
    targets = []
    pending = None

    with open(path, encoding="utf-8", errors="replace") as handle:
        for line in handle:
            running = RUNNING.match(line)
            if running:
                pending = identity(
                    HASHED.sub("", running.group("name")), running.group("source")
                )
                continue

            doctests = DOCTESTS.match(line)
            if doctests:
                pending = identity(doctests.group("name"), None)
                continue

            result = RESULT.match(line)
            if result and pending is not None:
                targets.append(
                    (
                        pending,
                        int(result.group("passed")),
                        int(result.group("failed")),
                        int(result.group("ignored")),
                    )
                )
                pending = None

    return targets


def read_list(path):
    with open(path, "rb") as handle:
        recorded = tomllib.load(handle)

    out = {}
    for section in ("all_ignored", "empty"):
        for entry in recorded.get(section, []):
            target = entry["target"]
            reason = entry["reason"]
            if not reason.strip():
                raise ValueError(f"{target} is recorded with no reason")
            out[target] = (section, reason)

    return out


def main(argv):
    if len(argv) < 2:
        print("REFUSED no cargo test log was named")
        return 1

    log = argv[1]
    listed = argv[2] if len(argv) > 2 else "test_data/all-ignored-binaries.toml"

    try:
        targets = read_log(log)
    except OSError as err:
        print(f"REFUSED {log} is unreadable: {err}")
        return 1

    try:
        recorded = read_list(listed)
    except (OSError, ValueError, KeyError, tomllib.TOMLDecodeError) as err:
        print(f"REFUSED {listed} is unusable: {err}")
        return 1

    if not targets:
        print(f"REFUSED {log} names no test binary, so it supports no inventory")
        return 1

    seen = set()
    duplicates = set()
    for name, *_ in targets:
        if name in seen:
            duplicates.add(name)
        seen.add(name)

    if duplicates:
        duplicates = sorted(duplicates)
        for name in duplicates:
            print(f"AMBIGUOUS {name} appears more than once in the log")
        return 1

    failures = sum(failed for _, _, failed, _ in targets)

    all_ignored = []
    empty = []
    unrecorded = []
    for name, passed, _, ignored in targets:
        if passed == 0 and ignored > 0:
            all_ignored.append((name, ignored))
        elif passed == 0 and ignored == 0:
            empty.append(name)

    for name, ignored in all_ignored:
        held = recorded.get(name)
        if held and held[0] == "all_ignored":
            print(f"ALLIGNORED {name} ignored {ignored}, recorded: {held[1]}")
        else:
            unrecorded.append(name)
            print(f"UNRECORDED {name} ignored {ignored} and passed nothing")

    for name in empty:
        held = recorded.get(name)
        if held and held[0] == "empty":
            print(f"EMPTY {name} recorded: {held[1]}")
        else:
            unrecorded.append(name)
            print(f"UNRECORDED {name} holds no test at all")

    asserting = {name for name, passed, _, _ in targets if passed > 0}
    stale = sorted(name for name in recorded if name in asserting)
    for name in stale:
        print(f"STALE {name} passes a test now, so the list no longer describes it")

    absent = sorted(name for name in recorded if name not in seen)
    for name in absent:
        print(f"ABSENT {name} is recorded but the log does not name it")

    if failures:
        print(
            f"REFUSED the log holds {failures} test failures, so it supports no inventory"
        )

    print(
        f"INVENTORY {len(targets)} binaries, {len(all_ignored)} all ignored, "
        f"{len(empty)} empty, {len(unrecorded)} unrecorded, {len(stale)} stale"
    )

    if unrecorded or stale or failures:
        return 1

    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
