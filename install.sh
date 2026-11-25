#!/bin/sh
# Installs the latest testd release unless pinned. Reads no configuration and needs no account.
# Env: TESTD_VERSION, TESTD_INSTALL_DIR, TESTD_REPO, TESTD_LIBC.
set -eu

repo=${TESTD_REPO:-stelmost/testd}
install_dir=${TESTD_INSTALL_DIR:-"$HOME/.local/bin"}
# Site and release builds replace this default with their version.
version=${TESTD_VERSION:-latest}

fail() {
	printf 'install.sh: %s\n' "$*" >&2
	exit 1
}

os=$(uname -s)
arch=$(uname -m)

case "$os:$arch" in
Darwin:arm64) target=aarch64-apple-darwin ;;
Darwin:x86_64) target=x86_64-apple-darwin ;;
Linux:x86_64)
	libc=${TESTD_LIBC:-}
	if [ -z "$libc" ]; then
		if getconf GNU_LIBC_VERSION >/dev/null 2>&1; then libc=gnu; else libc=musl; fi
	fi
	case "$libc" in
	gnu | musl) target=x86_64-unknown-linux-$libc ;;
	*) fail 'TESTD_LIBC must be gnu or musl' ;;
	esac
	;;
Linux:aarch64 | Linux:arm64)
	fail 'no Linux aarch64 release; build from source with: cargo install --path .'
	;;
*)
	fail "unsupported platform $os/$arch; see https://testd.dev/#install"
	;;
esac

if [ "$version" = latest ]; then
	release_url=$(curl -fsSL -o /dev/null -w '%{url_effective}' \
		"https://github.com/$repo/releases/latest") || fail 'could not resolve latest release'
	case "$release_url" in
	"https://github.com/$repo/releases/tag/v"[0-9]*) version=${release_url##*/v} ;;
	*) fail 'could not resolve latest release version' ;;
	esac
fi

archive="testd-$version-$target.tar.gz"
base="https://github.com/$repo/releases/download/v$version"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

printf 'testd %s (%s)\n' "$version" "$target"
curl -fsSL -o "$tmp/$archive" "$base/$archive" || fail "download failed: $base/$archive"
curl -fsSL -o "$tmp/SHA256SUMS" "$base/SHA256SUMS" || fail 'checksum file unavailable; refusing to install'

(
	cd "$tmp"
	grep " \*\?$archive\$" SHA256SUMS > expected
	[ -s expected ] || fail "$archive is not listed in SHA256SUMS"
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum -c expected >/dev/null || fail 'checksum mismatch'
	else
		shasum -a 256 -c expected >/dev/null || fail 'checksum mismatch'
	fi
)

tar -xzf "$tmp/$archive" -C "$tmp"
mkdir -p "$install_dir"
install -m 755 "$tmp/testd-$version-$target/testd" "$install_dir/testd"

printf 'installed %s\n' "$install_dir/testd"
case ":$PATH:" in
*":$install_dir:"*) ;;
*) printf 'note: add %s to PATH\n' "$install_dir" ;;
esac
printf 'next: testd init, then testd host\n'
