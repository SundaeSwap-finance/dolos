#!/bin/sh
# Shows that each check in tests/musashi_fixtures.rs refuses a fixture set it
# should refuse. A check that cannot fail says nothing about the fixtures, so
# every break here is one a passing run would have to catch.
#
# Run it on a COPY of the tree, because it edits fixture bytes in place and puts
# them back. It takes the fixture directory as its first argument and runs the
# test after each break.
#
# The three breaks and the verdicts they produced on 2026-09-21, against the
# prototype-2026w36 fixture set:
#
# 1. One hex character flipped inside a ranking block, which leaves the file
#    length and so the length assertion untouched:
#
#      assertion `left == right` failed: ranking-announce-with-txs does not hash
#      to the hash the node reported
#        left: Hash<32>("61920b9a960cd8f82cd0879d463c8bbe895da6ea00ae1e0569d9013fb041c241")
#       right: Hash<32>("2019145196aad7c71fb53ba34f5b02317f221628bbdfe7952cd69c8eefd709bb")
#      test result: FAILED. 6 passed; 1 failed
#
# 2. One absent kind dropped from the provenance:
#
#      assertion `left == right` failed: the absent kinds recorded are not the
#      absent kinds expected
#      test result: FAILED. 6 passed; 1 failed
#
# 3. One hex character flipped inside an endorser block body, again keeping the
#    length, so the real verifier is what has to object:
#
#      endorser-small does not verify against its announcement: endorser block
#      body hashes to c394e1c4bb94005f190c76061b34ba...
#      test result: FAILED. 6 passed; 1 failed
#
# With the fixtures absent altogether, all seven refuse on the provenance file
# itself, which is the state the test was written in:
#
#      test_data/musashi-w36/provenance.toml is unreadable: No such file or
#      directory (os error 2)
#      test result: FAILED. 0 passed; 7 failed
set -e
dir="${1:?the fixture directory}"
run="${2:-cargo test --test musashi_fixtures}"

flip() {
  python3 - "$1" "$2" <<'PY'
import sys
path, index = sys.argv[1], int(sys.argv[2])
s = open(path).read()
c = s[index]
s = s[:index] + ("a" if c != "a" else "b") + s[index + 1:]
open(path, "w").write(s)
print(f"flipped {path} index {index} from {c} to {s[index]}, length {len(s)}")
PY
}

drop_absent() {
  python3 - "$1" "$2" <<'PY'
import sys
path, kind = sys.argv[1], sys.argv[2]
s = open(path).read()
i = s.index(f'[[absent]]\nkind = "{kind}"')
j = s.index("[[absent]]", i + 10)
open(path, "w").write(s[:i] + s[j:])
print(f"dropped the {kind} entry")
PY
}

for step in block absent ebbody; do
  echo "=== break $step"
  case $step in
    block)
      cp "$dir/ranking-announce-with-txs.block" /tmp/refusal-keep
      flip "$dir/ranking-announce-with-txs.block" 40
      ;;
    absent)
      cp "$dir/provenance.toml" /tmp/refusal-keep
      drop_absent "$dir/provenance.toml" block_needing_lenient_apply
      ;;
    ebbody)
      cp "$dir/endorser-small.ebbody" /tmp/refusal-keep
      flip "$dir/endorser-small.ebbody" 60
      ;;
  esac
  set +e
  $run > /tmp/refusal-out 2>&1
  status=$?
  set -e
  tail -20 /tmp/refusal-out
  echo "exit=$status"
  case $step in
    block) cp /tmp/refusal-keep "$dir/ranking-announce-with-txs.block" ;;
    absent) cp /tmp/refusal-keep "$dir/provenance.toml" ;;
    ebbody) cp /tmp/refusal-keep "$dir/endorser-small.ebbody" ;;
  esac
done

echo "=== restored, which must pass"
set +e
$run > /tmp/refusal-out 2>&1
status=$?
set -e
tail -4 /tmp/refusal-out
echo "exit=$status"
test "$status" -eq 0
