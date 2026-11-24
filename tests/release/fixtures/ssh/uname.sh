#!/bin/sh
case "$1" in
	-s) printf '%s\n' "$TESTD_FIXTURE_OS" ;;
	-m) printf '%s\n' "$TESTD_FIXTURE_ARCH" ;;
	*) exit 2 ;;
esac
