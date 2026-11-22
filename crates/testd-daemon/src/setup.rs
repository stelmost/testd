//! First-run setup on an ephemeral loopback port.
//!
//! This is deliberately not part of the dashboard: without a config there is no
//! dashboard, and the setup surface must not be reachable at the dashboard name.
//! The tray opens its page and starts the daemon once the page asks to finish.

use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use testd_core::config::{
	CaddyDetection, DEFAULT_DNS_PORT, PendingConfig, TLD, config_dir, default_http_addr, detect_caddy_config,
	finish_pending_config, generate_operator_secret, read_operator_secret, read_pending_config, write_pending_config,
};

use crate::assets;
use crate::certificate::{self, CertificateStatus};
use crate::http::{header_value, read_body, read_http_line, respond, respond_json};

/// A custom header cannot be sent by a cross-site form and forces a CORS
/// preflight, which this server does not grant.
const REQUEST_HEADER: &str = "x-testd-request";
const MAX_HEAD: usize = 16 * 1024;
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// Shortest operator secret the setup page accepts; generated ones are longer.
const MIN_SECRET_LEN: usize = 16;

/// A running setup surface.
pub struct Setup {
	/// Loopback URL the protected browser handoff should open.
	pub url: String,
	/// The same address as `url`, for tests and callers that need it.
	pub addr: SocketAddr,
	finish: tokio::sync::mpsc::UnboundedReceiver<Finish>,
	state: Arc<State>,
	task: Option<crate::task::BackgroundTask<io::Result<()>>>,
}

impl Setup {
	/// Wait for startup, or report a failed listener rather than leaving a dead setup page.
	pub async fn next_finish(&mut self) -> Result<Finish, String> {
		let task = self.task.as_mut().ok_or_else(|| "the setup server has stopped".to_owned())?;
		tokio::select! {
			biased;
			finish = self.finish.recv() => {
				let finish = finish.ok_or_else(|| "the setup channel closed".to_owned())?;
				if *self.state.stop.borrow() {
					let _ = finish.complete(Err("the setup server is stopping".into())).await;
					return Err("the setup server stopped unexpectedly".into());
				}
				Ok(finish)
			},
			result = &mut task.0 => {
				self.task.take();
				Err(match result {
					Ok(Ok(())) => "the setup server stopped unexpectedly".into(),
					Ok(Err(error)) => format!("setup listener failed: {error}"),
					Err(error) => format!("setup server task failed: {error}"),
				})
			},
		}
	}

	/// Stop admission and account for accepted operations before releasing the desktop instance guard.
	pub async fn shutdown(&mut self) -> io::Result<()> {
		let _ = self.state.stop.send(true);
		self.finish.close();
		let mut failures = Vec::new();
		while let Some(finish) = self.finish.recv().await {
			if let Err(error) = finish.complete(Err("setup was stopped before host startup".into())).await {
				failures.push(error.to_string());
			}
		}
		if let Some(mut task) = self.task.take() {
			match (&mut task.0).await {
				Ok(Ok(())) => {},
				Ok(Err(error)) => failures.push(error.to_string()),
				Err(error) => failures.push(error.to_string()),
			}
		}
		if failures.is_empty() { Ok(()) } else { Err(io::Error::other(failures.join("; "))) }
	}
}

/// One startup request, retained until its HTTP response has finished writing.
pub struct Finish {
	reply: tokio::sync::oneshot::Sender<Result<String, String>>,
	delivered: tokio::sync::oneshot::Receiver<()>,
}

impl Finish {
	pub async fn complete(self, result: Result<String, String>) -> io::Result<()> {
		self.reply
			.send(result)
			.map_err(|_| io::Error::other("the setup response is no longer waiting"))?;
		self.delivered
			.await
			.map_err(|_| io::Error::other("the setup response did not complete"))
	}
}

#[derive(PartialEq, Eq)]
enum FinishState {
	Editing,
	Complete,
}

struct State {
	token: String,
	stop: tokio::sync::watch::Sender<bool>,
	finish: tokio::sync::mpsc::UnboundedSender<Finish>,
	finish_state: tokio::sync::Mutex<FinishState>,
}

