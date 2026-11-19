#!/usr/bin/env bash
# Exits zero when each pallas commit on stdin, one "owner/repo sha" per line,
# has a successful CI push run, waiting for a run still in progress to finish.
set -u

failed=0

while read -r repo sha; do
  # A pull request run tests a merge commit, so only a push run tests the pinned commit.
  filter="[.workflow_runs[] | select(.head_sha == \"$sha\" and .path == \".github/workflows/ci.yml\" and .event == \"push\")]"

  if ! runs=$(gh api "repos/$repo/actions/workflows/ci.yml/runs?head_sha=$sha&event=push" --jq "$filter" < /dev/null); then
    echo "::error::pallas $repo@$sha: its CI runs could not be read"
    failed=1
    continue
  fi

  url=$(jq -r '[.[] | select(.conclusion == "success")][0].html_url // ""' <<< "$runs")
  open=$(jq -r '[.[] | select(.status != "completed")][0].id // ""' <<< "$runs")

  if [ -z "$url" ] && [ -n "$open" ]; then
    echo "pallas $repo@$sha: waiting for CI run $open"
    if gh run watch "$open" -R "$repo" --exit-status < /dev/null > /dev/null; then
      url=$(jq -r "[.[] | select(.id == $open)][0].html_url" <<< "$runs")
    fi
  fi

  if [ -z "$url" ]; then
    echo "::error::pallas $repo@$sha has no successful CI push run"
    failed=1
  else
    echo "pallas $repo@$sha passed CI in $url"
  fi
done

exit "$failed"
