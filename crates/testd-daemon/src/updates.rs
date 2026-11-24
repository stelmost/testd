/*! Optional release checks. Downloading and installing an update remains an explicit user action. */

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::sync::{Notify, watch};

pub const FEED_URL: &str = "https://testd.dev/update.json";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_FEED_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Release {
	pub version: String,
	pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Check {
	NotChecked,
	Checking,
	Current,
	Available { release: Release },
	Failed { message: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Status {
	pub enabled: bool,
	pub current_version: String,
	pub checked_at: Option<u64>,
	#[serde(flatten)]
	pub check: Check,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Preferences {
	enabled: bool,
}

pub struct Updates {
	path: PathBuf,
	preference_write: Mutex<()>,
	status: watch::Sender<Status>,
	requested: Notify,
}

impl Updates {
	pub fn load() -> Result<Self> {
		Self::load_from(testd_core::config::config_dir().join("updates.json"))
	}

	pub(crate) fn load_from(path: PathBuf) -> Result<Self> {
		let enabled = match testd_core::private_file::read(&path) {
			Ok(bytes) => {
				serde_json::from_slice::<Preferences>(&bytes)
					.with_context(|| format!("invalid update preferences in {}", path.display()))?
					.enabled
			},
			Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
			Err(error) => return Err(error).context("could not read update preferences"),
		};
		let (status, _) = watch::channel(Status {
			enabled,
			current_version: CURRENT_VERSION.into(),
			checked_at: None,
			check: Check::NotChecked,
		});
		Ok(Self { path, preference_write: Mutex::new(()), status, requested: Notify::new() })
	}

	pub fn status(&self) -> Status {
		self.status.borrow().clone()
	}

	pub fn changes(&self) -> watch::Receiver<Status> {
		self.status.subscribe()
	}

	pub fn set_enabled(&self, enabled: bool) -> Result<()> {
		let _guard = self.preference_write.lock().expect("update preferences poisoned");
		let bytes = serde_json::to_vec(&Preferences { enabled })?;
		testd_core::config::write_private(&self.path, &bytes).context("could not save update preferences")?;
		self.status.send_modify(|status| {
			status.enabled = enabled;
			if matches!(status.check, Check::Checking) {
				status.check = Check::NotChecked;
			}
		});
		Ok(())
	}

	pub fn request_check(&self) -> Result<()> {
		ensure!(self.status().enabled, "update checks are disabled; enable them before checking");
		self.requested.notify_one();
		Ok(())
	}

	pub(crate) fn start(self: &Arc<Self>) -> crate::task::BackgroundTask {
		let updates = Arc::clone(self);
		crate::task::BackgroundTask(tokio::spawn(async move { updates.run(check_release).await }))
	}

	async fn run<F, Request>(&self, fetch: F)
	where
		F: Fn() -> Request,
		Request: std::future::Future<Output = Result<Option<Release>>>,
	{
		let mut changes = self.changes();
		let mut next_check = tokio::time::Instant::now();
		loop {
			let enabled = self.status().enabled;
			tokio::select! {
				biased;
				result = changes.changed() => {
					if result.is_err() { return; }
					if changes.borrow_and_update().enabled != enabled {
						next_check = tokio::time::Instant::now();
					}
					continue;
				},
				_ = self.requested.notified(), if enabled => {},
				_ = tokio::time::sleep_until(next_check), if enabled => {},
			}
			if !self.begin_check(&mut changes) {
				continue;
			}
			let result = tokio::select! {
				biased;
				_ = changes.changed() => continue,
				result = fetch() => result,
			};
			self.finish(result);
			changes.borrow_and_update();
			next_check = tokio::time::Instant::now() + CHECK_INTERVAL;
		}
	}

	fn begin_check(&self, changes: &mut watch::Receiver<Status>) -> bool {
		/* Do not consume a disable notification between testing the preference and starting a request. */
		let _guard = self.preference_write.lock().expect("update preferences poisoned");
		if !self.status().enabled {
			return false;
		}
		self.status.send_modify(|status| status.check = Check::Checking);
		changes.borrow_and_update();
		true
	}

	fn finish(&self, result: Result<Option<Release>>) {
		let _guard = self.preference_write.lock().expect("update preferences poisoned");
		if !self.status().enabled {
			return;
		}
		self.status.send_modify(|status| {
			status.checked_at = SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|time| time.as_secs());
			status.check = match result {
				Ok(Some(release)) => {
					eprintln!("[update] testd {} is available: {}", release.version, release.url);
					Check::Available { release }
				},
				Ok(None) => Check::Current,
				Err(error) => {
					eprintln!("[update] could not check for updates: {error:#}");
					Check::Failed { message: format!("{error:#}") }
				},
			};
		});
	}
}

/** Check the public release feed without sending local configuration or an installation identifier. */
pub async fn check_release() -> Result<Option<Release>> {
	let bytes = tokio::time::timeout(CHECK_TIMEOUT, download_feed(&crate::caddy::curl_binary()))
		.await
		.context("update check timed out")??;
	parse_release(&bytes, CURRENT_VERSION)
}

async fn download_feed(binary: &Path) -> Result<Vec<u8>> {
	let mut command = tokio::process::Command::new(binary);
	command
		.args([
			"--disable",
			"--fail",
			"--silent",
			"--show-error",
			"--location",
			"--max-redirs",
			"3",
			"--proto",
			"=https",
			"--proto-redir",
			"=https",
			"--connect-timeout",
			"5",
			"--max-time",
			"15",
			"--max-filesize",
			"16384",
			"--user-agent",
			"testd-update-check",
			FEED_URL,
		])
		.stdin(Stdio::null())
		.stdout(Stdio::piped())
		.stderr(Stdio::null())
		.kill_on_drop(true);
	/* CREATE_NO_WINDOW keeps background checks out of the Windows desktop. */
	#[cfg(windows)]
	command.creation_flags(0x08000000);
	let mut child = command.spawn().context("could not start curl to check for updates")?;
	let mut bytes = Vec::new();
	child
		.stdout
		.take()
		.expect("piped update feed")
		.take(MAX_FEED_BYTES as u64 + 1)
		.read_to_end(&mut bytes)
		.await?;
	ensure!(bytes.len() <= MAX_FEED_BYTES, "the update response is too large");
	let status = child.wait().await.context("could not finish the update request")?;
	ensure!(status.success(), "could not fetch the release feed ({status}); check network access to {FEED_URL}");
	Ok(bytes)
}

fn parse_release(bytes: &[u8], current: &str) -> Result<Option<Release>> {
	#[derive(Deserialize)]
	struct Feed {
		version: String,
	}
	ensure!(bytes.len() <= MAX_FEED_BYTES, "the update response is too large");
	let feed: Feed = serde_json::from_slice(bytes).context("invalid update response")?;
	ensure!(feed.version.len() <= 80, "the published version is too long");
	let latest = semver::Version::parse(&feed.version).context("invalid published version")?;
	let current = semver::Version::parse(current).context("invalid installed version")?;
	if !latest.cmp_precedence(&current).is_gt() {
		return Ok(None);
	}
	Ok(Some(Release {
		url: format!("https://github.com/stelmost/testd/releases/tag/v{}", feed.version),
		version: feed.version,
	}))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn releases_use_semver_precedence_and_never_offer_downgrades() {
		let current = "0.1.0-alpha.2";
		for version in ["0.1.0-alpha.1", current, "0.1.0-alpha.2+rebuilt"] {
			let feed = serde_json::json!({ "version": version });
			assert_eq!(parse_release(&serde_json::to_vec(&feed).unwrap(), current).unwrap(), None);
		}
		for version in ["0.1.0-alpha.10", "0.1.0", "0.2.0"] {
			let feed = serde_json::json!({ "version": version, "url": "https://untrusted.example" });
			let release = parse_release(&serde_json::to_vec(&feed).unwrap(), current).unwrap().unwrap();
			assert_eq!(release.version, version);
			assert_eq!(release.url, format!("https://github.com/stelmost/testd/releases/tag/v{version}"));
		}
	}

	#[test]
	fn invalid_or_oversized_responses_are_errors() {
		for feed in [b"<html>not a feed</html>".as_slice(), br#"{"version":"../../other"}"#, br#"{}"#] {
			assert!(parse_release(feed, CURRENT_VERSION).is_err());
		}
		assert!(parse_release(&vec![b' '; MAX_FEED_BYTES + 1], CURRENT_VERSION).is_err());
	}

	#[test]
	fn disabling_survives_restart_and_prevents_manual_requests() {
		let directory = tempfile::tempdir().unwrap();
		let path = directory.path().join("updates.json");
		let updates = Updates::load_from(path.clone()).unwrap();
		assert!(updates.status().enabled);
		updates.set_enabled(false).unwrap();
		let restarted = Updates::load_from(path).unwrap();
		assert!(!restarted.status().enabled);
		assert!(restarted.request_check().is_err());
		assert_eq!(restarted.status().check, Check::NotChecked);
	}

	#[test]
	fn failed_preference_write_keeps_the_previous_setting() {
		let directory = tempfile::tempdir().unwrap();
		let path = directory.path().join("updates.json");
		let updates = Updates::load_from(path.clone()).unwrap();
		std::fs::create_dir(path).unwrap();
		assert!(updates.set_enabled(false).is_err());
		assert!(updates.status().enabled);
	}

	#[tokio::test]
	async fn disabled_checks_make_no_request_and_disabling_cancels_an_inflight_request() {
		let directory = tempfile::tempdir().unwrap();
		let updates = Arc::new(Updates::load_from(directory.path().join("updates.json")).unwrap());
		updates.set_enabled(false).unwrap();
		let started = Arc::new(Notify::new());
		let cancelled = Arc::new(Notify::new());
		struct InFlight(Arc<Notify>);
		impl Drop for InFlight {
			fn drop(&mut self) {
				self.0.notify_one();
			}
		}
		let worker = Arc::clone(&updates);
		let started_request = Arc::clone(&started);
		let cancelled_request = Arc::clone(&cancelled);
		let task = crate::task::BackgroundTask(tokio::spawn(async move {
			worker
				.run(|| async {
					let _inflight = InFlight(Arc::clone(&cancelled_request));
					started_request.notify_one();
					std::future::pending().await
				})
				.await;
		}));
		assert!(
			tokio::time::timeout(Duration::from_millis(50), started.notified())
				.await
				.is_err()
		);
		updates.set_enabled(true).unwrap();
		tokio::time::timeout(Duration::from_secs(2), started.notified()).await.unwrap();
		assert_eq!(updates.status().check, Check::Checking);
		updates.set_enabled(false).unwrap();
		tokio::time::timeout(Duration::from_secs(2), cancelled.notified())
			.await
			.unwrap();
		assert_eq!(updates.status().check, Check::NotChecked);
		assert!(
			tokio::time::timeout(Duration::from_millis(50), started.notified())
				.await
				.is_err()
		);
		drop(task);
	}
}
