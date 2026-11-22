//! Dev-side client: connects to the host daemon, sends requests.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufRead, AsyncRead, AsyncWrite, BufReader};
#[cfg(test)]
use tokio::net::TcpStream;
use tokio::sync::{Notify, mpsc};

use testd_core::config::ClientConfig;
use testd_core::protocol::{self, Auth, ErrorDisposition, Request, Response, TokenRequest};

pub mod listeners;
pub mod ssh;
pub mod supervisor;
mod process;
mod forwarder;
mod transport;

pub use transport::{Pairing, begin_pairing};
use transport::Writer;

const RECONNECT_DELAY: Duration = Duration::from_secs(3);
/// A host that does not answer must not stall a run or a failover attempt.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(130);

type Reader = BufReader<Box<dyn AsyncRead + Unpin + Send>>;

/// The ordered reconnect list.
fn hosts(config: &ClientConfig) -> Vec<String> {
	config.hosts.clone()
}

/// Connect to one host daemon and authenticate.
///
/// With no credential for the profile, the host is asked for one instead. The
/// approved token is saved before the connection is used, so the next run does
/// not ask again.
async fn connect_host(config: &mut ClientConfig, host: &str, names: &[String]) -> Result<(Reader, Writer)> {
	let (mut reader, mut writer) = transport::open(host).await.map_err(|error| {
		let may_retry = error.downcast_ref::<std::io::Error>().is_some_and(|error| {
			!matches!(error.kind(), std::io::ErrorKind::InvalidData | std::io::ErrorKind::PermissionDenied)
		});
		if may_retry {
			anyhow::Error::new(ConnectFailure::unreachable(error))
		} else {
			anyhow::Error::new(ConnectFailure::rejected(error))
		}
	})?;
	match authenticate(config, host, &mut reader, &mut writer.inner, names).await {
		Ok(()) => Ok((reader, writer)),
		Err(error) => {
			let failure = if error.downcast_ref::<std::io::Error>().is_some() {
				ConnectFailure::unreachable(error)
			} else {
				ConnectFailure::rejected(error)
			};
			Err(anyhow::Error::new(failure))
		},
	}
}

/// Present a token, or wait for the host's operator to approve a request.
async fn authenticate(
	config: &mut ClientConfig,
	host: &str,
	reader: &mut Reader,
	writer: &mut (impl AsyncWrite + Unpin),
	names: &[String],
) -> Result<()> {
	let asking = config.token.is_empty();
	let request_token = asking.then(|| TokenRequest { label: client_label(), names: names.to_vec() });
	let auth = Auth { token: config.token.clone(), request_token, pairing: None };
	tokio::time::timeout(RESPONSE_TIMEOUT, protocol::send(writer, &auth))
		.await
		.map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "authentication write timed out"))??;

	let response = read_response(reader, RESPONSE_TIMEOUT).await?;

	match response {
		Response::Ok => {},
		Response::Pending { id } => {
			eprintln!(
				"[testd] [auth]  no credential for profile {:?}; requested access from {host} (request {id})",
				config.token_profile
			);
			eprintln!("[testd] [auth]  approve it on the node with `testd requests` and `testd approve {id}`");
			let response = read_response(reader, APPROVAL_TIMEOUT).await?;
			match response {
				Response::Token { token } => {
					testd_core::config::write_credential(&config.token_profile, &token).with_context(|| {
						format!("failed to save the credential for profile {:?}", config.token_profile)
					})?;
					eprintln!(
						"[testd] [auth]  token issued; saved to {}",
						testd_core::config::credential_path(&config.token_profile).display()
					);
					config.token = token;
				},
				Response::Error { message, .. } => bail!("access request failed: {message}"),
				_ => bail!("unexpected response to the access request"),
			}
		},
		Response::Error { message, .. } => bail!(
			"auth failed: {message}\nif the credential is stale, remove {} (or unset TESTD_TOKEN) and run again",
			testd_core::config::credential_path(&config.token_profile).display()
		),
		Response::Token { .. } | Response::Paired | Response::AppliedWithError { .. } => {
			bail!("unexpected credential, pairing, or claim response during auth")
		},
		Response::Stopped { .. }
		| Response::NameRevoked { .. }
		| Response::GrantRevoked
		| Response::PendingGrant { .. } => bail!("the host stopped this connection during auth"),
	}

	Ok(())
}

