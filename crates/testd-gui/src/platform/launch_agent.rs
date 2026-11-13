/*! LaunchAgent serialization and validation do not require AppKit or a live login session. */

use std::path::Path;

use anyhow::{Context, ensure};
use plist::{Dictionary, Value};

const LABEL: &str = "dev.testd.host";

pub(super) fn check(path: &Path, executable: &Path) -> anyhow::Result<()> {
	let Some(data) = crate::private::read_entry(path)? else {
		return Ok(());
	};
	let value = Value::from_reader(std::io::Cursor::new(data))?;
	let entry = value
		.as_dictionary()
		.context("the existing login entry is not a dictionary; left unchanged")?;
	ensure!(
		entry.len() == 3
			&& entry.get("Label").and_then(Value::as_string) == Some(LABEL)
			&& entry.get("RunAtLoad").and_then(Value::as_boolean) == Some(true)
			&& entry
				.get("ProgramArguments")
				.and_then(Value::as_array)
				.is_some_and(|arguments| arguments.len() == 1 && arguments[0].as_string() == executable.to_str()),
		"the existing login entry was changed or points elsewhere; left unchanged"
	);
	Ok(())
}

pub(super) fn create(path: &Path, executable: &Path) -> anyhow::Result<()> {
	ensure!(executable.is_absolute() && executable.is_file(), "the login executable must be an existing absolute file");
	let executable = executable.to_str().context("invalid login executable path")?;
	ensure!(
		!executable.chars().any(char::is_control),
		"control characters are not supported in a login executable path"
	);
	let mut entry = Dictionary::new();
	entry.insert("Label".into(), Value::String(LABEL.into()));
	entry.insert("ProgramArguments".into(), Value::Array(vec![Value::String(executable.into())]));
	entry.insert("RunAtLoad".into(), Value::Boolean(true));
	let directory = path.parent().context("the login entry has no parent directory")?;
	crate::private::directory(directory)?;
	let mut temporary = testd_core::private_file::temporary_in(directory)?;
	Value::Dictionary(entry).to_writer_xml(&mut temporary)?;
	temporary.as_file().sync_all()?;
	/* A new entry must not replace one created after the initial ownership check. */
	temporary.persist_noclobber(path).map_err(|error| error.error)?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn login_paths_round_trip_as_plist_data_without_changing_the_filesystem_path() {
		let home = tempfile::tempdir().unwrap();
		let directory = home.path().canonicalize().unwrap();
		let executable = directory.join("A & B <é> \"testd-gui\"");
		std::fs::write(&executable, b"fixture executable").unwrap();
		let path = directory.join("login.plist");
		create(&path, &executable).unwrap();
		check(&path, &executable).unwrap();
		let value = Value::from_file(&path).unwrap();
		let arguments = value.as_dictionary().unwrap()["ProgramArguments"].as_array().unwrap();
		assert_eq!(arguments[0].as_string(), executable.to_str());
		assert!(std::fs::read_to_string(&path).unwrap().contains("&amp;"));
		use std::os::unix::fs::PermissionsExt;
		assert_eq!(std::fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
	}

	#[test]
	fn compatible_and_changed_entries_are_never_replaced() {
		let home = tempfile::tempdir().unwrap();
		let directory = home.path().canonicalize().unwrap();
		let executable = directory.join("testd-gui");
		std::fs::write(&executable, b"fixture executable").unwrap();
		let path = directory.join("login.plist");
		create(&path, &executable).unwrap();
		let original = std::fs::read(&path).unwrap();
		assert!(create(&path, &executable).is_err());
		assert_eq!(std::fs::read(&path).unwrap(), original);
		let mut value = Value::from_file(&path).unwrap();
		value
			.as_dictionary_mut()
			.unwrap()
			.insert("KeepAlive".into(), Value::Boolean(true));
		value.to_file_xml(&path).unwrap();
		let changed = std::fs::read(&path).unwrap();
		assert!(check(&path, &executable).is_err());
		assert!(create(&path, &executable).is_err());
		assert_eq!(std::fs::read(path).unwrap(), changed);
	}

	#[test]
	fn missing_or_relative_executables_do_not_create_login_entries() {
		let home = tempfile::tempdir().unwrap();
		let directory = home.path().canonicalize().unwrap();
		let path = directory.join("login.plist");
		for executable in [Path::new("relative"), &directory.join("missing")] {
			assert!(create(&path, executable).is_err());
			assert!(!path.exists());
		}
	}
}
