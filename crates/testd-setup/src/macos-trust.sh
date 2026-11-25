# Included by macos_trust.rs. Public certificate, fingerprints, paths and owner are quoted by Rust.
keychain_snapshot() {
	security find-certificate -a -p "$keychain" > "$work/keychain.pem"
	file_hash "$work/keychain.pem"
}

trust_digest() {
	if ! security trust-settings-export -d "$work/trust.plist" > "$work/export.out" 2> "$work/export.err"; then
		if grep -Fq 'No Trust Settings were found' "$work/export.err"; then
			printf 'absent\n'
			return
		fi
		cat "$work/export.err" >&2
		return 1
	fi
	if ! /usr/libexec/PlistBuddy -c "Print :trustList:$sha1" "$work/trust.plist" \
		> "$work/trust-entry" 2> "$work/entry.err"; then
		if grep -Fq 'Does Not Exist' "$work/entry.err"; then
			printf 'absent\n'
			return
		fi
		cat "$work/entry.err" >&2
		return 1
	fi
	file_hash "$work/trust-entry"
}

prepare_trust() {
	[ -d "$directory" ] && [ ! -L "$directory" ] || exit 17
	access=$(stat -f '%u:%Lp' "$directory")
	case "$access" in "$(id -u):755"|"$(id -u):700") ;; *) exit 17 ;; esac
	work=$(mktemp -d "$directory/.work.XXXXXX")
	trap 'rm -rf "$work"' EXIT
	printf '%s' "$certificate" > "$work/root.crt"
	[ "$(keychain_snapshot)" = "$snapshot" ] || exit 17
}

verify_trust_marker() {
	verify_marker
	recorded_trust=$(sed -n '2p' "$marker")
	expected=$(printf '%s\n%s\n' "$owner" "$recorded_trust")
	[ "$(cat "$marker")" = "$expected" ] || exit 17
}

install_trust() {
	prepare_trust
	trust=$(trust_digest)
	if [ "$certificate_count" = 1 ]; then
		verify_trust_marker
		[ "$trust" = 'absent' ] || [ "$trust" = "$recorded_trust" ] || exit 17
	else
		if entry_exists "$marker"; then exit 17; fi
		[ "$trust" = 'absent' ] || exit 17
	fi
	security add-trusted-cert -d -r trustRoot -k "$keychain" "$work/root.crt"
	trust=$(trust_digest)
	[ "$trust" != 'absent' ] || exit 1
	# Keep the previous receipt until the new trust entry has been recorded in full.
	(umask 077; printf '%s\n%s\n' "$owner" "$trust" > "$work/receipt")
	if [ "$certificate_count" = 1 ]; then
		mv "$work/receipt" "$marker"
	else
		ln "$work/receipt" "$marker" || exit 17
	fi
}

remove_trust() {
	prepare_trust
	verify_trust_marker
	trust=$(trust_digest)
	if [ "$trust" != 'absent' ]; then
		[ "$trust" = "$recorded_trust" ] || exit 17
		# Remove only admin trust; delete-certificate -t also changes user trust.
		security remove-trusted-cert -d "$work/root.crt"
	fi
	if [ "$certificate_count" = 1 ]; then
		security delete-certificate -Z "$sha1" "$keychain"
	fi
	rm "$marker"
}