async fn read_response<R: AsyncBufRead + Unpin>(reader: &mut R, timeout: Duration) -> Result<Response> {
	tokio::time::timeout(timeout, protocol::recv(reader))
		.await
		.map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "host response timed out"))??
		.ok_or_else(|| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "host closed the connection").into())
}

/// The client's identity as shown to the operator deciding a request.
fn client_label() -> String {
	let user = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_default();
	format_label(&user, &host_name())
}

/// `user@host`, or whichever half is known.
fn format_label(user: &str, host: &str) -> String {
	match (user.trim(), host.trim()) {
		("", "") => "unknown client".into(),
		("", host) => host.into(),
		(user, "") => user.into(),
		(user, host) => format!("{user}@{host}"),
	}
}

/// The machine's name. Shells set `HOSTNAME` without exporting it, so the
/// environment often lacks it; ask the kernel when it does.
fn host_name() -> String {
	for variable in ["HOSTNAME", "COMPUTERNAME"] {
		if let Ok(name) = std::env::var(variable)
			&& !name.trim().is_empty()
		{
			return name;
		}
	}
	kernel_host_name().unwrap_or_default()
}

#[cfg(unix)]
fn kernel_host_name() -> Option<String> {
	let mut buffer = [0u8; 256];
	// SAFETY: the buffer is valid for its length, and gethostname writes at
	// most that many bytes.
	if unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } != 0 {
		return None;
	}
	let end = buffer.iter().position(|&byte| byte == 0).unwrap_or(buffer.len());
	String::from_utf8(buffer[..end].to_vec())
		.ok()
		.filter(|name| !name.trim().is_empty())
}

#[cfg(not(unix))]
fn kernel_host_name() -> Option<String> {
	None
}

/// Why one host could not be used. A host that answered but refused us is not
/// worth asking again on another host, which would queue a second request.
#[derive(Debug)]
struct ConnectFailure {
	error: anyhow::Error,
	retry: bool,
}

impl ConnectFailure {
	fn unreachable(error: anyhow::Error) -> Self {
		Self { error, retry: true }
	}

	fn rejected(error: anyhow::Error) -> Self {
		Self { error, retry: false }
	}

	/// Whether asking again could succeed. A host that refused us said no.
	pub(crate) fn is_retryable(&self) -> bool {
		self.retry
	}
}

impl std::fmt::Display for ConnectFailure {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "{}", self.error)
	}
}

impl std::error::Error for ConnectFailure {
	fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
		self.error.chain().nth(1)
	}
}

/// Try every host once, starting at `start`.
async fn connect_from(config: &mut ClientConfig, start: usize, names: &[String]) -> Result<(Reader, Writer, usize)> {
	let hosts = hosts(config);
	let mut last_error = None;
	for offset in 0..hosts.len() {
		let index = (start + offset) % hosts.len();
		match connect_host(config, &hosts[index], names).await {
			Ok((reader, writer)) => {
				eprintln!("[testd] [host]  {}", hosts[index]);
				return Ok((reader, writer, index));
			},
			Err(e) => {
				let failure = match e.downcast::<ConnectFailure>() {
					Ok(failure) => failure,
					Err(error) => ConnectFailure::unreachable(error),
				};
				if !failure.retry {
					return Err(anyhow::Error::new(failure));
				}
				last_error = Some(failure.error);
			},
		}
	}
	Err(last_error.unwrap_or_else(|| anyhow::anyhow!("no hosts configured")))
}

/// Connect to the first reachable host and authenticate.
pub(crate) async fn connect(config: &mut ClientConfig, names: &[String]) -> Result<(Reader, Writer)> {
	let (reader, writer, _) = connect_from(config, 0, names).await?;
	Ok((reader, writer))
}

