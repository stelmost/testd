/*! Private desktop state uses the same ownership boundary as daemon control files. */

use std::fs::File;
use std::io;
use std::path::Path;

pub fn directory(path: &Path) -> io::Result<()> {
	if !path.is_absolute() {
		return Err(io::Error::other("the private desktop directory must be absolute"));
	}
	#[cfg(unix)]
	{
		use std::os::unix::fs::{DirBuilderExt, MetadataExt};
		for parent in path.ancestors() {
			match testd_core::private_file::open_directory(parent) {
				Ok(file) => {
					let metadata = file.metadata()?;
					/* A root-owned sticky directory protects existing owned children, including test homes. */
					let sticky_root = metadata.uid() == 0 && metadata.mode() & 0o1000 != 0;
					if (metadata.uid() != unsafe { libc::geteuid() } && metadata.uid() != 0)
						|| (metadata.mode() & 0o022 != 0 && !sticky_root)
					{
						return Err(io::Error::new(io::ErrorKind::PermissionDenied, "unsafe private desktop parent"));
					}
				},
				Err(error) if error.kind() == io::ErrorKind::NotFound => {},
				Err(error) => return Err(error),
			}
		}
		std::fs::DirBuilder::new().recursive(true).mode(0o700).create(path)?;
	}
	#[cfg(not(unix))]
	std::fs::create_dir_all(path)?;
	testd_core::private_file::open_directory(path).map(|_| ())
}

#[cfg(unix)]
fn owned_file(path: &Path) -> io::Result<File> {
	use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
	let file = std::fs::OpenOptions::new()
		.read(true)
		.custom_flags(libc::O_NOFOLLOW)
		.open(path)?;
	let metadata = file.metadata()?;
	if !metadata.is_file()
		|| metadata.uid() != unsafe { libc::geteuid() }
		|| metadata.mode() & 0o022 != 0
		|| metadata.nlink() != 1
	{
		return Err(io::Error::new(io::ErrorKind::PermissionDenied, "desktop state has unsafe ownership or access"));
	}
	Ok(file)
}

pub fn open(path: &Path) -> io::Result<File> {
	#[cfg(unix)]
	{
		use std::os::unix::fs::MetadataExt;
		let file = owned_file(path)?;
		if file.metadata()?.mode() & 0o077 != 0 {
			return Err(io::Error::new(io::ErrorKind::PermissionDenied, "desktop state is not owner-private"));
		}
		Ok(file)
	}
	#[cfg(not(unix))]
	testd_core::private_file::open(path)
}

#[cfg(unix)]
pub fn read_entry(path: &Path) -> io::Result<Option<Vec<u8>>> {
	use std::io::Read;
	let file = match owned_file(path) {
		Ok(file) => file,
		Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
		Err(error) => return Err(error),
	};
	let mut data = Vec::new();
	file.take(16385).read_to_end(&mut data)?;
	if data.len() > 16384 {
		return Err(io::Error::other("existing desktop entry is too large; left unchanged"));
	}
	Ok(Some(data))
}

pub fn read(path: &Path) -> io::Result<Vec<u8>> {
	use std::io::Read;
	let mut data = Vec::new();
	open(path)?.take(4097).read_to_end(&mut data)?;
	if data.len() > 4096 {
		return Err(io::Error::other("private desktop descriptor is too large"));
	}
	Ok(data)
}

pub fn append_log(path: &Path, line: &str) -> io::Result<()> {
	use std::io::Write;
	let path = std::path::absolute(path)?;
	directory(path.parent().ok_or_else(|| io::Error::other("the trace path has no parent"))?)?;
	let mut file = match open(&path) {
		Ok(original) => {
			let mut options = std::fs::OpenOptions::new();
			options.append(true);
			#[cfg(unix)]
			{
				use std::os::unix::fs::OpenOptionsExt;
				options.custom_flags(libc::O_NOFOLLOW);
			}
			let file = options.open(&path)?;
			if !testd_core::private_file::same_file(&original, &file)? {
				return Err(io::Error::other("the private trace file was replaced; left unchanged"));
			}
			file
		},
		Err(error) if error.kind() == io::ErrorKind::NotFound => testd_core::private_file::create_new(&path)?,
		Err(error) => return Err(error),
	};
	writeln!(file, "{line}")
}

pub fn remove_owned(path: &Path, original: &File) -> io::Result<()> {
	let current = match open(path) {
		Ok(file) => file,
		Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
		Err(error) => return Err(error),
	};
	if !testd_core::private_file::same_file(original, &current)? {
		return Err(io::Error::other("desktop state was replaced; left untouched"));
	}
	std::fs::remove_file(path)
}

#[cfg(test)]
pub fn test_directory() -> tempfile::TempDir {
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		/* Fixture privacy must not depend on the developer's umask. */
		tempfile::Builder::new()
			.permissions(std::fs::Permissions::from_mode(0o700))
			.tempdir()
			.unwrap()
	}
	#[cfg(not(unix))]
	tempfile::tempdir().unwrap()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn diagnostic_logs_are_private_and_can_be_appended_without_replacement() {
		let home = test_directory();
		let path = home.path().join("tray.log");
		append_log(&path, "first").unwrap();
		let original = open(&path).unwrap();
		append_log(&path, "second").unwrap();
		assert_eq!(std::fs::read_to_string(&path).unwrap(), "first\nsecond\n");
		assert!(testd_core::private_file::same_file(&original, &open(&path).unwrap()).unwrap());
	}

	#[test]
	fn cleanup_preserves_replaced_files() {
		let home = test_directory();
		directory(home.path()).unwrap();
		let path = home.path().join("private");
		let original = testd_core::private_file::create_new(&path).unwrap();
		std::fs::rename(&path, home.path().join("retained")).unwrap();
		let _replacement = testd_core::private_file::create_new(&path).unwrap();
		assert!(remove_owned(&path, &original).is_err());
		assert!(path.exists());
	}

	#[cfg(unix)]
	#[test]
	fn shared_files_redirected_paths_and_writable_parents_are_refused() {
		use std::os::unix::fs::{PermissionsExt, symlink};
		let home = test_directory();
		let path = home.path().join("private");
		let _file = testd_core::private_file::create_new(&path).unwrap();
		let link = home.path().join("link");
		symlink(&path, &link).unwrap();
		assert!(open(&link).is_err());
		assert!(append_log(&link, "private diagnostic").is_err());
		std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
		assert!(open(&path).is_err());
		assert!(append_log(&path, "private diagnostic").is_err());
		assert!(std::fs::read(&path).unwrap().is_empty());
		let parent = home.path().join("shared");
		std::fs::create_dir(&parent).unwrap();
		std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o777)).unwrap();
		assert!(directory(&parent.join("child")).is_err());
		assert!(!parent.join("child").exists());
	}
}
