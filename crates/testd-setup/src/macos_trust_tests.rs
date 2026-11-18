use std::process::Command;

use super::*;

#[path = "macos_trust_digest_tests.rs"]
mod digest;

const PEM: &str = "-----BEGIN CERTIFICATE-----\nAAEC\n-----END CERTIFICATE-----\n";

fn root(marker: PathBuf) -> Root {
	Root { pem: PEM.into(), sha1: sha1_hex(&[0, 1, 2]).to_uppercase(), marker }
}

#[test]
fn matches_exact_bytes_and_refuses_duplicate_identities() {
	let root = root(PathBuf::new());
	let unrelated = "-----BEGIN CERTIFICATE-----\nBwgJ\n-----END CERTIFICATE-----\n";
	assert_eq!(matching_count(&root, unrelated).unwrap(), 0);
	assert_eq!(matching_count(&root, &format!("{unrelated}{PEM}")).unwrap(), 1);
	assert!(matching_count(&root, &PEM.repeat(2)).is_err());
	assert!(Root::new(PEM).is_err());
}

#[test]
fn trust_cleanup_preserves_unowned_or_changed_state() {
	let directory = tempfile::tempdir().unwrap();
	let marker = directory.path().join("root ' receipt");
	let trust = directory.path().join("trust");
	let log = directory.path().join("security.log");
	std::fs::write(&trust, "original trust").unwrap();
	let root = root(marker.clone());
	let run = |owner: &str, snapshot: &str, action: &str| {
		let script = root.script(owner, snapshot).replace(
			"directory='/Library/Keychains'",
			&format!("directory={}", shell_quote(&directory.path().to_string_lossy())),
		);
		let stubs = format!(
			"keychain_snapshot() {{ printf 'original snapshot'; }}\n\
			trust_digest() {{ cat {}; }}\nsecurity() {{ printf '%s\\n' \"$*\" >> {}; }}",
			shell_quote(&trust.to_string_lossy()),
			shell_quote(&log.to_string_lossy()),
		);
		Command::new("sh")
			.args(["-c", &format!("set -eu\n{script}\n{stubs}\ncertificate_count=1\n{action}")])
			.status()
			.unwrap()
	};
	assert_eq!(run("owner-a", "changed snapshot", "install_trust").code(), Some(macos::CONFLICT));
	assert!(!log.exists());
	assert!(run("owner-a", "original snapshot", "install_trust").success());
	assert_eq!(run("owner-b", "original snapshot", "remove_trust").code(), Some(macos::CONFLICT));
	std::fs::write(&trust, "replacement trust").unwrap();
	assert_eq!(run("owner-a", "original snapshot", "remove_trust").code(), Some(macos::CONFLICT));
	assert!(!std::fs::read_to_string(&log).unwrap().contains("delete-certificate"));
	std::fs::write(&trust, "original trust").unwrap();
	assert!(run("owner-a", "original snapshot", "remove_trust").success());
	assert!(!marker.exists());
	let calls = std::fs::read_to_string(&log).unwrap();
	assert!(calls.contains("remove-trusted-cert -d "));
	assert!(calls.contains("delete-certificate -Z "));
	assert!(!calls.contains("delete-certificate -t"));
	for certificate_count in [1, 0] {
		std::fs::write(&trust, "original trust").unwrap();
		assert!(run("owner-a", "original snapshot", "install_trust").success());
		let before = std::fs::read_to_string(&log).unwrap();
		std::fs::write(&trust, "absent").unwrap();
		let action = format!("certificate_count={certificate_count}\nremove_trust");
		assert!(run("owner-a", "original snapshot", &action).success());
		let after = std::fs::read_to_string(&log).unwrap();
		let cleanup = after.strip_prefix(&before).unwrap();
		assert!(!cleanup.contains("remove-trusted-cert"));
		assert_eq!(cleanup.contains("delete-certificate"), certificate_count == 1);
		assert!(!marker.exists());
	}
}
