#!/bin/sh
set -eu

printf '%s\n' "$*" >> "$FIXTURE_REQUESTS"
for url do :; done

case "$url" in
https://api.github.com/repos/fixture/testd/releases/latest)
	if [ "$FIXTURE_LOOKUP" = api-success ]; then
		[ "$#" = 4 ] && [ "$2" = -H ] && [ "$3" = 'Authorization: Bearer fixture-token' ] || exit 22
		printf '{"tag_name":"v%s"}\n' "$FIXTURE_VERSION"
		exit 0
	fi
	printf 'curl: (56) The requested URL returned error: 403\n' >&2
	exit 56
	;;
https://github.com/fixture/testd/releases/latest)
	[ "$#" = 6 ] && [ "$1" = -fsSL ] && [ "$2" = -o ] && [ "$3" = /dev/null ] \
		&& [ "$4" = -w ] && [ "$5" = '%{url_effective}' ] || exit 22
	case "$FIXTURE_LOOKUP" in
	forbidden)
		printf 'curl: (56) The requested URL returned error: 403\n' >&2
		exit 56
		;;
	missing)
		printf 'curl: (22) The requested URL returned error: 404\n' >&2
		exit 22
		;;
	network)
		printf 'curl: (6) Could not resolve host: github.com\n' >&2
		exit 6
		;;
	esac
	printf '%s' "$FIXTURE_REDIRECT"
	;;
*)
	[ "$#" = 4 ] && [ "$1" = -fsSL ] && [ "$2" = -o ] || exit 22
	file=${url##*/}
	[ "$url" = "https://github.com/fixture/testd/releases/download/v$FIXTURE_VERSION/$file" ] || exit 22
	cp "$FIXTURE_ASSETS/$file" "$3"
	;;
esac
