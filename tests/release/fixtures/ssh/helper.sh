#!/bin/sh
case "$1" in
	--version) printf 'testd %s\n' '@VERSION@' ;;
	ssh-session)
		umask > "$TESTD_UMASK_LOG"
		printf '{"type":"Hello","version":1}\n'
		IFS= read -r request || exit 0
		printf '%s\n' "$request" > "$TESTD_SESSION_LOG"
		;;
	*) exit 2 ;;
esac
