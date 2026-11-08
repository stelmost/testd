//! Create a macOS resolver route without replacing existing system settings.

use std::io;
use std::net::SocketAddr;
use std::path::Path;

#[cfg(target_os = "macos")]
use super::{Elevation, silent_command};

const CONFLICT: i32 = 17;

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

fn install_script(path: &Path, addr: SocketAddr, reload: &str) -> String {
	let directory = super::shell_quote(&path.parent().unwrap().to_string_lossy());
	let path = super::shell_quote(&path.to_string_lossy());
	let content = super::shell_quote(&format!("nameserver {}\nport {}\n", addr.ip(), addr.port()));
	/* Noclobber rechecks after authorization and refuses files, links, or directories created in the meantime. */
	format!(
		"PATH=/usr/sbin:/usr/bin:/sbin:/bin; export PATH\n\
		mkdir -p {directory} || exit 1\n\
		if [ -e {path} ] || [ -L {path} ]; then exit {CONFLICT}; fi\n\
		set -C\n\
		(umask 022; printf '%s' {content} > {path}) || exit {CONFLICT}\n\
		{reload}"
	)
}

#[cfg(target_os = "macos")]
pub(super) fn setup(tld: &str, addr: SocketAddr, elevation: Elevation) -> io::Result<()> {
	testd_core::validate::validate_domain_labels(tld)
		.map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
	let path = std::path::PathBuf::from(format!("/etc/resolver/{tld}"));
	if already_configured(&path, addr)? {
		return Ok(());
	}
	let script = install_script(&path, addr, "dscacheutil -flushcache && killall -HUP mDNSResponder");
	let status = match elevation {
		Elevation::Sudo => silent_command("sudo").args(["sh", "-c", &script]).status()?,
		Elevation::Gui => {
			let escaped = script.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
			let action = format!("do shell script \"{escaped}\" with administrator privileges");
			silent_command("osascript").args(["-e", &action]).status()?
		},
	};
	if !status.success() {
		return Err(io::Error::other(format!("resolver setup failed ({status}); existing state was not replaced")));
	}
	if !already_configured(&path, addr)? {
		return Err(io::Error::other("resolver setup could not be verified"));
	}
	Ok(())
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
		let script = install_script(&path, address(), "true");
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
			.args(["-c", &install_script(&raced, address(), "true")])
			.status()
			.unwrap();
		assert_eq!(status.code(), Some(CONFLICT));
		assert_eq!(std::fs::read_to_string(raced).unwrap(), "other tool's settings");
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
			.args(["-c", &install_script(&link, address(), "true")])
			.status()
			.unwrap();
		assert_eq!(status.code(), Some(CONFLICT));
		assert!(!target.exists());
		assert!(std::fs::symlink_metadata(link).unwrap().is_symlink());
		assert!(already_configured(dir.path(), address()).is_err());
	}
}