/// The names a leaf will ask the host to grant, as the client spells them.
pub(crate) fn entry_names(entries: &[Entry]) -> Vec<String> {
	entries.iter().map(|entry| entry.domain.clone()).collect()
}

/// Send a request and wait for its outcome. A registration can wait on a
/// pending name grant, so the reply is not always the first frame back.
async fn request(reader: &mut Reader, writer: &mut Writer, req: &Request) -> Result<()> {
	tokio::time::timeout(RESPONSE_TIMEOUT, writer.send(req))
		.await
		.map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "request write timed out"))?
		.map_err(|error| {
			if error.is::<transport::PreparationError>() {
				anyhow::Error::new(ConnectFailure::rejected(error))
			} else {
				error
			}
		})?;
	let mut timeout = RESPONSE_TIMEOUT;
	loop {
		let response = read_response(reader, timeout).await?;
		if !matches!(response, Response::PendingGrant { .. }) {
			writer.finish(&response);
		}
		match response {
			Response::Ok => return Ok(()),
			Response::AppliedWithError { message } => bail!("host error after applying the claim: {message}"),
			Response::PendingGrant { id, domain } => {
				timeout = APPROVAL_TIMEOUT;
				eprintln!("[testd] [grant] the host is waiting for approval to grant {domain} (request {id})");
			},
			Response::Error { message, disposition } => {
				let error = anyhow::anyhow!("host error: {message}");
				return if disposition == Some(ErrorDisposition::Terminal) {
					Err(anyhow::Error::new(ConnectFailure::rejected(error)))
				} else {
					Err(error)
				};
			},
			other => bail!("host returned an unexpected response: {other:?}"),
		}
	}
}

/// Connect, register all entries, and return the connection for monitoring.
async fn connect_and_register(
	entries: &[Entry],
	config: &mut ClientConfig,
	force: bool,
	start: usize,
) -> Result<(Reader, Writer, usize)> {
	let (mut reader, mut writer, index) = connect_from(config, start, &entry_names(entries)).await?;
	for entry in entries {
		request(
			&mut reader,
			&mut writer,
			&Request::Register { domain: entry.domain.clone(), target: entry.target.clone(), force },
		)
		.await?;
		eprintln!("[testd] [reg]   {}", entry.domain);
	}
	Ok((reader, writer, index))
}

/// Whether the host name `app.example.test` is `published` as `app.example`.
/// Clients do not know the host's TLD, so one extra label is accepted.
pub(crate) fn is_published_name(published: &str, host_name: &str) -> bool {
	if published == host_name {
		return true;
	}
	match host_name.strip_prefix(published).and_then(|rest| rest.strip_prefix('.')) {
		Some(tld) => !tld.is_empty() && !tld.contains('.'),
		None => false,
	}
}

/// How the registration loop ended.
enum LoopEnd {
	/// The caller asked it to stop. The writer is handed back when the
	/// connection was up, for a graceful deregistration, together with the
	/// names that are still published.
	Stopped { writer: Option<Writer>, entries: Vec<Entry> },
	/// The host rejected a registration or authentication without retry.
	Failed(anyhow::Error),
	/// An operator stopped this leaf. The grant is still valid, so a later run
	/// publishes the names again without asking.
	StoppedByHost,
	/// An operator deleted this leaf's grant. The stored credential is gone.
	GrantRevoked,
	/// Every granted name was deleted one by one; nothing is left to publish.
	Revoked,
}

