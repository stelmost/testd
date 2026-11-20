use std::path::Path;

use anyhow::{Context, bail};
use gio::prelude::*;
use gtk::prelude::*;

pub fn open_browser(file_url: &str) -> anyhow::Result<()> {
	let browser = gio::AppInfo::default_for_uri_scheme("http").context("no default HTTP browser is configured")?;
	browser.launch_uris(&[file_url], Some(&browser_context()))?;
	Ok(())
}

fn browser_context() -> gio::AppLaunchContext {
	let context = gio::AppLaunchContext::new();
	for (variable, original) in testd_core::appimage::host_environment() {
		restore_browser_variable(&context, &variable, original);
	}
	context
}

fn restore_browser_variable(
	context: &gio::AppLaunchContext,
	variable: &std::ffi::OsStr,
	original: Option<std::ffi::OsString>,
) {
	match original {
		Some(value) => context.setenv(variable, value),
		None => context.unsetenv(variable),
	}
}

pub fn show_error(message: &str) -> anyhow::Result<()> {
	gtk::init().context("cannot initialize GTK for the desktop diagnostic")?;
	let dialog = gtk::MessageDialog::builder()
		.message_type(gtk::MessageType::Error)
		.buttons(gtk::ButtonsType::Ok)
		.text("Testd")
		.secondary_text(message.replace('\0', "�"))
		.build();
	dialog.run();
	dialog.close();
	Ok(())
}

fn login_entry() -> std::path::PathBuf {
	testd_core::config::config_dir()
		.parent()
		.unwrap()
		.join("autostart/dev.testd.host.desktop")
}

pub fn check_autostart(executable: &Path) -> anyhow::Result<()> {
	let entry = login_entry();
	crate::private::directory(entry.parent().unwrap())?;
	check_login_entry(&entry, executable)
}

pub fn enable_autostart(command: &str) -> anyhow::Result<()> {
	write_login_entry(&login_entry(), command)
}

fn write_login_entry(path: &Path, command: &str) -> anyhow::Result<()> {
	use std::io::Write;
	crate::private::directory(path.parent().unwrap())?;
	let mut file = testd_core::private_file::create_new(path)?;
	write!(
		file,
		"[Desktop Entry]\nType=Application\nVersion=1.0\nName=dev.testd.host\n\
		Comment=dev.testd.hoststartup script\nExec={command}\nStartupNotify=false\nTerminal=false\n"
	)?;
	file.sync_all()?;
	Ok(())
}

fn check_login_entry(path: &Path, executable: &Path) -> anyhow::Result<()> {
	let Some(data) = crate::private::read_entry(path)? else {
		return Ok(());
	};
	let entry = gtk::glib::KeyFile::new();
	entry.load_from_data(std::str::from_utf8(&data)?, gtk::glib::KeyFileFlags::NONE)?;
	let expected = gtk::glib::KeyFile::new();
	let raw = executable.to_str().context("invalid login executable path")?;
	expected.load_from_data(
		&format!("[Desktop Entry]\nExec={}\n", super::desktop_exec(raw)),
		gtk::glib::KeyFileFlags::NONE,
	)?;
	let command = entry.string("Desktop Entry", "Exec")?;
	let keys = entry.keys("Desktop Entry")?;
	let allowed = ["Type", "Version", "Name", "Comment", "Exec", "StartupNotify", "Terminal"];
	if keys.len() != allowed.len()
		|| keys.iter().any(|key| !allowed.contains(&key.as_str()))
		|| entry.string("Desktop Entry", "Name")? != "dev.testd.host"
		|| entry.string("Desktop Entry", "Type")? != "Application"
		|| entry.string("Desktop Entry", "Version")? != "1.0"
		|| entry.string("Desktop Entry", "Comment")? != "dev.testd.hoststartup script"
		|| entry.string("Desktop Entry", "StartupNotify")? != "false"
		|| entry.string("Desktop Entry", "Terminal")? != "false"
		|| (command.trim_end() != expected.string("Desktop Entry", "Exec")?.as_str() && command.trim_end() != raw)
	{
		bail!("the existing login entry was changed or points elsewhere; left unchanged");
	}
	Ok(())
}

