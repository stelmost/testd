# Included by macos_trust.rs. Public certificate, fingerprints, paths and owner are quoted by Rust.
keychain_snapshot() {
	security find-certificate -a -p "$keychain" > "$work/keychain.pem"
	file_hash "$work/keychain.pem"
}

trust_digest() {
	security trust-settings-export -d "$work/trust.plist" >/dev/null
	/usr/libexec/PlistBuddy -c "Print :trustList:$sha1" "$work/trust.plist" > "$work/trust-entry"
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

install_trust() {
	prepare_trust
	if entry_exists "$marker"; then exit 17; fi
	security add-trusted-cert -d -r trustRoot -k "$keychain" "$work/root.crt"
	trust=$(trust_digest)
	(set -C; umask 077; printf '%s\n%s\n' "$owner" "$trust" > "$marker") || exit 17
}

remove_trust() {
	prepare_trust
	verify_marker
	trust=$(trust_digest)
	expected=$(printf '%s\n%s\n' "$owner" "$trust")
	[ "$(cat "$marker")" = "$expected" ] || exit 17
	security delete-certificate -t -Z "$sha1" "$keychain"
	rm "$marker"
}