/// Maintain registration with automatic reconnect. Runs until `stop` fires or
/// an operator stops the leaf or deletes its grant.
async fn maintain_registration(
	mut entries: Vec<Entry>,
	mut config: ClientConfig,
	stop: Arc<Notify>,
	force: bool,
) -> LoopEnd {
	let mut retrying = false;
	let mut next_host = 0usize;
	let count = hosts(&config).len();

	loop {
		// Stop must also cancel a connection that is waiting for token approval.
		let connection = tokio::select! {
			result = connect_and_register(&entries, &mut config, force, next_host) => result,
			() = stop.notified() => return LoopEnd::Stopped { writer: None, entries },
		};
		let (reader, mut writer, index) = match connection {
			Ok(conn) => {
				retrying = false;
				conn
			},
			Err(e) => {
				// Definitive rejections must not retry or queue another approval request.
				if e.downcast_ref::<ConnectFailure>().is_some_and(|failure| !failure.retry) {
					return LoopEnd::Failed(e);
				}
				if !retrying {
					// `{e:#}` prints the full context chain instead of only the outermost context.
					eprintln!("[testd] [error] {e:#}, retrying...");
					retrying = true;
				}
				next_host = (next_host + 1) % count;
				tokio::select! {
					() = tokio::time::sleep(RECONNECT_DELAY) => {}
					() = stop.notified() => return LoopEnd::Stopped { writer: None, entries },
				}
				continue;
			},
		};
		next_host = (index + 1) % count;

		match watch(reader, &mut writer, &stop, &mut entries).await {
			Watch::Stopped => return LoopEnd::Stopped { writer: Some(writer), entries },
			Watch::StoppedByHost => return LoopEnd::StoppedByHost,
			Watch::Revoked => return LoopEnd::Revoked,
			Watch::GrantRevoked => {
				// The stored copy is dead; drop it so the next run asks again.
				let path = testd_core::config::credential_path(&config.token_profile);
				if let Err(e) = testd_core::config::clear_credential(&config.token_profile) {
					eprintln!("[testd] [warn]  could not forget the deleted credential {}: {e}", path.display());
				}
				eprintln!(
					"[testd] [grant] the host operator deleted this leaf's grant; run again to request a new one \
					 (unset TESTD_TOKEN if it holds the old grant)"
				);
				return LoopEnd::GrantRevoked;
			},
			Watch::Disconnected => {
				eprintln!("[testd] [warn]  connection to {} lost, reconnecting...", hosts(&config)[index]);
			},
		}
	}
}

enum Watch {
	Disconnected,
	Stopped,
	StoppedByHost,
	GrantRevoked,
	Revoked,
}

/// Read host frames until the connection drops, the caller stops the loop, or
/// an operator stops the leaf or deletes its grant.
async fn watch(mut reader: Reader, writer: &mut Writer, stop: &Notify, entries: &mut Vec<Entry>) -> Watch {
	let (frames_tx, mut frames) = mpsc::channel(8);
	// Reading happens in its own task: `protocol::recv` is not cancellation
	// safe, so it must not race the stop signal directly.
	let reader_task = tokio::spawn(async move {
		while let Ok(Some(response)) = protocol::recv::<_, Response>(&mut reader).await {
			if frames_tx.send(response).await.is_err() {
				break;
			}
		}
	});

	let outcome = loop {
		tokio::select! {
			frame = frames.recv() => match frame {
				Some(Response::Stopped { domain }) => {
					eprintln!("[testd] [stop]  {domain} was stopped by the host operator (grant kept)");
					break Watch::StoppedByHost;
				},
				Some(Response::GrantRevoked) => break Watch::GrantRevoked,
				Some(Response::NameRevoked { domain }) => {
					writer.revoke(&domain);
					eprintln!("[testd] [grant] the host operator deleted the grant for {domain}");
					entries.retain(|entry| !is_published_name(&entry.domain, &domain));
					if entries.is_empty() {
						eprintln!("[testd] [grant] no granted names remain; stopping");
						break Watch::Revoked;
					}
				},
				Some(_) => {},
				None => break Watch::Disconnected,
			},
			() = stop.notified() => break Watch::Stopped,
		}
	};
	reader_task.abort();
	outcome
}

/// Best-effort deregister on a writer (reader may be dead).
async fn deregister_best_effort(writer: &mut Writer, entries: &[Entry]) {
	for entry in entries {
		let _ = writer.send(&Request::Deregister { domain: entry.domain.clone() }).await;
		eprintln!("[testd] [dereg] {}", entry.domain);
	}
	if let Err(error) = writer.shutdown().await {
		eprintln!("[testd] [warn] registration shutdown: {error}");
	}
}

