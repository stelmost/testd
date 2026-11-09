//! Real CLI pairing and publication through both TLS hops, without OS integration or external services.

use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rustls::pki_types::ServerName;
use testd_core::config::{HostConfig, host_tld};
use testd_daemon::access::{Access, PendingView};
use testd_daemon::edge::{self, Edge, EdgeContext};
use testd_daemon::engine::{HostEngine, Registrations};
use testd_daemon::resolver::{ResolvedName, Resolver};
use testd_pki::transport::{self, Identity};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::process::{Child, Command};
use tokio::task::{JoinHandle, JoinSet};
use tokio_rustls::{TlsAcceptor, TlsConnector};

const WAIT: Duration = Duration::from_secs(8);
const PAYLOAD: &[u8] = b"application payload through both TLS paths";
const UNVERIFIED_TOKEN: &str = "do-not-send-my-token";

#[derive(Default)]
struct NoSystemResolver {
	fail_next: AtomicBool,
}

impl Resolver for NoSystemResolver {
	fn apply(&self, _: &[ResolvedName]) -> io::Result<()> {
		if self.fail_next.swap(false, Ordering::SeqCst) {
			return Err(io::Error::other("fixture resolver failure"));
		}
		Ok(())
	}

	fn cleanup(&self) -> io::Result<()> {
		Ok(())
	}
}

struct Host {
	_directory: tempfile::TempDir,
	access: Arc<Access>,
	resolver: Arc<NoSystemResolver>,
	registrations: Registrations,
	address: SocketAddr,
	edge: Edge,
	root: Vec<u8>,
	server: JoinHandle<()>,
}

impl Host {
	async fn start() -> Self {
		let directory = tempfile::tempdir().unwrap();
		let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = listener.local_addr().unwrap();
		let mut config: HostConfig = toml::from_str("").unwrap();
		config.listen_addr = address;
		config.pki_dir = directory.path().join("pki");
		let root = Identity::load(&config.pki_dir, transport::HOST_NAME)
			.unwrap()
			.root_der()
			.unwrap();
		let access = Arc::new(Access::load(directory.path().join("tokens.json")).unwrap());
		let resolver = Arc::new(NoSystemResolver::default());
		let engine = HostEngine::new(config.clone(), resolver.clone(), Arc::clone(&access));
		let registrations = engine.registrations();
		let context = Arc::new(EdgeContext::new(host_tld(), Arc::clone(&registrations)));
		let edge =
			edge::start(context, Some("127.0.0.1:0".parse().unwrap()), "127.0.0.1:0".parse().unwrap(), &config.pki_dir)
				.unwrap();
		let server = tokio::spawn(async move { engine.serve(listener).await.unwrap() });
		Self { _directory: directory, access, resolver, registrations, address, edge, root, server }
	}

	async fn pending(&self) -> PendingView {
		tokio::time::timeout(WAIT, async {
			loop {
				if let Some(pending) = self.access.pending().into_iter().next() {
					return pending;
				}
				tokio::time::sleep(Duration::from_millis(10)).await;
			}
		})
		.await
		.expect("the CLI did not queue its request")
	}

	async fn pair(&self, home: &Path) {
		let mut child = candidate(home).args(["pair", &self.address.to_string()]).spawn().unwrap();
		let request = self.pending().await;
		let code = request.verification.unwrap();
		self.access.approve(&request.id).unwrap();
		child
			.stdin
			.take()
			.unwrap()
			.write_all(format!("{code}\n").as_bytes())
			.await
			.unwrap();
		let output = tokio::time::timeout(WAIT, child.wait_with_output()).await.unwrap().unwrap();
		assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
		assert!(String::from_utf8_lossy(&output.stderr).contains(&code));
		assert!(self.access.tokens().is_empty(), "pairing must not issue grants or credentials");
	}

	async fn publish(&self, home: &Path, app: SocketAddr) -> Child {
		let mut command = candidate(home);
		command.args(["run", "--host", &self.address.to_string(), &format!("web.fixture:{}", app.port())]);
		let child = command.spawn().unwrap();
		let request = self.pending().await;
		assert!(request.verification.is_none());
		self.access.approve(&request.id).unwrap();
		tokio::time::timeout(WAIT, async {
			while self.registrations.lock().await.is_empty() {
				tokio::time::sleep(Duration::from_millis(10)).await;
			}
		})
		.await
		.expect("the leaf did not publish its TLS endpoint");
		child
	}

