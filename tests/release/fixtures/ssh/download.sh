#!/bin/sh
set -eu
output=
url=
while [ "$#" -gt 0 ]; do
	case "$1" in
		--output|-O) output=$2; shift 2 ;;
		--proto|--proto-redir|--connect-timeout|--max-time) shift 2 ;;
		--*) shift ;;
		*) url=$1; shift ;;
	esac
done
printf '%s\n' "$url" >> "$TESTD_DOWNLOAD_LOG"
[ "${TESTD_FAIL_DOWNLOAD:-0}" = 0 ] || exit 22
cp "$TESTD_RELEASE_FIXTURE/${url##*/}" "$output"
