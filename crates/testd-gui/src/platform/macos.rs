use std::path::Path;

use anyhow::Context;
use objc2_app_kit::NSWorkspace;
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

fn launch_agent_path() -> anyhow::Result<std::path::PathBuf> {
	let home = std::env::var_os("HOME").context("HOME is not set")?;
	let directory = std::path::PathBuf::from(home).join("Library/LaunchAgents");
	crate::private::directory(&directory)?;
	Ok(directory.join("dev.testd.host.plist"))
}

pub fn check_autostart(executable: &Path) -> anyhow::Result<()> {
	super::launch_agent::check(&launch_agent_path()?, executable)
}

pub fn enable_autostart(executable: &Path) -> anyhow::Result<()> {
	super::launch_agent::create(&launch_agent_path()?, executable)
}

pub fn register_scheme(_executable: &Path) -> anyhow::Result<()> {
	/* Launch Services reads the scheme from the installed bundle's Info.plist. */
	Ok(())
}