/// Install shutdown handlers before starting owned processes or services.
pub fn prepare_shutdown_signal() -> std::io::Result<impl std::future::Future<Output = std::io::Result<()>>> {
	#[cfg(unix)]
	{
		use tokio::signal::unix::{SignalKind, signal};

		let mut terminate = signal(SignalKind::terminate())?;
		let mut interrupt = signal(SignalKind::interrupt())?;
		Ok(async move {
			tokio::select! {
				_ = interrupt.recv() => Ok(()),
				_ = terminate.recv() => Ok(()),
			}
		})
	}
	#[cfg(windows)]
	{
		let mut interrupt = tokio::signal::windows::ctrl_c()?;
		Ok(async move {
			interrupt.recv().await;
			Ok(())
		})
	}
	#[cfg(not(any(unix, windows)))]
	Ok(tokio::signal::ctrl_c())
}

/// Wait for Ctrl+C or the service manager's normal termination signal.
pub async fn shutdown_signal() -> std::io::Result<()> {
	prepare_shutdown_signal()?.await
}

/// Register entries, run a command, deregister on exit.
pub async fn run(entries: &[Entry], command: &[String], force: bool, config: ClientConfig) -> Result<()> {
	let shutdown = prepare_shutdown_signal()?;
	let stop = Arc::new(Notify::new());

	// Maintain registration in the background.
	let bg_entries: Vec<Entry> = entries.to_vec();
	let bg_stop = stop.clone();
	let mut registration = tokio::spawn(async move { maintain_registration(bg_entries, config, bg_stop, force).await });

	// Spawn child process.
	let mut process = tokio::process::Command::new(&command[0]);
	process
		.args(&command[1..])
		.stdin(Stdio::inherit())
		.stdout(Stdio::inherit())
		.stderr(Stdio::inherit());
	let (mut child, mut process_tree) =
		process::ProcessTree::spawn(&mut process).with_context(|| format!("failed to spawn: {}", command[0]))?;

	// Wait for child, signal, or a registration loop that stopped on its own.
	let status = tokio::select! {
		status = child.wait() => status?,
		signal = shutdown => {
			signal?;
			process_tree.stop(&mut child).await?;
			child.wait().await?
		},
		result = &mut registration => {
			process_tree.stop(&mut child).await?;
			match result {
				// The operator's action already printed why; the child is ours
				// to stop, and exiting cleanly matches the operator's intent.
				Ok(LoopEnd::StoppedByHost | LoopEnd::GrantRevoked | LoopEnd::Revoked) => std::process::exit(0),
				Ok(LoopEnd::Failed(error)) => return Err(error.context("registration failed")),
				Ok(LoopEnd::Stopped { writer: None, .. }) => bail!("registration stopped; see the error above"),
				Ok(LoopEnd::Stopped { writer: Some(_), .. }) => bail!("registration stopped unexpectedly"),
				Err(e) => bail!("registration task failed: {e}"),
			}
		}
	};

	process_tree.stop(&mut child).await?;

	// Stop the registration loop and deregister the names still published.
	stop.notify_one();
	if let Ok(LoopEnd::Stopped { writer: Some(mut writer), entries }) = registration.await {
		deregister_best_effort(&mut writer, &entries).await;
	}

	std::process::exit(status.code().unwrap_or(1));
}

