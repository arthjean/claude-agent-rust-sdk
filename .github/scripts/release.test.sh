#!/usr/bin/env bash
# Tests release.sh against stand-ins for `cargo`, `curl`, and `gh`, so every
# case runs offline and gives the same verdict each time. The registry is a
# directory of index files and archives served by the `curl` stand-in; the
# repository is a git repository built by the test, with a commit on main
# and one off it.
#
# Run it with `bash .github/scripts/release.test.sh`.
set -uo pipefail

script="$(cd "$(dirname "$0")" && pwd)/release.sh"
root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT

mkdir "$root/bin"
cat > "$root/bin/cargo" << 'EOF'
#!/usr/bin/env bash
set -euo pipefail
echo "cargo $*" >> "$FAKE/calls"
case "$1" in
  metadata)
    printf '{"packages":[{"name":"claude-agent-rust-sdk-macros","version":"%s"},{"name":"claude-agent-rust-sdk","version":"%s"}]}\n' \
      "$(cat "$FAKE/version")" "$(cat "$FAKE/version")"
    ;;
  xtask)
    if [ -e "$FAKE/section" ]; then
      cat "$FAKE/section"
    else
      echo "xtask: changelog-section: CHANGELOG.md has no section for $3" >&2
      exit 1
    fi
    ;;
  publish)
    name="$4"
    if [ -e "$FAKE/publish-fails-$name" ]; then
      echo "error: failed to publish $name" >&2
      exit 101
    fi
    echo "$name" >> "$FAKE/published"
    ;;
  *)
    echo "cargo stand-in: unexpected call: $*" >&2
    exit 2
    ;;
esac
EOF
cat > "$root/bin/curl" << 'EOF'
#!/usr/bin/env bash
set -euo pipefail
output="" url=""
while [ $# -gt 0 ]; do
  case "$1" in
    --output) output="$2"; shift 2 ;;
    --write-out | --user-agent) shift 2 ;;
    --*) shift ;;
    *) url="$1"; shift ;;
  esac
done
echo "curl $url" >> "$FAKE/calls"
if [ -e "$FAKE/curl-fails" ]; then
  echo "curl: (6) Could not resolve host" >&2
  exit 6
fi
if [ -e "$FAKE/curl-status" ]; then
  : > "$output"
  printf '%s' "$(cat "$FAKE/curl-status")"
  exit 0
fi
path="$FAKE/registry/${url#*://}"
if [ -f "$path" ]; then
  cp "$path" "$output"
  printf 200
else
  : > "$output"
  printf 404
fi
EOF
cat > "$root/bin/gh" << 'EOF'
#!/usr/bin/env bash
set -euo pipefail
echo "gh $*" >> "$FAKE/calls"
case "$1 ${2:-}" in
  "release view")
    if [ -e "$FAKE/release-exists" ]; then
      echo '{"tagName":"v0.1.3"}'
    elif [ -e "$FAKE/view-fails" ]; then
      echo "HTTP 401: Bad credentials" >&2
      exit 1
    else
      echo "release not found" >&2
      exit 1
    fi
    ;;
  "release create")
    echo "https://github.com/owner/repo/releases/tag/$3"
    ;;
  *)
    echo "gh stand-in: unexpected call: $*" >&2
    exit 2
    ;;
esac
EOF
chmod +x "$root/bin/cargo" "$root/bin/curl" "$root/bin/gh"

# The repository: main at one commit, and a branch with a commit off main.
repo="$root/repo"
mkdir "$repo"
git_repo() {
  git -C "$repo" -c user.name=Test -c user.email=test@example.invalid \
    -c commit.gpgsign=false "$@" > /dev/null
}
git_repo init --quiet --initial-branch=main
git_repo commit --quiet --allow-empty --message "on main"
on_main="$(git -C "$repo" rev-parse HEAD)"
git_repo checkout --quiet -b topic
git_repo commit --quiet --allow-empty --message "off main"
off_main="$(git -C "$repo" rev-parse HEAD)"
git_repo checkout --quiet main

