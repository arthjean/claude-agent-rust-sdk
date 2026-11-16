#!/usr/bin/env bash
# Lists the actions used by the workflows that have a newer release, and fails
# when there is one. The lockfile keeps every action on a fixed commit, so
# nothing else says when an action has moved on.
#
# A release counts once it is older than COOLDOWN_DAYS (default 14), so a
# version published a few days ago is never adopted on the spot.
#
# To act on a failure: change the `uses:` lines to the versions listed, then
# run `gh actions-lock` and commit the workflows with the lockfile.
set -euo pipefail

cooldown_days="${COOLDOWN_DAYS:-14}"
cutoff="$(date -u -d "$cooldown_days days ago" +%Y-%m-%dT%H:%M:%SZ)"
lockfile=.github/workflows/actions.lock
full_version='^v?[0-9]+\.[0-9]+\.[0-9]+$'

report() {
  echo "$1"
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    echo "- $1" >> "$GITHUB_STEP_SUMMARY"
  fi
}

refs="$(grep -hoE 'uses: +[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+@[A-Za-z0-9_.-]+' .github/workflows/*.yml |
  sed -E 's/uses: +//' | sort -u)"

outdated=0
for ref in $refs; do
  repository="${ref%@*}"
  current="${ref#*@}"

  if [[ "$current" =~ $full_version ]]; then
    # A full version: look for a newer release that is past the cooldown.
    latest="$(gh api "repos/$repository/releases?per_page=100" --jq "
      .[] | select((.draft | not) and (.prerelease | not))
          | select(.published_at <= \"$cutoff\")
          | .tag_name" | grep -E "$full_version" | sort -V | tail -n 1 || true)"
    if [ -z "$latest" ] || [ "$latest" = "$current" ]; then
      continue
    fi
    newest="$(printf '%s\n%s\n' "$current" "$latest" | sort -V | tail -n 1)"
    if [ "$newest" = "$latest" ]; then
      report "$repository: $current -> $latest"
      outdated=1
    fi
  else
    # A movable tag or a branch: compare the locked commit with the commit the
    # reference names today.
    key="$(printf '%s' "$ref" | tr '[:upper:]' '[:lower:]')"
    locked="$(grep -F -A 2 "    '$key':" "$lockfile" | sed -nE "s/.*commit: 'sha1-([0-9a-f]+)'.*/\1/p" | head -n 1)"
    if [ -z "$locked" ]; then
      report "$repository@$current: no entry in $lockfile"
      outdated=1
      continue
    fi
    head_commit="$(gh api "repos/$repository/commits/$current" --jq '.sha + " " + .commit.committer.date')"
    head_sha="${head_commit%% *}"
    head_date="${head_commit##* }"
    if [ "$head_sha" != "$locked" ] && [[ "$head_date" < "$cutoff" || "$head_date" == "$cutoff" ]]; then
      report "$repository@$current: locked at ${locked:0:7}, now at ${head_sha:0:7} (run \`gh actions-lock --relock\`)"
      outdated=1
    fi
  fi
done

if [ "$outdated" -eq 0 ]; then
  echo "Every action is on its newest release older than $cooldown_days days."
fi
exit "$outdated"
