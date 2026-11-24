#!/bin/sh
# Installs the current testd release. Reads no configuration and needs no account.
# Env: TESTD_VERSION, TESTD_INSTALL_DIR, TESTD_REPO, TESTD_LIBC, GITHUB_TOKEN.
set -eu

repo=${TESTD_REPO:-stelmost/testd}
install_dir=${TESTD_INSTALL_DIR:-"$HOME/.local/bin"}
version=${TESTD_VERSION:-}

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

if [ -z "$version" ]; then
	lookup_error="release lookup failed for $repo; check GitHub access or set TESTD_VERSION from the release notes"
	if [ -n "${GITHUB_TOKEN:-}" ]; then
		api="https://api.github.com/repos/$repo/releases/latest"
		release=$(curl -fsSL -H "Authorization: Bearer $GITHUB_TOKEN" "$api") || fail "$lookup_error"
		version=$(printf '%s' "$release" | sed -n 's/.*"tag_name": *"v\{0,1\}\([^"]*\)".*/\1/p')
	else
		# The public redirect avoids GitHub's unauthenticated API rate limit.
		release_url=$(curl -fsSL -o /dev/null -w '%{url_effective}' \
			"https://github.com/$repo/releases/latest") || fail "$lookup_error"
		tag_url="https://github.com/$repo/releases/tag/v"
		case "$release_url" in
		"$tag_url"*) version=${release_url#"$tag_url"} ;;
		*) fail "could not read the release tag for $repo; set TESTD_VERSION from the release notes" ;;
		esac
	fi
	[ -n "$version" ] || fail "could not read the release tag for $repo"
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
