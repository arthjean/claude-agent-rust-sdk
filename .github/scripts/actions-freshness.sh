#!/usr/bin/env bash
# Lists the actions used by the workflows that have a newer release, and fails
# when there is one. The lockfile keeps every action on a fixed commit, so
# nothing else says when an action has moved on.
#
# A release counts once it is older than COOLDOWN_DAYS (default 14), so a
# version published a few days ago is never adopted on the spot. The age of a
# release is not taken from its tag, which its owner can move, nor from the
# dates inside a commit, which its author sets freely. The release must be
# immutable, or its commit must already have been on the default branch when
# the cooldown started, according to GitHub's own log of branch updates.
#
# Anything the script cannot check is reported and fails the run too: a `uses:`
# line in a form it does not read, an action that publishes no release, a
# lookup that did not answer, a commit whose age nothing shows.
#
# To act on a failure, run the script locally with --apply. It rewrites the
# `uses:` lines to the versions listed and refreshes the lockfile with
# `gh actions-lock`. It then compares the lockfile with the commits it checked
# and puts every file back when another entry changed. Review the diff, then
# commit the workflows with the lockfile.
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
workflow_dir=.github/workflows
lockfile="$workflow_dir/actions.lock"
workflows=("$workflow_dir"/*.yml "$workflow_dir"/*.yaml)
full_version='v?[0-9]+\.[0-9]+\.[0-9]+'
# owner/name, an optional path inside the repository, then the reference.
action_ref='^([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)(/[A-Za-z0-9_./-]+)?@([A-Za-z0-9_./-]+)$'

report() {
  echo "$1"
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    echo "- $1" >> "$GITHUB_STEP_SUMMARY"
  fi
}

# Says where a commit stood when the cooldown started: "settled" when the
# default branch of its repository already held it, "fresh" when it landed
# since, "unknown" when nothing shows either.
commit_age() {
  local repository="$1" sha="$2" branch base status
  branch="$(gh api "repos/$repository" --jq .default_branch)" || branch=''
  if [ -n "$branch" ]; then
    # The newest update of the branch that is older than the cooldown.
    base="$(gh api "repos/$repository/activity?ref=refs/heads/$branch&per_page=100" \
      --jq "[.[] | select(.timestamp <= \"$cutoff\")][0].after // empty")" || base=''
  fi
  if [ -n "${base:-}" ]; then
    status="$(gh api "repos/$repository/compare/$base...$sha" --jq .status)" || status=''
  fi
  case "${status:-}" in
    behind | identical) echo settled ;;
    ahead) echo fresh ;;
    *) echo unknown ;;
  esac
}

# Points every `uses:` line of an action at another version.
rewrite() {
  local action="${1//./\\.}" from="${2//./\\.}" to="$3"
  sed -i -E "s|^([[:space:]]*(-[[:space:]]+)?uses:[[:space:]]*['\"]?${action}(/[A-Za-z0-9_./-]+)?@)${from}(['\"]?[[:space:]]*(#.*)?\r?)$|\1${to}\4|" "${workflows[@]}"
}

lock_key() {
  printf '%s@%s' "$1" "$2" | tr '[:upper:]' '[:lower:]'
}

# Every locked action as "key commit".
lock_entries() {
  awk -F"'" '
    /^    [^ ].*:\r?$/ { key = $2 }
    $1 ~ /^ +commit: $/ { sub(/^sha1-/, "", $2); print key, $2 }
  ' "$lockfile"
}

# Refreshes the lockfile, then holds it to the commits checked above: an entry
# the script approved must name the commit it checked, and no other entry that
# was already locked may change.
lock_and_check() {
  local key sha ok=0
  declare -A before=() after=()
  while read -r key sha; do before["$key"]="$sha"; done < <(lock_entries)

  if [ "$rewritten" -eq 1 ]; then
    gh actions-lock --no-interactive || return 1
  fi
  if [ "$relock" -eq 1 ]; then
    gh actions-lock --no-interactive --relock || return 1
  fi

  while read -r key sha; do
    after["$key"]="$sha"
    if [ -n "${approved["$key"]:-}" ]; then
      if [ "$sha" != "${approved["$key"]}" ]; then
        echo "$key: locked at ${sha:0:7}, but ${approved["$key"]:0:7} is the commit that was checked" >&2
        ok=1
      fi
    elif [ -z "${before["$key"]:-}" ]; then
      echo "$key: new entry at ${sha:0:7}, brought in by another action and not checked here"
    elif [ "$sha" != "${before["$key"]}" ]; then
      echo "$key: moved from ${before["$key"]:0:7} to ${sha:0:7} without being checked" >&2
      ok=1
    fi
  done < <(lock_entries)
  for key in "${!approved[@]}"; do
    if [ -z "${after["$key"]:-}" ]; then
      echo "$key: missing from $lockfile" >&2
      ok=1
    fi
  done
  return "$ok"
}

if [ "$apply" -eq 1 ]; then
  # A full version that no longer names its locked commit fails this check,
  # while a movable tag that moved does not.
  gh actions-lock --verify
  backup="$(mktemp -d)"
  trap 'rm -rf "$backup"' EXIT
  cp -a "$workflow_dir/." "$backup/"
fi

# Every `uses:` value, without its quotes, trailing comment or line ending.
values="$({ grep -hE '^[[:space:]]*(-[[:space:]]+)?uses:' "${workflows[@]}" || true; } |
  sed -E "s/\r$//; s/^[[:space:]]*(-[[:space:]]+)?uses:[[:space:]]*//; s/[[:space:]]+#.*$//; s/[[:space:]]+$//; s/^(['\"])(.*)\1$/\2/" |
  sort -u)"

