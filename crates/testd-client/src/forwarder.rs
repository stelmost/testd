//! Connection-scoped, mutually authenticated TLS endpoints for local HTTP applications.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use testd_core::protocol::{Request, Response};
use testd_pki::transport::{Identity, LEAF_NAME};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{JoinHandle, JoinSet};
use tokio_rustls::TlsAcceptor;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CONNECTIONS: usize = 64;

pub(crate) struct Forwarders {
	identity: Arc<Identity>,
	server: Arc<rustls::ServerConfig>,
	bind_ip: IpAddr,
	active: HashMap<String, Forwarder>,
	pending: Option<Pending>,
}

enum Pending {
	Register { domain: String, forwarder: Forwarder },
	Deregister { domain: String },
}

impl Forwarders {
	pub(crate) fn new(bind_ip: IpAddr, host_root: &[u8]) -> anyhow::Result<Self> {
		let identity = Identity::ephemeral(LEAF_NAME)?;
		let server = identity.mutual_server_config(host_root)?;
		Ok(Self { identity, server, bind_ip, active: HashMap::new(), pending: None })
	}

	pub(crate) async fn prepare(&mut self, request: &Request) -> io::Result<Request> {
		match request {
			Request::Register { domain, target, force } => {
				if !self.active.contains_key(domain) && self.active.len() >= 64 {
					return Err(io::Error::new(
						io::ErrorKind::PermissionDenied,
						"a leaf connection supports at most 64 application endpoints",
					));
				}
				self.pending = None;
				let address = match self.active.get(domain) {
					Some(forwarder) if forwarder.target == *target => forwarder.address,
					_ => {
						let forwarder = Forwarder::start(self.bind_ip, target, Arc::clone(&self.server)).await?;
						let address = forwarder.address;
						self.pending = Some(Pending::Register { domain: domain.clone(), forwarder });
						address
					},
				};
				Ok(Request::RegisterTls {
					domain: domain.clone(),
					target: address.to_string(),
					force: *force,
					root: self.identity.root_der().map_err(io::Error::other)?,
				})
			},
			Request::Deregister { domain } => {
				self.pending = Some(Pending::Deregister { domain: domain.clone() });
				Ok(request.clone())
			},
			Request::RegisterTls { .. } => {
				Err(io::Error::new(io::ErrorKind::InvalidInput, "TLS claims are generated locally"))
			},
		}
	}

	pub(crate) fn revoke(&mut self, domain: &str) {
		self.active.retain(|name, _| !crate::is_published_name(name, domain));
		if matches!(&self.pending,
			Some(Pending::Register { domain: name, .. } | Pending::Deregister { domain: name })
				if crate::is_published_name(name, domain))
		{
			self.pending = None;
		}
	}

	pub(crate) fn finish(&mut self, response: &Response) {
		let pending = self.pending.take();
		if !matches!(response, Response::Ok | Response::AppliedWithError { .. }) {
			return;
		}
		match pending {
			Some(Pending::Register { domain, forwarder }) => {
				self.active.insert(domain, forwarder);
			},
			Some(Pending::Deregister { domain }) => {
				self.active.remove(&domain);
			},
			None => {},
		}
	}
}

struct Forwarder {
	target: String,
	address: SocketAddr,
	task: JoinHandle<()>,
}

impl Forwarder {
	async fn start(bind_ip: IpAddr, target: &str, server: Arc<rustls::ServerConfig>) -> io::Result<Self> {
		let local = local_target(target).await?;
		let listener = TcpListener::bind(SocketAddr::new(bind_ip, 0)).await?;
		let address = listener.local_addr()?;
		let task = tokio::spawn(async move {
			if let Err(error) = serve(listener, local, server).await {
				eprintln!("[testd] [error] leaf TLS forwarder: {error}");
			}
		});
		Ok(Self { target: target.to_owned(), address, task })
	}
}

impl Drop for Forwarder {
	fn drop(&mut self) {
		/* Dropping the listener's JoinSet also closes in-flight HTTP, SSE, and WebSocket connections. */
		self.task.abort();
	}
}

async fn local_target(target: &str) -> io::Result<SocketAddr> {
	let target = match target.parse::<u16>() {
		Ok(port) => SocketAddr::from(([127, 0, 0, 1], port)).to_string(),
		Err(_) => target.to_owned(),
	};
	let addresses = tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::lookup_host(&target))
		.await
		.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "application address lookup timed out"))??;
	let interfaces = if_addrs::get_if_addrs()?;
	for address in addresses {
		if address.port() != 0
			&& (address.ip().is_loopback() || interfaces.iter().any(|iface| iface.ip() == address.ip()))
		{
			return Ok(address);
		}
	}
	Err(io::Error::new(
		io::ErrorKind::PermissionDenied,
		"a secure leaf can forward only to an application on this machine; run the leaf beside the application",
	))
}

async fn serve(listener: TcpListener, target: SocketAddr, server: Arc<rustls::ServerConfig>) -> io::Result<()> {
	let capacity = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
	let acceptor = TlsAcceptor::from(server);
	let mut connections = JoinSet::new();
	loop {
		tokio::select! {
			accepted = listener.accept() => {
				let (stream, _) = accepted?;
				let Ok(permit) = Arc::clone(&capacity).try_acquire_owned() else { continue };
				let acceptor = acceptor.clone();
				connections.spawn(async move {
					let _permit = permit;
					if let Err(error) = forward(stream, target, acceptor).await {
						eprintln!("[testd] [warn] leaf TLS connection: {error}");
					}
				});
			},
			Some(result) = connections.join_next(), if !connections.is_empty() => {
				if let Err(error) = result {
					eprintln!("[testd] [error] leaf TLS connection task: {error}");
				}
			},
		}
	}
}

