use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr::{addr_of, null, null_mut};

use windows_sys::Win32::Foundation::{
	ERROR_INSUFFICIENT_BUFFER, GENERIC_EXECUTE, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
	ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
	SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
	ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetTokenInformation,
	INHERIT_ONLY_ACE, IsValidAcl, IsValidSid, IsWellKnownSid, OWNER_SECURITY_INFORMATION, PSID, SECURITY_ATTRIBUTES,
	TOKEN_QUERY, TOKEN_USER, TokenUser, WinBuiltinAdministratorsSid, WinCreatorOwnerRightsSid, WinLocalSystemSid,
};
use windows_sys::Win32::Storage::FileSystem::{
	BY_HANDLE_FILE_INFORMATION, CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
	FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ,
	FILE_GENERIC_WRITE, FILE_GENERIC_EXECUTE, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
	FILE_SHARE_WRITE, GetFileInformationByHandle, OPEN_EXISTING, READ_CONTROL,
};
use windows_sys::Win32::System::SystemServices::{ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

struct LocalAllocation(*mut c_void);

impl Drop for LocalAllocation {
	fn drop(&mut self) {
		/* SAFETY: these buffers are returned by Windows APIs that require LocalFree. */
		unsafe {
			LocalFree(self.0);
		}
	}
}

struct User(Vec<usize>);

impl User {
	fn current() -> io::Result<Self> {
		let mut token = null_mut();
		/* SAFETY: the token handle and length outputs point to initialized local storage. */
		if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
			return Err(io::Error::last_os_error());
		}
		/* SAFETY: OpenProcessToken returned an owned, non-null handle. */
		let token = unsafe { OwnedHandle::from_raw_handle(token) };
		let mut length = 0;
		/* SAFETY: the zero-length query does not dereference the null buffer. */
		unsafe {
			GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut length);
		}
		if length == 0 || io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
			return Err(io::Error::last_os_error());
		}
		let mut buffer = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
		/* SAFETY: usize storage provides TOKEN_USER alignment and at least length writable bytes. */
		if unsafe {
			GetTokenInformation(token.as_raw_handle(), TokenUser, buffer.as_mut_ptr().cast(), length, &mut length)
		} == 0
		{
			return Err(io::Error::last_os_error());
		}
		Ok(Self(buffer))
	}

	fn sid(&self) -> PSID {
		/* SAFETY: GetTokenInformation initialized TOKEN_USER and its SID within this retained buffer. */
		unsafe { (*self.0.as_ptr().cast::<TOKEN_USER>()).User.Sid }
	}

	fn text(&self) -> io::Result<String> {
		let mut text = null_mut();
		/* SAFETY: the current user's SID remains valid and the API allocates a terminated UTF-16 string. */
		if unsafe { ConvertSidToStringSidW(self.sid(), &mut text) } == 0 {
			return Err(io::Error::last_os_error());
		}
		let _allocation = LocalAllocation(text.cast());
		let mut length = 0;
		/* SAFETY: Windows guarantees this allocation ends with a UTF-16 NUL. */
		unsafe {
			while *text.add(length) != 0 {
				length += 1;
			}
			String::from_utf16(std::slice::from_raw_parts(text, length))
				.map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
		}
	}
}

fn descriptor(user: &User) -> io::Result<LocalAllocation> {
	let sid = user.text()?;
	parse_descriptor(&format!("O:{sid}D:P(A;;FA;;;{sid})(A;;FA;;;SY)(A;;FA;;;BA)"))
}

fn parse_descriptor(sddl: &str) -> io::Result<LocalAllocation> {
	let sddl: Vec<u16> = sddl.encode_utf16().chain([0]).collect();
	let mut descriptor = null_mut();
	/* SAFETY: the SDDL is terminated, and Windows allocates the output descriptor. */
	if unsafe {
		ConvertStringSecurityDescriptorToSecurityDescriptorW(
			sddl.as_ptr(),
			SDDL_REVISION_1,
			&mut descriptor,
			null_mut(),
		)
	} == 0
	{
		return Err(io::Error::last_os_error());
	}
	Ok(LocalAllocation(descriptor))
}

fn path_wide(path: &Path) -> io::Result<Vec<u16>> {
	let absolute = std::path::absolute(path)?;
	let path: Vec<u16> = absolute.as_os_str().encode_wide().collect();
	if path.contains(&0) {
		return Err(io::Error::new(io::ErrorKind::InvalidInput, "private path contains a NUL"));
	}
	let prefix: Vec<u16> = r"\\?\".encode_utf16().collect();
	let mut wide = if path.starts_with(&prefix) {
		path
	} else if path.starts_with(&[b'\\' as u16, b'\\' as u16]) {
		r"\\?\UNC\".encode_utf16().chain(path.into_iter().skip(2)).collect()
	} else {
		prefix.into_iter().chain(path).collect()
	};
	wide.push(0);
	Ok(wide)
}

