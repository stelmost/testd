/*! Browser launchers receive a private file URL, never a bearer credential. */

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use url::Url;

pub struct Handoff {
	path: PathBuf,
	file: File,
	file_url: Url,
}

impl Handoff {
	pub fn new(directory: &Path, destination: &str) -> anyhow::Result<Self> {
		let page = handoff_page(destination)?;
		crate::private::directory(directory)?;
		let name = testd_core::config::generate_operator_secret()?;
		let path = directory.join(format!("browser-{name}.html"));
		let file_url = Url::from_file_path(&path).map_err(|_| anyhow::anyhow!("invalid browser handoff path"))?;
		let file = testd_core::private_file::create_new(&path)?;
		let mut handoff = Self { path, file, file_url };
		handoff.file.write_all(page.as_bytes())?;
		handoff.file.sync_all()?;
		Ok(handoff)
	}

	pub fn file_url(&self) -> String {
		self.file_url.to_string()
	}
}

impl Drop for Handoff {
	fn drop(&mut self) {
		if let Err(error) = crate::private::remove_owned(&self.path, &self.file) {
			eprintln!("[warn] browser handoff cleanup failed: {error}");
		}
	}
}

fn handoff_page(destination: &str) -> anyhow::Result<String> {
	/* URL parser diagnostics may include their input, which here is a credential. */
	let url = Url::parse(destination).map_err(|_| anyhow::anyhow!("invalid local browser destination"))?;
	let is_loopback = matches!(url.host(), Some(url::Host::Ipv4(ip)) if ip.is_loopback())
		|| matches!(url.host(), Some(url::Host::Ipv6(ip)) if ip.is_loopback());
	let token = url.fragment().and_then(|fragment| fragment.strip_prefix("token="));
	if url.scheme() != "http"
		|| !is_loopback
		|| !url.username().is_empty()
		|| url.password().is_some()
		|| url.path() != "/"
		|| url.query().is_some()
		|| !token.is_some_and(|token| {
			token.len() >= 16
				&& token
					.bytes()
					.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~'))
		}) {
		bail!("browser bootstrap requires an authenticated numeric loopback URL");
	}
	let destination = serde_json::to_string(url.as_str())?;
	Ok(format!(
		"<!doctype html><html lang='en'><meta charset='utf-8'>\n\
		<meta name='referrer' content='no-referrer'>\n\
		<meta http-equiv='Content-Security-Policy' content=\"default-src 'none'; script-src 'unsafe-inline'; base-uri 'none'\">\n\
		<title>Opening testd</title><p>Opening testd. If this page does not continue, reopen it from the tray.</p>\n\
		<script>location.replace({destination});</script></html>\n"
	))
}

pub fn open(file_url: &str) -> anyhow::Result<()> {
	/* Select the HTTP handler: the default HTML file handler may be an editor. */
	crate::platform::open_browser(file_url).context("could not open the default browser; use the tray to retry")
}

#[cfg(test)]
mod tests {
	use super::*;

	const TOKEN: &str = "fixture-owner-credential";

	#[test]
	fn launch_argument_has_no_credential_and_the_owned_file_is_removed() {
		let home = tempfile::tempdir().unwrap();
		let destination = format!("http://127.0.0.1:7737/#token={TOKEN}");
		let handoff = Handoff::new(home.path(), &destination).unwrap();
		assert!(!handoff.file_url().contains(TOKEN));
		assert!(handoff.file_url().starts_with("file://"));
		let page = String::from_utf8(std::fs::read(&handoff.path).unwrap()).unwrap();
		assert!(page.contains(&format!("location.replace(\"{destination}\")")));
		assert!(page.contains("no-referrer"));
		let path = handoff.path.clone();
		drop(handoff);
		assert!(!path.exists());
	}

	#[test]
	fn bootstrap_refuses_remote_malformed_or_scriptable_destinations_without_echoing_credentials() {
		for destination in [
			format!("https://127.0.0.1/#token={TOKEN}"),
			format!("http://foreign.example/#token={TOKEN}"),
			format!("http://localhost/#token={TOKEN}"),
			format!("http://127.0.0.1/?token={TOKEN}"),
			format!("http://user@127.0.0.1/#token={TOKEN}"),
			format!("http://127.0.0.1/#token={TOKEN}</script>"),
			format!("javascript:{TOKEN}"),
			"http://127.0.0.1/#token=short".into(),
		] {
			let error = handoff_page(&destination).unwrap_err().to_string();
			assert!(!error.contains(TOKEN));
		}
		assert!(handoff_page(&format!("http://[::1]:7737/#token={TOKEN}")).is_ok());
	}
}