declare -A seen=() approved=()
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
          | .tag_name + " " + .published_at + " " + (.immutable == true | tostring)')"; then
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
    if [ "$newest" != "$latest" ]; then
      continue
    fi

    if ! sha="$(gh api "repos/$repository/commits/$latest" --jq .sha)"; then
      report "$repository: could not read the commit of $latest"
      findings=1
      continue
    fi
    # An immutable release keeps its tag on the commit it was published with.
    immutable="$(awk -v tag="$latest" '$1 == tag { print $3 }' <<< "$versions")"
    if [ "$immutable" != true ] && [ "$(commit_age "$repository" "$sha")" != settled ]; then
      report "$repository: $current -> $latest, but nothing shows that its commit ${sha:0:7} is $cooldown_days days old: check it by hand"
      findings=1
      continue
    fi

    if [ "$apply" -eq 1 ]; then
      rewrite "$repository" "$current" "$latest"
      approved["$(lock_key "$repository" "$latest")"]="$sha"
      report "$repository: $current -> $latest (applied)"
      rewritten=1
    else
      report "$repository: $current -> $latest"
      findings=1
    fi
  else
    # A movable tag or a branch: compare the locked commit with the commit the
    # reference names today.
    key="$(lock_key "$repository" "$current")"
    locked="$({ grep -F -A 2 "    '$key':" "$lockfile" || true; } |
      sed -nE "s/.*commit: 'sha1-([0-9a-f]+)'.*/\1/p" | head -n 1)"
    if [ -z "$locked" ]; then
      report "$repository@$current: no entry in $lockfile"
      findings=1
      continue
    fi
    if ! head_sha="$(gh api "repos/$repository/commits/$current" --jq .sha)"; then
      report "$repository@$current: could not read the commit it names"
      findings=1
      continue
    fi
    if [ "$head_sha" = "$locked" ]; then
      continue
    fi
    case "$(commit_age "$repository" "$head_sha")" in
      fresh) ;; # Still inside the cooldown.
      settled)
        if [ "$apply" -eq 1 ]; then
          approved["$key"]="$head_sha"
          report "$repository@$current: locked at ${locked:0:7}, now at ${head_sha:0:7} (relocked)"
          relock=1
        else
          report "$repository@$current: locked at ${locked:0:7}, now at ${head_sha:0:7}"
          findings=1
        fi
        ;;
      *)
        report "$repository@$current: locked at ${locked:0:7}, now at ${head_sha:0:7}, but nothing shows that this commit is $cooldown_days days old: check it by hand"
        findings=1
        ;;
    esac
  fi
done 3<<< "$values"

if [ "$rewritten" -eq 1 ] || [ "$relock" -eq 1 ]; then
  if ! lock_and_check; then
    cp -a "$backup/." "$workflow_dir/"
    echo "Nothing was kept: the workflows and $lockfile are back as they were." >&2
    exit 1
  fi
  echo "Review the diff, then commit the workflows with $lockfile."
elif [ "$findings" -eq 0 ]; then
  echo "Every action is on its newest release older than $cooldown_days days."
elif [ "$apply" -eq 0 ]; then
  echo "Run \`bash .github/scripts/actions-freshness.sh --apply\` locally to update the outdated ones."
fi
exit "$findings"
