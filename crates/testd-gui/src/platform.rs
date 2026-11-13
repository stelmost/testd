/*! Desktop integration without a webview or framework plugin. */

use std::path::PathBuf;

use anyhow::{Context, bail};
use auto_launch::{AutoLaunch, AutoLaunchBuilder};

#[cfg(target_os = "linux")]
#[path = "platform/linux.rs"]
mod native;
#[cfg(target_os = "macos")]
#[path = "platform/macos.rs"]
mod native;
#[cfg(windows)]
#[path = "platform/windows.rs"]
mod native;

#[cfg(any(target_os = "macos", all(test, unix)))]
#[path = "platform/launch_agent.rs"]
mod launch_agent;

pub use native::open_browser;
#[cfg(target_os = "linux")]
pub use native::show_error;

#[cfg(any(windows, target_os = "macos"))]
pub fn show_error(message: &str) -> anyhow::Result<()> {
	/* Startup can fail before Tao creates the macOS application object. */
	#[cfg(target_os = "macos")]
	let main_thread = objc2::MainThreadMarker::new().context("desktop diagnostics require the main thread")?;
	#[cfg(target_os = "macos")]
	let _application = objc2_app_kit::NSApplication::sharedApplication(main_thread);
	native_dialog::DialogBuilder::message()
		.set_title("Testd")
		.set_text(message.replace('\0', "�"))
		.set_level(native_dialog::MessageLevel::Error)
		.alert()
		.show()
		.context("could not display the native desktop diagnostic")
}

pub fn executable() -> anyhow::Result<PathBuf> {
	let current = std::env::current_exe()?;
	#[cfg(target_os = "linux")]
	if let Some(path) = std::env::var_os("APPIMAGE") {
		let directory = std::env::var_os("APPDIR").map(PathBuf::from);
		if directory.is_some_and(|directory| directory.is_absolute() && current.starts_with(directory)) {
			let path = PathBuf::from(path);
			if !path.is_absolute() || !path.is_file() {
				bail!("APPIMAGE must identify the installed executable, not a mounted inner binary");
			}
			return Ok(path);
		}
		eprintln!("[warn] inherited AppImage metadata does not belong to this executable; using the actual binary");
	}
	Ok(current)
}

fn autostart() -> anyhow::Result<AutoLaunch> {
	let path = executable()?;
	native::check_autostart(&path)?;
	let path = path.to_str().context("the executable path must be valid Unicode")?;
	if path.chars().any(char::is_control) {
		bail!("control characters are not supported in a login executable path");
	}
	#[cfg(target_os = "linux")]
	let path = desktop_exec(path);
	#[cfg(windows)]
	let path = format!("\"{path}\"");
	let mut builder = AutoLaunchBuilder::new();
	builder.set_app_name("dev.testd.host").set_app_path(&path);
	#[cfg(target_os = "macos")]
	builder.set_use_launch_agent(true);
	builder
		.build()
		.map_err(|error| anyhow::anyhow!("cannot configure launch at login: {error}"))
}

fn login_enabled(manager: &AutoLaunch) -> anyhow::Result<bool> {
	match manager.is_enabled() {
		Ok(enabled) => Ok(enabled),
		Err(auto_launch::Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
		Err(error) => Err(anyhow::anyhow!("cannot query launch at login: {error}")),
	}
}

pub fn autostart_enabled() -> anyhow::Result<bool> {
	login_enabled(&autostart()?)
}

fn remember_login_choice() -> anyhow::Result<()> {
	let marker = testd_core::config::config_dir().join("autostart.initialized");
	if !marker.try_exists()? {
		testd_core::config::write_private(&marker, b"1\n")?;
	}
	Ok(())
}

pub fn enable_autostart_once() -> anyhow::Result<bool> {
	let marker = testd_core::config::config_dir().join("autostart.initialized");
	if !marker.try_exists()? {
		return enable_autostart();
	}
	autostart_enabled()
}

pub fn enable_autostart() -> anyhow::Result<bool> {
	let manager = autostart()?;
	if !login_enabled(&manager)? {
		#[cfg(target_os = "macos")]
		native::enable_autostart(std::path::Path::new(manager.get_app_path()))
			.context("cannot enable launch at login")?;
		#[cfg(not(target_os = "macos"))]
		manager
			.enable()
			.map_err(|error| anyhow::anyhow!("cannot enable launch at login: {error}"))?;
	}
	remember_login_choice()?;
	login_enabled(&manager)
}

pub fn disable_autostart() -> anyhow::Result<bool> {
	let manager = autostart()?;
	match manager.disable() {
		Ok(()) => {},
		Err(auto_launch::Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {},
		Err(error) => return Err(anyhow::anyhow!("cannot disable launch at login: {error}")),
	}
	remember_login_choice()?;
	login_enabled(&manager)
}

pub fn register_scheme() -> anyhow::Result<()> {
	if cfg!(debug_assertions) && std::env::var_os("TESTD_REGISTER_SCHEME").is_none() {
		return Ok(());
	}
	native::register_scheme(&executable()?)
}

#[cfg(target_os = "linux")]
fn desktop_exec(path: &str) -> String {
	/* Exec quoting is followed by Desktop Entry string unescaping, not a shell. */
	let mut quoted = String::from("\"");
	for character in path.chars() {
		if matches!(character, '\\' | '"' | '`' | '$') {
			quoted.push('\\');
		}
		quoted.push(character);
		if character == '%' {
			quoted.push('%');
		}
	}
	quoted.push('"');
	quoted.replace('\\', "\\\\")
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
	use super::*;

	#[test]
	fn desktop_paths_round_trip_without_field_code_or_command_injection() {
		let path = "/home/me/a space/é\"`$\\%u.AppImage";
		let entry = format!("[Desktop Entry]\nType=Application\nName=Test\nExec={}\n", desktop_exec(path));
		let key = gtk::glib::KeyFile::new();
		key.load_from_data(&entry, gtk::glib::KeyFileFlags::NONE).unwrap();
		let command = key.string("Desktop Entry", "Exec").unwrap();
		let arguments = gtk::glib::shell_parse_argv(command).unwrap();
		assert_eq!(arguments.len(), 1);
		assert_eq!(arguments[0].to_str().unwrap().replace("%%", "%"), path);
	}
}
