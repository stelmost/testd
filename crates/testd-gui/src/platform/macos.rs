use std::path::Path;

use anyhow::Context;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSAlert, NSWorkspace};
use objc2_foundation::{NSString, NSURL};

pub fn open_browser(file_url: &str) -> anyhow::Result<()> {
	let http = NSURL::URLWithString(&NSString::from_str("http://127.0.0.1/"))
		.context("could not construct the HTTP handler lookup")?;
	let application = NSWorkspace::sharedWorkspace()
		.URLForApplicationToOpenURL(&http)
		.context("no default HTTP browser is configured")?;
	let path = application.path().context("the default browser has no application path")?;
	open::with(file_url, path.to_string())?;
	Ok(())
}

pub fn show_error(message: &str) {
	let Some(main_thread) = MainThreadMarker::new() else {
		return;
	};
	let _application = objc2_app_kit::NSApplication::sharedApplication(main_thread);
	let alert = NSAlert::new(main_thread);
	alert.setMessageText(&NSString::from_str("Testd"));
	alert.setInformativeText(&NSString::from_str(message));
	alert.runModal();
}

pub fn check_autostart(executable: &Path) -> anyhow::Result<()> {
	let home = std::env::var_os("HOME").context("HOME is not set")?;
	let directory = std::path::PathBuf::from(home).join("Library/LaunchAgents");
	crate::private::directory(&directory)?;
	let Some(data) = crate::private::read_entry(&directory.join("dev.testd.host.plist"))? else {
		return Ok(());
	};
	let value = plist::Value::from_reader(std::io::Cursor::new(data))?;
	let entry = value
		.as_dictionary()
		.context("the existing login entry is not a dictionary; left unchanged")?;
	anyhow::ensure!(
		entry.len() == 3
			&& entry.get("Label").and_then(plist::Value::as_string) == Some("dev.testd.host")
			&& entry.get("RunAtLoad").and_then(plist::Value::as_boolean) == Some(true)
			&& entry
				.get("ProgramArguments")
				.and_then(plist::Value::as_array)
				.is_some_and(|arguments| arguments.len() == 1 && arguments[0].as_string() == executable.to_str()),
		"the existing login entry was changed or points elsewhere; left unchanged"
	);
	Ok(())
}

pub fn register_scheme(_executable: &Path) -> anyhow::Result<()> {
	/* Launch Services reads the scheme from the installed bundle's Info.plist. */
	Ok(())
}