fn existing(path: &Path, flags: u32) -> io::Result<File> {
	let path = path_wide(path)?;
	/* SAFETY: the path is terminated; OPEN_REPARSE_POINT avoids following a final link. */
	let handle = unsafe {
		CreateFileW(
			path.as_ptr(),
			FILE_GENERIC_READ | READ_CONTROL | FILE_READ_ATTRIBUTES,
			FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
			null(),
			OPEN_EXISTING,
			flags | FILE_FLAG_OPEN_REPARSE_POINT,
			null_mut(),
		)
	};
	owned_file(handle)
}

fn owned_file(handle: HANDLE) -> io::Result<File> {
	if handle == INVALID_HANDLE_VALUE {
		return Err(io::Error::last_os_error());
	}
	/* SAFETY: a successful CreateFileW transfers ownership of this handle to File. */
	Ok(unsafe { File::from_raw_handle(handle) })
}

pub(super) fn create_new(path: &Path) -> io::Result<File> {
	let user = User::current()?;
	let parent = path
		.parent()
		.filter(|parent| !parent.as_os_str().is_empty())
		.unwrap_or(Path::new("."));
	let parent = existing(parent, FILE_FLAG_BACKUP_SEMANTICS)?;
	validate_directory(&parent, &user)?;
	let descriptor = descriptor(&user)?;
	let attributes = SECURITY_ATTRIBUTES {
		nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
		lpSecurityDescriptor: descriptor.0,
		bInheritHandle: 0,
	};
	let wide = path_wide(path)?;
	/* SAFETY: all pointers are valid for this call; CREATE_NEW cannot replace a raced-in entry. */
	let handle = unsafe {
		CreateFileW(
			wide.as_ptr(),
			FILE_GENERIC_READ | FILE_GENERIC_WRITE | READ_CONTROL,
			FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
			&attributes,
			CREATE_NEW,
			FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
			null_mut(),
		)
	};
	let file = owned_file(handle)?;
	validate_file(&file, &user)?;
	Ok(file)
}

pub(super) fn open_private(path: &Path) -> io::Result<File> {
	let file = existing(path, FILE_ATTRIBUTE_NORMAL)?;
	validate_file(&file, &User::current()?).map_err(|error| {
		io::Error::new(error.kind(), format!("{} is not a private owned file: {error}", path.display()))
	})?;
	Ok(file)
}

pub(super) fn open_directory(path: &Path) -> io::Result<File> {
	let directory = existing(path, FILE_FLAG_BACKUP_SEMANTICS)?;
	validate_directory(&directory, &User::current()?)?;
	Ok(directory)
}

pub(super) fn same_file(first: &File, second: &File) -> io::Result<bool> {
	let first = information(first)?;
	let second = information(second)?;
	if first.nFileIndexHigh == 0 && first.nFileIndexLow == 0 || second.nFileIndexHigh == 0 && second.nFileIndexLow == 0
	{
		return Err(io::Error::new(
			io::ErrorKind::Unsupported,
			"filesystem does not provide verifiable file identities",
		));
	}
	Ok(first.dwVolumeSerialNumber == second.dwVolumeSerialNumber
		&& first.nFileIndexHigh == second.nFileIndexHigh
		&& first.nFileIndexLow == second.nFileIndexLow)
}

fn information(file: &File) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
	/* SAFETY: the output is a plain Windows structure initialized by GetFileInformationByHandle. */
	let mut information = unsafe { std::mem::zeroed() };
	if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
		return Err(io::Error::last_os_error());
	}
	Ok(information)
}

fn validate_file(file: &File, user: &User) -> io::Result<()> {
	let information = information(file)?;
	if information.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0
		|| information.nNumberOfLinks != 1
	{
		return Err(denied("private files must be regular files without links or reparse points"));
	}
	let security = Security::load(file)?;
	/* SAFETY: both SIDs belong to retained Windows allocations. */
	if unsafe { EqualSid(security.owner, user.sid()) } == 0 {
		return Err(denied("private file belongs to another owner; it was left untouched"));
	}
	if security.untrusted_access(user)? != 0 {
		return Err(denied(
			"access is granted outside the owner, SYSTEM, or Administrators; restrict the ACL manually",
		));
	}
	Ok(())
}

fn validate_directory(file: &File, user: &User) -> io::Result<()> {
	let information = information(file)?;
	if information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
		|| information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
	{
		return Err(denied("private file parent must be an ordinary directory"));
	}
	let security = Security::load(file)?;
	if !trusted_sid(security.owner, user) {
		return Err(denied("private file parent belongs to an untrusted owner"));
	}
	/* Directory listing is harmless; untrusted creation, replacement, or ACL changes are not. */
	let read_access = FILE_GENERIC_READ | FILE_GENERIC_EXECUTE | GENERIC_READ | GENERIC_EXECUTE;
	if security.untrusted_access(user)? & !read_access != 0 {
		return Err(denied("private file parent can be modified by untrusted users"));
	}
	Ok(())
}