impl State {
	fn begin_change(&self) -> Result<tokio::sync::MutexGuard<'_, FinishState>, &'static str> {
		if *self.stop.borrow() {
			return Err("the setup server is stopping");
		}
		let state = self
			.finish_state
			.try_lock()
			.map_err(|_| "another setup change is in progress")?;
		if *state == FinishState::Complete {
			return Err("setup has already completed; open the dashboard from the tray");
		}
		Ok(state)
	}
}

/// Bind the setup listener on an ephemeral loopback port and start serving.
pub fn start(rt: &tokio::runtime::Handle) -> io::Result<Setup> {
	let _entered = rt.enter();
	let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
	listener.set_nonblocking(true)?;
	let listener = TcpListener::from_std(listener)?;
	let addr = listener.local_addr()?;
	let (finish, finish_rx) = tokio::sync::mpsc::unbounded_channel();
	let (stop, stop_rx) = tokio::sync::watch::channel(false);
	let token = testd_core::config::generate_operator_secret()?;
	let url = format!("http://{addr}/#token={token}");
	let state = Arc::new(State { token, stop, finish, finish_state: tokio::sync::Mutex::new(FinishState::Editing) });
	let task = crate::task::BackgroundTask(rt.spawn(serve(listener, Arc::clone(&state), stop_rx)));
	Ok(Setup { url, addr, finish: finish_rx, state, task: Some(task) })
}

async fn serve(
	listener: TcpListener,
	state: Arc<State>,
	mut stop: tokio::sync::watch::Receiver<bool>,
) -> io::Result<()> {
	let mut requests = tokio::task::JoinSet::new();
	let mut failure = None;
	loop {
		tokio::select! {
			biased;
			_ = stop.changed() => break,
			accepted = listener.accept() => match accepted {
				Ok((stream, peer)) if peer.ip().is_loopback() && requests.len() < 64 => {
					let state = Arc::clone(&state);
					requests.spawn(async move {
						if let Err(error) = handle(stream, state).await { eprintln!("[setup] {error}"); }
					});
				},
				Ok(_) => {},
				Err(error) => { failure = Some(error); break; },
			},
			result = requests.join_next(), if !requests.is_empty() => {
				if let Some(Err(error)) = result { failure = Some(io::Error::other(error)); break; }
			},
		}
	}
	drop(listener);
	let _ = state.stop.send(true);
	while let Some(result) = requests.join_next().await {
		if let Err(error) = result {
			failure.get_or_insert_with(|| io::Error::other(error));
		}
	}
	match failure {
		Some(error) => Err(error),
		None => Ok(()),
	}
}

