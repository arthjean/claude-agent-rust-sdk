#!/usr/bin/env bash
# Tests actions-freshness.sh against a stand-in for `gh`, so every case runs
# offline and gives the same verdict each time. The stand-in answers `gh api`
# from files written by the case, and replaces `gh actions-lock` with the
# lockfile the case says the tool would produce.
#
# Run it with `bash .github/scripts/actions-freshness.test.sh`. It needs `jq`.
set -uo pipefail

script="$(cd "$(dirname "$0")" && pwd)/actions-freshness.sh"
root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT

mkdir "$root/bin"
cat > "$root/bin/gh" << 'EOF'
#!/usr/bin/env bash
set -euo pipefail
echo "$*" >> "$FAKE/calls"
case "$1" in
  api)
    file="$FAKE/api/$(printf '%s' "$2" | sed 's/[^A-Za-z0-9._-]/_/g')"
    filter=.
    shift 2
    while [ $# -gt 0 ]; do
      if [ "$1" = --jq ]; then
        filter="$2"
        shift
      fi
      shift
    done
    if [ ! -f "$file" ]; then
      echo "gh: Not Found (HTTP 404)" >&2
      exit 1
    fi
    jq -r "$filter" "$file" | tr -d '\r'
    ;;
  actions-lock)
    case " $* " in
      *" --verify "*) [ ! -e "$FAKE/verify-fails" ] ;;
      *" --relock "*) cp "$FAKE/after-relock" .github/workflows/actions.lock ;;
      *) cp "$FAKE/after-lock" .github/workflows/actions.lock ;;
    esac
    ;;
esac
EOF
chmod +x "$root/bin/gh"

# --- What a case can set up -------------------------------------------------

# Forty times the same hexadecimal digit: a commit that is easy to read.
sha() { printf '%040d' 0 | tr 0 "$1"; }
A="$(sha a)" B="$(sha b)" C="$(sha c)" D="$(sha d)" E="$(sha e)"
OLD_HEAD="$(sha 1)" NEW_HEAD="$(sha 2)"

days_ago() { date -u -d "$1 days ago" +%Y-%m-%dT%H:%M:%SZ; }

# api PATH JSON: the answer of `gh api PATH`.
api() {
  printf '%s\n' "$2" > "$FAKE/api/$(printf '%s' "$1" | sed 's/[^A-Za-z0-9._-]/_/g')"
}

# workflow LINE...: a workflow with one `uses:` line per argument, in CRLF.
workflow() {
  local line
  {
    printf 'jobs:\r\n  a:\r\n    steps:\r\n'
    for line in "$@"; do printf '      - uses: %s\r\n' "$line"; done
  } > .github/workflows/ci.yml
}

# lockfile FILE "KEY COMMIT"...: a lockfile holding these entries.
lockfile() {
  local file="$1" entry key
  shift
  {
    printf "version: 'v0.0.3'\ndependencies:\n"
    for entry in "$@"; do
      key="${entry%% *}"
      printf "    '%s':\n        ref: '%s'\n        commit: 'sha1-%s'\n        owner_id: 1\n        repo_id: 1\n" \
        "$key" "${key#*@}" "${entry#* }"
    done
  } > "$file"
}
locked() { lockfile .github/workflows/actions.lock "$@"; }

# releases REPO "TAG DAYS_OLD [true]"...: its releases, immutable when marked.
releases() {
  local repo="$1" json='' release tag days immutable
  shift
  for release in "$@"; do
    read -r tag days immutable <<< "$release"
    json+="${json:+,}{\"tag_name\":\"$tag\",\"published_at\":\"$(days_ago "$days")\",\"draft\":false,\"prerelease\":false,\"immutable\":${immutable:-false}}"
  done
  api "repos/$repo/releases?per_page=100" "[$json]"
}

# The commit a tag names, and the commit a bare name (tag or branch) names.
tag_commit() { api "repos/$1/commits/refs/tags/$2" "{\"sha\":\"$3\"}"; }
name_commit() { api "repos/$1/commits/$2" "{\"sha\":\"$3\"}"; }

# The default branch was at OLD_HEAD 20 days ago and moved to NEW_HEAD 2 days
# ago. STATUS is how COMMIT compares with OLD_HEAD: "behind" or "identical"
# when the branch already held it, "ahead" when it landed since.
branch_log() {
  local repo="$1" commit="$2" status="$3"
  api "repos/$repo" '{"default_branch":"main"}'
  api "repos/$repo/activity?ref=refs/heads/main&per_page=100" \
    "[{\"timestamp\":\"$(days_ago 2)\",\"after\":\"$NEW_HEAD\"},{\"timestamp\":\"$(days_ago 20)\",\"after\":\"$OLD_HEAD\"}]"
  api "repos/$repo/compare/$OLD_HEAD...$commit" "{\"status\":\"$status\"}"
}

# --- What a case can check --------------------------------------------------

