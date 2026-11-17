//! Privileged macOS operations with installation-specific ownership records.

#[cfg(target_os = "macos")]
use std::io;
use std::path::Path;

#[cfg(target_os = "macos")]
use super::{Elevation, silent_command};
use super::shell_quote;

pub(super) const CONFLICT: i32 = 17;

#[cfg(target_os = "macos")]
pub(super) fn run(script: &str, elevation: Elevation) -> io::Result<()> {
	let script = format!("PATH=/usr/sbin:/usr/bin:/sbin:/bin; export PATH\nset -eu\n{script}");
	let status = match elevation {
		Elevation::Sudo => silent_command("sudo").args(["sh", "-c", &script]).status()?,
		Elevation::Gui => {
			let escaped = script.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
			let action = format!("do shell script \"{escaped}\" with administrator privileges");
			silent_command("osascript").args(["-e", &action]).status()?
		},
	};
	match status.code() {
		Some(0) => Ok(()),
		Some(CONFLICT) => Err(io::Error::new(
			io::ErrorKind::AlreadyExists,
			"system integration is unowned or has changed; testd left it untouched",
		)),
		_ => Err(io::Error::other(format!("macOS system integration failed ({status}); inspect it before retrying"))),
	}
}

#[cfg(target_os = "macos")]
pub(super) fn present(path: &Path) -> io::Result<bool> {
	match std::fs::symlink_metadata(path) {
		Ok(_) => Ok(true),
		Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
		Err(error) => Err(error),
	}
}

pub(super) fn file_script(path: &Path, owner: &str) -> String {
	let marker = shell_quote(&format!("{}.owner", path.to_string_lossy()));
	let path = shell_quote(&path.to_string_lossy());
	let helpers = include_str!("macos-owned-file.sh");
	#[cfg(not(target_os = "macos"))]
	let helpers = helpers
		.replace("stat -f '%d:%i:%u:%g:%Lp'", "stat -c '%d:%i:%u:%g:%a'")
		.replace("stat -f '%u:%Lp:%l'", "stat -c '%u:%a:%h'");
	format!("path={path}\nmarker={marker}\nowner={}\n{helpers}", shell_quote(owner))
}