async fn handle(stream: TcpStream, state: Arc<State>) -> io::Result<()> {
	let (reader, mut writer) = tokio::io::split(stream);
	let mut reader = BufReader::new(reader);

	let deadline = tokio::time::Instant::now() + REQUEST_TIMEOUT;
	let Some(request_line) = read_http_line(&mut reader, 8 * 1024, deadline).await? else { return Ok(()) };
	let mut parts = request_line.split_whitespace();
	let method = parts.next().unwrap_or("").to_owned();
	let target = parts.next().unwrap_or("/").to_owned();

	let mut headers: Vec<(String, String)> = Vec::new();
	let mut remaining = MAX_HEAD.saturating_sub(request_line.len());
	loop {
		let line = read_http_line(&mut reader, remaining, deadline)
			.await?
			.ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "incomplete HTTP headers"))?;
		remaining -= line.len();
		let line = line.trim_end();
		if line.is_empty() {
			break;
		}
		if let Some((name, value)) = line.split_once(':') {
			headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
		}
	}

	// The setup surface must stay a loopback page, not a rebinding target.
	if !is_loopback_host(header_value(&headers, "host")) {
		return respond(&mut writer, "403 Forbidden", "text/plain", b"Loopback Host required.").await;
	}
	if method != "GET" && method != "POST" {
		return respond(&mut writer, "405 Method Not Allowed", "text/plain", b"Method Not Allowed").await;
	}
	if method == "POST" && header_value(&headers, REQUEST_HEADER) != Some("1") {
		return respond(&mut writer, "403 Forbidden", "text/plain", b"A setup request header is required.").await;
	}

	let path = target.split('?').next().unwrap_or("/").to_owned();
	let is_public = method == "GET" && matches!(path.as_str(), "/" | "/dashboard.css");
	if !is_public && !crate::local_auth::authorized(&headers, Some(&state.token)) {
		return respond_json(
			&mut writer,
			"401 Unauthorized",
			&serde_json::json!({
				"error": "Open setup from the testd tray to authorize this session."
			}),
		)
		.await;
	}
	let body = if method == "POST" {
		tokio::time::timeout(REQUEST_TIMEOUT, read_body(&mut reader, &headers))
			.await
			.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "HTTP body timed out"))??
	} else {
		Vec::new()
	};

	let _change = if method == "POST" && path != "/api/setup/finish" {
		match state.begin_change() {
			Ok(guard) => Some(guard),
			Err(error) => {
				return respond_json(&mut writer, "409 Conflict", &serde_json::json!({ "error": error })).await;
			},
		}
	} else {
		None
	};

	match (method.as_str(), path.as_str()) {
		("GET", "/") => {
			let page = assets::load_page(assets::SETUP_HTML);
			respond(&mut writer, "200 OK", "text/html; charset=utf-8", page.as_bytes()).await
		},
		("GET", "/dashboard.css") => {
			let css = assets::load(assets::DASHBOARD_CSS);
			respond(&mut writer, "200 OK", "text/css; charset=utf-8", css.as_bytes()).await
		},
		("GET", "/api/setup/state") => {
			let status = setup_state().await;
			respond_json(&mut writer, "200 OK", &status).await
		},
		("POST", "/api/setup/resolver") => handle_resolver(&mut writer).await,
		("POST", "/api/setup/certificate") => handle_certificate(&mut writer).await,
		("POST", "/api/setup/secret") => handle_secret(&mut writer, &body).await,
		("POST", "/api/setup/finish") => handle_finish(&mut writer, &state, &body).await,
		_ => respond(&mut writer, "404 Not Found", "text/plain", b"Not Found").await,
	}
}

fn is_loopback_host(host: Option<&str>) -> bool {
	let Some(host) = host else { return false };
	let name = match host.strip_prefix('[') {
		Some(rest) => rest.split(']').next().unwrap_or(""),
		None => host.split(':').next().unwrap_or(""),
	};
	matches!(name, "localhost" | "127.0.0.1" | "::1")
}

fn resolver_addr() -> SocketAddr {
	SocketAddr::from(([127, 0, 0, 1], DEFAULT_DNS_PORT))
}

#[derive(Serialize)]
struct SetupState {
	resolver: bool,
	certificate: Option<CertificateStatus>,
}

async fn setup_state() -> SetupState {
	let resolver = tokio::task::spawn_blocking(|| testd_setup::is_os_resolver_configured(TLD, resolver_addr()))
		.await
		.unwrap_or(false);
	let status = tokio::task::spawn_blocking(|| certificate::status(&pki_dir()))
		.await
		.ok()
		.and_then(Result::ok);
	SetupState { resolver, certificate: status }
}

async fn handle_resolver(writer: &mut (impl AsyncWriteExt + Unpin)) -> io::Result<()> {
	let result = tokio::task::spawn_blocking(|| {
		let addr = resolver_addr();
		testd_setup::setup_os_resolver_gui(TLD, addr)?;
		if !testd_setup::is_os_resolver_configured(TLD, addr) {
			return Err(io::Error::other("could not verify the system DNS route after setup"));
		}
		Ok(())
	})
	.await;
	match result {
		Ok(Ok(())) => respond_json(writer, "200 OK", &serde_json::json!({ "ok": true })).await,
		Ok(Err(e)) => {
			respond_json(writer, "500 Internal Server Error", &serde_json::json!({ "error": e.to_string() })).await
		},
		Err(e) => {
			respond_json(writer, "500 Internal Server Error", &serde_json::json!({ "error": e.to_string() })).await
		},
	}
}

async fn handle_certificate(writer: &mut (impl AsyncWriteExt + Unpin)) -> io::Result<()> {
	match tokio::task::spawn_blocking(|| certificate::install(&pki_dir())).await {
		Ok(Ok(status)) => respond_json(writer, "200 OK", &status).await,
		Ok(Err(e)) => respond_json(writer, "500 Internal Server Error", &serde_json::json!({ "error": e })).await,
		Err(e) => {
			respond_json(writer, "500 Internal Server Error", &serde_json::json!({ "error": e.to_string() })).await
		},
	}
}