pub fn register_scheme(executable: &Path) -> anyhow::Result<()> {
	let name = "testd-gui-handler.desktop";
	if let Some(current) = gio::AppInfo::default_for_uri_scheme("testd") {
		/* Reuse compatible installed/legacy handlers without adopting or rewriting them. */
		let registered = current.executable();
		let registered =
			if registered.is_absolute() { Some(registered) } else { gtk::glib::find_program_in_path(registered) };
		if registered.as_deref() == Some(executable) {
			return Ok(());
		}
		/* A bundled AppImage entry is transient; register its persistent wrapper instead. */
		let is_own_transient_image = registered.as_deref() == Some(std::env::current_exe()?.as_path());
		if current.id().as_deref() != Some(name) && !is_own_transient_image {
			bail!("another testd URL handler is configured; it was left unchanged");
		}
	}
	let path = executable.to_str().context("the URL-handler path must be valid Unicode")?;
	if path.chars().any(char::is_control) {
		bail!("control characters are not supported in a URL-handler path");
	}
	let directory = gtk::glib::user_data_dir().join("applications");
	crate::private::directory(&directory)?;
	let path = directory.join(name);
	let content = format!(
		"[Desktop Entry]\nType=Application\nName=Testd\nTerminal=false\n\
		Exec={} %u\nMimeType=x-scheme-handler/testd;\n",
		super::desktop_exec(executable.to_str().unwrap()),
	);
	match std::fs::symlink_metadata(&path) {
		Ok(metadata) if metadata.is_file() => {
			if std::fs::read_to_string(&path)? != content {
				bail!("the existing testd desktop handler differs; it was left unchanged");
			}
		},
		Ok(_) => bail!("the existing testd desktop handler is not an ordinary file; left unchanged"),
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
			use std::io::Write;
			let mut file = testd_core::private_file::create_new(&path)?;
			file.write_all(content.as_bytes())?;
			file.sync_all()?;
		},
		Err(error) => return Err(error.into()),
	}
	gio::DesktopAppInfo::from_filename(&path)
		.context("the URL-handler desktop entry could not be loaded")?
		.set_as_default_for_type("x-scheme-handler/testd")?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn browser_environment_restoration_never_mutates_the_tray_process() {
		let variable = "TESTD_BROWSER_ENVIRONMENT_FIXTURE";
		let before = std::env::var_os(variable);
		let context = gio::AppLaunchContext::new();
		context.setenv(variable, "packaged");
		restore_browser_variable(&context, variable.as_ref(), Some("host".into()));
		assert!(
			context
				.environment()
				.iter()
				.any(|entry| entry == "TESTD_BROWSER_ENVIRONMENT_FIXTURE=host")
		);
		restore_browser_variable(&context, variable.as_ref(), None);
		assert!(
			!context
				.environment()
				.iter()
				.any(|entry| entry.to_string_lossy().starts_with(variable))
		);
		assert_eq!(std::env::var_os(variable), before);
	}

	#[test]
	fn new_login_entries_are_private_valid_and_never_replace_existing_state() {
		use std::os::unix::fs::{MetadataExt, symlink};
		let home = crate::private::test_directory();
		let entry = home.path().join("autostart/login.desktop");
		let executable = Path::new("/home/me/a space/é%u.AppImage");
		let command = crate::platform::desktop_exec(executable.to_str().unwrap());
		write_login_entry(&entry, &command).unwrap();
		assert_eq!(std::fs::metadata(&entry).unwrap().mode() & 0o777, 0o600);
		check_login_entry(&entry, executable).unwrap();
		let original = std::fs::read(&entry).unwrap();
		assert!(write_login_entry(&entry, "/bin/sh").is_err());
		assert_eq!(std::fs::read(&entry).unwrap(), original);
		std::fs::rename(&entry, home.path().join("retained")).unwrap();
		symlink(home.path().join("retained"), &entry).unwrap();
		assert!(write_login_entry(&entry, &command).is_err());
		assert_eq!(std::fs::read(home.path().join("retained")).unwrap(), original);
	}

	#[test]
	fn login_validation_accepts_legacy_entries_but_preserves_changed_entries() {
		let home = tempfile::tempdir().unwrap();
		let entry = home.path().join("login.desktop");
		let path = "/home/me/a space/Testd%u.AppImage";
		for executable in [path.to_owned(), crate::platform::desktop_exec(path), "/bin/sh".into()] {
			let content = format!(
				"[Desktop Entry]\nType=Application\nVersion=1.0\nName=dev.testd.host\n\
				Comment=dev.testd.hoststartup script\nExec={executable} \nStartupNotify=false\nTerminal=false"
			);
			testd_core::config::write_private(&entry, content.as_bytes()).unwrap();
			let result = check_login_entry(&entry, Path::new(path));
			assert_eq!(result.is_ok(), executable != "/bin/sh");
			assert_eq!(std::fs::read_to_string(&entry).unwrap(), content);
		}
	}
}