async fn forward(stream: TcpStream, target: SocketAddr, acceptor: TlsAcceptor) -> io::Result<()> {
	let connecting = async {
		let stream = acceptor.accept(stream).await?;
		/* Recheck interface ownership before opening a formerly local target after network changes. */
		if !target.ip().is_loopback() && !if_addrs::get_if_addrs()?.iter().any(|iface| iface.ip() == target.ip()) {
			return Err(io::Error::new(io::ErrorKind::AddrNotAvailable, "the application address is no longer local"));
		}
		let application = TcpStream::connect(target).await?;
		Ok::<_, io::Error>((stream, application))
	};
	let (mut stream, mut application) = tokio::time::timeout(CONNECT_TIMEOUT, connecting)
		.await
		.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "leaf TLS or application connection timed out"))??;
	tokio::io::copy_bidirectional(&mut stream, &mut application).await?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	fn registration(target: String) -> Request {
		Request::Register { domain: "app".into(), target, force: false }
	}

	#[tokio::test]
	async fn failed_rebinds_keep_the_old_tls_endpoint_and_discard_the_provisional_listener() {
		let host = Identity::ephemeral(testd_pki::transport::HOST_NAME).unwrap();
		let mut forwarders = Forwarders::new("127.0.0.1".parse().unwrap(), &host.root_der().unwrap()).unwrap();
		let first = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let second = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let request = registration(first.local_addr().unwrap().to_string());
		forwarders.prepare(&request).await.unwrap();
		forwarders.finish(&Response::Ok);
		let previous = forwarders.active["app"].address;
		assert!(forwarders.prepare(&registration("192.0.2.1:3000".into())).await.is_err());
		assert_eq!(forwarders.active["app"].address, previous);
		let replacement = forwarders
			.prepare(&registration(second.local_addr().unwrap().to_string()))
			.await
			.unwrap();
		let Request::RegisterTls { target, .. } = replacement else { panic!("expected a TLS claim") };
		forwarders.finish(&Response::error("resolver update failed"));
		assert_eq!(forwarders.active["app"].address, previous);
		assert!(TcpStream::connect(previous).await.is_ok());
		tokio::time::timeout(Duration::from_secs(2), async {
			while TcpStream::connect(&target).await.is_ok() {
				tokio::time::sleep(Duration::from_millis(10)).await;
			}
		})
		.await
		.unwrap();
	}

	#[tokio::test]
	async fn partial_cleanup_errors_keep_committed_transports_and_failed_deregistrations_keep_the_old_endpoint() {
		let host = Identity::ephemeral(testd_pki::transport::HOST_NAME).unwrap();
		let mut forwarders = Forwarders::new("127.0.0.1".parse().unwrap(), &host.root_der().unwrap()).unwrap();
		forwarders.prepare(&registration("3000".into())).await.unwrap();
		forwarders.finish(&Response::Ok);
		let changed = forwarders.prepare(&registration("3001".into())).await.unwrap();
		let Request::RegisterTls { target, .. } = changed else { panic!("expected a TLS claim") };
		let applied = Response::AppliedWithError { message: "claim committed; provider cleanup failed".into() };
		forwarders.finish(&applied);
		assert_eq!(forwarders.active["app"].address.to_string(), target);
		let deregister = Request::Deregister { domain: "app".into() };
		forwarders.prepare(&deregister).await.unwrap();
		forwarders.finish(&Response::error("resolver rollback restored the claim"));
		assert_eq!(forwarders.active["app"].address.to_string(), target);
		forwarders.prepare(&deregister).await.unwrap();
		forwarders.finish(&applied);
		assert!(forwarders.active.is_empty());
	}

	#[tokio::test]
	async fn revocation_closes_the_leaf_listener_and_its_active_application_streams() {
		use tokio::io::{AsyncReadExt, AsyncWriteExt};

		let host = Identity::ephemeral(testd_pki::transport::HOST_NAME).unwrap();
		let mut forwarders = Forwarders::new("127.0.0.1".parse().unwrap(), &host.root_der().unwrap()).unwrap();
		let application = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let registration = registration(application.local_addr().unwrap().to_string());
		let request = forwarders.prepare(&registration).await.unwrap();
		forwarders.finish(&Response::Ok);
		let Request::RegisterTls { target, root, .. } = request else { panic!("expected a TLS claim") };
		let connector = tokio_rustls::TlsConnector::from(host.mutual_client_config(&root).unwrap());
		let socket = TcpStream::connect(&target).await.unwrap();
		let name = rustls::pki_types::ServerName::try_from(LEAF_NAME).unwrap();
		let mut stream = connector.connect(name, socket).await.unwrap();
		stream.write_all(b"hello").await.unwrap();
		let mut application = application.accept().await.unwrap().0;
		let mut payload = [0; 5];
		application.read_exact(&mut payload).await.unwrap();
		assert_eq!(&payload, b"hello");
		forwarders.revoke("app.test");
		let mut byte = [0];
		let read = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut byte))
			.await
			.unwrap();
		assert!(!matches!(read, Ok(count) if count > 0));
		assert_eq!(
			tokio::time::timeout(Duration::from_secs(2), application.read(&mut byte))
				.await
				.unwrap()
				.unwrap(),
			0
		);
		assert!(forwarders.active.is_empty());
	}

	#[tokio::test]
	async fn secure_forwarding_does_not_add_an_unencrypted_remote_hop() {
		assert_eq!(local_target("3000").await.unwrap(), "127.0.0.1:3000".parse().unwrap());
		assert_eq!(local_target("[::1]:3000").await.unwrap(), "[::1]:3000".parse().unwrap());
		assert!(local_target("192.0.2.1:3000").await.is_err());
		assert!(local_target("0").await.is_err());
	}
}