#[derive(Deserialize, Default)]
struct SecretRequest {
	#[serde(default)]
	secret: String,
	#[serde(default)]
	regenerate: bool,
}

async fn handle_secret(writer: &mut (impl AsyncWriteExt + Unpin), body: &[u8]) -> io::Result<()> {
	let request: SecretRequest = serde_json::from_slice(body).unwrap_or_default();
	let mut pending = match pending_or_default() {
		Ok(pending) => pending,
		Err(error) => {
			return respond_json(
				writer,
				"500 Internal Server Error",
				&serde_json::json!({ "error": error.to_string() }),
			)
			.await;
		},
	};
	if !request.secret.is_empty() {
		match validate_operator_secret(&request.secret) {
			Ok(secret) => pending.secret = secret,
			Err(e) => return respond_json(writer, "400 Bad Request", &serde_json::json!({ "error": e })).await,
		}
	} else if request.regenerate || pending.secret.is_empty() {
		let existing = if request.regenerate {
			None
		} else {
			match read_operator_secret() {
				Ok(secret) => secret,
				Err(error) => {
					return respond_json(
						writer,
						"500 Internal Server Error",
						&serde_json::json!({ "error": error.to_string() }),
					)
					.await;
				},
			}
		};
		pending.secret = match existing {
			Some(secret) => secret,
			None => match generate_operator_secret() {
				Ok(secret) => secret,
				Err(e) => {
					return respond_json(
						writer,
						"500 Internal Server Error",
						&serde_json::json!({ "error": e.to_string() }),
					)
					.await;
				},
			},
		};
	}
	if let Err(e) = write_pending_config(&pending) {
		return respond_json(writer, "500 Internal Server Error", &serde_json::json!({ "error": e.to_string() })).await;
	}
	respond_json(writer, "200 OK", &serde_json::json!({ "secret": pending.secret })).await
}

#[derive(Deserialize)]
struct FinishBody {
	#[serde(default)]
	secret: String,
}

async fn handle_finish(writer: &mut (impl AsyncWriteExt + Unpin), state: &State, body: &[u8]) -> io::Result<()> {
	let mut finish_state = match state.begin_change() {
		Ok(guard) => guard,
		Err(error) => return respond_json(writer, "409 Conflict", &serde_json::json!({ "error": error })).await,
	};
	let request: FinishBody = serde_json::from_slice(body).unwrap_or(FinishBody { secret: String::new() });
	let secret = match validate_operator_secret(&request.secret) {
		Ok(secret) => secret,
		Err(e) => return respond_json(writer, "400 Bad Request", &serde_json::json!({ "error": e })).await,
	};

	let prepare_secret = secret.clone();
	match tokio::task::spawn_blocking(move || prepare_finish(&prepare_secret)).await {
		Ok(Ok(())) => {},
		Ok(Err(error)) => {
			return respond_json(
				writer,
				"500 Internal Server Error",
				&serde_json::json!({ "error": error.to_string() }),
			)
			.await;
		},
		Err(error) => {
			return respond_json(
				writer,
				"500 Internal Server Error",
				&serde_json::json!({ "error": error.to_string() }),
			)
			.await;
		},
	}

	let (reply, reply_rx) = tokio::sync::oneshot::channel();
	let (delivery, delivered) = tokio::sync::oneshot::channel();
	if state.finish.send(Finish { reply, delivered }).is_err() {
		let error = match rollback_config(&secret) {
			Ok(()) => "the setup owner is gone".to_owned(),
			Err(recovery) => format!("the setup owner is gone; recovery is incomplete: {recovery}"),
		};
		return respond_json(writer, "503 Service Unavailable", &serde_json::json!({ "error": error })).await;
	}

	let response = match reply_rx.await {
		Ok(Ok(url)) => {
			*finish_state = FinishState::Complete;
			let response = respond_json(writer, "200 OK", &serde_json::json!({ "url": url })).await;
			let _ = state.stop.send(true);
			response
		},
		Ok(Err(error)) => {
			let error = match rollback_config(&secret) {
				Ok(()) => error,
				Err(recovery) => format!("{error}; setup recovery is incomplete: {recovery}"),
			};
			respond_json(writer, "500 Internal Server Error", &serde_json::json!({ "error": error })).await
		},
		Err(_) => {
			respond_json(writer, "503 Service Unavailable", &serde_json::json!({ "error": "the setup owner is gone" }))
				.await
		},
	};
	let _ = delivery.send(());
	response
}

