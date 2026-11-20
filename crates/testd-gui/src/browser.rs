/*! Browser launchers receive a private file URL, never a bearer credential. */

use std::fs::File;
use std::io::{Read, Write};
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

/* The caller must hold the tray lock so no other live handoff is removed. */
pub fn cleanup(directory: &Path) -> anyhow::Result<()> {
	crate::private::directory(directory)?;
	for entry in std::fs::read_dir(directory)? {
		let entry = entry?;
		let name = entry.file_name();
		let Some(name) = name.to_str() else { continue };
		if !name.starts_with("browser-") || !name.ends_with(".html") {
			continue;
		}
		let path = entry.path();
		let file = crate::private::open(&path)?;
		let mut page = String::new();
		(&file).take(4097).read_to_string(&mut page)?;
		let Some((_, destination)) = page.rsplit_once("<script>location.replace(") else { continue };
		let Some(destination) = destination.strip_suffix(");</script></html>\n") else { continue };
		let Ok(destination) = serde_json::from_str::<String>(destination) else { continue };
		if handoff_page(&destination).ok().as_deref() == Some(page.as_str()) {
			crate::private::remove_owned(&path, &file)?;
		}
	}
	Ok(())
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
	/* Never give a library's launcher or diagnostics a credential-bearing destination. */
	let uri = Url::parse(file_url).map_err(|_| anyhow::anyhow!("invalid browser handoff URI"))?;
	if uri.scheme() != "file"
		|| !uri.username().is_empty()
		|| uri.password().is_some()
		|| uri.query().is_some()
		|| uri.fragment().is_some()
		|| file_url.contains('"')
	{
		bail!("the browser launcher requires a credential-free file URI");
	}
	/* Select the HTTP handler: the default HTML file handler may be an editor. */
	crate::platform::open_browser(uri.as_str()).context("could not open the default browser; use the tray to retry")
}

#[cfg(test)]
mod tests {
	use super::*;

	const TOKEN: &str = "fixture-owner-credential";

	#[test]
	fn launch_rejects_credentials_before_invoking_any_platform_library() {
		for input in [
			format!("http://127.0.0.1/#token={TOKEN}"),
			format!("https://foreign.example/{TOKEN}"),
			format!("file:///private/handoff.html#token={TOKEN}"),
			format!("file:///private/handoff.html?token={TOKEN}"),
			format!("javascript:{TOKEN}"),
			format!("file:///private/\"{TOKEN}.html"),
		] {
			let error = open(&input).unwrap_err().to_string();
			assert!(!error.contains(TOKEN));
		}
	}

	#[test]
	fn launch_argument_has_no_credential_and_the_owned_file_is_removed() {
		let home = crate::private::test_directory();
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
	fn cleanup_removes_only_verified_private_handoffs() {
		let home = crate::private::test_directory();
		let handoff = Handoff::new(home.path(), &format!("http://127.0.0.1:7737/#token={TOKEN}")).unwrap();
		let path = handoff.path.clone();
		let unrelated = home.path().join("browser-unrelated.html");
		let _file = testd_core::private_file::create_new(&unrelated).unwrap();
		std::fs::write(&unrelated, "keep this private document").unwrap();
		cleanup(home.path()).unwrap();
		assert!(!path.exists());
		assert_eq!(std::fs::read_to_string(unrelated).unwrap(), "keep this private document");
		drop(handoff);
	}

	#[cfg(unix)]
	#[test]
	fn cleanup_refuses_symlinks_shared_files_and_hard_links() {
		use std::os::unix::fs::{PermissionsExt, symlink};
		for kind in ["symlink", "shared", "hard-link"] {
			let home = crate::private::test_directory();
			let original = home.path().join("retained");
			let _file = testd_core::private_file::create_new(&original).unwrap();
			let page = handoff_page(&format!("http://127.0.0.1:7737/#token={TOKEN}")).unwrap();
			std::fs::write(&original, &page).unwrap();
			let path = home.path().join("browser-unsafe.html");
			match kind {
				"symlink" => symlink(&original, &path).unwrap(),
				"hard-link" => std::fs::hard_link(&original, &path).unwrap(),
				_ => {
					std::fs::copy(&original, &path).unwrap();
					std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
				},
			}
			assert!(cleanup(home.path()).is_err());
			assert!(path.exists());
			assert_eq!(std::fs::read_to_string(original).unwrap(), page);
		}
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
