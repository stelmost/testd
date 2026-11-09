/*! Private file creation and reads. Windows must enforce ACLs, not Unix-style readonly bits. */

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

#[cfg(windows)]
mod windows;

pub fn open(path: &Path) -> io::Result<File> {
	#[cfg(windows)]
	return windows::open_private(path);
	#[cfg(not(windows))]
	File::open(path)
}

pub fn open_directory(path: &Path) -> io::Result<File> {
	#[cfg(windows)]
	return windows::open_directory(path);
	#[cfg(not(windows))]
	{
		use std::os::unix::fs::OpenOptionsExt;
		std::fs::OpenOptions::new()
			.read(true)
			.custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
			.open(path)
	}
}

pub fn same_file(first: &File, second: &File) -> io::Result<bool> {
	#[cfg(windows)]
	return windows::same_file(first, second);
	#[cfg(not(windows))]
	{
		use std::os::unix::fs::MetadataExt;
		let first = first.metadata()?;
		let second = second.metadata()?;
		Ok(first.dev() == second.dev() && first.ino() == second.ino())
	}
}

pub fn read(path: &Path) -> io::Result<Vec<u8>> {
	let mut file = open(path)?;
	let mut data = Vec::new();
	file.read_to_end(&mut data)?;
	Ok(data)
}

pub fn read_to_string(path: &Path) -> io::Result<String> {
	String::from_utf8(read(path)?).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub fn create_new(path: &Path) -> io::Result<File> {
	#[cfg(windows)]
	return windows::create_new(path);
	#[cfg(not(windows))]
	{
		use std::os::unix::fs::OpenOptionsExt;
		std::fs::OpenOptions::new()
			.read(true)
			.write(true)
			.create_new(true)
			.mode(0o600)
			.open(path)
	}
}

pub fn temporary_in(directory: &Path) -> io::Result<tempfile::NamedTempFile> {
	#[cfg(windows)]
	{
		for _ in 0..10 {
			let name = crate::config::generate_operator_secret()?;
			let path = directory.join(format!(".testd-private-{name}"));
			match create_new(&path) {
				Ok(file) => {
					return Ok(tempfile::NamedTempFile::from_parts(file, tempfile::TempPath::try_from_path(path)?));
				},
				Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
				Err(error) => return Err(error),
			}
		}
		Err(io::Error::new(io::ErrorKind::AlreadyExists, "could not reserve a private temporary file"))
	}
	#[cfg(not(windows))]
	tempfile::NamedTempFile::new_in(directory)
}

#[cfg(windows)]
pub(crate) fn verbatim_path(path: &Path) -> io::Result<std::path::PathBuf> {
	use std::os::windows::ffi::OsStringExt;

	let wide = windows::path_wide(path)?;
	Ok(std::ffi::OsString::from_wide(&wide[..wide.len() - 1]).into())
}

pub(crate) fn validate_existing(path: &Path) -> io::Result<()> {
	#[cfg(windows)]
	drop(windows::open_private(path)?);
	#[cfg(not(windows))]
	let _ = path;
	Ok(())
}
