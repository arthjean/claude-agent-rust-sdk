#!/usr/bin/env bash
# Tests install-cli.sh against a release host built in a temporary
# directory (curl reads it through file://), so every case runs offline.
#
# Run it with `bash .github/scripts/install-cli.test.sh` (needs `jq`).
set -uo pipefail

script="$(cd "$(dirname "$0")" && pwd)/install-cli.sh"
root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT

case "$(uname -s)" in
  Linux) os=linux ;;
  Darwin) os=darwin ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch=x64 ;;
  arm64 | aarch64) arch=arm64 ;;
esac
platform="$os-$arch"
if [ "$os" = linux ] && ldd /bin/ls 2>&1 | grep -q musl; then
  platform="$platform-musl"
fi

sha256() {
  if command -v sha256sum > /dev/null; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

# A release whose manifest gives `$2` as the checksum of this platform.
release() {
  local version="$1" checksum="$2"
  mkdir -p "$root/host/$version/$platform"
  printf '#!/bin/sh\necho "%s (Claude Code)"\n' "$version" > "$root/host/$version/$platform/claude"
  if [ "$checksum" = right ]; then
    checksum="$(sha256 "$root/host/$version/$platform/claude")"
  fi
  printf '{"version": "%s", "platforms": {"%s": {"checksum": "%s", "size": 1}}}\n' \
    "$version" "$platform" "$checksum" > "$root/host/$version/manifest.json"
}

failures=0
check() {
  local name="$1"
  shift
  if "$@"; then
    echo "ok: $name"
  else
    echo "FAIL: $name" >&2
    failures=$((failures + 1))
  fi
}

run() {
  CLAUDE_CLI_DOWNLOAD_BASE="file://$root/host" CLAUDE_CLI_INSTALL_DIR="$root/bin" \
    bash "$script" "$@" > "$root/out" 2>&1
}

release 2.1.329 right
check "a release whose checksum matches is installed" run 2.1.329
check "the installed file runs as the CLI" \
  bash -c "[ \"\$('$root/bin/claude')\" = '2.1.329 (Claude Code)' ]"
check "the log names the checksum" grep -q "sha256 $(sha256 "$root/bin/claude")" "$root/out"

rm -rf "$root/bin"
release 2.1.330 "$(printf '0%.0s' {1..64})"
check "a checksum mismatch fails" bash -c "! CLAUDE_CLI_DOWNLOAD_BASE='file://$root/host' CLAUDE_CLI_INSTALL_DIR='$root/bin' bash '$script' 2.1.330 > '$root/out' 2>&1"
check "a checksum mismatch installs nothing" test ! -e "$root/bin/claude"
check "a checksum mismatch says so" grep -q "checksum mismatch" "$root/out"

mkdir -p "$root/host/2.1.331"
echo '{"platforms": {}}' > "$root/host/2.1.331/manifest.json"
check "a manifest without this platform fails" bash -c "! CLAUDE_CLI_DOWNLOAD_BASE='file://$root/host' CLAUDE_CLI_INSTALL_DIR='$root/bin' bash '$script' 2.1.331 > '$root/out' 2>&1"
check "a manifest without this platform says so" grep -q "no checksum for $platform" "$root/out"

for version in latest stable "2.1" "2.1.329; rm -rf /" ""; do
  check "the version '$version' is refused" bash -c "! CLAUDE_CLI_DOWNLOAD_BASE='file://$root/host' CLAUDE_CLI_INSTALL_DIR='$root/bin' bash '$script' '$version' > /dev/null 2>&1"
done
check "a refused version installs nothing" test ! -e "$root/bin/claude"

if [ "$failures" -ne 0 ]; then
  echo "$failures check(s) failed" >&2
  exit 1
fi
echo "all checks passed"
