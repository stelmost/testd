set -eu

fail() {
	printf '%s\n' "$1" >&2
	printf '{"type":"Failed","message":"%s"}\n' "$1"
	exit 1
}

installed=null
check_helper() {
	[ -x "$1" ] || return 0
	actual=$("$1" --version 2>/dev/null) || actual=unknown
	if [ "$actual" = "testd $version" ]; then
		exec "$1" ssh-session
	fi
	actual=${actual#testd }
	case "$actual" in
		''|*[!0-9A-Za-z.+-]*) actual=unknown ;;
	esac
	[ "${#actual}" -le 80 ] || actual=unknown
	installed="\"$actual\""
}

destination=$HOME/.local/bin/testd
check_helper "$destination"
on_path=$(command -v testd || true)
if [ -n "$on_path" ] && [ "$on_path" != "$destination" ]; then
	check_helper "$on_path"
fi

case "$(uname -s)/$(uname -m)" in
	Linux/x86_64) target=x86_64-unknown-linux-musl ;;
	Darwin/x86_64) target=x86_64-apple-darwin ;;
	Darwin/arm64|Darwin/aarch64) target=aarch64-apple-darwin ;;
	*) fail 'No automatic testd install for this host; install the matching version manually' ;;
esac

printf '{"type":"Install","version":%s}\n' "$installed"
IFS= read -r answer || exit 1
[ "$answer" = '{"type":"Install"}' ] || exit 1

download() {
	if command -v curl >/dev/null 2>&1; then
		curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
			--connect-timeout 15 --max-time 90 --output "$2" "$1"
	elif command -v wget >/dev/null 2>&1; then
		wget --quiet --https-only --timeout=30 --tries=1 -O "$2" "$1"
	else
		fail 'Install curl or wget on the SSH host to download testd'
	fi
}

verify() {
	digest=$(awk -v name="$archive" '$2 == name { print $1 }' "$staging/SHA256SUMS")
	case "$digest" in
		''|*[!0-9a-fA-F]*) fail 'The testd release has no valid checksum for this host' ;;
	esac
	[ "${#digest}" -eq 64 ] || fail 'The testd release checksum is invalid'
	if command -v sha256sum >/dev/null 2>&1; then
		printf '%s  %s\n' "$digest" "$staging/$archive" | sha256sum -c - >&2
	elif command -v shasum >/dev/null 2>&1; then
		printf '%s  %s\n' "$digest" "$staging/$archive" | shasum -a 256 -c - >&2
	else
		fail 'Install sha256sum or shasum on the SSH host to verify testd'
	fi
}

install_helper() {
	base=testd-$version-$target
	archive=$base.tar.gz
	release=https://github.com/stelmost/testd/releases/download/v$version
	download "$release/$archive" "$staging/$archive" || fail 'Could not download the matching testd release'
	download "$release/SHA256SUMS" "$staging/SHA256SUMS" || fail 'Could not download testd release checksums'
	verify || fail 'The testd download failed checksum verification'
	tar -xOzf "$staging/$archive" "$base/testd" > "$staging/testd" || fail 'Could not extract testd'
	chmod 755 "$staging/testd"
	actual=$("$staging/testd" --version) || fail 'The downloaded testd cannot run on this host'
	[ "$actual" = "testd $version" ] || fail 'The downloaded testd version does not match this installation'
	[ ! -d "$destination" ] || fail 'The remote testd installation path is a directory'
	if [ -e "$destination" ] || [ -L "$destination" ]; then
		backups=$HOME/.local/share/testd/backups
		mkdir -p "$backups"
		backup=$(mktemp -d "$backups/ssh-install.XXXXXXXX")
		cp -pP "$destination" "$backup/testd" || fail 'Could not back up the existing remote testd'
		printf 'Previous testd saved to %s/testd\n' "$backup" >&2
	fi
	mv -f "$staging/testd" "$destination" || fail 'Could not install testd into ~/.local/bin'
}

previous_umask=$(umask)
umask 077
mkdir -p "$HOME/.local/bin"
staging=$(mktemp -d "$HOME/.local/bin/.testd-install.XXXXXXXX")
trap 'rm -rf "$staging"' EXIT
trap 'exit 1' HUP INT TERM
install_helper
rm -rf "$staging"
trap - EXIT HUP INT TERM
umask "$previous_umask"
exec "$destination" ssh-session
