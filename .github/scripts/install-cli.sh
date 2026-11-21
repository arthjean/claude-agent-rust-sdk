#!/usr/bin/env bash
# Installs one exact version of the Claude Code CLI without running a
# downloaded script: it fetches the manifest of that version, downloads the
# binary of this platform, checks it against the SHA-256 the manifest gives,
# and only then puts it on disk as `claude`.
#
# `curl https://claude.ai/install.sh | bash` would run whatever script the
# server sends, in a job that later holds an API key; this script is
# reviewed in the repository and runs nothing it downloads before the
# checksum matches.
#
# Usage: bash .github/scripts/install-cli.sh <version>
#
# CLAUDE_CLI_DOWNLOAD_BASE overrides the release host (the test points it at
# a local directory), and CLAUDE_CLI_INSTALL_DIR the target directory
# (default: ~/.local/bin, which the workflows add to the PATH).
set -euo pipefail

version="${1:-}"
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "install-cli: expected a version such as 2.1.329, got '$version'" >&2
  exit 2
fi
base="${CLAUDE_CLI_DOWNLOAD_BASE:-https://downloads.claude.ai/claude-code-releases}"
install_dir="${CLAUDE_CLI_INSTALL_DIR:-$HOME/.local/bin}"

case "$(uname -s)" in
  Linux) os=linux ;;
  Darwin) os=darwin ;;
  *) echo "install-cli: unsupported system $(uname -s)" >&2; exit 2 ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch=x64 ;;
  arm64 | aarch64) arch=arm64 ;;
  *) echo "install-cli: unsupported architecture $(uname -m)" >&2; exit 2 ;;
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

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

curl -fsSL "$base/$version/manifest.json" -o "$work/manifest.json"
expected="$(jq -r --arg platform "$platform" '.platforms[$platform].checksum // empty' "$work/manifest.json")"
if [[ ! "$expected" =~ ^[a-f0-9]{64}$ ]]; then
  echo "install-cli: the manifest of $version has no checksum for $platform" >&2
  exit 1
fi

curl -fsSL "$base/$version/$platform/claude" -o "$work/claude"
actual="$(sha256 "$work/claude")"
if [ "$actual" != "$expected" ]; then
  echo "install-cli: checksum mismatch for $version $platform: expected $expected, got $actual" >&2
  exit 1
fi

mkdir -p "$install_dir"
chmod +x "$work/claude"
mv "$work/claude" "$install_dir/claude"
echo "installed Claude Code $version ($platform, sha256 $actual) at $install_dir/claude"