	async fn browser(&self) -> tokio_rustls::client::TlsStream<TcpStream> {
		let connector = TlsConnector::from(transport::client_config(&self.root).unwrap());
		let name = ServerName::try_from(format!("web.fixture.{}", host_tld())).unwrap();
		let stream = TcpStream::connect(self.edge.tls_addr).await.unwrap();
		connector.connect(name, stream).await.unwrap()
	}
}

impl Drop for Host {
	fn drop(&mut self) {
		self.server.abort();
	}
}

fn candidate(home: &Path) -> Command {
	let mut command = Command::new(env!("CARGO_BIN_EXE_testd"));
	for (key, _) in std::env::vars_os() {
		if key.to_string_lossy().starts_with("TESTD_") {
			command.env_remove(key);
		}
	}
	command
		.env("HOME", home)
		.env("USERPROFILE", home)
		.env("APPDATA", home.join("appdata"));
	command
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.kill_on_drop(true);
	command
}

async fn replace_paired_host(host: &mut Host) -> TcpListener {
	host.server.abort();
	assert!((&mut host.server).await.unwrap_err().is_cancelled());
	/* Pairing leaves a TIME_WAIT connection: the replacement must reuse the listener's Unix address. */
	let socket = TcpSocket::new_v4().unwrap();
	#[cfg(unix)]
	socket.set_reuseaddr(true).unwrap();
	socket.bind(host.address).unwrap();
	socket.listen(128).unwrap()
}

async fn headers<S: tokio::io::AsyncRead + Unpin>(reader: &mut BufReader<S>) -> Vec<String> {
	let mut lines = Vec::new();
	loop {
		let mut line = String::new();
		reader.read_line(&mut line).await.unwrap();
		assert!(!line.is_empty(), "HTTP connection ended before its headers");
		if line == "\r\n" {
			return lines;
		}
		lines.push(line);
	}
}

