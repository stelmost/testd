//! Host and leaf transport identities, pinned trust, and TLS-bound pairing.

use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::sign::CertifiedKey;
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, ServerConfig, SignatureScheme};
use sha2::{Digest, Sha256};

use crate::{Ca, ExtendedKeyUsagePurpose, LEAF_LIFETIME_HOURS, leaf_params};

pub const HOST_NAME: &str = "testd-host.test";
pub const LEAF_NAME: &str = "testd-leaf.test";
pub const PAIRING_LABEL: &[u8] = b"EXPORTER-testd-pairing-v1";
pub const PAIRING_BYTES: usize = 16;

/// The CA remains local; transport leaves rotate without changing the trust pin.
pub struct Identity {
	ca: Ca,
	name: &'static str,
	cached: Mutex<Option<(Instant, Arc<CertifiedKey>)>>,
}

impl fmt::Debug for Identity {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter
			.debug_struct("Identity")
			.field("name", &self.name)
			.finish_non_exhaustive()
	}
}

impl Identity {
	pub fn load(dir: &Path, name: &'static str) -> Result<Arc<Self>> {
		let identity = Arc::new(Self { ca: Ca::load_or_init(dir)?, name, cached: Mutex::new(None) });
		identity.key()?;
		Ok(identity)
	}

	pub fn root_der(&self) -> Result<Vec<u8>> {
		Ok(self.ca.root_cert_der()?.as_ref().to_vec())
	}

	pub fn key(&self) -> Result<Arc<CertifiedKey>> {
		let mut cached = self.cached.lock().expect("transport certificate lock poisoned");
		let renew_after = Duration::from_secs(LEAF_LIFETIME_HOURS as u64 * 3600 * 3 / 4);
		if let Some((created, key)) = &*cached
			&& created.elapsed() < renew_after
		{
			return Ok(Arc::clone(key));
		}
		let mut params = leaf_params(self.name)?;
		params.extended_key_usages.push(ExtendedKeyUsagePurpose::ClientAuth);
		let (mut chain, key) = self.ca.mint_der_with_params(params)?;
		/* Bootstrap clients obtain this public root, but must confirm the TLS-bound pairing code before pinning it. */
		chain.push(self.ca.root_cert_der()?);
		let key = rustls::crypto::ring::sign::any_supported_type(&key)?;
		let key = Arc::new(CertifiedKey::new(chain, key));
		*cached = Some((Instant::now(), Arc::clone(&key)));
		Ok(key)
	}

	pub fn server_config(self: &Arc<Self>) -> Arc<ServerConfig> {
		let config = ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
			.with_no_client_auth()
			.with_cert_resolver(self.clone());
		Arc::new(config)
	}

	/// Accept application requests only from the host whose CA the leaf paired.
	pub fn mutual_server_config(self: &Arc<Self>, peer_root: &[u8]) -> Result<Arc<ServerConfig>> {
		let verifier = rustls::server::WebPkiClientVerifier::builder(roots(peer_root)?).build()?;
		let config = ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
			.with_client_cert_verifier(verifier)
			.with_cert_resolver(self.clone());
		Ok(Arc::new(config))
	}

	pub fn mutual_client_config(self: &Arc<Self>, peer_root: &[u8]) -> Result<Arc<ClientConfig>> {
		let config = ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
			.with_root_certificates(roots(peer_root)?)
			.with_client_cert_resolver(self.clone());
		Ok(Arc::new(config))
	}
}

impl rustls::server::ResolvesServerCert for Identity {
	fn resolve(&self, _hello: rustls::server::ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
		match self.key() {
			Ok(key) => Some(key),
			Err(error) => {
				eprintln!("[testd] [error] transport certificate: {error:#}");
				None
			},
		}
	}
}

impl rustls::client::ResolvesClientCert for Identity {
	fn resolve(&self, _hints: &[&[u8]], schemes: &[SignatureScheme]) -> Option<Arc<CertifiedKey>> {
		match self.key() {
			Ok(key) => key.key.choose_scheme(schemes).map(|_| key),
			Err(error) => {
				eprintln!("[testd] [error] transport client certificate: {error:#}");
				None
			},
		}
	}

	fn has_certs(&self) -> bool {
		true
	}
}

fn roots(der: &[u8]) -> Result<Arc<RootCertStore>> {
	let mut roots = RootCertStore::empty();
	roots.add(CertificateDer::from(der.to_vec())).context("load the paired CA")?;
	Ok(Arc::new(roots))
}

pub fn client_config(peer_root: &[u8]) -> Result<Arc<ClientConfig>> {
	Ok(Arc::new(
		ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
			.with_root_certificates(roots(peer_root)?)
			.with_no_client_auth(),
	))
}

/// Bootstrap only: certificate signatures are checked, but trust requires human confirmation of the TLS exporter.
#[derive(Debug)]
struct PairingVerifier;