passed=0
failed=0

# Each case gets a fresh directory for the stand-ins, the registry, and the
# outputs.
new_case() {
  FAKE="$(mktemp -d "$root/case.XXXXXX")"
  export FAKE
  : > "$FAKE/calls"
  echo 0.1.3 > "$FAKE/version"
  printf 'Upstream: Python SDK 0.2.183, CLI 2.1.326\n\n### Fixed\n\n- A fix.\n' > "$FAKE/section"
  mkdir -p "$FAKE/registry/index.crates.io/cl/au" "$FAKE/registry/static.crates.io/crates" "$FAKE/dist"
}

# Publishes crate $1 at version $2 in the stand-in registry: an archive and
# its index line.
registry_has() {
  local name="$1" version="$2" archive checksum
  mkdir -p "$FAKE/registry/static.crates.io/crates/$name"
  archive="$FAKE/registry/static.crates.io/crates/$name/$name-$version.crate"
  printf 'archive of %s %s' "$name" "$version" > "$archive"
  checksum="$(sha256sum "$archive" | cut -d ' ' -f 1)"
  printf '{"name":"%s","vers":"0.0.1","cksum":"00"}\n{"name":"%s","vers":"%s","cksum":"%s"}\n' \
    "$name" "$name" "$version" "$checksum" >> "$FAKE/registry/index.crates.io/cl/au/$name"
}

run() {
  (cd "$repo" && PATH="$root/bin:$PATH" GITHUB_REPOSITORY=owner/repo GITHUB_SHA="${SHA:-$on_main}" \
    MAIN_REF=main GITHUB_OUTPUT="$FAKE/output" bash "$script" "$@" > "$FAKE/stdout" 2> "$FAKE/stderr")
  echo $? > "$FAKE/status"
}

check() {
  local name="$1"
  shift
  if "$@"; then
    passed=$((passed + 1))
  else
    failed=$((failed + 1))
    echo "FAIL: $name"
    for file in status stdout stderr calls output published notes.md; do
      echo "--- $file"
      cat "$FAKE/$file" 2> /dev/null
    done
  fi
}

status_is() { [ "$(cat "$FAKE/status")" = "$1" ]; }
contains() { grep -qF -- "$2" "$FAKE/$1"; }
lacks() { ! grep -qF -- "$2" "$FAKE/$1" 2> /dev/null; }

# --- verify -----------------------------------------------------------------
new_case
run verify v0.1.3 "$FAKE/notes.md"
check "verify: green" status_is 0
check "verify: version output" contains output "version=0.1.3"
check "verify: not a pre-release" contains output "prerelease=false"
check "verify: notes start with the section" test "$(head -n 1 "$FAKE/notes.md")" = "Upstream: Python SDK 0.2.183, CLI 2.1.326"
check "verify: notes hold the entries" contains notes.md "- A fix."
check "verify: notes link crates.io" contains notes.md "- crates.io: https://crates.io/crates/claude-agent-rust-sdk/0.1.3"
check "verify: notes link docs.rs" contains notes.md "- docs.rs: https://docs.rs/claude-agent-rust-sdk/0.1.3"
check "verify: reads the section through xtask" contains calls "cargo xtask changelog-section 0.1.3"

new_case
echo 0.1.2 > "$FAKE/version"
run verify v0.1.3 "$FAKE/notes.md"
check "tag mismatch: fails" status_is 1
check "tag mismatch: names both" contains stderr "tag v0.1.3 does not match the workspace version 0.1.2"
check "tag mismatch: no notes" test ! -e "$FAKE/notes.md"

new_case
SHA="$off_main" run verify v0.1.3 "$FAKE/notes.md"
check "off main: fails" status_is 1
check "off main: says so" contains stderr "v0.1.3 is not on main"
check "off main: no version output" lacks output "version="