fn prepare_finish(secret: &str) -> io::Result<()> {
	let mut pending = pending_or_default()?;
	pending.step = 3;
	pending.caddy_config_path = configure_external_front();
	pending.secret = secret.to_owned();
	finish_pending_config(&pending)?;
	Ok(())
}

/// A config that the daemon cannot start with would strand the next launch in a
/// failed start. Move it aside and keep the setup state, so a restart resumes.
fn rollback_config(secret: &str) -> Result<(), String> {
	let backup = backup_failed_config(&testd_core::config::config_path());
	let pending = PendingConfig { step: 2, caddy_config_path: String::new(), secret: secret.to_owned() };
	let pending = write_pending_config(&pending);
	match (backup, pending) {
		(Ok(()), Ok(())) => Ok(()),
		(Err(error), Ok(())) => Err(format!("cannot retain failed config: {error}")),
		(Ok(()), Err(error)) => Err(format!("cannot restore pending setup: {error}")),
		(Err(backup), Err(pending)) => Err(format!("cannot retain failed config: {backup}; pending setup: {pending}")),
	}
}

fn backup_failed_config(config: &Path) -> io::Result<()> {
	match std::fs::symlink_metadata(config) {
		Ok(metadata) if metadata.is_file() => {},
		Ok(_) => return Err(io::Error::other("failed config is linked or changed; left untouched")),
		Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
		Err(error) => return Err(error),
	}
	let mut original = testd_core::private_file::open(config)?;
	let mut data = Vec::new();
	std::io::Read::read_to_end(&mut original, &mut data)?;
	let name = format!("config.toml.failed-{}", generate_operator_secret()?);
	let backup = config.with_file_name(name);
	let mut file = testd_core::private_file::create_new(&backup)?;
	std::io::Write::write_all(&mut file, &data)?;
	file.sync_all()?;
	if !std::fs::symlink_metadata(config)?.is_file()
		|| !testd_core::private_file::same_file(&original, &testd_core::private_file::open(config)?)?
	{
		return Err(io::Error::other(
			"config changed during recovery; backup retained and current config left untouched",
		));
	}
	std::fs::remove_file(config)
}

fn pki_dir() -> std::path::PathBuf {
	config_dir().join("pki")
}

fn pending_or_default() -> io::Result<PendingConfig> {
	pending_or_default_from(read_pending_config())
}

fn pending_or_default_from(result: io::Result<PendingConfig>) -> io::Result<PendingConfig> {
	match result {
		Ok(pending) => Ok(pending),
		Err(error) if error.kind() == io::ErrorKind::NotFound => {
			Ok(PendingConfig { step: 0, caddy_config_path: String::new(), secret: String::new() })
		},
		Err(error) => Err(error),
	}
}

fn validate_operator_secret(secret: &str) -> Result<String, String> {
	let secret = secret.trim().to_owned();
	if secret.len() < MIN_SECRET_LEN {
		return Err(format!("the operator secret must be at least {MIN_SECRET_LEN} characters"));
	}
	if !secret
		.chars()
		.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~'))
	{
		return Err("the operator secret may use ASCII letters, digits, and - _ . ~".into());
	}
	Ok(secret)
}

/// Configure an external Caddy front when one is installed.
///
/// The built-in edge needs no Caddy, so this is best-effort: a failure leaves
/// the path empty instead of blocking setup. Mirrors the CLI, which only
/// touches a Caddyfile that already exists.
fn configure_external_front() -> String {
	let CaddyDetection::Found { path } = detect_caddy_config() else {
		return String::new();
	};

	let managed = config_dir().join("Caddyfile");
	let content = crate::edge::caddyfile(TLD, default_http_addr());
	if let Err(e) = write_managed_caddyfile(&managed, &content) {
		eprintln!("[warn]   could not write {}: {e}", managed.display());
		return String::new();
	}
	if let Err(e) = testd_setup::write_caddyfile_import_gui(Path::new(&path), &managed) {
		eprintln!("[warn]   could not update {path}: {e}");
		return String::new();
	}
	path
}

