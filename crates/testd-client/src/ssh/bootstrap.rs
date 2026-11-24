/*! Prepare a version-matched helper through the existing SSH command channel. */

use tokio::sync::oneshot;

pub const HELPER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug)]
pub struct InstallRequest {
	pub version: Option<String>,
	pub respond: oneshot::Sender<bool>,
}

/** OpenSSH joins command arguments into a remote shell command. */
pub fn bootstrap_command() -> String {
	let script = format!("version='{HELPER_VERSION}'\n{}", include_str!("bootstrap.sh"));
	format!("sh -c '{}'", script.replace('\'', "'\\''"))
}

#[cfg(all(test, unix))]
mod tests {
	use super::*;
	use std::os::unix::fs::PermissionsExt;
	use std::process::Stdio;
	use std::time::Duration;

	const HELPER: &str = r#"#!/bin/sh
case "$1" in
	--version) printf 'testd %s\n' '@VERSION@' ;;
	ssh-session) printf 'started\n' ;;
	*) exit 2 ;;
esac
"#;

	#[tokio::test]
	async fn openssh_command_preserves_the_script_and_quoted_home_path() {
		let root = tempfile::tempdir().unwrap();
		let home = root.path().join("user's home");
		let directory = home.join(".local/bin");
		std::fs::create_dir_all(&directory).unwrap();
		let helper = directory.join("testd");
		std::fs::write(&helper, HELPER.replace("@VERSION@", HELPER_VERSION)).unwrap();
		std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
		let output = tokio::time::timeout(
			Duration::from_secs(3),
			tokio::process::Command::new("sh")
				.args(["-c", &bootstrap_command()])
				.env("HOME", &home)
				.stdin(Stdio::null())
				.kill_on_drop(true)
				.output(),
		)
		.await
		.unwrap()
		.unwrap();
		assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
		assert_eq!(output.stdout, b"started\n");
	}
}