new_case
rm "$FAKE/section"
run verify v0.1.3 "$FAKE/notes.md"
check "missing section: fails" status_is 1
check "missing section: says so" contains stderr "CHANGELOG.md has no section for 0.1.3"

new_case
echo 0.2.0-rc.1 > "$FAKE/version"
run verify v0.2.0-rc.1 "$FAKE/notes.md"
check "pre-release verify: green" status_is 0
check "pre-release verify: flagged" contains output "prerelease=true"

new_case
run verify 'v1; rm -rf /' "$FAKE/notes.md"
check "malformed tag: fails" status_is 1
check "malformed tag: no cargo call" lacks calls "cargo"

# --- publish ----------------------------------------------------------------
new_case
run publish
check "publish: green" status_is 0
check "publish: macros first, then the library" \
  test "$(cat "$FAKE/published")" = "$(printf 'claude-agent-rust-sdk-macros\nclaude-agent-rust-sdk')"
check "publish: looks up the index" contains calls "curl https://index.crates.io/cl/au/claude-agent-rust-sdk-macros"
check "publish: with the lockfile" contains calls "cargo publish --locked --package claude-agent-rust-sdk"

new_case
registry_has claude-agent-rust-sdk-macros 0.1.3
registry_has claude-agent-rust-sdk 0.1.3
run publish
check "already published: green" status_is 0
check "already published: says so" contains stdout "claude-agent-rust-sdk 0.1.3 is already published; skipping it"
check "already published: publishes nothing" test ! -e "$FAKE/published"

new_case
registry_has claude-agent-rust-sdk-macros 0.1.2
touch "$FAKE/publish-fails-claude-agent-rust-sdk"
run publish
check "second crate fails: fails" status_is 1
check "second crate fails: names it" contains stderr "cargo publish of claude-agent-rust-sdk 0.1.3 failed"
check "second crate fails: macros published" test "$(cat "$FAKE/published")" = claude-agent-rust-sdk-macros
# The rerun after the failure: the macros crate is in the index now.
registry_has claude-agent-rust-sdk-macros 0.1.3
rm "$FAKE/publish-fails-claude-agent-rust-sdk" "$FAKE/published"
run publish
check "rerun: green" status_is 0
check "rerun: skips the macros crate" contains stdout "claude-agent-rust-sdk-macros 0.1.3 is already published"
check "rerun: publishes only the library" test "$(cat "$FAKE/published")" = claude-agent-rust-sdk

new_case
touch "$FAKE/curl-fails"
run publish
check "lookup error: fails" status_is 1
check "lookup error: names the index" contains stderr "cannot read the registry index"
check "lookup error: publishes nothing" test ! -e "$FAKE/published"
check "lookup error: no cargo publish" lacks calls "cargo publish"

new_case
echo 503 > "$FAKE/curl-status"
run publish
check "index HTTP error: fails" status_is 1
check "index HTTP error: names the status" contains stderr "cannot read the registry index: HTTP 503"
check "index HTTP error: publishes nothing" test ! -e "$FAKE/published"

# --- missing ----------------------------------------------------------------
new_case
run missing
check "nothing published: green" status_is 0
check "nothing published: publish" contains output "publish=true"
check "nothing published: names both" contains stdout "claude-agent-rust-sdk-macros 0.1.3 is missing from the registry"

new_case
registry_has claude-agent-rust-sdk-macros 0.1.3
run missing
check "macros published: publish" contains output "publish=true"
check "macros published: names the library" contains stdout "claude-agent-rust-sdk 0.1.3 is missing from the registry"

new_case
registry_has claude-agent-rust-sdk-macros 0.1.3
registry_has claude-agent-rust-sdk 0.1.3
run missing
check "both published: green" status_is 0
check "both published: no token needed" contains output "publish=false"
check "both published: says so" contains stdout "nothing to publish"
check "both published: no cargo publish" lacks calls "cargo publish"

