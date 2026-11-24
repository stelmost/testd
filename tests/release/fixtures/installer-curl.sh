#!/bin/sh
set -eu

printf '%s\n' "$*" >> "$FIXTURE_REQUESTS"

# Discovery requests and credentials are rejected; only exact-version assets work.
[ "$#" = 4 ] && [ "$1" = -fsSL ] && [ "$2" = -o ] || {
	printf 'curl: (56) The requested URL returned error: 403\n' >&2
	exit 56
}
url=$4
file=${url##*/}
[ "$url" = "https://github.com/fixture/testd/releases/download/v$FIXTURE_VERSION/$file" ] || exit 22

if [ "$FIXTURE_DOWNLOAD" = forbidden ]; then
	printf 'curl: (56) The requested URL returned error: 403\n' >&2
	exit 56
fi
if [ "$FIXTURE_DOWNLOAD" = missing-checksums ] && [ "$file" = SHA256SUMS ]; then
	exit 22
fi
cp "$FIXTURE_ASSETS/$file" "$3"
