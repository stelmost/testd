/*! Run only the audited helper, using the system PowerShell and a verified in-memory source snapshot. */

use std::path::Path;

const SOURCE: &str = include_str!("../../../scripts/testd-elevated.ps1");
#[cfg(windows)]
const COMMAND_FLAGS: [&str; 3] = ["-NoProfile", "-NonInteractive", "-Command"];

pub(super) fn quote(value: &str) -> String {
	let mut literal = String::with_capacity(value.len() + 2);
	literal.push('\'');
	for character in value.chars() {
		/* PowerShell accepts curly single quotes as delimiters too. */
		if matches!(character, '\'' | '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}') {
			literal.push(character);
		}
		literal.push(character);
	}
	literal.push('\'');
	literal
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

/* Start-Process takes a Windows command line, with backslash escaping before double quotes. */
fn windows_argument(value: &str) -> String {
	let mut argument = String::from("\"");
	let mut backslashes = 0;
	for character in value.chars() {
		match character {
			'\\' => {
				backslashes += 1;
				continue;
			},
			'"' => backslashes = backslashes * 2 + 1,
			_ => {},
		}
		argument.extend(std::iter::repeat_n('\\', backslashes));
		argument.push(character);
		backslashes = 0;
	}
	argument.extend(std::iter::repeat_n('\\', backslashes * 2));
	argument.push('"');
	argument
}

#[cfg(windows)]
fn elevation_command(powershell: &Path, script: &str) -> String {
	let arguments = format!("{} {}", COMMAND_FLAGS.join(" "), windows_argument(script));
	format!(
		"$ErrorActionPreference = 'Stop'; \
		$p = Start-Process -FilePath {} -Verb RunAs -Wait -PassThru -ArgumentList {}; \
		exit $p.ExitCode",
		quote(&powershell.to_string_lossy()),
		quote(&arguments),
	)
}

#[cfg(windows)]
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
		.args(COMMAND_FLAGS)
		.arg(command)
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
	if action == "RemoveInstallation" && owner.is_none() {
		return Err(std::io::Error::new(
			std::io::ErrorKind::NotFound,
			"private installation ownership ID is missing; restore it before retrying uninstall",
		));
	}
	let owner = owner.map(|owner| owner.fingerprint()).unwrap_or_default();
	let mut args = args.to_vec();
	args.push(("Owner", &owner));
	let mut helper = testd_core::private_file::temporary_in(&std::env::temp_dir())?;
	helper.write_all(SOURCE.as_bytes())?;
	helper.as_file().sync_all()?;
	/* Windows readers cannot open the file while its writer is still open. */
	let helper = helper.into_temp_path();
	let script = command(&helper, action, &args);
	let powershell = powershell()?;
	let script = if is_elevated(&powershell) { script } else { elevation_command(&powershell, &script) };
	let status = silent_command(&powershell).args(COMMAND_FLAGS).arg(script).status()?;
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
	fn plaintext_invocation_names_the_action_and_verifies_a_separate_source_file() {
		let command = command(
			Path::new(r"C:\Users\owner's home\helper"),
			"InstallRoot",
			&[("CertPath", r"C:\Users\owner's home\root.cer"), ("Fingerprint", &"a".repeat(64))],
		);
		assert!(command.contains("owner''s home"));
		assert!(command.contains("ReadAllBytes"));
		assert!(command.contains("source changed; no action was run"));
		assert!(command.contains("-Action 'InstallRoot'"));
		assert!(!command.contains(SOURCE));
	}

	#[test]
	fn windows_argument_preserves_backslashes_and_embedded_quotes() {
		assert_eq!(windows_argument(r#"C:\a b\"c\"#), r#""C:\a b\\\"c\\""#);
	}

	#[cfg(windows)]
	#[test]
	fn start_process_passes_plaintext_and_preserves_exit_codes() {
		let powershell = powershell().unwrap();
		let value = "C:\\owner's 🧪\u{2018}\u{2019}\u{201a}\u{201b}\\\"leaf\"\\";
		let script = format!(
			"$value = {}; \
			$expected = 'C:\\owner''s 🧪' + [char]0x2018 + [char]0x2019 + [char]0x201a + [char]0x201b + \
			'\\' + [char]34 + 'leaf' + [char]34 + '\\'; \
			if ($value -ceq $expected) {{ exit 17 }}; exit 1",
			quote(value),
		);
		/* Test the launcher without opening a UAC prompt. */
		let launch = elevation_command(&powershell, &script).replacen(" -Verb RunAs", "", 1);
		let status = super::super::silent_command(powershell)
			.args(COMMAND_FLAGS)
			.arg(launch)
			.status()
			.unwrap();
		assert_eq!(status.code(), Some(17));
	}

	#[cfg(windows)]
	#[test]
	fn a_modified_staging_file_cannot_execute_an_action() {
		let mut temporary = testd_core::private_file::temporary_in(&std::env::temp_dir()).unwrap();
		use std::io::Write;
		temporary.write_all(b"throw 'fixture source executed'").unwrap();
		temporary.as_file().sync_all().unwrap();
		let temporary = temporary.into_temp_path();
		let output = super::super::silent_command(powershell().unwrap())
			.args(COMMAND_FLAGS)
			.arg(command(&temporary, "RemoveNrpt", &[("Tld", "testdcheck")]))
			.output()
			.unwrap();
		assert!(!output.status.success());
		let error = String::from_utf8_lossy(&output.stderr);
		assert!(error.contains("source changed"), "{error}");
		assert!(!error.contains("fixture source executed"), "{error}");
	}

	#[cfg(windows)]
	#[test]
	fn an_unchanged_staging_file_is_readable_without_elevation() {
		use std::io::Write;

		let mut temporary = testd_core::private_file::temporary_in(&std::env::temp_dir()).unwrap();
		temporary.write_all(SOURCE.as_bytes()).unwrap();
		temporary.as_file().sync_all().unwrap();
		let temporary = temporary.into_temp_path();
		/* ValidateSet rejects this action before the helper can inspect or modify system state. */
		let output = super::super::silent_command(powershell().unwrap())
			.args(COMMAND_FLAGS)
			.arg(command(&temporary, "InvalidFixtureAction", &[]))
			.output()
			.unwrap();
		assert!(!output.status.success());
		let error = String::from_utf8_lossy(&output.stderr);
		assert!(error.contains("InvalidFixtureAction"), "{error}");
		assert!(!error.contains("source changed"), "{error}");
	}
}