run() {
  out="$(PATH="$root/bin:$PATH" bash "$script" "$@" 2>&1)"
  status=$?
}
fail() {
  if [ "$failed" -eq 0 ]; then
    # The first failure of a case also shows what the script printed.
    printf '%s\n' "$out" | sed 's/^/    | /'
  fi
  failed=1
  printf '  %s\n' "$1"
}
exits() { [ "$status" -eq "$1" ] || fail "exit $status instead of $1"; }
says() { [[ "$out" == *"$1"* ]] || fail "does not say: $1"; }
never_says() { [[ "$out" != *"$1"* ]] || fail "says: $1"; }
called() { grep -qF -- "$1" "$FAKE/calls" || fail "never ran: gh $1"; }
never_called() { ! grep -qF -- "$1" "$FAKE/calls" || fail "ran: gh $1"; }
snapshot() { cp -a .github/workflows "$sandbox/snapshot"; }
untouched() { diff -r .github/workflows "$sandbox/snapshot" > /dev/null || fail "the workflows or the lockfile changed"; }
workflow_has() { grep -qF -- "$1" .github/workflows/ci.yml || fail "the workflow lacks: $1"; }

# --- Report mode ------------------------------------------------------------

case_up_to_date() {
  workflow 'o/a@v1.0.0' './.github/actions/local'
  locked "o/a@v1.0.0 $A"
  releases o/a 'v1.0.0 40'
  run
  exits 0
  says 'Every action is on its newest release'
}

case_newer_release_past_the_cooldown() {
  workflow 'o/a@v1.0.0'
  locked "o/a@v1.0.0 $A"
  releases o/a 'v1.0.0 60' 'v1.1.0 30' 'v1.2.0 3'
  tag_commit o/a v1.1.0 "$B"
  branch_log o/a "$B" behind
  run
  exits 1
  says 'o/a: v1.0.0 -> v1.1.0'
  never_says 'v1.2.0'
  never_says 'nothing shows'
}

case_release_inside_the_cooldown_is_ignored() {
  workflow 'o/a@v1.0.0'
  locked "o/a@v1.0.0 $A"
  releases o/a 'v1.0.0 60' 'v1.1.0 3'
  run
  exits 0
}

case_old_release_whose_tag_names_a_new_commit() {
  workflow 'o/a@v1.0.0'
  locked "o/a@v1.0.0 $A"
  releases o/a 'v1.0.0 60' 'v1.1.0 30'
  tag_commit o/a v1.1.0 "$C"
  branch_log o/a "$C" ahead
  snapshot
  run --apply
  exits 1
  says "nothing shows that its commit ${C:0:7} is 14 days old"
  never_called 'actions-lock --no-interactive'
  untouched
}

case_age_that_no_branch_log_shows() {
  workflow 'o/a@v1.0.0'
  locked "o/a@v1.0.0 $A"
  releases o/a 'v1.0.0 60' 'v1.1.0 30'
  tag_commit o/a v1.1.0 "$B"
  run
  exits 1
  says 'nothing shows'
}

case_immutable_release_needs_no_branch_log() {
  workflow 'o/a@v1.0.0'
  locked "o/a@v1.0.0 $A"
  releases o/a 'v1.0.0 60' 'v1.1.0 30 true'
  tag_commit o/a v1.1.0 "$B"
  run
  exits 1
  says 'o/a: v1.0.0 -> v1.1.0'
  never_says 'nothing shows'
}

case_branch_named_like_the_tag_is_not_what_gets_checked() {
  workflow 'o/a@v1.0.0'
  locked "o/a@v1.0.0 $A"
  releases o/a 'v1.0.0 60' 'v1.1.0 30'
  name_commit o/a v1.1.0 "$B" # A branch called v1.1.0, on an old commit.
  tag_commit o/a v1.1.0 "$C"  # The tag, on a commit that just landed.
  branch_log o/a "$B" behind
  api "repos/o/a/compare/$OLD_HEAD...$C" '{"status":"ahead"}'
  run
  exits 1
  says "nothing shows that its commit ${C:0:7}"
}

case_failed_lookup() {
  workflow 'o/a@v1.0.0'
  locked "o/a@v1.0.0 $A"
  run
  exits 1
  says 'o/a: could not read its releases'
}

case_action_without_release() {
  workflow 'o/a@v1.0.0'
  locked "o/a@v1.0.0 $A"
  api 'repos/o/a/releases?per_page=100' '[]'
  run
  exits 1
  says 'o/a: publishes no release with a full version'
}

case_reference_it_cannot_read() {
  workflow 'docker://alpine:3.20'
  locked
  run
  exits 1
  says 'uses: docker://alpine:3.20: not a form this script can check'
}

case_movable_tag_that_moved_long_ago() {
  workflow 'o/m@v1'
  locked "o/m@v1 $A"
  name_commit o/m v1 "$B"
  branch_log o/m "$B" identical
  run
  exits 1
  says "o/m@v1: locked at ${A:0:7}, now at ${B:0:7}"
  never_says 'nothing shows'
}

case_movable_tag_that_just_moved_waits() {
  workflow 'o/m@v1'
  locked "o/m@v1 $A"
  name_commit o/m v1 "$B"
  branch_log o/m "$B" ahead
  run
  exits 0
}