impl ServerCertVerifier for PairingVerifier {
	fn verify_server_cert(
		&self,
		end_entity: &CertificateDer<'_>,
		intermediates: &[CertificateDer<'_>],
		server_name: &ServerName<'_>,
		ocsp_response: &[u8],
		now: UnixTime,
	) -> Result<ServerCertVerified, rustls::Error> {
		let (root, intermediates) = intermediates.split_last().ok_or(rustls::Error::NoCertificatesPresented)?;
		let mut roots = RootCertStore::empty();
		roots.add(root.clone())?;
		let verifier = rustls::client::WebPkiServerVerifier::builder(Arc::new(roots))
			.build()
			.map_err(|error| rustls::Error::General(error.to_string()))?;
		verifier.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
	}

	fn verify_tls12_signature(
		&self,
		message: &[u8],
		cert: &CertificateDer<'_>,
		signature: &DigitallySignedStruct,
	) -> Result<HandshakeSignatureValid, rustls::Error> {
		rustls::crypto::verify_tls12_signature(
			message,
			cert,
			signature,
			&rustls::crypto::ring::default_provider().signature_verification_algorithms,
		)
	}

	fn verify_tls13_signature(
		&self,
		message: &[u8],
		cert: &CertificateDer<'_>,
		signature: &DigitallySignedStruct,
	) -> Result<HandshakeSignatureValid, rustls::Error> {
		rustls::crypto::verify_tls13_signature(
			message,
			cert,
			signature,
			&rustls::crypto::ring::default_provider().signature_verification_algorithms,
		)
	}

	fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
		rustls::crypto::ring::default_provider()
			.signature_verification_algorithms
			.supported_schemes()
	}
}

pub fn pairing_config() -> Arc<ClientConfig> {
	Arc::new(
		ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
			.dangerous()
			.with_custom_certificate_verifier(Arc::new(PairingVerifier))
			.with_no_client_auth(),
	)
}

pub fn fingerprint(bytes: &[u8]) -> String {
	format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
	use super::*;
	use rustls::{ClientConnection, Connection, ServerConnection};

	fn exchange(from: &mut Connection, to: &mut Connection) -> Result<()> {
		let mut bytes = Vec::new();
		while from.wants_write() {
			from.write_tls(&mut bytes)?;
		}
		let mut bytes = bytes.as_slice();
		while !bytes.is_empty() {
			to.read_tls(&mut bytes)?;
			to.process_new_packets()?;
		}
		Ok(())
	}

	fn handshake(client: Arc<ClientConfig>, server: Arc<ServerConfig>) -> Result<(Connection, Connection)> {
		let mut client = Connection::Client(ClientConnection::new(client, ServerName::try_from(HOST_NAME)?)?);
		let mut server = Connection::Server(ServerConnection::new(server)?);
		for _ in 0..16 {
			exchange(&mut client, &mut server)?;
			exchange(&mut server, &mut client)?;
			if !client.is_handshaking() && !server.is_handshaking() {
				return Ok((client, server));
			}
		}
		anyhow::bail!("TLS handshake did not finish")
	}

	fn exporter(connection: &Connection) -> [u8; PAIRING_BYTES] {
		connection
			.export_keying_material([0; PAIRING_BYTES], PAIRING_LABEL, None)
			.unwrap()
	}

	#[test]
	fn paired_roots_authenticate_both_directions_and_reject_other_identities() {
		let directory = tempfile::tempdir().unwrap();
		let host = Identity::load(&directory.path().join("host"), HOST_NAME).unwrap();
		let peer = Identity::load(&directory.path().join("peer"), HOST_NAME).unwrap();
		let host_root = host.root_der().unwrap();
		let peer_root = peer.root_der().unwrap();
		let (client, server) = handshake(client_config(&host_root).unwrap(), host.server_config()).unwrap();
		assert_eq!(exporter(&client), exporter(&server));
		assert!(handshake(client_config(&peer_root).unwrap(), host.server_config()).is_err());
		let server = host.mutual_server_config(&peer_root).unwrap();
		assert!(handshake(client_config(&host_root).unwrap(), Arc::clone(&server)).is_err());
		assert!(handshake(peer.mutual_client_config(&host_root).unwrap(), Arc::clone(&server)).is_ok());
		assert!(handshake(host.mutual_client_config(&host_root).unwrap(), server).is_err());
	}

	#[test]
	fn pairing_codes_bind_the_session_and_cannot_be_relayed_between_tls_connections() {
		let directory = tempfile::tempdir().unwrap();
		let host = Identity::load(&directory.path().join("host"), HOST_NAME).unwrap();
		let middleman = Identity::load(&directory.path().join("middleman"), HOST_NAME).unwrap();
		let (leaf, attacker_server) = handshake(pairing_config(), middleman.server_config()).unwrap();
		let (attacker_client, host) = handshake(pairing_config(), host.server_config()).unwrap();
		assert_eq!(exporter(&leaf), exporter(&attacker_server));
		assert_eq!(exporter(&attacker_client), exporter(&host));
		assert_ne!(exporter(&leaf), exporter(&host));
	}

	#[test]
	fn transport_leaves_renew_without_replacing_the_pinned_root() {
		let directory = tempfile::tempdir().unwrap();
		let host = Identity::load(directory.path(), HOST_NAME).unwrap();
		let root = host.root_der().unwrap();
		let first = host.key().unwrap().cert[0].clone();
		host.cached.lock().unwrap().take();
		assert_ne!(first, host.key().unwrap().cert[0]);
		assert!(handshake(client_config(&root).unwrap(), host.server_config()).is_ok());
		assert_eq!(root, host.root_der().unwrap());
	}
}
