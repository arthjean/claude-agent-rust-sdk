#!/usr/bin/env bash
# Tests upstream-watch.sh against stand-ins for `gh` and `cargo`, so every
# case runs offline and gives the same verdict each time. The upstream
# checkout is a git repository built by the test, with the pinned version
# and a tagged newer release.
#
# Run it with `bash .github/scripts/upstream-watch.test.sh`.
set -uo pipefail

script="$(cd "$(dirname "$0")" && pwd)/upstream-watch.sh"
root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT

mkdir "$root/bin"
cat > "$root/bin/gh" << 'EOF'
#!/usr/bin/env bash
set -euo pipefail
echo "gh $*" >> "$FAKE/calls"
case "$1 ${2:-}" in
  "api repos/anthropics/claude-agent-sdk-python/releases/latest"*)
    if [ -e "$FAKE/api-fails" ]; then
      echo "gh: HTTP 502" >&2
      exit 1
    fi
    cat "$FAKE/latest"
    ;;
  "issue list")
    if [ -e "$FAKE/list-fails" ]; then
      echo "gh: HTTP 401" >&2
      exit 1
    fi
    cat "$FAKE/existing" 2> /dev/null || true
    ;;
  "issue create")
    echo "https://github.com/owner/repo/issues/12"
    ;;
  *)
    echo "gh stand-in: unexpected call: $*" >&2
    exit 2
    ;;
esac
EOF
cat > "$root/bin/cargo" << 'EOF'
#!/usr/bin/env bash
set -euo pipefail
echo "cargo $*" >> "$FAKE/calls"
if [ -e "$FAKE/diff-fails" ]; then
  echo "xtask: cannot fetch tags" >&2
  exit 1
fi
printf 'Upstream 4720d36 -> %s\n\nExported names: +1 -0\n  + NewExport\n' "$3"
EOF
chmod +x "$root/bin/gh" "$root/bin/cargo"

# The upstream checkout: the pin at 0.2.182, and a tag v0.2.183 whose
# changelog has a section for it.
upstream="$root/upstream"
mkdir "$upstream"
git_upstream() {
  git -C "$upstream" -c user.name=Test -c user.email=test@example.invalid \
    -c commit.gpgsign=false -c tag.gpgsign=false "$@" > /dev/null
}
git_upstream init --quiet
printf '[project]\nname = "claude-agent-sdk"\nversion = "0.2.182"\n' > "$upstream/pyproject.toml"
printf '# Changelog\n\n## 0.2.182\n\n- Old change\n' > "$upstream/CHANGELOG.md"
git_upstream add --all
git_upstream commit --quiet --message "Release 0.2.182"
git_upstream tag v0.2.182
printf '# Changelog\n\n## 0.2.183\n\n### New Features\n\n- A new option\n\n## 0.2.182\n\n- Old change\n' \
  > "$upstream/CHANGELOG.md"
git_upstream commit --quiet --all --message "Release 0.2.183"
git_upstream tag v0.2.183
git_upstream checkout --quiet v0.2.182

passed=0
failed=0

# Each case gets a fresh directory for the stand-ins and the outputs.
new_case() {
  FAKE="$(mktemp -d "$root/case.XXXXXX")"
  export FAKE
  : > "$FAKE/calls"
}

run() {
  PATH="$root/bin:$PATH" UPSTREAM_DIR="$upstream" GITHUB_REPOSITORY=owner/repo \
    GITHUB_OUTPUT="$FAKE/output" WATCH_BODY_FILE="$FAKE/body.md" \
    bash "$script" "$@" > "$FAKE/stdout" 2> "$FAKE/stderr"
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
    for file in status stdout stderr calls output; do
      echo "--- $file"
      cat "$FAKE/$file" 2> /dev/null
    done
  fi
}

status_is() { [ "$(cat "$FAKE/status")" = "$1" ]; }
contains() { grep -qF -- "$2" "$FAKE/$1"; }
lacks() { ! grep -qF -- "$2" "$FAKE/$1" 2> /dev/null; }

# --- No new tag: the newest release is the pin ----------------------------
new_case
echo v0.2.182 > "$FAKE/latest"
run detect
check "no new tag: green" status_is 0
check "no new tag: says up to date" contains stdout "up to date: the newest upstream release is v0.2.182"
check "no new tag: empty tag output" contains output "tag="
check "no new tag: no diff" lacks calls "cargo"
check "no new tag: no body" test ! -e "$FAKE/body.md"