async fn application(stream: TcpStream) {
	let mut reader = BufReader::new(stream);
	let lines = headers(&mut reader).await;
	if lines[0].contains("/socket ") {
		reader
			.get_mut()
			.write_all(b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n")
			.await
			.unwrap();
		let mut ping = [0; 4];
		reader.read_exact(&mut ping).await.unwrap();
		assert_eq!(&ping, b"ping");
		reader.get_mut().write_all(b"pong").await.unwrap();
	} else {
		assert!(
			lines
				.iter()
				.any(|line| line.to_ascii_lowercase().starts_with("x-forwarded-proto: https"))
		);
		let mut body = vec![0; PAYLOAD.len()];
		reader.read_exact(&mut body).await.unwrap();
		assert_eq!(body, PAYLOAD);
		let response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
		reader.get_mut().write_all(response.as_bytes()).await.unwrap();
		reader.get_mut().write_all(&body).await.unwrap();
	}
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pairing_and_publication_encrypt_both_paths_and_preserve_upgrades() {
	tokio::time::timeout(Duration::from_secs(25), async {
		let host = Host::start().await;
		let home = tempfile::tempdir().unwrap();
		host.pair(home.path()).await;
		let app = Arc::new(TcpListener::bind("127.0.0.1:0").await.unwrap());
		let mut leaf = host.publish(home.path(), app.local_addr().unwrap()).await;
		let name = format!("web.fixture.{}", host_tld());
		let claim = host.registrations.lock().await.claim(&name).unwrap().clone();
		let leaf_root = claim.transport.unwrap().root;
		assert_ne!(claim.registration.target, app.local_addr().unwrap().to_string());

		let application_listener = Arc::clone(&app);
		let service = tokio::spawn(async move {
			let mut requests = JoinSet::new();
			for _ in 0..2 {
				requests.spawn(application(application_listener.accept().await.unwrap().0));
			}
			while let Some(result) = requests.join_next().await {
				result.unwrap()
			}
		});
		let mut browser = host.browser().await;
		let request = format!(
			"POST /echo HTTP/1.1\r\nHost: {name}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
			PAYLOAD.len(),
		);
		browser.write_all(request.as_bytes()).await.unwrap();
		browser.write_all(PAYLOAD).await.unwrap();
		let mut browser = BufReader::new(browser);
		assert!(headers(&mut browser).await[0].contains("200 OK"));
		let mut body = vec![0; PAYLOAD.len()];
		browser.read_exact(&mut body).await.unwrap();
		assert_eq!(body, PAYLOAD);

		let mut browser = host.browser().await;
		browser
			.write_all(
				format!("GET /socket HTTP/1.1\r\nHost: {name}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n",)
					.as_bytes(),
			)
			.await
			.unwrap();
		let mut browser = BufReader::new(browser);
		assert!(headers(&mut browser).await[0].contains("101 Switching Protocols"));
		browser.get_mut().write_all(b"ping").await.unwrap();
		let mut pong = [0; 4];
		browser.read_exact(&mut pong).await.unwrap();
		assert_eq!(&pong, b"pong");
		service.await.unwrap();

		/* Knowing the leaf's public CA is not enough to access its application endpoint. */
		let connector = TlsConnector::from(transport::client_config(&leaf_root).unwrap());
		let stream = TcpStream::connect(&claim.registration.target).await.unwrap();
		if let Ok(mut stream) = connector
			.connect(ServerName::try_from(transport::LEAF_NAME).unwrap(), stream)
			.await
		{
			let _ = stream.write_all(b"GET / HTTP/1.1\r\n\r\n").await;
			let mut byte = [0];
			let read = tokio::time::timeout(WAIT, stream.read(&mut byte)).await.unwrap();
			assert!(!matches!(read, Ok(count) if count > 0));
		}
		leaf.kill().await.unwrap();
		tokio::time::timeout(WAIT, async {
			while !host.registrations.lock().await.is_empty() {
				tokio::time::sleep(Duration::from_millis(10)).await;
			}
		})
		.await
		.expect("disconnect left a live TLS claim");
	})
	.await
	.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incorrect_pairing_codes_cannot_install_host_trust_or_issue_credentials() {
	let host = Host::start().await;
	let home = tempfile::tempdir().unwrap();
	let mut child = candidate(home.path())
		.args(["pair", &host.address.to_string()])
		.spawn()
		.unwrap();
	let request = host.pending().await;
	host.access.approve(&request.id).unwrap();
	child.stdin.take().unwrap().write_all(b"not-the-code\n").await.unwrap();
	let output = tokio::time::timeout(WAIT, child.wait_with_output()).await.unwrap().unwrap();
	assert!(!output.status.success());
	assert!(String::from_utf8_lossy(&output.stderr).contains("codes do not match"));
	assert!(!home.path().join(".config/testd/trusted-hosts").exists());
	assert!(!home.path().join("appdata/testd/trusted-hosts").exists());
	assert!(host.access.tokens().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_changed_host_identity_is_rejected_before_credentials_are_sent() {
	let mut host = Host::start().await;
	let home = tempfile::tempdir().unwrap();
	host.pair(home.path()).await;
	let listener = replace_paired_host(&mut host).await;
	let attacker = Identity::ephemeral(transport::HOST_NAME).unwrap();
	let serving = tokio::spawn(async move {
		let (stream, _) = listener.accept().await.unwrap();
		assert!(TlsAcceptor::from(attacker.server_config()).accept(stream).await.is_err());
	});
	let output = tokio::time::timeout(
		WAIT,
		candidate(home.path())
			.env("TESTD_TOKEN", UNVERIFIED_TOKEN)
			.args(["run", "--host", &host.address.to_string(), "web.fixture:3000"])
			.output(),
	)
	.await
	.unwrap()
	.unwrap();
	assert!(!output.status.success());
	assert!(String::from_utf8_lossy(&output.stderr).contains("host TLS identity could not be verified"));
	tokio::time::timeout(WAIT, serving).await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plaintext_server_cannot_downgrade_a_paired_connection() {
	let mut host = Host::start().await;
	let home = tempfile::tempdir().unwrap();
	host.pair(home.path()).await;
	let listener = replace_paired_host(&mut host).await;
	let (finished, until_finished) = tokio::sync::oneshot::channel();
	let serving = tokio::spawn(async move {
		let (mut stream, _) = listener.accept().await.unwrap();
		let mut hello = [0; 4096];
		let count = stream.read(&mut hello).await.unwrap();
		assert_eq!(hello[0], 0x16);
		assert!(
			!hello[..count]
				.windows(UNVERIFIED_TOKEN.len())
				.any(|bytes| bytes == UNVERIFIED_TOKEN.as_bytes())
		);
		stream.write_all(b"{\"type\":\"Ok\"}\n").await.unwrap();
		tokio::select! {
			biased;
			accepted = listener.accept() => panic!("the client attempted plaintext fallback: {accepted:?}"),
			_ = until_finished => {},
		}
	});
	let output = tokio::time::timeout(
		WAIT,
		candidate(home.path())
			.env("TESTD_TOKEN", UNVERIFIED_TOKEN)
			.args(["run", "--host", &host.address.to_string(), "web.fixture:3000"])
			.output(),
	)
	.await
	.unwrap()
	.unwrap();
	assert!(!output.status.success());
	assert!(String::from_utf8_lossy(&output.stderr).contains("host TLS identity could not be verified"));
	finished.send(()).unwrap();
	tokio::time::timeout(WAIT, serving).await.unwrap().unwrap();
}

#[tokio::test]
async fn a_network_host_must_be_paired_even_when_a_credential_is_supplied() {
	let home = tempfile::tempdir().unwrap();
	let output = tokio::time::timeout(
		WAIT,
		candidate(home.path())
			.env("TESTD_TOKEN", "not-permission-to-skip-host-verification")
			.args(["run", "--host", "192.0.2.1:7737", "web.fixture:3000"])
			.output(),
	)
	.await
	.unwrap()
	.unwrap();
	assert!(!output.status.success());
	assert!(String::from_utf8_lossy(&output.stderr).contains("host is not paired; run `testd pair"));
}

#[tokio::test]
async fn failed_deregistration_restores_the_tls_identity_not_a_plaintext_route() {
	use testd_core::protocol::{Auth, Request, Response, recv, send};

	tokio::time::timeout(WAIT, async {
		let host = Host::start().await;
		let name = format!("web.fixture.{}", host_tld());
		let (request, _decision) = host.access.request("fixture", "127.0.0.1", vec![name.clone()]).unwrap();
		let testd_daemon::access::Approved::Token(issued) = host.access.approve(&request).unwrap() else {
			panic!("expected a scoped token")
		};
		let connector = TlsConnector::from(transport::client_config(&host.root).unwrap());
		let socket = TcpStream::connect(host.address).await.unwrap();
		let stream = connector
			.connect(ServerName::try_from(transport::HOST_NAME).unwrap(), socket)
			.await
			.unwrap();
		let (reader, mut writer) = tokio::io::split(stream);
		let mut reader = BufReader::new(reader);
		send(&mut writer, &Auth { token: issued.token, ..Auth::default() })
			.await
			.unwrap();
		assert!(matches!(recv::<_, Response>(&mut reader).await.unwrap(), Some(Response::Ok)));
		let leaf = Identity::ephemeral(transport::LEAF_NAME).unwrap();
		let root = leaf.root_der().unwrap();
		send(
			&mut writer,
			&Request::RegisterTls {
				domain: "web.fixture".into(),
				target: "127.0.0.1:9443".into(),
				root: root.clone(),
				force: false,
			},
		)
		.await
		.unwrap();
		assert!(matches!(recv::<_, Response>(&mut reader).await.unwrap(), Some(Response::Ok)));
		host.resolver.fail_next.store(true, Ordering::SeqCst);
		send(&mut writer, &Request::Deregister { domain: "web.fixture".into() })
			.await
			.unwrap();
		assert!(matches!(recv::<_, Response>(&mut reader).await.unwrap(), Some(Response::Error { .. })));
		let claim = host.registrations.lock().await.claim(&name).unwrap().clone();
		assert_eq!(claim.transport.unwrap().root, root);
	})
	.await
	.unwrap();
}
