//! Pinned host trust, explicit pairing, and authenticated registration streams.

use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use rustls::pki_types::ServerName;
use testd_core::config::{config_dir, write_private};
use testd_core::protocol::{self, Auth, Request, Response};
use testd_pki::transport::{self, HOST_NAME};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use crate::forwarder::Forwarders;
use crate::{APPROVAL_TIMEOUT, CONNECT_TIMEOUT, Reader};

type Output = Box<dyn AsyncWrite + Unpin + Send>;

pub(crate) struct Writer {
	pub(crate) inner: Output,
	forwarders: Option<Box<Forwarders>>,
}

impl Writer {
	pub(crate) async fn send(&mut self, request: &Request) -> Result<()> {
		let request = match &mut self.forwarders {
			Some(forwarders) => forwarders.prepare(request).await.map_err(PreparationError)?,
			None => request.clone(),
		};
		protocol::send(&mut self.inner, &request).await?;
		Ok(())
	}

	pub(crate) async fn shutdown(&mut self) -> io::Result<()> {
		tokio::time::timeout(CONNECT_TIMEOUT, self.inner.shutdown())
			.await
			.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "registration shutdown timed out"))?
	}

	pub(crate) fn revoke(&mut self, domain: &str) {
		if let Some(forwarders) = &mut self.forwarders {
			forwarders.revoke(domain);
		}
	}

	pub(crate) fn finish(&mut self, response: &Response) {
		if let Some(forwarders) = &mut self.forwarders {
			forwarders.finish(response);
		}
	}
}

#[derive(Debug)]
pub(crate) struct PreparationError(io::Error);

impl std::fmt::Display for PreparationError {
	fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		self.0.fmt(formatter)
	}
}

impl std::error::Error for PreparationError {
	fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
		Some(&self.0)
	}
}

pub(crate) fn split<S>(stream: S, forwarders: Option<Forwarders>) -> (Reader, Writer)
where
	S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
	let (reader, writer) = tokio::io::split(stream);
	let reader = BufReader::new(Box::new(reader) as Box<dyn AsyncRead + Unpin + Send>);
	(reader, Writer { inner: Box::new(writer), forwarders: forwarders.map(Box::new) })
}

pub(crate) async fn open(host: &str) -> Result<(Reader, Writer)> {
	let root = read_root(host)?;
	ensure!(root.is_some() || is_local_endpoint(host), "host is not paired; run `testd pair {host}` first");
	let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(host))
		.await
		.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "registration connection timed out"))?
		.with_context(|| format!("failed to connect to {host}"))?;
	match root {
		Some(root) => {
			let bind_ip = stream.local_addr()?.ip();
			let connector = TlsConnector::from(transport::client_config(&root)?);
			let name = ServerName::try_from(HOST_NAME)?;
			let stream = tokio::time::timeout(CONNECT_TIMEOUT, connector.connect(name, stream))
				.await
				.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "registration TLS handshake timed out"))?
				.context(
					"host TLS identity could not be verified; compare pairing codes before explicitly re-pairing",
				)?;
			let forwarders = Forwarders::new(bind_ip, &root)?;
			Ok(split(stream, Some(forwarders)))
		},
		None => {
			ensure!(stream.peer_addr()?.ip().is_loopback(), "a local registration endpoint resolved off this machine");
			Ok(split(stream, None))
		},
	}
}

fn is_local_endpoint(host: &str) -> bool {
	host.parse::<SocketAddr>().is_ok_and(|address| address.ip().is_loopback())
		|| host
			.split_once(':')
			.is_some_and(|(name, port)| name.eq_ignore_ascii_case("localhost") && port.parse::<u16>().is_ok())
}

fn trust_path(host: &str) -> PathBuf {
	let canonical = host
		.parse::<SocketAddr>()
		.map(|address| address.to_string())
		.unwrap_or_else(|_| host.to_lowercase());
	config_dir()
		.join("trusted-hosts")
		.join(format!("{}.crt", transport::fingerprint(canonical.as_bytes())))
}