struct Security {
	_allocation: LocalAllocation,
	owner: PSID,
	acl: *mut ACL,
}

impl Security {
	fn load(file: &File) -> io::Result<Self> {
		let mut descriptor = null_mut();
		let mut owner = null_mut();
		let mut acl = null_mut();
		/* SAFETY: the handle is live and each output points to initialized local storage. */
		let status = unsafe {
			GetSecurityInfo(
				file.as_raw_handle(),
				SE_FILE_OBJECT,
				OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
				&mut owner,
				null_mut(),
				&mut acl,
				null_mut(),
				&mut descriptor,
			)
		};
		if status != 0 {
			return Err(io::Error::from_raw_os_error(status as i32));
		}
		let allocation = LocalAllocation(descriptor);
		if owner.is_null() || acl.is_null() || unsafe { IsValidAcl(acl) } == 0 {
			return Err(denied("a verifiable owner and explicit access list are required"));
		}
		Ok(Self { _allocation: allocation, owner, acl })
	}

	fn untrusted_access(&self, user: &User) -> io::Result<u32> {
		if !trusted_sid(self.owner, user) {
			return Err(denied("private object belongs to an untrusted owner"));
		}
		let mut access = 0;
		/* SAFETY: Security retains the valid descriptor and GetAce checks every index. */
		for index in 0..unsafe { (*self.acl).AceCount } as u32 {
			let mut ace = null_mut();
			if unsafe { GetAce(self.acl, index, &mut ace) } == 0 {
				return Err(io::Error::last_os_error());
			}
			let header = unsafe { &*ace.cast::<ACE_HEADER>() };
			if header.AceFlags as u32 & INHERIT_ONLY_ACE != 0 {
				continue;
			}
			match header.AceType as u32 {
				ACCESS_DENIED_ACE_TYPE => {},
				ACCESS_ALLOWED_ACE_TYPE => {
					let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
					let sid = addr_of!(allowed.SidStart).cast_mut().cast();
					/* OWNER RIGHTS resolves to the already verified owner, not to an arbitrary user. */
					if !trusted_sid(sid, user) && unsafe { IsWellKnownSid(sid, WinCreatorOwnerRightsSid) } == 0 {
						access |= allowed.Mask;
					}
				},
				_ => return Err(denied("an unsupported access rule prevents proving file privacy")),
			}
		}
		Ok(access)
	}
}

fn trusted_sid(sid: PSID, user: &User) -> bool {
	/* SAFETY: validity is checked before inspecting each descriptor SID. */
	unsafe {
		IsValidSid(sid) != 0
			&& (EqualSid(sid, user.sid()) != 0
				|| IsWellKnownSid(sid, WinLocalSystemSid) != 0
				|| IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0)
	}
}

fn denied(message: &str) -> io::Error {
	io::Error::new(io::ErrorKind::PermissionDenied, message)
}

#[cfg(test)]
mod tests {
	use super::*;
	use windows_sys::Win32::Security::Authorization::SetNamedSecurityInfoW;
	use windows_sys::Win32::Security::{
		GetSecurityDescriptorControl, GetSecurityDescriptorDacl, GetSecurityDescriptorOwner,
		PROTECTED_DACL_SECURITY_INFORMATION, SE_DACL_PROTECTED,
	};

