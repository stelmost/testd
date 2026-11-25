//! Privileged macOS operations with installation-specific ownership records.

use std::io;
use std::os::fd::AsFd;
use std::path::Path;

use super::{Elevation, shell_quote, silent_command};

pub(super) const CONFLICT: i32 = 17;

pub(super) fn run(script: &str, elevation: Elevation) -> io::Result<()> {
	let script = format!("PATH=/usr/sbin:/usr/bin:/sbin:/bin; export PATH\nset -eu\n{script}");
	/* Authorization utility output must not precede machine-readable CLI output. */
	let stdout = std::io::stderr().as_fd().try_clone_to_owned()?;
	let status = match elevation {
		Elevation::Sudo => silent_command("sudo").args(["sh", "-c", &script]).stdout(stdout).status()?,
		Elevation::Gui => {
			let escaped = script.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
			let action = format!("do shell script \"{escaped}\" with administrator privileges");
			silent_command("osascript").args(["-e", &action]).stdout(stdout).status()?
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

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
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
		.replace(
			"volume=$(diskutil info -plist \"$device\" | plutil -extract VolumeUUID raw -o - -)",
			"volume=$(stat -c '%d' \"$1\")",
		)
		.replace("stat -f '%i:%u:%g:%Lp'", "stat -c '%i:%u:%g:%a'")
		.replace("stat -f '%d:%i:%u:%g:%Lp'", "stat -c '%d:%i:%u:%g:%a'")
		.replace("stat -f '%u:%Lp:%l'", "stat -c '%u:%a:%h'")
		.replace("stat -f '%l'", "stat -c '%h'");
	format!("path={path}\nmarker={marker}\nowner={}\n{helpers}", shell_quote(owner))
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::os::unix::fs::PermissionsExt;
	use std::process::Command;

	#[test]
	fn privileged_progress_does_not_pollute_standard_output() {
		const FLAG: &str = "TESTD_MACOS_PRIVILEGED_PROGRESS_FIXTURE";
		if std::env::var_os(FLAG).is_some() {
			for elevation in [Elevation::Sudo, Elevation::Gui] {
				run("true", elevation).unwrap();
			}
			return;
		}
		let directory = tempfile::tempdir().unwrap();
		for name in ["sudo", "osascript"] {
			let path = directory.path().join(name);
			std::fs::write(&path, "#!/bin/sh\nprintf privileged-progress\n").unwrap();
			std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
		}
		let output = Command::new(std::env::current_exe().unwrap())
			.args(["--exact", "macos::tests::privileged_progress_does_not_pollute_standard_output", "--nocapture"])
			.env(FLAG, "1")
			.env("PATH", directory.path())
			.output()
			.unwrap();
		assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
		assert!(!String::from_utf8_lossy(&output.stdout).contains("privileged-progress"));
		assert_eq!(String::from_utf8_lossy(&output.stderr).matches("privileged-progress").count(), 2);
	}
}
