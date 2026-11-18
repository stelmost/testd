use std::path::Path;

use anyhow::{Context, bail};
use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

pub fn open_browser(file_url: &str) -> anyhow::Result<()> {
	webbrowser::open(file_url).context("could not launch the default HTTP browser")
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