fn write_managed_caddyfile(path: &Path, content: &str) -> io::Result<()> {
	if let Some(parent) = path.parent() {
		std::fs::create_dir_all(parent)?;
	}
	std::fs::write(path, content)
}

#[cfg(test)]
mod tests {
	use super::*;
	use tokio::io::{AsyncReadExt, AsyncWriteExt};

	async fn http(addr: SocketAddr, request: &str) -> String {
		let mut stream = TcpStream::connect(addr).await.unwrap();
		stream.write_all(request.as_bytes()).await.unwrap();
		let mut out = Vec::new();
		stream.read_to_end(&mut out).await.unwrap();
		String::from_utf8_lossy(&out).into_owned()
	}

	fn get(path: &str, host: &str) -> String {
		format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n")
	}

	#[tokio::test]
	async fn the_setup_page_is_loopback_only() {
		let setup = start(&tokio::runtime::Handle::current()).unwrap();
		let addr = setup.addr;

		let page = http(addr, &get("/", "127.0.0.1")).await;
		assert!(page.starts_with("HTTP/1.1 200"), "{page}");
		assert!(page.contains("Set up testd"), "{page}");
		assert!(page.contains("/dashboard.css"), "{page}");

		let css = http(addr, &get("/dashboard.css", "localhost")).await;
		assert!(css.starts_with("HTTP/1.1 200"), "{css}");
		assert!(css.contains(":root"), "{css}");

		// A rebinding page uses its own name; the setup surface refuses it.
		let foreign = http(addr, &get("/", "evil.test")).await;
		assert!(foreign.starts_with("HTTP/1.1 403"), "{foreign}");
	}

	#[tokio::test]
	async fn dropping_setup_releases_its_listener() {
		let setup = start(&tokio::runtime::Handle::current()).unwrap();
		let address = setup.addr;
		drop(setup);
		tokio::time::timeout(std::time::Duration::from_secs(1), async {
			loop {
				if TcpListener::bind(address).await.is_ok() {
					break;
				}
				tokio::task::yield_now().await;
			}
		})
		.await
		.unwrap();
	}

	#[test]
	fn inaccessible_or_corrupt_pending_setup_is_not_replaced() {
		assert_eq!(
			pending_or_default_from(Err(io::Error::from(io::ErrorKind::NotFound)))
				.unwrap()
				.step,
			0
		);
		for kind in [io::ErrorKind::PermissionDenied, io::ErrorKind::InvalidData] {
			assert_eq!(pending_or_default_from(Err(io::Error::from(kind))).unwrap_err().kind(), kind);
		}
	}

	#[tokio::test]
	async fn setup_credentials_are_required_and_do_not_carry_to_another_session() {
		let first = start(&tokio::runtime::Handle::current()).unwrap();
		let second = start(&tokio::runtime::Handle::current()).unwrap();
		let first_token = first.url.split("#token=").nth(1).unwrap();
		let second_token = second.url.split("#token=").nth(1).unwrap();
		assert_ne!(first_token, second_token);
		let request = |token: &str| {
			format!("GET /api/setup/unknown HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\n\r\n")
		};
		assert!(http(second.addr, &request(first_token)).await.starts_with("HTTP/1.1 401"));
		assert!(http(second.addr, &request(second_token)).await.starts_with("HTTP/1.1 404"));
		let unauthorized = "POST /api/setup/secret HTTP/1.1\r\nHost: 127.0.0.1\r\n\
			X-Testd-Request: 1\r\nContent-Length: 2\r\n\r\n{}";
		assert!(http(second.addr, unauthorized).await.starts_with("HTTP/1.1 401"));
	}

	#[tokio::test]
	async fn all_setup_mutations_are_excluded_during_a_transaction() {
		let setup = start(&tokio::runtime::Handle::current()).unwrap();
		let _change = setup.state.begin_change().unwrap();
		let token = setup.url.split("#token=").nth(1).unwrap();
		let request = format!(
			"POST /api/setup/unknown HTTP/1.1\r\nHost: 127.0.0.1\r\n\
			Authorization: Bearer {token}\r\nX-Testd-Request: 1\r\nContent-Length: 2\r\n\r\n{{}}"
		);
		let response = http(setup.addr, &request).await;
		assert!(response.starts_with("HTTP/1.1 409"));
	}

