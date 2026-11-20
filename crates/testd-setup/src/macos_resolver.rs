//! Create a macOS resolver route without replacing existing system settings.

use std::io;
use std::net::SocketAddr;
use std::path::Path;

#[cfg(target_os = "macos")]
use super::{Elevation, ownership};

use super::macos;

const CONFLICT: i32 = macos::CONFLICT;

pub(super) fn matches_route(content: &str, addr: SocketAddr) -> bool {
	let mut servers = Vec::new();
	let mut ports = Vec::new();
	for line in content.lines() {
		let mut words = line.split('#').next().unwrap_or("").split_whitespace();
		match words.next() {
			Some("nameserver") => servers.push(words.next().and_then(|value| value.parse().ok())),
			Some("port") => ports.push(words.next().and_then(|value| value.parse::<u16>().ok())),
			_ => {},
		}
	}
	servers == [Some(addr.ip())] && (ports == [Some(addr.port())] || ports.is_empty() && addr.port() == 53)
}

fn conflict(path: &Path) -> io::Error {
	io::Error::new(
		io::ErrorKind::AlreadyExists,
		format!("{} conflicts or has changed; testd left it untouched", path.display()),
	)
}

fn already_configured(path: &Path, addr: SocketAddr) -> io::Result<bool> {
	let metadata = match std::fs::symlink_metadata(path) {
		Ok(metadata) => metadata,
		Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
		Err(error) => return Err(error),
	};
	if !metadata.is_file() || !matches_route(&std::fs::read_to_string(path)?, addr) {
		return Err(conflict(path));
	}
	Ok(true)
}

fn install_script(path: &Path, addr: SocketAddr, owner: &str, reload: &str) -> String {
	let directory = super::shell_quote(&path.parent().unwrap().to_string_lossy());
	let content = super::shell_quote(&format!("nameserver {}\nport {}\n", addr.ip(), addr.port()));
	let helpers = macos::file_script(path, owner);
	format!(
		"set -eu\n{helpers}\n\
		[ ! -L {directory} ] || exit {CONFLICT}\n\
		mkdir -p {directory}\n\
		content={content}\ninstall_file\n{reload}"
	)
}

fn remove_script(path: &Path, owner: &str, reload: &str) -> String {
	let helpers = macos::file_script(path, owner);
	format!("set -eu\n{helpers}\nreload={}\nremove_file", super::shell_quote(reload))
}

#[cfg(target_os = "macos")]
pub(super) fn setup(tld: &str, addr: SocketAddr, elevation: Elevation) -> io::Result<()> {
	testd_core::validate::validate_domain_labels(tld)
		.map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
	let path = std::path::PathBuf::from(format!("/etc/resolver/{tld}"));
	if already_configured(&path, addr)? {
		return Ok(());
	}
	let owner = ownership::InstallationId::load_or_create()?;
	let script = install_script(&path, addr, &owner.fingerprint(), RELOAD);
	macos::run(&script, elevation)?;
	if !already_configured(&path, addr)? {
		return Err(io::Error::other("resolver setup could not be verified"));
	}
	Ok(())
}

#[cfg(target_os = "macos")]
const RELOAD: &str = "dscacheutil -flushcache && killall -HUP mDNSResponder";