new_case
touch "$FAKE/curl-fails"
run missing
check "missing lookup error: fails" status_is 1
check "missing lookup error: names the index" contains stderr "cannot read the registry index"
check "missing lookup error: no decision" lacks output "publish="

# --- download ---------------------------------------------------------------
new_case
registry_has claude-agent-rust-sdk-macros 0.1.3
registry_has claude-agent-rust-sdk 0.1.3
run download "$FAKE/dist"
check "download: green" status_is 0
check "download: both archives" test -f "$FAKE/dist/claude-agent-rust-sdk-macros-0.1.3.crate" -a -f "$FAKE/dist/claude-agent-rust-sdk-0.1.3.crate"

new_case
registry_has claude-agent-rust-sdk-macros 0.1.3
registry_has claude-agent-rust-sdk 0.1.3
printf 'tampered' > "$FAKE/registry/static.crates.io/crates/claude-agent-rust-sdk/claude-agent-rust-sdk-0.1.3.crate"
run download "$FAKE/dist"
check "checksum mismatch: fails" status_is 1
check "checksum mismatch: says so" contains stderr "claude-agent-rust-sdk-0.1.3.crate has SHA-256"

new_case
registry_has claude-agent-rust-sdk-macros 0.1.3
run download "$FAKE/dist"
check "unpublished archive: fails" status_is 1
check "unpublished archive: names it" contains stderr "claude-agent-rust-sdk 0.1.3 is not in the registry index"

# --- github-release -----------------------------------------------------------
release_case() {
  new_case
  printf 'Upstream: Python SDK 0.2.183, CLI 2.1.326\n\n- A fix.\n' > "$FAKE/notes.md"
  for name in claude-agent-rust-sdk-macros claude-agent-rust-sdk; do
    printf 'archive' > "$FAKE/dist/$name-$1.crate"
  done
}

release_case 0.1.3
run github-release v0.1.3 "$FAKE/notes.md" "$FAKE/dist"
check "release: green" status_is 0
check "release: created with the notes and both archives" contains calls \
  "gh release create v0.1.3 --repo owner/repo --verify-tag --title v0.1.3 --notes-file $FAKE/notes.md $FAKE/dist/claude-agent-rust-sdk-macros-0.1.3.crate $FAKE/dist/claude-agent-rust-sdk-0.1.3.crate"
check "release: not a pre-release" lacks calls "--prerelease"

release_case 0.2.0-rc.1
run github-release v0.2.0-rc.1 "$FAKE/notes.md" "$FAKE/dist"
check "pre-release: green" status_is 0
check "pre-release: flagged" contains calls "--notes-file $FAKE/notes.md --prerelease $FAKE/dist/"

release_case 0.1.3
touch "$FAKE/release-exists"
run github-release v0.1.3 "$FAKE/notes.md" "$FAKE/dist"
check "existing release: green" status_is 0
check "existing release: says so" contains stdout "the GitHub Release v0.1.3 already exists; changing nothing"
check "existing release: creates none" lacks calls "release create"

release_case 0.1.3
touch "$FAKE/view-fails"
run github-release v0.1.3 "$FAKE/notes.md" "$FAKE/dist"
check "lookup failure: fails" status_is 1
check "lookup failure: names the cause" contains stderr "cannot look up the GitHub Release v0.1.3: HTTP 401"
check "lookup failure: creates none" lacks calls "release create"

release_case 0.1.3
rm "$FAKE/dist/claude-agent-rust-sdk-0.1.3.crate"
run github-release v0.1.3 "$FAKE/notes.md" "$FAKE/dist"
check "missing archive: fails" status_is 1
check "missing archive: creates none" lacks calls "release create"

# --- No subcommand commits, tags, or pushes ------------------------------------
check "no git write in the script" bash -c "! grep -Eq 'git (commit|tag|push)' '$script'"

echo "release.test.sh: $passed passed, $failed failed"
[ "$failed" -eq 0 ]