case_movable_tag_on_a_commit_outside_the_branch() {
  workflow 'o/m@v1'
  locked "o/m@v1 $A"
  name_commit o/m v1 "$B"
  branch_log o/m "$B" diverged
  run
  exits 1
  says 'nothing shows that this commit is 14 days old'
}

case_movable_tag_missing_from_the_lockfile() {
  workflow 'o/m@v1'
  locked
  run
  exits 1
  says 'o/m@v1: no entry in'
}

# --- Apply mode -------------------------------------------------------------

# One outdated full version, written three ways, and one movable tag that moved.
outdated_repository() {
  workflow '"o/a/sub@v1.0.0"' 'o/a@v1.0.0 # keep' 'o/m@v1'
  locked "o/a@v1.0.0 $A" "o/m@v1 $A"
  releases o/a 'v1.0.0 60' 'v1.1.0 30'
  tag_commit o/a v1.1.0 "$B"
  branch_log o/a "$B" behind
  name_commit o/m v1 "$C"
  branch_log o/m "$C" behind
  lockfile "$FAKE/after-lock" "o/a@v1.1.0 $B" "o/m@v1 $A"
  lockfile "$FAKE/after-relock" "o/a@v1.1.0 $B" "o/m@v1 $C"
  snapshot
}

case_apply_rewrites_and_locks() {
  outdated_repository
  run --apply
  exits 0
  says 'o/a: v1.0.0 -> v1.1.0 (applied)'
  says 'o/m@v1: locked at aaaaaaa, now at ccccccc (relocked)'
  workflow_has '- uses: "o/a/sub@v1.1.0"'
  workflow_has '- uses: o/a@v1.1.0 # keep'
  called 'actions-lock --verify'
  cmp -s .github/workflows/actions.lock "$FAKE/after-relock" || fail 'the lockfile is not the relocked one'
}

case_apply_stops_when_the_lockfile_does_not_verify() {
  outdated_repository
  touch "$FAKE/verify-fails"
  run --apply
  exits 1
  never_called 'api'
  untouched
}

case_apply_refuses_a_commit_it_did_not_check() {
  outdated_repository
  lockfile "$FAKE/after-lock" "o/a@v1.1.0 $D" "o/m@v1 $A"
  lockfile "$FAKE/after-relock" "o/a@v1.1.0 $D" "o/m@v1 $C"
  run --apply
  exits 1
  says "o/a@v1.1.0: locked at ${D:0:7}, but ${B:0:7} is the commit that was checked"
  untouched
}

case_apply_refuses_a_relock_that_moves_another_tag() {
  workflow 'o/m@v1' 'o/n@v2'
  locked "o/m@v1 $A" "o/n@v2 $A"
  name_commit o/m v1 "$C"
  branch_log o/m "$C" behind
  name_commit o/n v2 "$D" # Moved too, but still inside the cooldown.
  branch_log o/n "$D" ahead
  lockfile "$FAKE/after-relock" "o/m@v1 $C" "o/n@v2 $D"
  snapshot
  run --apply
  exits 1
  says "o/n@v2: moved from ${A:0:7} to ${D:0:7} without being checked"
  untouched
}

case_apply_keeps_an_old_commit_brought_in_by_another_action() {
  outdated_repository
  lockfile "$FAKE/after-lock" "o/a@v1.1.0 $B" "o/m@v1 $A" "o/t@v3.0.0 $E"
  lockfile "$FAKE/after-relock" "o/a@v1.1.0 $B" "o/m@v1 $C" "o/t@v3.0.0 $E"
  branch_log o/t "$E" behind
  run --apply
  exits 0
  says "o/t@v3.0.0: new entry at ${E:0:7}"
}

case_apply_refuses_a_new_commit_brought_in_by_another_action() {
  outdated_repository
  lockfile "$FAKE/after-lock" "o/a@v1.1.0 $B" "o/m@v1 $A" "o/t@v3.0.0 $E"
  lockfile "$FAKE/after-relock" "o/a@v1.1.0 $B" "o/m@v1 $C" "o/t@v3.0.0 $E"
  branch_log o/t "$E" ahead
  run --apply
  exits 1
  says "o/t@v3.0.0: new entry at ${E:0:7}"
  untouched
}

# --- Runner -----------------------------------------------------------------

failures=0
count=0
for name in $(declare -F | sed -n 's/^declare -f \(case_.*\)$/\1/p'); do
  count=$((count + 1))
  sandbox="$root/$count"
  export FAKE="$sandbox/fake"
  mkdir -p "$sandbox/repository/.github/workflows" "$FAKE/api"
  : > "$FAKE/calls"
  cd "$sandbox/repository" || exit 1
  failed=0
  out=''
  status=0
  details="$("$name" 2>&1)" || true
  if [ -n "$details" ]; then
    failures=$((failures + 1))
    echo "FAIL ${name#case_}"
    echo "$details"
  else
    echo "ok   ${name#case_}"
  fi
done

echo "$((count - failures)) of $count cases passed."
[ "$failures" -eq 0 ]