	fn set_acl(path: &Path, sddl: &str) {
		let descriptor = parse_descriptor(sddl).unwrap();
		let mut present = 0;
		let mut defaulted = 0;
		let mut acl = null_mut();
		/* SAFETY: all outputs are writable and the descriptor allocation remains live. */
		assert_ne!(unsafe { GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut acl, &mut defaulted) }, 0);
		let mut path = path_wide(path).unwrap();
		/* SAFETY: the terminated fixture path and ACL remain valid during this call. */
		assert_eq!(
			unsafe {
				SetNamedSecurityInfoW(
					path.as_mut_ptr(),
					SE_FILE_OBJECT,
					DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
					null_mut(),
					null_mut(),
					acl,
					null(),
				)
			},
			0
		);
	}

	fn grant_everyone(path: &Path, rights: &str) {
		let user = User::current().unwrap().text().unwrap();
		set_acl(path, &format!("D:P(A;;FA;;;{user})(A;;FA;;;SY)(A;;FA;;;BA)(A;OICI;{rights};;;WD)"));
	}

	#[test]
	fn owner_rights_acl_allows_only_a_verified_owner() {
		let directory = tempfile::tempdir().unwrap();
		let python_acl = "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;OW)";
		set_acl(directory.path(), python_acl);
		let nested = directory.path().join("created-inside-private-report");
		std::fs::create_dir(&nested).unwrap();
		let path = nested.join("credential");
		crate::config::write_private(&path, b"fixture-token").unwrap();
		assert_eq!(crate::private_file::read(&path).unwrap(), b"fixture-token");

		let broad_acl = format!("{python_acl}(A;OICI;FA;;;WD)");
		set_acl(&nested, &broad_acl);
		assert_eq!(create_new(&nested.join("other")).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
	}

	#[test]
	fn owner_rights_never_make_an_untrusted_owner_safe() {
		let descriptor = parse_descriptor("O:WDD:P(A;;FA;;;OW)").unwrap();
		let mut owner = null_mut();
		let mut acl = null_mut();
		let mut present = 0;
		let mut defaulted = 0;
		/* SAFETY: the live descriptor owns the returned SID and ACL pointers. */
		assert_ne!(unsafe { GetSecurityDescriptorOwner(descriptor.0, &mut owner, &mut defaulted) }, 0);
		assert_ne!(unsafe { GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut acl, &mut defaulted) }, 0);
		let security = Security { _allocation: descriptor, owner, acl };
		assert_eq!(
			security.untrusted_access(&User::current().unwrap()).unwrap_err().kind(),
			io::ErrorKind::PermissionDenied
		);
	}

	#[test]
	fn temporary_files_are_owned_and_protected_before_any_secret_is_written() {
		let directory = tempfile::tempdir().unwrap();
		let temporary = crate::private_file::temporary_in(directory.path()).unwrap();
		let security = Security::load(temporary.as_file()).unwrap();
		let mut control = 0;
		let mut revision = 0;
		/* SAFETY: the descriptor is live and both output pointers reference local storage. */
		assert_ne!(unsafe { GetSecurityDescriptorControl(security._allocation.0, &mut control, &mut revision) }, 0);
		assert_ne!(control & SE_DACL_PROTECTED, 0);
		validate_file(temporary.as_file(), &User::current().unwrap()).unwrap();
		assert!(crate::private_file::read(temporary.path()).unwrap().is_empty());
	}

	#[test]
	fn broad_acls_are_not_read_repaired_or_overwritten() {
		let directory = tempfile::tempdir().unwrap();
		let path = directory.path().join("credential");
		crate::config::write_private(&path, b"fixture-token").unwrap();
		grant_everyone(&path, "GR");
		assert_eq!(crate::private_file::read(&path).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
		assert_eq!(
			crate::config::write_private(&path, b"replacement").unwrap_err().kind(),
			io::ErrorKind::PermissionDenied
		);
		assert_eq!(std::fs::read(&path).unwrap(), b"fixture-token");
		assert_eq!(crate::private_file::read(&path).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
	}

	#[test]
	fn private_files_are_not_created_inside_a_shared_directory() {
		let directory = tempfile::tempdir().unwrap();
		grant_everyone(directory.path(), "FA");
		let path = directory.path().join("credential");
		assert_eq!(create_new(&path).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
		assert!(!path.exists());
	}

	#[test]
	fn directory_readers_do_not_inherit_access_to_new_private_files() {
		let directory = tempfile::tempdir().unwrap();
		grant_everyone(directory.path(), "GR");
		let path = directory.path().join("key");
		crate::config::write_private(&path, b"fixture-key").unwrap();
		assert_eq!(crate::private_file::read(&path).unwrap(), b"fixture-key");
	}

	#[test]
	fn directory_identity_is_the_open_object_not_its_name_or_timestamps() {
		let fixture = tempfile::tempdir().unwrap();
		let path = fixture.path().join("state");
		std::fs::create_dir(&path).unwrap();
		let original = open_directory(&path).unwrap();
		assert!(same_file(&original, &open_directory(&path).unwrap()).unwrap());
		std::fs::rename(&path, fixture.path().join("previous")).unwrap();
		std::fs::create_dir(&path).unwrap();
		assert!(!same_file(&original, &open_directory(&path).unwrap()).unwrap());
	}

	#[test]
	fn linked_private_files_are_neither_read_nor_replaced() {
		let directory = tempfile::tempdir().unwrap();
		let path = directory.path().join("key");
		let link = directory.path().join("linked-key");
		crate::config::write_private(&path, b"fixture-key").unwrap();
		std::fs::hard_link(&path, &link).unwrap();
		assert_eq!(crate::private_file::read(&path).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
		assert!(crate::config::write_private(&link, b"replacement").is_err());
		assert_eq!(std::fs::read(&path).unwrap(), b"fixture-key");
		assert_eq!(std::fs::read(&link).unwrap(), b"fixture-key");
	}
}