fn read_root(host: &str) -> io::Result<Option<Vec<u8>>> {
	let path = trust_path(host);
	let metadata = match std::fs::symlink_metadata(&path) {
		Ok(metadata) => metadata,
		Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
		Err(error) => return Err(error),
	};
	if !metadata.is_file() || metadata.len() > protocol::MAX_FRAME_BYTES as u64 {
		return Err(io::Error::new(io::ErrorKind::InvalidData, "host trust must be a regular CA certificate file"));
	}
	std::fs::read(path).map(Some)
}

/// First-contact TLS session. A successful host response alone never authorizes pinning its identity.
pub struct Pairing {
	pub host: String,
	pub request: String,
	pub verification: String,
	root: Vec<u8>,
	previous: Option<Vec<u8>>,
	reader: Reader,
	_writer: Writer,
}

pub async fn begin_pairing(host: &str) -> Result<Pairing> {
	testd_core::config::validate_host_addr(host)?;
	let previous = read_root(host)?;
	let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(host))
		.await
		.context("pairing connection timed out")??;
	let connector = TlsConnector::from(transport::pairing_config());
	let stream = tokio::time::timeout(CONNECT_TIMEOUT, connector.connect(ServerName::try_from(HOST_NAME)?, stream))
		.await
		.context("pairing TLS handshake timed out")??;
	let material =
		stream
			.get_ref()
			.1
			.export_keying_material([0; transport::PAIRING_BYTES], transport::PAIRING_LABEL, None)?;
	let root = stream
		.get_ref()
		.1
		.peer_certificates()
		.and_then(|chain| chain.last())
		.context("the host did not advertise a transport root")?
		.as_ref()
		.to_vec();
	let (mut reader, mut writer) = split(stream, None);
	let greeting = Auth { pairing: Some(crate::client_label()), ..Auth::default() };
	protocol::send(&mut writer.inner, &greeting).await?;
	let response = crate::read_response(&mut reader, Duration::from_secs(10)).await?;
	let request = match response {
		Response::Pending { id } => id,
		Response::Error { message, .. } => bail!("pairing refused: {message}"),
		_ => bail!("the host did not queue a pairing request"),
	};
	Ok(Pairing {
		host: host.to_owned(),
		request,
		verification: transport::pairing_code(material),
		root,
		previous,
		reader,
		_writer: writer,
	})
}

impl Pairing {
	/// Confirmation must come from the trusted host's display, not an echoed value received over the network.
	pub async fn confirm(mut self, host_code: &str) -> Result<()> {
		ensure!(host_code.trim() == self.verification, "pairing codes do not match; host trust was not changed");
		match crate::read_response(&mut self.reader, APPROVAL_TIMEOUT).await? {
			Response::Paired => {},
			Response::Error { message, .. } => bail!("pairing failed: {message}"),
			_ => bail!("the host did not approve this pairing session"),
		}
		ensure!(read_root(&self.host)? == self.previous, "host trust changed during pairing; no pin was replaced");
		let path = trust_path(&self.host);
		let directory = path.parent().context("missing trust directory")?;
		std::fs::create_dir_all(directory)?;
		if let Some(previous) = &self.previous
			&& previous != &self.root
		{
			let backup = path.with_extension(format!("{}.previous.crt", transport::fingerprint(previous)));
			if !backup.try_exists()? {
				write_private(&backup, previous)?;
			}
		}
		write_private(&path, &self.root).context("save the explicitly paired host identity")
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn plaintext_is_local_only_not_a_hostname_resolution_or_tls_failure_fallback() {
		for host in ["127.0.0.1:7737", "[::1]:7737", "localhost:7737"] {
			assert!(is_local_endpoint(host), "{host}");
		}
		for host in ["192.0.2.1:7737", "host.example:7737", "localhost.attacker:7737", "localhost:not-a-port"] {
			assert!(!is_local_endpoint(host), "{host}");
		}
	}
}