# --- An older release than the pin counts as up to date --------------------
new_case
echo v0.2.181 > "$FAKE/latest"
run detect
check "older release: green" status_is 0
check "older release: no diff" lacks calls "cargo"

# --- New tag: the diff and the changelog section make the body ------------
new_case
echo v0.2.183 > "$FAKE/latest"
run detect
check "new tag: green" status_is 0
check "new tag: runs the diff" contains calls "cargo xtask upstream-diff v0.2.183"
check "new tag: tag output" contains output "tag=v0.2.183"
check "new tag: body holds the diff" contains body.md "  + NewExport"
check "new tag: body holds the changelog section" contains body.md "- A new option"
check "new tag: body stops at the next section" lacks body.md "- Old change"
check "new tag: body names the pin" contains body.md "the submodule is pinned to v0.2.182"
check "new tag: body in the output" contains output "Exported names: +1 -0"
check "new tag: does not say up to date" lacks stdout "up to date"
cp "$FAKE/body.md" "$root/body.md"

new_case
run report v0.2.183 "$root/body.md"
check "new tag: report green" status_is 0
check "new tag: searches every issue by title" \
  contains calls 'gh issue list --repo owner/repo --state all --limit 100 --search "Port upstream release v0.2.183" in:title'
check "new tag: opens one issue titled with the tag" \
  contains calls "gh issue create --repo owner/repo --title Port upstream release v0.2.183 --body-file $root/body.md"
check "new tag: one creation only" test "$(grep -c 'issue create' "$FAKE/calls")" = 1

# --- A release whose changelog has no section yet -------------------------
new_case
git_upstream tag v0.2.184 v0.2.182
echo v0.2.184 > "$FAKE/latest"
run detect
check "missing section: green" status_is 0
check "missing section: says so" contains body.md "CHANGELOG.md at v0.2.184 has no section for 0.2.184."

# --- An issue for that tag already exists ---------------------------------
new_case
echo 7 > "$FAKE/existing"
run report v0.2.183 "$root/body.md"
check "existing issue: green" status_is 0
check "existing issue: names it" contains stdout "issue #7 already tracks v0.2.183"
check "existing issue: opens none" lacks calls "issue create"

# --- Dry run --------------------------------------------------------------
new_case
run report v0.2.183 "$root/body.md" --dry-run
check "dry run: green" status_is 0
check "dry run: prints the title" contains stdout 'would open the issue "Port upstream release v0.2.183" in owner/repo'
check "dry run: prints the body" contains stdout "  + NewExport"
check "dry run: opens none" lacks calls "issue create"

# --- Lookup failure: the run fails and never says up to date ---------------
new_case
touch "$FAKE/api-fails"
run detect
check "lookup failure: fails" status_is 1
check "lookup failure: names the cause" contains stderr "cannot read the latest release of anthropics/claude-agent-sdk-python"
check "lookup failure: not up to date" lacks stdout "up to date"
check "lookup failure: no tag output" lacks output "tag="

new_case
echo "nightly" > "$FAKE/latest"
run detect
check "malformed tag: fails" status_is 1
check "malformed tag: names it" contains stderr "has tag 'nightly', not vX.Y.Z"

new_case
echo "" > "$FAKE/latest"
run detect
check "empty tag: fails" status_is 1

# --- Diff failure ----------------------------------------------------------
new_case
echo v0.2.183 > "$FAKE/latest"
touch "$FAKE/diff-fails"
run detect
check "diff failure: fails" status_is 1
check "diff failure: names the command" contains stderr "cargo xtask upstream-diff v0.2.183 failed"
check "diff failure: no tag output" lacks output "tag=v0.2.183"

# --- Issue search failure: no issue is opened blind ------------------------
new_case
touch "$FAKE/list-fails"
run report v0.2.183 "$root/body.md"
check "search failure: fails" status_is 1
check "search failure: names the cause" contains stderr "cannot search the issues of owner/repo"
check "search failure: opens none" lacks calls "issue create"

# --- Report refuses a tag that is not a release ----------------------------
new_case
run report 'v1; rm -rf /' "$root/body.md"
check "bad report tag: fails" status_is 1
check "bad report tag: no gh call" lacks calls "gh"

echo "upstream-watch.test.sh: $passed passed, $failed failed"
[ "$failed" -eq 0 ]
