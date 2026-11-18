#!/usr/bin/env bash
# The decisions of .github/workflows/release.yml, one subcommand per job.
#
#   release.sh verify <tag> <notes-file>
#       Fails unless <tag> is `v` plus the workspace version, the tagged
#       commit ($GITHUB_SHA, default HEAD) is an ancestor of $MAIN_REF
#       (default origin/main), and CHANGELOG.md has a non-empty section for
#       the version. Writes the release notes to <notes-file>: that section,
#       then the crates.io and docs.rs links of the version. Writes
#       `version` and `prerelease` to $GITHUB_OUTPUT when it is set.
#   release.sh missing
#       Prints the crates whose version the registry index does not list
#       yet, and writes `publish=true` to $GITHUB_OUTPUT when there is one,
#       `publish=false` otherwise, so the workflow obtains a registry token
#       only when something is left to publish. A failed lookup fails the
#       run.
#   release.sh publish
#       Publishes the macros crate, then the library, with the lockfile.
#       Before each crate it asks the registry index whether that name and
#       version exist, and skips the crate when they do, so a rerun
#       publishes only what is missing. A failed lookup fails the run before
#       anything else is published.
#   release.sh download <directory>
#       Downloads the two published `.crate` archives of the version from
#       the registry into <directory> and checks each against the SHA-256
#       the index records for it.
#   release.sh github-release <tag> <notes-file> <directory>
#       Creates the GitHub Release <tag> in $GITHUB_REPOSITORY with the notes
#       and the archives of <directory>, marked as a pre-release when the
#       version has a pre-release part. When the release exists, changes
#       nothing and succeeds.
#
# Every lookup fails closed: an answer the script cannot read stops the run.
# No subcommand commits, tags, or pushes.
#
# Environment: CRATES_INDEX (default https://index.crates.io), CRATES_STATIC
# (default https://static.crates.io/crates), CARGO_REGISTRY_TOKEN for
# publish (the short-lived token of trusted publishing), GH_TOKEN for gh.
#
# Tested offline by release.test.sh.
set -euo pipefail

index="${CRATES_INDEX:-https://index.crates.io}"
static="${CRATES_STATIC:-https://static.crates.io/crates}"
main_ref="${MAIN_REF:-origin/main}"
agent="claude-agent-rust-sdk-release (https://github.com/arthjean/claude-agent-rust-sdk)"
library=claude-agent-rust-sdk
crates=(claude-agent-rust-sdk-macros "$library")
tag_pattern='^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'

fail() {
  echo "release: $*" >&2
  exit 1
}

output() {
  if [ -n "${GITHUB_OUTPUT:-}" ]; then
    printf '%s\n' "$1" >> "$GITHUB_OUTPUT"
  fi
}

# The version of the library, as cargo reads the workspace.
workspace_version() {
  local metadata version
  metadata="$(cargo metadata --no-deps --format-version 1 --locked)" ||
    fail "cannot read the workspace metadata"
  version="$(printf '%s' "$metadata" |
    jq -r --arg name "$library" '.packages[] | select(.name == $name) | .version')" ||
    fail "cannot read the version of $library"
  [ -n "$version" ] || fail "the workspace has no package $library"
  printf '%s' "$version"
}

# The sparse index path of a crate name of four characters or more.
index_path() {
  printf '%s/%s/%s' "${1:0:2}" "${1:2:2}" "$1"
}

# Prints the index line of crate $1 at version $2, nothing when the version
# is not published; fails when the index cannot be read.
index_entry() {
  local name="$1" version="$2" file status
  file="$(mktemp)"
  status="$(curl --silent --show-error --location --user-agent "$agent" \
    --output "$file" --write-out '%{http_code}' "$index/$(index_path "$name")")" ||
    fail "cannot read the registry index: the lookup of $name failed"
  case "$status" in
    200)
      jq -c --arg version "$version" 'select(.vers == $version)' "$file" ||
        fail "cannot read the registry index: the entry of $name is not JSON lines"
      ;;
    404) ;;
    *) fail "cannot read the registry index: HTTP $status for $name" ;;
  esac
  rm -f "$file"
}