	#[tokio::test]
	async fn shutdown_rejects_pending_startup_and_waits_for_its_response() {
		let mut setup = start(&tokio::runtime::Handle::current()).unwrap();
		let address = setup.addr;
		let (reply, result) = tokio::sync::oneshot::channel();
		let (delivery, delivered) = tokio::sync::oneshot::channel();
		assert!(setup.state.finish.send(Finish { reply, delivered }).is_ok());
		let task = tokio::spawn(async move { setup.shutdown().await });
		assert!(result.await.unwrap().is_err());
		assert!(!task.is_finished());
		delivery.send(()).unwrap();
		task.await.unwrap().unwrap();
		assert!(TcpListener::bind(address).await.is_ok());
	}

	#[tokio::test]
	async fn setup_owner_retains_the_surface_until_the_response_is_accounted_for() {
		let (reply, result) = tokio::sync::oneshot::channel();
		let (delivery, delivered) = tokio::sync::oneshot::channel();
		let task = tokio::spawn(Finish { reply, delivered }.complete(Ok("fixture dashboard".into())));
		assert_eq!(result.await.unwrap().unwrap(), "fixture dashboard");
		assert!(!task.is_finished());
		delivery.send(()).unwrap();
		task.await.unwrap().unwrap();
	}

	#[tokio::test]
	async fn concurrent_or_completed_finishes_cannot_change_configuration() {
		let (stop, _) = tokio::sync::watch::channel(false);
		let (finish, _) = tokio::sync::mpsc::unbounded_channel();
		let state = State {
			token: "fixture".into(),
			stop,
			finish,
			finish_state: tokio::sync::Mutex::new(FinishState::Editing),
		};
		let mut guard = state.begin_change().unwrap();
		let mut response = Vec::new();
		handle_finish(&mut response, &state, b"{}").await.unwrap();
		assert!(response.starts_with(b"HTTP/1.1 409"));
		*guard = FinishState::Complete;
		drop(guard);
		assert!(state.begin_change().is_err());
		*state.finish_state.lock().await = FinishState::Editing;
		assert!(state.begin_change().is_ok());
	}

	#[cfg(unix)]
	#[test]
	fn failed_config_recovery_preserves_links_and_their_targets() {
		let directory = tempfile::tempdir().unwrap();
		let target = directory.path().join("other-config");
		std::fs::write(&target, b"other state").unwrap();
		let config = directory.path().join("config.toml");
		std::os::unix::fs::symlink(&target, &config).unwrap();
		assert!(backup_failed_config(&config).is_err());
		assert!(std::fs::symlink_metadata(&config).unwrap().is_symlink());
		assert_eq!(std::fs::read(target).unwrap(), b"other state");
	}

	#[test]
	fn failed_config_backups_never_replace_previous_recovery_data() {
		let directory = tempfile::tempdir().unwrap();
		let config = directory.path().join("config.toml");
		let previous = directory.path().join("config.toml.failed");
		testd_core::config::write_private(&config, b"new failed config").unwrap();
		std::fs::write(&previous, b"old recovery data").unwrap();
		backup_failed_config(&config).unwrap();
		assert!(!config.exists());
		assert_eq!(std::fs::read(previous).unwrap(), b"old recovery data");
		let backup = std::fs::read_dir(directory.path())
			.unwrap()
			.map(|entry| entry.unwrap().path())
			.find(|path| path.file_name().unwrap().to_string_lossy().starts_with("config.toml.failed-"))
			.unwrap();
		assert_eq!(testd_core::private_file::read(&backup).unwrap(), b"new failed config");
	}

	#[tokio::test]
	async fn setup_mutations_need_the_request_header() {
		let setup = start(&tokio::runtime::Handle::current()).unwrap();
		let request = "POST /api/setup/resolver HTTP/1.1\r\nHost: 127.0.0.1\r\n\
			Content-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
		let response = http(setup.addr, request).await;
		assert!(response.starts_with("HTTP/1.1 403"), "{response}");
	}
}
