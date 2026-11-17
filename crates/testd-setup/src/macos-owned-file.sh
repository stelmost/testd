# Included by macos.rs; path, marker, and the installation fingerprint are quoted by Rust.
entry_exists() {
	[ -e "$1" ] || [ -L "$1" ]
}

file_hash() {
	value=$(openssl dgst -sha256 -r "$1") || return 1
	printf '%s\n' "${value%% *}"
}

file_identity() {
	stat -f '%d:%i:%u:%g:%Lp' "$1"
}

verify_marker() {
	[ -f "$marker" ] && [ ! -L "$marker" ] || exit 17
	[ "$(stat -f '%u:%Lp:%l' "$marker")" = "$(id -u):600:1" ] || exit 17
	[ "$(sed -n '1p' "$marker")" = "$owner" ] || exit 17
}

install_file() {
	if entry_exists "$path" || entry_exists "$marker"; then exit 17; fi
	# Noclobber refuses a file, directory, or symlink created during authorization.
	(set -C; umask 022; printf '%s' "$content" > "$path") || exit 17
	identity=$(file_identity "$path")
	digest=$(file_hash "$path")
	(set -C; umask 077; printf '%s\n%s\n%s\n' "$owner" "$identity" "$digest" > "$marker") || exit 17
}

remove_file() {
	if ! entry_exists "$path" && ! entry_exists "$marker"; then return; fi
	verify_marker
	if entry_exists "$path"; then
		[ -f "$path" ] && [ ! -L "$path" ] || exit 17
		identity=$(file_identity "$path")
		digest=$(file_hash "$path")
		expected=$(printf '%s\n%s\n%s\n' "$owner" "$identity" "$digest")
		[ "$(cat "$marker")" = "$expected" ] || exit 17
		rm "$path"
	fi
	# Retain the receipt if the consumer reload fails so cleanup can be retried.
	eval "$reload"
	rm "$marker"
}