/// Register entries and hold the connection until interrupted.
pub async fn register(entries: &[Entry], force: bool, config: ClientConfig) -> Result<()> {
	let shutdown = prepare_shutdown_signal()?;
	let stop = Arc::new(Notify::new());

	let bg_entries: Vec<Entry> = entries.to_vec();
	let bg_stop = stop.clone();
	let mut registration = tokio::spawn(async move { maintain_registration(bg_entries, config, bg_stop, force).await });

	eprintln!("[testd] Press Ctrl+C to deregister.");
	tokio::select! {
		signal = shutdown => {
			signal?;
			stop.notify_one();
		},
		result = &mut registration => match result {
			// The operator's action already printed why.
			Ok(LoopEnd::StoppedByHost | LoopEnd::GrantRevoked | LoopEnd::Revoked) => return Ok(()),
			Ok(LoopEnd::Failed(error)) => return Err(error.context("registration failed")),
			Ok(LoopEnd::Stopped { writer: None, .. }) => bail!("registration stopped; see the error above"),
			Ok(LoopEnd::Stopped { writer: Some(_), .. }) => bail!("registration stopped unexpectedly"),
			Err(e) => bail!("registration task failed: {e}"),
		},
	}

	if let Ok(LoopEnd::Stopped { writer: Some(mut writer), entries }) = registration.await {
		deregister_best_effort(&mut writer, &entries).await;
	}

	Ok(())
}

/// A name to register and the target it should proxy to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
	pub domain: String,
	/// The port (or host:port) sent to the host engine.
	pub target: String,
}

