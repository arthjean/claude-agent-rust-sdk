#!/usr/bin/env bash
# Watches the upstream Python SDK for a release this port has not caught up
# with, for .github/workflows/upstream-watch.yml.
#
#   upstream-watch.sh detect
#       Resolves the newest release tag of the upstream repository and
#       compares it with the version of the submodule pin. When the release
#       is newer, runs `cargo xtask upstream-diff <tag>` and writes the body
#       of a tracking issue: that output and the matching section of the
#       upstream changelog. Writes `tag` (empty when nothing is newer) and
#       `body` to $GITHUB_OUTPUT when it is set, and the body to
#       $WATCH_BODY_FILE when that is set.
#   upstream-watch.sh report <tag> <body-file> [--dry-run]
#       Opens one issue titled after the tag in $GITHUB_REPOSITORY, unless
#       an issue with that title already exists, open or closed. With
#       --dry-run, prints the issue instead of opening it.
#
# Every lookup fails closed: when the release, the pin, the diff, or the
# issue search cannot be read, the script exits non-zero and never reports
# "up to date". It makes no commit and opens no pull request.
#
# Environment: UPSTREAM_REPO (default anthropics/claude-agent-sdk-python),
# UPSTREAM_DIR (default upstream/claude-agent-sdk-python), GH_TOKEN for gh.
#
# Tested offline by upstream-watch.test.sh.
set -euo pipefail

upstream_repo="${UPSTREAM_REPO:-anthropics/claude-agent-sdk-python}"
upstream_dir="${UPSTREAM_DIR:-upstream/claude-agent-sdk-python}"
tag_pattern='^v[0-9]+\.[0-9]+\.[0-9]+$'
body_limit=60000

fail() {
  echo "upstream-watch: $*" >&2
  exit 1
}

title_of() {
  printf 'Port upstream release %s' "$1"
}

# The version of the pinned upstream package, from its pyproject.toml.
pinned_version() {
  local manifest="$upstream_dir/pyproject.toml" version
  [ -f "$manifest" ] || fail "$manifest is missing; check out the submodule"
  version="$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$manifest" | head -n 1)"
  [[ "v$version" =~ $tag_pattern ]] || fail "no release version in $manifest (read '$version')"
  printf '%s' "$version"
}

# Whether version $1 is strictly newer than version $2.
newer() {
  [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -n 1)" = "$1" ]
}

# The section of CHANGELOG.md at revision $1 whose heading is version $2.
changelog_section() {
  local changelog
  changelog="$(git -C "$upstream_dir" show "$1:CHANGELOG.md")" ||
    fail "cannot read CHANGELOG.md at $1"
  printf '%s\n' "$changelog" | awk -v heading="## $2" '
    $0 == heading { inside = 1; next }
    inside && /^## / { exit }
    inside { print }
  '
}

output() {
  if [ -n "${GITHUB_OUTPUT:-}" ]; then
    printf '%s\n' "$1" >> "$GITHUB_OUTPUT"
  fi
}

detect() {
  local latest pinned version diff section body delimiter
  latest="$(gh api "repos/$upstream_repo/releases/latest" --jq .tag_name)" ||
    fail "cannot read the latest release of $upstream_repo"
  [[ "$latest" =~ $tag_pattern ]] ||
    fail "the latest release of $upstream_repo has tag '$latest', not vX.Y.Z"
  pinned="$(pinned_version)"
  version="${latest#v}"

  if ! newer "$version" "$pinned"; then
    echo "up to date: the newest upstream release is $latest and the pin is v$pinned"
    output "tag="
    return 0
  fi

  echo "upstream released $latest; the pin is v$pinned"
  diff="$(cargo xtask upstream-diff "$latest")" ||
    fail "cargo xtask upstream-diff $latest failed"
  section="$(changelog_section "$latest" "$version")"
  if [ -z "$section" ]; then
    section="CHANGELOG.md at $latest has no section for $version."
  fi
  body="$(
    printf 'Upstream published [%s](https://github.com/%s/releases/tag/%s); the submodule is pinned to v%s.\n\n' \
      "$latest" "$upstream_repo" "$latest" "$pinned"
    printf '## `cargo xtask upstream-diff %s`\n\n```text\n%s\n```\n\n' "$latest" "$diff"
    printf '## Changelog of %s\n\n%s\n\n' "$version" "$section"
    printf 'To port it: move the submodule pin to %s, port the changes, and add or remove ledger entries until `cargo xtask parity-check --deny-pending` passes.\n' "$latest"
  )"
  if [ "${#body}" -gt "$body_limit" ]; then
    body="${body:0:$body_limit}

(Truncated: run \`cargo xtask upstream-diff $latest\` for the full list.)"
  fi

  if [ -n "${WATCH_BODY_FILE:-}" ]; then
    printf '%s\n' "$body" > "$WATCH_BODY_FILE"
  fi
  output "tag=$latest"
  delimiter="body_$(od -An -N8 -tx1 /dev/urandom | tr -d ' \n')"
  output "body<<$delimiter"
  output "$body"
  output "$delimiter"
}

report() {
  local tag="${1:-}" body_file="${2:-}" dry_run="${3:-}" title existing
  [[ "$tag" =~ $tag_pattern ]] || fail "report needs a tag of the form vX.Y.Z, got '$tag'"
  [ -f "$body_file" ] || fail "report needs the body file, got '$body_file'"
  [ -z "$dry_run" ] || [ "$dry_run" = --dry-run ] || fail "unknown option $dry_run"
  [ -n "${GITHUB_REPOSITORY:-}" ] || fail "GITHUB_REPOSITORY is not set"
  title="$(title_of "$tag")"

  existing="$(gh issue list --repo "$GITHUB_REPOSITORY" --state all --limit 100 \
    --search "\"$title\" in:title" --json number,title \
    --jq ".[] | select(.title == \"$title\") | .number")" ||
    fail "cannot search the issues of $GITHUB_REPOSITORY"
  if [ -n "$existing" ]; then
    echo "issue #${existing%%$'\n'*} already tracks $tag; opening none"
    return 0
  fi

  if [ "$dry_run" = --dry-run ]; then
    echo "dry run: would open the issue \"$title\" in $GITHUB_REPOSITORY with this body:"
    echo
    cat "$body_file"
    return 0
  fi
  gh issue create --repo "$GITHUB_REPOSITORY" --title "$title" --body-file "$body_file"
}

case "${1:-}" in
  detect) detect ;;
  report)
    shift
    report "$@"
    ;;
  *) fail "usage: upstream-watch.sh detect | report <tag> <body-file> [--dry-run]" ;;
esac
