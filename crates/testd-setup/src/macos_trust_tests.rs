use std::process::Command;

use super::*;

#[cfg(target_os = "macos")]
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
fn existing_untrusted_root_reaches_gui_authorization() {
	use std::os::unix::fs::PermissionsExt;

	const FIXTURE: &str = "TESTD_MACOS_TRUST_INSTALL_FIXTURE";
	if let Some(directory) = std::env::var_os(FIXTURE) {
		let pem = std::fs::read_to_string(PathBuf::from(directory).join("root.crt")).unwrap();
		assert_eq!(install(&pem, Elevation::Gui).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
		return;
	}
	let directory = tempfile::tempdir().unwrap();
	std::fs::write(directory.path().join("root.crt"), include_str!("fixtures/isrg-root-x2.pem")).unwrap();
	for (name, script) in [
		(
			"security",
			"#!/bin/sh\ncase \"$1\" in\n\
			find-certificate) /bin/cat \"$TESTD_MACOS_TRUST_INSTALL_FIXTURE/root.crt\" ;;\n\
			verify-cert) exit 1 ;;\n*) exit 2 ;;\nesac\n",
		),
		(
			"osascript",
			"#!/bin/sh\nprintf '%s\\n' \"$*\" > \"$TESTD_MACOS_TRUST_INSTALL_FIXTURE/authorization\"\nexit 17\n",
		),
	] {
		let path = directory.path().join(name);
		std::fs::write(&path, script).unwrap();
		std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
	}
	let output = Command::new(std::env::current_exe().unwrap())
		.args(["--exact", "macos_trust::tests::existing_untrusted_root_reaches_gui_authorization", "--nocapture"])
		.env(FIXTURE, directory.path())
		.env("HOME", directory.path())
		.env("PATH", directory.path())
		.output()
		.unwrap();
	assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
	let authorization = std::fs::read_to_string(directory.path().join("authorization")).unwrap();
	assert!(authorization.contains("with administrator privileges"));
	assert!(authorization.contains("certificate_count=1\\ninstall_trust"));
}

fn run_trust_script(root: &Root, owner: &str, snapshot: &str, action: &str) -> std::process::ExitStatus {
	let directory = root.marker.parent().unwrap();
	let trust = shell_quote(&directory.join("trust").to_string_lossy());
	let log = shell_quote(&directory.join("security.log").to_string_lossy());
	let script = root
		.script(owner, snapshot)
		.replace("directory='/Library/Keychains'", &format!("directory={}", shell_quote(&directory.to_string_lossy())));
	#[cfg(not(target_os = "macos"))]
	let script = script.replace("stat -f '%u:%Lp'", "stat -c '%u:%a'");
	let stubs = format!(
		"keychain_snapshot() {{ printf 'original snapshot'; }}\n\
		trust_digest() {{ cat {trust}; }}\n\
		security() {{\n\
		printf '%s\\n' \"$*\" >> {log}\n\
		case \"$1\" in\n\
		add-trusted-cert)\n\
		[ \"${{install_failure:-0}}\" = 0 ] || return 1\n\
		printf '%s' \"${{trust_after_install:-original trust}}\" > {trust} ;;\n\
		remove-trusted-cert) printf absent > {trust} ;;\n\
		esac\n}}",
	);
	Command::new("sh")
		.args(["-c", &format!("set -eu\n{script}\n{stubs}\ncertificate_count=0\n{action}")])
		.status()
		.unwrap()
}

#[test]
fn repairs_owned_trust_and_refreshes_the_receipt_for_cleanup() {
	use std::os::unix::fs::PermissionsExt;

	let directory = tempfile::tempdir().unwrap();
	let root = root(directory.path().join("root ' receipt"));
	let trust = directory.path().join("trust");
	let run = |action: &str| run_trust_script(&root, "owner-a", "original snapshot", action);
	std::fs::write(&trust, "absent").unwrap();
	assert!(run("install_trust").success());
	for previous_trust in ["original trust", "absent"] {
		std::fs::write(&trust, previous_trust).unwrap();
		let receipt = std::fs::read(&root.marker).unwrap();
		assert!(!run("certificate_count=1\ninstall_failure=1\ninstall_trust").success());
		assert_eq!(std::fs::read(&root.marker).unwrap(), receipt);
		assert_eq!(std::fs::read_to_string(&trust).unwrap(), previous_trust);
		assert!(run("certificate_count=1\ntrust_after_install='restored trust'\ninstall_trust").success());
		assert_eq!(std::fs::read_to_string(&root.marker).unwrap(), "owner-a\nrestored trust\n");
		assert_eq!(std::fs::metadata(&root.marker).unwrap().permissions().mode() & 0o777, 0o600);
	}
	assert!(run("certificate_count=1\nremove_trust").success());
	assert!(!root.marker.exists());
	assert_eq!(std::fs::read_to_string(trust).unwrap(), "absent");
}

#[test]
fn repair_preserves_unowned_changed_or_linked_state() {
	let directory = tempfile::tempdir().unwrap();
	let root = root(directory.path().join("receipt"));
	let trust = directory.path().join("trust");
	let log = directory.path().join("security.log");
	let run = |owner: &str, action: &str| run_trust_script(&root, owner, "original snapshot", action);
	std::fs::write(&trust, "absent").unwrap();
	assert!(run("owner-a", "install_trust").success());
	let receipt = std::fs::read(&root.marker).unwrap();
	let calls = std::fs::read(&log).unwrap();
	for (owner, current_trust) in [("owner-b", "absent"), ("owner-a", "changed trust")] {
		std::fs::write(&trust, current_trust).unwrap();
		assert_eq!(run(owner, "certificate_count=1\ninstall_trust").code(), Some(macos::CONFLICT));
		assert_eq!(std::fs::read(&root.marker).unwrap(), receipt);
		assert_eq!(std::fs::read_to_string(&trust).unwrap(), current_trust);
		assert_eq!(std::fs::read(&log).unwrap(), calls);
	}
	std::fs::write(&trust, "absent").unwrap();
	let linked = directory.path().join("linked receipt");
	std::fs::hard_link(&root.marker, &linked).unwrap();
	assert_eq!(run("owner-a", "certificate_count=1\ninstall_trust").code(), Some(macos::CONFLICT));
	std::fs::remove_file(&root.marker).unwrap();
	std::os::unix::fs::symlink(&linked, &root.marker).unwrap();
	assert_eq!(run("owner-a", "certificate_count=1\ninstall_trust").code(), Some(macos::CONFLICT));
	std::fs::remove_file(&root.marker).unwrap();
	assert_eq!(run("owner-a", "certificate_count=1\ninstall_trust").code(), Some(macos::CONFLICT));
	std::fs::write(&trust, "unowned trust").unwrap();
	assert_eq!(run("owner-a", "install_trust").code(), Some(macos::CONFLICT));
	assert_eq!(std::fs::read(&log).unwrap(), calls);
	assert_eq!(std::fs::read_to_string(trust).unwrap(), "unowned trust");
	assert_eq!(std::fs::read(linked).unwrap(), receipt);
}

#[test]
fn trust_cleanup_preserves_unowned_or_changed_state() {
	let directory = tempfile::tempdir().unwrap();
	let marker = directory.path().join("root ' receipt");
	let trust = directory.path().join("trust");
	let log = directory.path().join("security.log");
	std::fs::write(&trust, "absent").unwrap();
	let root = root(marker.clone());
	let run = |owner: &str, snapshot: &str, action: &str| run_trust_script(&root, owner, snapshot, action);
	let remove = "certificate_count=1\nremove_trust";
	assert_eq!(run("owner-a", "changed snapshot", "install_trust").code(), Some(macos::CONFLICT));
	assert!(!log.exists());
	assert!(run("owner-a", "original snapshot", "install_trust").success());
	assert_eq!(run("owner-b", "original snapshot", remove).code(), Some(macos::CONFLICT));
	std::fs::write(&trust, "replacement trust").unwrap();
	assert_eq!(run("owner-a", "original snapshot", remove).code(), Some(macos::CONFLICT));
	assert!(!std::fs::read_to_string(&log).unwrap().contains("delete-certificate"));
	std::fs::write(&trust, "original trust").unwrap();
	assert!(run("owner-a", "original snapshot", remove).success());
	assert!(!marker.exists());
	let calls = std::fs::read_to_string(&log).unwrap();
	assert!(calls.contains("remove-trusted-cert -d "));
	assert!(calls.contains("delete-certificate -Z "));
	assert!(!calls.contains("delete-certificate -t"));
	for certificate_count in [1, 0] {
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