impl Entry {
	pub fn new(domain: impl Into<String>, target: impl Into<String>) -> Self {
		Self { domain: domain.into(), target: target.into() }
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	async fn request_error(response: Option<Response>) -> anyhow::Error {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let addr = listener.local_addr().unwrap();
		let server = tokio::spawn(async move {
			let (stream, _) = listener.accept().await.unwrap();
			let (reader, mut writer) = stream.into_split();
			let mut reader = BufReader::new(reader);
			let _: Request = protocol::recv(&mut reader).await.unwrap().unwrap();
			if let Some(response) = response {
				protocol::send(&mut writer, &response).await.unwrap();
			}
		});
		let stream = TcpStream::connect(addr).await.unwrap();
		let (mut reader, mut writer) = transport::split(stream, None);
		let registration = Request::Register { domain: "app".into(), target: "3000".into(), force: false };
		let error = tokio::time::timeout(Duration::from_secs(5), request(&mut reader, &mut writer, &registration))
			.await
			.expect("host response timed out")
			.unwrap_err();
		server.await.unwrap();
		error
	}

	#[cfg(target_os = "linux")]
	#[tokio::test]
	async fn shutdown_signals_are_registered_before_the_future_is_polled() {
		for signal in [libc::SIGINT, libc::SIGTERM] {
			let shutdown = prepare_shutdown_signal().unwrap();
			/* SAFETY: the handler is installed and the signal targets only this test process. */
			assert_eq!(unsafe { libc::raise(signal) }, 0);
			tokio::time::timeout(Duration::from_secs(3), shutdown)
				.await
				.unwrap_or_else(|_| panic!("shutdown signal {signal} was lost"))
				.unwrap();
		}
	}

	#[test]
	fn terminal_connection_errors_keep_the_underlying_cause() {
		let cause = std::io::Error::other("fixture storage failure");
		let error = anyhow::Error::new(cause).context("failed to save the credential");
		let error = anyhow::Error::new(ConnectFailure::rejected(error)).context("registration failed");
		assert_eq!(format!("{error:#}"), "registration failed: failed to save the credential: fixture storage failure");
	}

	#[tokio::test]
	async fn terminal_host_errors_stop_reconnection() {
		let error = request_error(Some(Response::terminal_error("already registered"))).await;
		let failure = error.downcast_ref::<ConnectFailure>().expect("terminal failure");
		assert!(!failure.is_retryable());
		assert!(error.to_string().contains("already registered"));
	}

	#[tokio::test]
	async fn unclassified_and_transport_errors_keep_the_existing_retry_policy() {
		for response in [
			Some(Response::error("legacy host")),
			Some(Response::Error { message: "temporary".into(), disposition: Some(ErrorDisposition::Retryable) }),
			Some(Response::Error { message: "unknown".into(), disposition: Some(ErrorDisposition::Unknown) }),
			None,
		] {
			let error = request_error(response).await;
			assert!(error.downcast_ref::<ConnectFailure>().is_none(), "{error:#}");
		}
	}

	#[tokio::test]
	async fn an_unclassified_registration_error_is_retried() {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let addr = listener.local_addr().unwrap();
		let (registered, ready) = tokio::sync::oneshot::channel();
		let (client_stopped, wait_for_stop) = tokio::sync::oneshot::channel();
		let server = tokio::spawn(async move {
			for attempt in 0..2 {
				let (stream, _) = listener.accept().await.unwrap();
				let (reader, mut writer) = stream.into_split();
				let mut reader = BufReader::new(reader);
				let _: Auth = protocol::recv(&mut reader).await.unwrap().unwrap();
				protocol::send(&mut writer, &Response::Ok).await.unwrap();
				let _: Request = protocol::recv(&mut reader).await.unwrap().unwrap();
				if attempt == 0 {
					protocol::send(&mut writer, &Response::error("temporary failure"))
						.await
						.unwrap();
				} else {
					protocol::send(&mut writer, &Response::Ok).await.unwrap();
					registered.send(()).unwrap();
					/* Closing an unread reply may reset the socket instead of producing EOF. */
					wait_for_stop.await.unwrap();
					break;
				}
			}
		});
		let config = ClientConfig {
			token: "test".into(),
			token_profile: "test".into(),
			hosts: vec![addr.to_string()],
			node_profile: "test".into(),
		};
		let stop = Arc::new(Notify::new());
		let registration =
			tokio::spawn(maintain_registration(vec![Entry::new("app", "3000")], config, Arc::clone(&stop), false));
		tokio::time::timeout(Duration::from_secs(8), ready)
			.await
			.expect("client did not retry")
			.unwrap();
		stop.notify_one();
		let outcome = tokio::time::timeout(Duration::from_secs(5), registration)
			.await
			.expect("client did not stop")
			.unwrap();
		// Stop may win while the second registration reply is still in flight.
		assert!(matches!(outcome, LoopEnd::Stopped { .. }));
		drop(outcome);
		client_stopped.send(()).unwrap();
		tokio::time::timeout(Duration::from_secs(5), server)
			.await
			.expect("server did not stop")
			.unwrap();
	}

	#[tokio::test]
	async fn a_silent_authentication_peer_does_not_block_failover() {
		let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let ready = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let mut config = ClientConfig {
			token: "fixture".into(),
			token_profile: "fixture".into(),
			node_profile: "fixture".into(),
			hosts: vec![silent.local_addr().unwrap().to_string(), ready.local_addr().unwrap().to_string()],
		};
		let silent_task = tokio::spawn(async move {
			let (stream, _) = silent.accept().await.unwrap();
			let _stream = stream;
			std::future::pending::<()>().await;
		});
		let ready_task = tokio::spawn(async move {
			let (stream, _) = ready.accept().await.unwrap();
			let (reader, mut writer) = stream.into_split();
			let _: Auth = protocol::recv(&mut BufReader::new(reader)).await.unwrap().unwrap();
			protocol::send(&mut writer, &Response::Ok).await.unwrap();
		});
		let result =
			tokio::time::timeout(RESPONSE_TIMEOUT + Duration::from_secs(3), connect_from(&mut config, 0, &[])).await;
		silent_task.abort();
		assert_eq!(result.unwrap().unwrap().2, 1);
		ready_task.await.unwrap();
	}

	#[test]
	fn labels_name_the_user_and_host() {
		assert_eq!(format_label("bar", "foo"), "bar@foo");
		assert_eq!(format_label(" bar ", " foo "), "bar@foo");
		assert_eq!(format_label("", "foo"), "foo");
		assert_eq!(format_label("bar", ""), "bar");
		assert_eq!(format_label("", ""), "unknown client");
	}

	#[test]
	fn published_names_match_under_the_hosts_tld() {
		assert!(is_published_name("app", "app.test"));
		assert!(is_published_name("app.example", "app.example.test"));
		assert!(is_published_name("app.example", "app.example"));
		assert!(!is_published_name("app", "app.example.test"), "a shorter name is not the same name");
		assert!(!is_published_name("app.example", "api.app.example.test"));
	}
}