#[cfg(target_os = "macos")]
pub(super) fn teardown(tld: &str) -> io::Result<()> {
	testd_core::validate::validate_domain_labels(tld)
		.map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
	let path = std::path::PathBuf::from(format!("/etc/resolver/{tld}"));
	let marker = std::path::PathBuf::from(format!("/etc/resolver/{tld}.owner"));
	if !macos::present(&path)? && !macos::present(&marker)? {
		return Ok(());
	}
	let owner = ownership::InstallationId::load()?.ok_or_else(|| conflict(&path))?;
	macos::run(&remove_script(&path, &owner.fingerprint(), RELOAD), Elevation::Sudo)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn address() -> SocketAddr {
		"127.0.0.1:7753".parse().unwrap()
	}

	#[test]
	fn only_the_exact_dns_route_is_compatible() {
		assert!(matches_route("nameserver 127.0.0.1\nport 7753\n", address()));
		for content in [
			"nameserver 192.0.2.1\nport 7753\n",
			"nameserver 127.0.0.1\n",
			"nameserver 127.0.0.1\nnameserver 192.0.2.1\nport 7753\n",
			"nameserver bad\nport 7753\n",
		] {
			assert!(!matches_route(content, address()), "{content}");
		}
	}

	#[test]
	fn creates_a_route_but_never_replaces_a_preexisting_or_racing_entry() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("test ' route");
		assert!(!already_configured(&path, address()).unwrap());
		let script = install_script(&path, address(), "installation-a", "true");
		let run = || std::process::Command::new("sh").args(["-c", &script]).status().unwrap();
		assert!(run().success());
		assert!(already_configured(&path, address()).unwrap());
		assert_eq!(run().code(), Some(CONFLICT));
		assert_eq!(std::fs::read_to_string(&path).unwrap(), "nameserver 127.0.0.1\nport 7753\n");

		let raced = dir.path().join("raced");
		assert!(!already_configured(&raced, address()).unwrap());
		std::fs::write(&raced, "other tool's settings").unwrap();
		assert!(already_configured(&raced, address()).is_err());
		let status = std::process::Command::new("sh")
			.args(["-c", &install_script(&raced, address(), "installation-a", "true")])
			.status()
			.unwrap();
		assert_eq!(status.code(), Some(CONFLICT));
		assert_eq!(std::fs::read_to_string(raced).unwrap(), "other tool's settings");
	}

	#[test]
	fn cleanup_requires_unchanged_owned_files() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("owned ' route");
		let marker = std::path::PathBuf::from(format!("{}.owner", path.display()));
		let run = |script: String| std::process::Command::new("sh").args(["-c", &script]).status().unwrap();
		assert!(run(install_script(&path, address(), "installation-a", "true")).success());
		let original = std::fs::read(&path).unwrap();
		assert_eq!(run(remove_script(&path, "installation-b", "true")).code(), Some(CONFLICT));
		std::fs::write(&path, "another tool's route").unwrap();
		assert_eq!(run(remove_script(&path, "installation-a", "true")).code(), Some(CONFLICT));
		assert_eq!(std::fs::read_to_string(&path).unwrap(), "another tool's route");
		std::fs::write(&path, &original).unwrap();
		assert!(run(remove_script(&path, "installation-a", "true")).success());
		assert!(!path.exists());
		assert!(!marker.exists());
		assert!(run(remove_script(&path, "installation-a", "true")).success());
		std::fs::write(&path, original).unwrap();
		assert_eq!(run(remove_script(&path, "installation-a", "true")).code(), Some(CONFLICT));
		assert!(path.exists());
	}

	#[test]
	fn cleanup_uses_a_persistent_volume_and_refuses_same_content_replacements() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("owned route");
		let marker = std::path::PathBuf::from(format!("{}.owner", path.display()));
		let run = |script: String| std::process::Command::new("sh").args(["-c", &script]).status().unwrap();
		assert!(run(install_script(&path, address(), "installation-a", "true")).success());
		let receipt = std::fs::read_to_string(&marker).unwrap();
		let identity = receipt.lines().nth(1).unwrap();
		assert!(identity.starts_with("volume:"));
		let (_, remainder) = identity.strip_prefix("volume:").unwrap().split_once(':').unwrap();
		let other_volume = receipt.replace(identity, &format!("volume:another-volume:{remainder}"));
		std::fs::write(&marker, other_volume).unwrap();
		assert_eq!(run(remove_script(&path, "installation-a", "true")).code(), Some(CONFLICT));
		std::fs::write(&marker, &receipt).unwrap();
		let retained = dir.path().join("retained");
		std::fs::rename(&path, &retained).unwrap();
		std::fs::copy(&retained, &path).unwrap();
		assert_eq!(run(remove_script(&path, "installation-a", "true")).code(), Some(CONFLICT));
		assert!(path.exists() && marker.exists());
		std::fs::remove_file(&path).unwrap();
		std::fs::rename(retained, &path).unwrap();
		assert!(run(remove_script(&path, "installation-a", "true")).success());
	}

	#[test]
	fn legacy_receipts_keep_their_original_device_identity_guard() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("legacy route");
		let marker = std::path::PathBuf::from(format!("{}.owner", path.display()));
		let run = |script: String| std::process::Command::new("sh").args(["-c", &script]).status().unwrap();
		assert!(run(install_script(&path, address(), "installation-a", "true")).success());
		let receipt = std::fs::read_to_string(&marker).unwrap();
		let output = std::process::Command::new("sh")
			.args(["-c", &format!("{}\nlegacy_file_identity \"$path\"", macos::file_script(&path, "installation-a"))])
			.output()
			.unwrap();
		assert!(output.status.success());
		let identity = String::from_utf8(output.stdout).unwrap();
		let identity = identity.trim();
		let original_identity = receipt.lines().nth(1).unwrap();
		let legacy = receipt.replace(original_identity, identity);
		let (_, remainder) = identity.split_once(':').unwrap();
		std::fs::write(&marker, receipt.replace(original_identity, &format!("changed-device:{remainder}"))).unwrap();
		assert_eq!(run(remove_script(&path, "installation-a", "true")).code(), Some(CONFLICT));
		assert!(path.exists());
		std::fs::write(&marker, legacy).unwrap();
		assert!(run(remove_script(&path, "installation-a", "true")).success());
	}

	#[cfg(unix)]
	#[test]
	fn hard_linked_routes_are_left_untouched() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("owned route");
		let run = |script: String| std::process::Command::new("sh").args(["-c", &script]).status().unwrap();
		assert!(run(install_script(&path, address(), "installation-a", "true")).success());
		std::fs::hard_link(&path, dir.path().join("linked route")).unwrap();
		assert_eq!(run(remove_script(&path, "installation-a", "true")).code(), Some(CONFLICT));
		assert!(path.exists());
	}

	#[cfg(unix)]
	#[test]
	fn links_and_directories_are_left_untouched() {
		let dir = tempfile::tempdir().unwrap();
		let target = dir.path().join("missing");
		let link = dir.path().join("test");
		std::os::unix::fs::symlink(&target, &link).unwrap();
		assert!(already_configured(&link, address()).is_err());
		let status = std::process::Command::new("sh")
			.args(["-c", &install_script(&link, address(), "installation-a", "true")])
			.status()
			.unwrap();
		assert_eq!(status.code(), Some(CONFLICT));
		assert!(!target.exists());
		assert!(std::fs::symlink_metadata(link).unwrap().is_symlink());
		assert!(already_configured(dir.path(), address()).is_err());
	}
}
