#!/usr/bin/env bash
# Lists the actions used by the workflows that have a newer release, and fails
# when there is one. The lockfile keeps every action on a fixed commit, so
# nothing else says when an action has moved on.
#
# A release counts once it is older than COOLDOWN_DAYS (default 14), so a
# version published a few days ago is never adopted on the spot.
#
# Anything the script cannot check is reported and fails the run too: a `uses:`
# line in a form it does not read, an action that publishes no release, a
# lookup that did not answer.
#
# To act on a failure, run the script locally with --apply. It rewrites the
# `uses:` lines to the versions listed and refreshes the lockfile with
# `gh actions-lock`. Review the diff, then commit the workflows with the
# lockfile.
set -euo pipefail
shopt -s nullglob

apply=0
case "${1:-}" in
  '') ;;
  --apply) apply=1 ;;
  *)
    echo "usage: $0 [--apply]" >&2
    exit 2
    ;;
esac

cooldown_days="${COOLDOWN_DAYS:-14}"
cutoff="$(date -u -d "$cooldown_days days ago" +%Y-%m-%dT%H:%M:%SZ)"
lockfile=.github/workflows/actions.lock
workflows=(.github/workflows/*.yml .github/workflows/*.yaml)
full_version='v?[0-9]+\.[0-9]+\.[0-9]+'
# owner/name, an optional path inside the repository, then the reference.
action_ref='^([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)(/[A-Za-z0-9_./-]+)?@([A-Za-z0-9_./-]+)$'

report() {
  echo "$1"
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    echo "- $1" >> "$GITHUB_STEP_SUMMARY"
  fi
}

# Points every `uses:` line of an action at another version.
rewrite() {
  local action="${1//./\\.}" from="${2//./\\.}" to="$3"
  sed -i -E "s|^([[:space:]]*(-[[:space:]]+)?uses:[[:space:]]*['\"]?${action}(/[A-Za-z0-9_./-]+)?@)${from}(['\"]?[[:space:]]*(#.*)?\r?)$|\1${to}\4|" "${workflows[@]}"
}

if [ "$apply" -eq 1 ]; then
  # A full version that no longer names its locked commit fails this check,
  # while a movable tag that moved does not. Passing it first means the relock
  # below can only advance movable tags, never accept a rewritten version.
  gh actions-lock --verify
fi

# Every `uses:` value, without its quotes, trailing comment or line ending.
values="$({ grep -hE '^[[:space:]]*(-[[:space:]]+)?uses:' "${workflows[@]}" || true; } |
  sed -E "s/\r$//; s/^[[:space:]]*(-[[:space:]]+)?uses:[[:space:]]*//; s/[[:space:]]+#.*$//; s/[[:space:]]+$//; s/^(['\"])(.*)\1$/\2/" |
  sort -u)"

declare -A seen=()
findings=0
rewritten=0
relock=0
while IFS= read -r value <&3; do
  case "$value" in
    '' | ./* | '$/'*) continue ;; # An action of this repository: nothing to compare.
  esac

  if ! [[ "$value" =~ $action_ref ]]; then
    report "uses: $value: not a form this script can check"
    findings=1
    continue
  fi
  repository="${BASH_REMATCH[1]}"
  current="${BASH_REMATCH[3]}"

  # Several paths of one repository share its releases and its lockfile entry.
  if [ -n "${seen["$repository@$current"]:-}" ]; then
    continue
  fi
  seen["$repository@$current"]=1

  if [[ "$current" =~ ^$full_version$ ]]; then
    # A full version: look for a newer release that is past the cooldown.
    if ! releases="$(gh api "repos/$repository/releases?per_page=100" --jq '
      .[] | select((.draft | not) and (.prerelease | not))
          | .tag_name + " " + .published_at')"; then
      report "$repository: could not read its releases"
      findings=1
      continue
    fi
    versions="$(grep -E "^$full_version " <<< "$releases" || true)"
    if [ -z "$versions" ]; then
      report "$repository: publishes no release with a full version, so $current cannot be checked"
      findings=1
      continue
    fi
    latest="$(awk -v cutoff="$cutoff" '$2 <= cutoff { print $1 }' <<< "$versions" | sort -V | tail -n 1)"
    if [ -z "$latest" ] || [ "$latest" = "$current" ]; then
      continue
    fi
    newest="$(printf '%s\n%s\n' "$current" "$latest" | sort -V | tail -n 1)"
    if [ "$newest" = "$latest" ]; then
      if [ "$apply" -eq 1 ]; then
        rewrite "$repository" "$current" "$latest"
        report "$repository: $current -> $latest (applied)"
        rewritten=1
      else
        report "$repository: $current -> $latest"
        findings=1
      fi
    fi
  else
    # A movable tag or a branch: compare the locked commit with the commit the
    # reference names today.
    key="$(printf '%s@%s' "$repository" "$current" | tr '[:upper:]' '[:lower:]')"
    locked="$({ grep -F -A 2 "    '$key':" "$lockfile" || true; } |
      sed -nE "s/.*commit: 'sha1-([0-9a-f]+)'.*/\1/p" | head -n 1)"
    if [ -z "$locked" ]; then
      report "$repository@$current: no entry in $lockfile"
      findings=1
      continue
    fi
    if ! head_commit="$(gh api "repos/$repository/commits/$current" --jq '.sha + " " + .commit.committer.date')"; then
      report "$repository@$current: could not read the commit it names"
      findings=1
      continue
    fi
    head_sha="${head_commit%% *}"
    head_date="${head_commit##* }"
    if [ "$head_sha" != "$locked" ] && [[ "$head_date" < "$cutoff" || "$head_date" == "$cutoff" ]]; then
      if [ "$apply" -eq 1 ]; then
        report "$repository@$current: locked at ${locked:0:7}, now at ${head_sha:0:7} (relocked)"
        relock=1
      else
        report "$repository@$current: locked at ${locked:0:7}, now at ${head_sha:0:7}"
        findings=1
      fi
    fi
  fi
done 3<<< "$values"

if [ "$rewritten" -eq 1 ]; then
  gh actions-lock --no-interactive
fi
if [ "$relock" -eq 1 ]; then
  gh actions-lock --no-interactive --relock
fi

if [ "$rewritten" -eq 1 ] || [ "$relock" -eq 1 ]; then
  echo "Review the diff, then commit the workflows with $lockfile."
elif [ "$findings" -eq 0 ]; then
  echo "Every action is on its newest release older than $cooldown_days days."
elif [ "$apply" -eq 0 ]; then
  echo "Run \`bash .github/scripts/actions-freshness.sh --apply\` locally to update the outdated ones."
fi
exit "$findings"
