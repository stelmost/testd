use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::Path;

use anyhow::{Context, bail};
use windows_sys::Win32::UI::Shell::{ASSOCF_IS_PROTOCOL, ASSOCSTR_EXECUTABLE, AssocQueryStringW, ShellExecuteW};
use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW, SW_SHOWNORMAL};
use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

fn wide(value: &str) -> Vec<u16> {
	value.replace('\0', "�").encode_utf16().chain(Some(0)).collect()
}

pub fn open_browser(file_url: &str) -> anyhow::Result<()> {
	let scheme = wide("http");
	let verb = wide("open");
	let mut length = 0;
	/* SAFETY: the terminated inputs and output length remain valid for both calls. */
	unsafe {
		AssocQueryStringW(
			ASSOCF_IS_PROTOCOL,
			ASSOCSTR_EXECUTABLE,
			scheme.as_ptr(),
			verb.as_ptr(),
			std::ptr::null_mut(),
			&mut length,
		);
	}
	if length == 0 || length > 32768 {
		bail!("no default HTTP browser is configured");
	}
	let mut buffer = vec![0; length as usize];
	let result = unsafe {
		AssocQueryStringW(
			ASSOCF_IS_PROTOCOL,
			ASSOCSTR_EXECUTABLE,
			scheme.as_ptr(),
			verb.as_ptr(),
			buffer.as_mut_ptr(),
			&mut length,
		)
	};
	if result < 0 {
		bail!("the default HTTP browser lookup failed: HRESULT {result:#x}");
	}
	let end = buffer.iter().position(|&character| character == 0).unwrap_or(buffer.len());
	let executable = OsString::from_wide(&buffer[..end]);
	let executable = Path::new(&executable);
	if !executable.is_absolute()
		|| executable
			.extension()
			.is_none_or(|extension| !extension.eq_ignore_ascii_case("exe"))
	{
		bail!("the default HTTP handler does not identify a browser executable");
	}
	if file_url.contains('"') {
		bail!("invalid browser handoff URI");
	}
	let executable = wide(executable.to_str().context("the default browser path is not valid Unicode")?);
	let parameters = wide(&format!("\"{file_url}\""));
	/* SAFETY: all inputs are terminated and live through ShellExecuteW. No browser process is waited on. */
	let launched = unsafe {
		ShellExecuteW(
			std::ptr::null_mut(),
			verb.as_ptr(),
			executable.as_ptr(),
			parameters.as_ptr(),
			std::ptr::null(),
			SW_SHOWNORMAL,
		)
	};
	if launched as isize <= 32 {
		bail!("the default browser could not be launched: {}", launched as isize);
	}
	Ok(())
}

pub fn show_error(message: &str) {
	let message = wide(message);
	let title = wide("Testd");
	/* SAFETY: MessageBoxW reads the terminated strings only during this call. */
	unsafe {
		MessageBoxW(std::ptr::null_mut(), message.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR);
	}
}

pub fn check_autostart(executable: &Path) -> anyhow::Result<()> {
	let user = RegKey::predef(HKEY_CURRENT_USER);
	let key = match user.open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run") {
		Ok(key) => key,
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
		Err(error) => return Err(error.into()),
	};
	let existing: String = match key.get_value("dev.testd.host") {
		Ok(value) => value,
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
		Err(error) => return Err(error.into()),
	};
	let path = executable.to_str().context("invalid login executable path")?;
	if existing.trim_end() != path && existing.trim_end() != format!("\"{path}\"") {
		bail!("the existing login entry was changed or points elsewhere; left unchanged");
	}
	Ok(())
}

pub fn register_scheme(executable: &Path) -> anyhow::Result<()> {
	let command = format!("\"{}\" \"%1\"", executable.to_str().context("the URL-handler path is not valid Unicode")?);
	let user = RegKey::predef(HKEY_CURRENT_USER);
	let path = "Software\\Classes\\testd";
	let command_key = format!("{path}\\shell\\open\\command");
	match user.open_subkey(&command_key) {
		Ok(key) => {
			let existing: String = key.get_value("")?;
			if existing != command {
				bail!("the existing testd URL handler differs; it was left unchanged");
			}
			return Ok(());
		},
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
		Err(error) => return Err(error.into()),
	}
	match user.open_subkey(path) {
		Ok(_) => bail!("the existing testd URL registration is incomplete; left unchanged"),
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
		Err(error) => return Err(error.into()),
	}
	let (key, _) = user.create_subkey(path)?;
	key.set_value("", &"URL:dev.testd.host protocol")?;
	key.set_value("URL Protocol", &"")?;
	let (icon, _) = user.create_subkey(format!("{path}\\DefaultIcon"))?;
	icon.set_value("", &format!("\"{}\",0", executable.display()))?;
	let (key, _) = user.create_subkey(command_key)?;
	key.set_value("", &command)?;
	Ok(())
}
