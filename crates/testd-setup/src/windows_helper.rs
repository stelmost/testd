/*! Run only the audited helper, using the system PowerShell and a verified in-memory source snapshot. */

use std::path::Path;

const SOURCE: &str = include_str!("../../../scripts/testd-elevated.ps1");

pub(super) fn quote(value: &str) -> String {
	format!("'{}'", value.replace('\'', "''"))
}

fn command(path: &Path, action: &str, args: &[(&str, &str)]) -> String {
	let fingerprint = super::sha256_hex(SOURCE.as_bytes());
	/* Hash the bytes already in memory: replacing the staging path cannot replace verified code. */
	let mut invocation = format!(
		"$ErrorActionPreference = 'Stop'; \
		$env:PSModulePath = [IO.Path]::Combine([Environment]::SystemDirectory, 'WindowsPowerShell\\v1.0\\Modules'); \
		$bytes = [IO.File]::ReadAllBytes({}); \
		$hash = [Security.Cryptography.SHA256]::Create(); \
		try {{ $actual = ([BitConverter]::ToString($hash.ComputeHash($bytes))).Replace('-', '').ToLowerInvariant() }} \
		finally {{ $hash.Dispose() }}; \
		if ($actual -cne '{fingerprint}') {{ throw 'audited testd helper source changed; no action was run' }}; \
		& ([ScriptBlock]::Create([Text.Encoding]::UTF8.GetString($bytes))) -Action {}",
		quote(&path.to_string_lossy()),
		quote(action),
	);
	for (name, value) in args {
		invocation.push_str(&format!(" -{name} {}", quote(value)));
	}
	invocation
}

pub(super) fn encoded(script: &str) -> String {
	use base64::Engine;
	let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
	base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(windows)]
pub(super) fn powershell() -> std::io::Result<std::path::PathBuf> {
	use std::os::windows::ffi::OsStringExt;
	use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;

	let mut buffer = vec![0u16; 32768];
	/* SAFETY: the initialized buffer has the advertised writable UTF-16 capacity. */
	let length = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
	if length == 0 {
		return Err(std::io::Error::last_os_error());
	}
	if length >= buffer.len() {
		return Err(std::io::Error::other("Windows system directory path is too long"));
	}
	Ok(std::path::PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length]))
		.join(r"WindowsPowerShell\v1.0\powershell.exe"))
}

#[cfg(windows)]
fn is_elevated(powershell: &Path) -> bool {
	let command = concat!(
		"([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent())",
		".IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)",
	);
	super::silent_command(powershell)
		.args(["-NoProfile", "-NonInteractive", "-Command", command])
		.output()
		.ok()
		.is_some_and(|output| {
			output.status.success() && String::from_utf8_lossy(&output.stdout).trim().eq_ignore_ascii_case("true")
		})
}

#[cfg(windows)]
pub(super) fn run(action: &str, args: &[(&str, &str)]) -> std::io::Result<()> {
	use std::io::Write;
	use super::{ownership::InstallationId, silent_command};

	let owner =
		if action.starts_with("Install") { Some(InstallationId::load_or_create()?) } else { InstallationId::load()? };
	let owner = owner.map(|owner| owner.fingerprint()).unwrap_or_default();
	let mut args = args.to_vec();
	args.push(("Owner", &owner));
	let mut helper = testd_core::private_file::temporary_in(&std::env::temp_dir())?;
	helper.write_all(SOURCE.as_bytes())?;
	helper.as_file().sync_all()?;
	let encoded = encoded(&command(helper.path(), action, &args));
	let powershell = powershell()?;
	let status = if is_elevated(&powershell) {
		silent_command(&powershell)
			.args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded])
			.status()?
	} else {
		let arguments = format!("-NoProfile -NonInteractive -EncodedCommand {encoded}");
		let launch = format!(
			"$ErrorActionPreference = 'Stop'; \
			$p = Start-Process -FilePath {} -Verb RunAs -Wait -PassThru -ErrorAction Stop -ArgumentList {}; \
			exit $p.ExitCode",
			quote(&powershell.to_string_lossy()),
			quote(&arguments),
		);
		silent_command(&powershell)
			.args(["-NoProfile", "-NonInteractive", "-Command", &launch])
			.status()?
	};
	match status.code() {
		Some(0) => Ok(()),
		Some(17) => Err(std::io::Error::new(
			std::io::ErrorKind::AlreadyExists,
			"system integration is unowned, changed, or incomplete; unverified state was left untouched",
		)),
		_ => Err(std::io::Error::new(
			std::io::ErrorKind::PermissionDenied,
			"elevated testd helper failed (cancelled, denied, invalid input, or system I/O error)",
		)),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn encoded_invocations_fit_the_windows_command_line_limit_without_embedding_the_source() {
		let command = command(
			Path::new(r"C:\Users\owner's home\helper"),
			"InstallRoot",
			&[("CertPath", r"C:\Users\owner's home\root.cer"), ("Fingerprint", &"a".repeat(64))],
		);
		assert!(command.contains("owner''s home"));
		assert!(command.contains("ReadAllBytes"));
		assert!(command.contains("source changed; no action was run"));
		assert!(!command.contains(SOURCE));
		assert!(encoded(&command).len() < 30_000);
	}

	#[test]
	fn encoded_commands_preserve_unicode_as_utf16_little_endian() {
		use base64::Engine;
		let source = "'owner’s helper 🧪'";
		let bytes = base64::engine::general_purpose::STANDARD.decode(encoded(source)).unwrap();
		let words: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
		assert_eq!(String::from_utf16(&words).unwrap(), source);
	}

	#[cfg(windows)]
	#[test]
	fn a_modified_staging_file_cannot_execute_an_action() {
		let mut temporary = testd_core::private_file::temporary_in(&std::env::temp_dir()).unwrap();
		use std::io::Write;
		temporary.write_all(b"throw 'fixture source executed'").unwrap();
		temporary.flush().unwrap();
		let output = super::super::silent_command(powershell().unwrap())
			.args([
				"-NoProfile",
				"-NonInteractive",
				"-EncodedCommand",
				&encoded(&command(temporary.path(), "RemoveNrpt", &[("Tld", "testdcheck")])),
			])
			.output()
			.unwrap();
		assert!(!output.status.success());
		let error = String::from_utf8_lossy(&output.stderr);
		assert!(error.contains("source changed"), "{error}");
		assert!(!error.contains("fixture source executed"), "{error}");
	}
}
