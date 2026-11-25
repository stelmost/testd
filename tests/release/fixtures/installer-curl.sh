#!/bin/sh
set -eu

printf '%s\n' "$*" >> "$FIXTURE_REQUESTS"

if [ "$#" = 6 ] && [ "$1" = -fsSL ] && [ "$2" = -o ] && [ "$3" = /dev/null ] &&
	[ "$4" = -w ] && [ "$5" = '%{url_effective}' ] &&
	[ "$6" = https://github.com/fixture/testd/releases/latest ]; then
	[ "$FIXTURE_DOWNLOAD" != missing-release ] || exit 22
	printf '%s' "${FIXTURE_RELEASE_URL:-https://github.com/fixture/testd/releases/tag/v$FIXTURE_VERSION}"
	exit 0
fi

# Reject credentials and unexpected requests; downloads must use exact-version assets.
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