verify() {
  local tag="${1:-}" notes="${2:-}" version section commit
  [ -n "$notes" ] || fail "usage: release.sh verify <tag> <notes-file>"
  [[ "$tag" =~ $tag_pattern ]] || fail "$tag is not a release tag of the form vX.Y.Z[-pre]"
  version="$(workspace_version)"
  [ "$tag" = "v$version" ] || fail "tag $tag does not match the workspace version $version"
  commit="${GITHUB_SHA:-HEAD}"
  git merge-base --is-ancestor "$commit" "$main_ref" 2> /dev/null || fail "$tag is not on main"
  section="$(cargo xtask changelog-section "$version")" ||
    fail "CHANGELOG.md has no section for $version"
  {
    printf '%s\n\n' "$section"
    printf -- '- crates.io: https://crates.io/crates/%s/%s\n' "$library" "$version"
    printf -- '- docs.rs: https://docs.rs/%s/%s\n' "$library" "$version"
  } > "$notes"
  echo "$tag matches the workspace version, is on main, and has its changelog section"
  output "version=$version"
  if [[ "$version" == *-* ]]; then
    output "prerelease=true"
  else
    output "prerelease=false"
  fi
}

missing() {
  local version name entry missing=()
  version="$(workspace_version)"
  for name in "${crates[@]}"; do
    entry="$(index_entry "$name" "$version")"
    if [ -n "$entry" ]; then
      echo "$name $version is already published"
    else
      echo "$name $version is missing from the registry"
      missing+=("$name")
    fi
  done
  if [ "${#missing[@]}" -gt 0 ]; then
    output "publish=true"
  else
    echo "nothing to publish"
    output "publish=false"
  fi
}

publish() {
  local version name entry
  version="$(workspace_version)"
  for name in "${crates[@]}"; do
    entry="$(index_entry "$name" "$version")"
    if [ -n "$entry" ]; then
      echo "$name $version is already published; skipping it"
      continue
    fi
    echo "publishing $name $version"
    cargo publish --locked --package "$name" || fail "cargo publish of $name $version failed"
  done
}

download() {
  local directory="${1:-}" version name entry checksum file status actual
  [ -n "$directory" ] || fail "usage: release.sh download <directory>"
  version="$(workspace_version)"
  mkdir -p "$directory"
  for name in "${crates[@]}"; do
    entry="$(index_entry "$name" "$version")"
    [ -n "$entry" ] || fail "$name $version is not in the registry index"
    checksum="$(printf '%s' "$entry" | jq -r .cksum)"
    file="$directory/$name-$version.crate"
    status="$(curl --silent --show-error --location --user-agent "$agent" \
      --output "$file" --write-out '%{http_code}' "$static/$name/$name-$version.crate")" ||
      fail "cannot download $name $version"
    [ "$status" = 200 ] || fail "cannot download $name $version: HTTP $status"
    actual="$(sha256sum "$file" | cut -d ' ' -f 1)"
    [ "$actual" = "$checksum" ] ||
      fail "$name-$version.crate has SHA-256 $actual, but the index records $checksum"
    echo "downloaded $name-$version.crate ($actual)"
  done
}

github_release() {
  local tag="${1:-}" notes="${2:-}" directory="${3:-}" version error prerelease=()
  [[ "$tag" =~ $tag_pattern ]] || fail "$tag is not a release tag of the form vX.Y.Z[-pre]"
  [ -f "$notes" ] || fail "the notes file '$notes' is missing"
  [ -d "$directory" ] || fail "the archive directory '$directory' is missing"
  [ -n "${GITHUB_REPOSITORY:-}" ] || fail "GITHUB_REPOSITORY is not set"
  version="${tag#v}"
  if error="$(gh release view "$tag" --repo "$GITHUB_REPOSITORY" --json tagName 2>&1 > /dev/null)"; then
    echo "the GitHub Release $tag already exists; changing nothing"
    return 0
  fi
  case "$error" in
    *"release not found"*) ;;
    *) fail "cannot look up the GitHub Release $tag: $error" ;;
  esac
  local archives=()
  for name in "${crates[@]}"; do
    [ -f "$directory/$name-$version.crate" ] || fail "$directory/$name-$version.crate is missing"
    archives+=("$directory/$name-$version.crate")
  done
  if [[ "$version" == *-* ]]; then
    prerelease=(--prerelease)
  fi
  gh release create "$tag" --repo "$GITHUB_REPOSITORY" --verify-tag --title "$tag" \
    --notes-file "$notes" "${prerelease[@]}" "${archives[@]}"
}

case "${1:-}" in
  verify)
    shift
    verify "$@"
    ;;
  missing) missing ;;
  publish) publish ;;
  download)
    shift
    download "$@"
    ;;
  github-release)
    shift
    github_release "$@"
    ;;
  *) fail "usage: release.sh verify | missing | publish | download | github-release" ;;
esac
