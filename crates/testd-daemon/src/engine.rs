//! Host engine: TCP server, connection handling, registration management.
//!
//! A client authenticates with the host's operator secret, or with the
//! daemon's loopback control token for local supervised launches.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, broadcast, mpsc, oneshot, watch};
use std::time::Duration;

use testd_core::Registration;
use testd_core::config::HostConfig;
use testd_core::naming::access_label;
use testd_core::protocol::{Auth, Request, Response, TokenRequest, recv, send};
use testd_core::validate;

use crate::access::{Access, Decision, IssuedToken};
use crate::http::{self, HttpContext, PeekedStream};
use crate::launch::LaunchBook;
use crate::registry::{Claim, DeregisterError, RegisterError, Registry, UpstreamTls};
use testd_pki::transport::{self, Identity};
use crate::resolver::Resolver;

/// Replies buffered for one connection before its writer task stalls the caller.
const OUTBOX_DEPTH: usize = 16;
/// Idle authenticated connections remain valid; setup and writes are bounded.
const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_INTAKE_CONNECTIONS: usize = 256;
/// A single source cannot consume every intake slot.
const MAX_INTAKE_CONNECTIONS_PER_IP: usize = 64;

/// Event emitted when the registration set changes.
#[derive(Debug, Clone)]
pub enum EngineEvent {
	/// The registration set changed. Contains the current snapshot.
	Changed(Vec<Registration>),
}

/// Shared registration state.
pub type Registrations = Arc<Mutex<Registry>>;

/// The host engine. Manages a TCP server and domain registrations.
pub struct HostEngine {
	config: HostConfig,
	tld: String,
	registrations: Registrations,
	resolver: Arc<dyn Resolver>,
	access: Arc<Access>,
	shares: Option<Arc<crate::share::Shares>>,
	event_tx: broadcast::Sender<EngineEvent>,
	/// Accepted from loopback only; lets the local dashboard register supervised
	/// launches without carrying the operator secret.
	local_token: Option<String>,
	/// Separate from both registration and control credentials.
	dashboard_token: Option<String>,
	/// Local-only quick launches, shared with the loopback dashboard.
	launches: Option<Arc<LaunchBook>>,
	updates: Option<Arc<crate::updates::Updates>>,
	/// Re-probe health after a local certificate change.
	health_refresh: Option<Arc<dyn Fn() + Send + Sync>>,
	/// Operator actions from outside the serve loop. Taken once by `serve`.
	operator_tx: mpsc::Sender<OperatorRequest>,
	operator_rx: StdMutex<Option<mpsc::Receiver<OperatorRequest>>>,
}

impl HostEngine {
	pub fn new(config: HostConfig, resolver: Arc<dyn Resolver>, access: Arc<Access>) -> Self {
		let (event_tx, _) = broadcast::channel(16);
		let (operator_tx, operator_rx) = mpsc::channel(16);
		Self {
			config,
			tld: testd_core::config::host_tld().to_owned(),
			registrations: Arc::new(Mutex::new(Registry::default())),
			resolver,
			access,
			shares: None,
			event_tx,
			local_token: None,
			dashboard_token: None,
			launches: None,
			updates: None,
			health_refresh: None,
			operator_tx,
			operator_rx: StdMutex::new(Some(operator_rx)),
		}
	}

	/// Set the per-run token accepted from loopback, used by the local GUI.
	pub fn set_local_token(&mut self, token: String) {
		self.local_token = Some(token);
	}

	pub fn set_dashboard_token(&mut self, token: String) {
		assert!(!token.is_empty(), "the dashboard credential must not be empty");
		self.dashboard_token = Some(token);
	}

	/// Attach the share manager. Only the dashboard and the status API use it.
	pub fn set_shares(&mut self, shares: Arc<crate::share::Shares>) {
		assert!(Arc::ptr_eq(&self.registrations, shares.registrations()), "shares must use the engine's registry");
		self.shares = Some(shares);
	}

	pub fn set_launches(&mut self, launches: Arc<LaunchBook>) {
		self.launches = Some(launches);
	}

	pub fn set_updates(&mut self, updates: Arc<crate::updates::Updates>) {
		self.updates = Some(updates);
	}

	/// Attach the health re-probe used after local certificate changes.
	pub fn set_health_refresh(&mut self, refresh: impl Fn() + Send + Sync + 'static) {
		self.health_refresh = Some(Arc::new(refresh));
	}

	/// The loopback dashboard URL: this listener serves HTTP on the same port.
	pub fn local_dashboard_url(&self) -> String {
		let url = self.config.local_dashboard_url();
		match &self.dashboard_token {
			Some(token) => format!("{url}/#token={token}"),
			None => url,
		}
	}

	pub fn domain_url(&self, domain: &str) -> String {
		testd_core::config::https_url(domain, self.config.https_port())
	}

	/// The live registration set, shared with the edge.
	pub fn registrations(&self) -> Registrations {
		self.registrations.clone()
	}

	/// Subscribe to engine events (registration changes).
	pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
		self.event_tx.subscribe()
	}

	/// Run an operator action in the serve loop, which owns the registration
	/// lock and the live connections.
	pub async fn operator(&self, action: OperatorAction) -> Result<(), String> {
		let (respond, response) = oneshot::channel();
		self.operator_tx
			.send(OperatorRequest { action, respond })
			.await
			.map_err(|_| "the engine is not serving".to_owned())?;
		response.await.map_err(|_| "the engine is not serving".to_owned())?
	}

	/// Bind the intake listener.
	pub async fn bind(&self) -> io::Result<TcpListener> {
		let listener = TcpListener::bind(self.config.listen_addr).await?;
		eprintln!("[listen] {}", self.config.listen_addr);
		Ok(listener)
	}

	/// Run the TCP server. Blocks until the listener fails.
	pub async fn run(&self) -> io::Result<()> {
		let listener = self.bind().await?;
		self.serve(listener).await
	}

	/// Run the TCP server on an already-bound listener.
	pub async fn serve(&self, listener: TcpListener) -> io::Result<()> {
		/* A specific remote bind does not accept loopback dashboard or local launch connections. */
		let local_listener =
			if !self.config.listen_addr.ip().is_loopback() && !self.config.listen_addr.ip().is_unspecified() {
				Some(TcpListener::bind(self.config.local_client_addr()).await?)
			} else {
				None
			};
		let mut operator_rx = self
			.operator_rx
			.lock()
			.expect("engine operator receiver poisoned")
			.take()
			.ok_or_else(|| io::Error::new(io::ErrorKind::AlreadyExists, "the engine is already serving"))?;
		let mut revocations = self.access.revocations();
		let connections = Arc::new(Connections::default());
		let capacity = Arc::new(tokio::sync::Semaphore::new(MAX_INTAKE_CONNECTIONS));
		let per_ip = Arc::new(PerIpConnections::default());

		let http_ctx = Arc::new(HttpContext {
			dashboard_token: self.dashboard_token.clone(),
			tld: self.tld.clone(),
			https_port: self.config.https_port(),
			pki_dir: self.config.pki_dir.clone(),
			registrations: self.registrations.clone(),
			connections: Arc::clone(&connections),
			operator_tx: self.operator_tx.clone(),
			launches: self.launches.clone(),
			updates: self.updates.clone(),
			health_refresh: self.health_refresh.clone(),
			shares: self.shares.clone(),
			access: Arc::clone(&self.access),
			engine_events: self.event_tx.clone(),
		});

		let shared = Arc::new(Shared {
			tld: self.tld.clone(),
			registrations: self.registrations.clone(),
			resolver: self.resolver.clone(),
			apply_lock: Arc::new(Mutex::new(())),
			access: Arc::clone(&self.access),
			shares: self.shares.clone(),
			event_tx: self.event_tx.clone(),
			resolver_checked: AtomicBool::new(false),
			answer_addr: self.config.answer_addr,
			secret: self.config.secret.clone(),
			allow_remote_targets: self.config.allow_remote_targets,
			local_token: self.local_token.clone(),
			pki_dir: self.config.pki_dir.clone(),
			identity: StdMutex::new(None),
			connections,
			#[cfg(test)]
			registration_checked: StdMutex::new(None),
		});

		loop {
			tokio::select! {
				result = async {
					tokio::select! {
						result = listener.accept() => result,
						result = async {
							match &local_listener {
								Some(local) => local.accept().await,
								None => std::future::pending().await,
							}
						} => result,
					}
				} => {
					let (stream, addr) = result?;
					let Ok(permit) = Arc::clone(&capacity).try_acquire_owned() else {
						eprintln!("[warn]   intake connection limit reached: {}", addr.ip());
						continue;
					};
					let Some(per_ip_permit) = per_ip.acquire(addr.ip()) else {
						eprintln!("[warn]   intake per-source connection limit reached: {}", addr.ip());
						continue;
					};
					let shared = shared.clone();
					let http_ctx = http_ctx.clone();

					tokio::spawn(async move {
						let _permit = permit;
						let _per_ip = per_ip_permit;
						if let Err(e) = dispatch(stream, addr, shared, http_ctx).await {
							eprintln!("[error]  {}: {e}", addr.ip());
						}
					});
				}
				Some(request) = operator_rx.recv() => {
					let result = match request.action {
						OperatorAction::Stop { domain } => stop_leaf(&shared, &domain).await,
						OperatorAction::DeleteGrant { domain } => delete_grant(&shared, &domain).await,
					};
					let _ = request.respond.send(result);
				}
				changed = revocations.changed() => {
					changed.map_err(io::Error::other)?;
					for grant in shared.connections.grants() {
						if !shared.access.has_token(&grant) {
							close_grant(&shared, &grant).await;
						}
					}
				}
			}
		}
	}
}

/// A dashboard request that must run in the serve loop, which owns the
/// registration lock and the live connections.
pub struct OperatorRequest {
	pub action: OperatorAction,
	/// Resolves once the action is carried out or refused.
	pub respond: oneshot::Sender<Result<(), String>>,
}

/// The operator's actions on a granted name.
pub enum OperatorAction {
	/// Stop the leaf that publishes `domain`. Its grant is kept, so a later
	/// run publishes the name again without asking.
	Stop { domain: String },
	/// Delete the grant for one name. The leaf keeps its other names and must
	/// request access for this one again.
	DeleteGrant { domain: String },
}

fn same_claim(left: &Claim, right: &Claim) -> bool {
	left.connection == right.connection
		&& left.registration == right.registration
		&& left.transport.as_ref().map(|tls| &tls.root) == right.transport.as_ref().map(|tls| &tls.root)
}

/// Acquire after the registry lock and before making a claim mutation.
async fn claim_transition(shared: &Shared) -> Option<crate::share::ClaimTransition<'_>> {
	match &shared.shares {
		Some(shares) => Some(shares.claim_transition().await),
		None => None,
	}
}

fn begin_changed_share_claim(
	transition: Option<&mut crate::share::ClaimTransition<'_>>,
	registrations: &mut Registry,
	before: Option<&Claim>,
	after: Option<&Claim>,
) -> (Option<crate::share::ClaimChange>, crate::share::DetachedShares) {
	if matches!((before, after), (None, None))
		|| matches!((before, after), (Some(left), Some(right)) if same_claim(left, right))
	{
		return (None, crate::share::DetachedShares::default());
	}
	let domain = before
		.or(after)
		.expect("a changed claim has a domain")
		.registration
		.domain
		.as_str();
	match transition {
		Some(transition) => {
			let (change, detached) = transition.begin_change(registrations, domain);
			(Some(change), detached)
		},
		None => (None, crate::share::DetachedShares::default()),
	}
}

async fn finish_share_claim_change(change: Option<crate::share::ClaimChange>) -> Result<(), String> {
	let result = match change {
		Some(change) => change
			.finish()
			.await
			.map_err(|error| format!("share reconciliation failed: {error:#}")),
		None => Ok(()),
	};
	if let Err(error) = &result {
		eprintln!("[error]  {error}");
	}
	result
}

fn detach_changed_shares(
	transition: Option<&mut crate::share::ClaimTransition<'_>>,
	registrations: &mut Registry,
	before: Option<&Claim>,
	after: Option<&Claim>,
) -> crate::share::DetachedShares {
	let Some(previous) = before else { return crate::share::DetachedShares::default() };
	if after.is_some_and(|current| same_claim(previous, current)) {
		return crate::share::DetachedShares::default();
	}
	match transition {
		Some(transition) => transition.detach_claims(registrations, std::slice::from_ref(previous)),
		None => crate::share::DetachedShares::default(),
	}
}

async fn stop_unverified_share(shared: &Shared, domain: &str) {
	let detached = {
		let mut registrations = shared.registrations.lock().await;
		let mut transition = claim_transition(shared).await;
		transition
			.as_mut()
			.map(|transition| transition.detach_domain(&mut registrations, domain))
			.unwrap_or_default()
	};
	let _ = finish_changed_shares(shared, detached).await;
}

async fn finish_changed_shares(shared: &Shared, detached: crate::share::DetachedShares) -> Result<(), String> {
	let result = match &shared.shares {
		Some(shares) => shares
			.finish_detached(detached)
			.await
			.map_err(|error| format!("share cleanup failed: {error:#}")),
		None => Ok(()),
	};
	if let Err(error) = &result {
		eprintln!("[error]  {error}");
	}
	result
}

/// Stop the leaf that publishes `fqdn` and unpublish the names it holds.
async fn stop_leaf(shared: &Shared, fqdn: &str) -> Result<(), String> {
	let (closing, evicted, detached) = {
		let mut registrations = shared.registrations.lock().await;
		let connection = registrations
			.claim(fqdn)
			.map(|claim| claim.connection)
			.ok_or_else(|| format!("{fqdn} is not registered"))?;
		let mut transition = claim_transition(shared).await;
		let evicted = registrations.release(connection);
		let detached = transition
			.as_mut()
			.map(|transition| transition.detach_connections(&mut registrations, &[connection]))
			.unwrap_or_default();
		(shared.connections.take(connection), evicted, detached)
	};
	for registration in &evicted {
		eprintln!("[evict]  {} (leaf stopped)", registration.domain);
	}
	// Tell the owner so it stops instead of reconnecting. The credential is
	// untouched, so a later run publishes the names again.
	if let Some(connection) = closing {
		connection.close(Some(Response::Stopped { domain: fqdn.to_owned() }));
	}
	let cleanup = finish_changed_shares(shared, detached).await;
	apply(shared)
		.await
		.map_err(|e| format!("{fqdn} was stopped, but the edge could not be updated: {e}"))?;
	cleanup?;
	eprintln!("[stop]   {fqdn} (dashboard)");
	Ok(())
}

/// Delete the grant for one name. The leaf keeps its credential and every
/// other granted name, and has to ask for this one again.
async fn delete_grant(shared: &Shared, fqdn: &str) -> Result<(), String> {
	let (claim, removed, detached) = {
		let mut registrations = shared.registrations.lock().await;
		let claim = registrations
			.claim(fqdn)
			.cloned()
			.ok_or_else(|| format!("{fqdn} is not registered"))?;
		let Some(grant) = shared.connections.grant(claim.connection) else {
			return Err(format!("{fqdn} has no issued grant to delete"));
		};
		/* Permission changes and claim writes share this lock. Legacy tokens are pinned to their live names. */
		let owners: HashSet<u64> = shared.connections.with_token(&grant).into_iter().collect();
		let mut names: Vec<String> = registrations
			.claims()
			.into_iter()
			.filter(|claim| owners.contains(&claim.connection))
			.map(|claim| claim.registration.domain)
			.collect();
		names.sort();
		names.dedup();
		let mut transition = claim_transition(shared).await;
		shared
			.access
			.remove_name(&grant, fqdn, &names)
			.map_err(|e| format!("could not delete the grant: {e}"))?;
		let removed = withdraw_granted_claim(&mut registrations, &claim);
		let after = registrations.claim(fqdn).cloned();
		let detached = detach_changed_shares(transition.as_mut(), &mut registrations, Some(&claim), after.as_ref());
		(claim, removed, detached)
	};
	if removed && let Some(out) = shared.connections.outbox(claim.connection) {
		let notice = Response::NameRevoked { domain: fqdn.to_owned() };
		if out.try_send(notice).is_err() {
			/* A stalled leaf cannot hold up deletion; its other grants remain valid on reconnect. */
			if let Some(connection) = shared.connections.take(claim.connection) {
				connection.close(None);
			}
		}
	}
	let cleanup = finish_changed_shares(shared, detached).await;
	apply(shared)
		.await
		.map_err(|e| format!("{fqdn} was deleted, but the edge could not be updated: {e}"))?;
	cleanup?;
	eprintln!("[grant]  {fqdn} deleted (dashboard)");
	Ok(())
}

fn withdraw_granted_claim(registrations: &mut Registry, claim: &Claim) -> bool {
	registrations.deregister(claim.connection, &claim.registration.domain).is_ok()
}

/// Close every connection admitted with `grant` and release its names.
///
/// A dashboard deletion and the access store's own revocation both lead here;
/// the second pass finds nothing left to do.
async fn close_grant(shared: &Shared, grant: &str) {
	let mut closing = Vec::new();
	let detached = {
		let mut registrations = shared.registrations.lock().await;
		let owners = shared.connections.with_token(grant);
		let mut transition = claim_transition(shared).await;
		for &connection in &owners {
			if let Some(closing_connection) = shared.connections.take(connection) {
				closing.push(closing_connection);
			}
			for registration in registrations.release(connection) {
				eprintln!("[evict]  {} (grant deleted)", registration.domain);
			}
		}
		transition
			.as_mut()
			.map(|transition| transition.detach_connections(&mut registrations, &owners))
			.unwrap_or_default()
	};
	for connection in closing {
		connection.close(Some(Response::GrantRevoked));
	}
	let _ = finish_changed_shares(shared, detached).await;
	if let Err(e) = apply(shared).await {
		eprintln!("[error]  grant deletion could not update the edge: {e}");
	}
}

/// State shared by every connection handler, independent of the connection itself.
struct Shared {
	tld: String,
	registrations: Registrations,
	resolver: Arc<dyn Resolver>,
	/// Resolver snapshots must be applied in the order they are taken.
	apply_lock: Arc<Mutex<()>>,
	access: Arc<Access>,
	shares: Option<Arc<crate::share::Shares>>,
	event_tx: broadcast::Sender<EngineEvent>,
	resolver_checked: AtomicBool,
	answer_addr: std::net::Ipv4Addr,
	/// The operator secret from `[host] secret`.
	secret: String,
	/// Allow a remote leaf to name a target other than its own address.
	allow_remote_targets: bool,
	local_token: Option<String>,
	pki_dir: std::path::PathBuf,
	identity: StdMutex<Option<Arc<Identity>>>,
	/// Authenticated connections the operator can act on.
	connections: Arc<Connections>,
	#[cfg(test)]
	registration_checked: StdMutex<Option<oneshot::Sender<()>>>,
}

/// Closure is independent of the bounded reply queue and preempts stalled I/O.
#[derive(Clone)]
enum ConnectionState {
	Open,
	Closing { notice: Option<Response> },
}

struct Connection {
	/// The issued token that authorizes this connection, if it has one.
	grant: Option<String>,
	out: mpsc::Sender<Response>,
	closing: watch::Sender<ConnectionState>,
}

impl Connection {
	fn close(self, notice: Option<Response>) {
		self.closing.send_replace(ConnectionState::Closing { notice });
	}
}

/// Connections reachable from the dashboard, keyed by connection id.
///
/// A plain mutex keeps every lookup independent of the registration lock, so a
/// dropped connection can clean itself up from a destructor.
#[derive(Default)]
struct PerIpConnections {
	counts: std::sync::Mutex<HashMap<std::net::IpAddr, usize>>,
}

struct PerIpPermit {
	connections: Arc<PerIpConnections>,
	ip: std::net::IpAddr,
}

impl PerIpConnections {
	fn acquire(self: &Arc<Self>, ip: std::net::IpAddr) -> Option<PerIpPermit> {
		let mut counts = self.counts.lock().expect("intake connection map poisoned");
		let count = counts.get(&ip).copied().unwrap_or(0);
		if count >= MAX_INTAKE_CONNECTIONS_PER_IP {
			return None;
		}
		counts.insert(ip, count + 1);
		Some(PerIpPermit { connections: Arc::clone(self), ip })
	}
}

impl Drop for PerIpPermit {
	fn drop(&mut self) {
		let mut counts = self.connections.counts.lock().expect("intake connection map poisoned");
		if let Some(count) = counts.get_mut(&self.ip) {
			*count = count.saturating_sub(1);
			if *count == 0 {
				counts.remove(&self.ip);
			}
		}
	}
}

#[derive(Default)]
pub struct Connections {
	inner: std::sync::Mutex<HashMap<u64, Connection>>,
}

impl Connections {
	fn add(
		&self,
		id: u64,
		grant: Option<String>,
		out: mpsc::Sender<Response>,
		closing: watch::Sender<ConnectionState>,
	) {
		self.inner
			.lock()
			.expect("connection map poisoned")
			.insert(id, Connection { grant, out, closing });
	}

	fn remove(&self, id: u64) {
		self.inner.lock().expect("connection map poisoned").remove(&id);
	}

	fn take(&self, id: u64) -> Option<Connection> {
		self.inner.lock().expect("connection map poisoned").remove(&id)
	}

	fn contains(&self, id: u64) -> bool {
		self.inner.lock().expect("connection map poisoned").contains_key(&id)
	}

	fn grants(&self) -> HashSet<String> {
		self.inner
			.lock()
			.expect("connection map poisoned")
			.values()
			.filter_map(|entry| entry.grant.clone())
			.collect()
	}

	/// Ids of the connections admitted with a given issued token.
	fn with_token(&self, grant: &str) -> Vec<u64> {
		self.inner
			.lock()
			.expect("connection map poisoned")
			.iter()
			.filter(|(_, connection)| connection.grant.as_deref() == Some(grant))
			.map(|(id, _)| *id)
			.collect()
	}

	fn outbox(&self, id: u64) -> Option<mpsc::Sender<Response>> {
		let inner = self.inner.lock().expect("connection map poisoned");
		inner.get(&id).map(|connection| connection.out.clone())
	}

	/// The issued grant behind a connection, if it has one.
	pub(crate) fn grant(&self, id: u64) -> Option<String> {
		let inner = self.inner.lock().expect("connection map poisoned");
		inner.get(&id).and_then(|connection| connection.grant.clone())
	}
}

/// Removes a connection from the operator's map when its handler exits.
struct ConnectionGuard {
	connections: Arc<Connections>,
	connection: u64,
}

impl Drop for ConnectionGuard {
	fn drop(&mut self) {
		self.connections.remove(self.connection);
	}
}

/// How a connection was admitted.
enum Credential {
	/// The daemon's per-run token, used by local supervised launches.
	Local,
	/// An issued leaf token, by id.
	Token(String),
	/// The operator secret.
	Secret,
}

impl Credential {
	/// The issued token to track for revocation, if there is one.
	fn issued(self) -> Option<String> {
		match self {
			Self::Token(id) => Some(id),
			Self::Local | Self::Secret => None,
		}
	}
}

/// Peek the first byte to distinguish NDJSON (`{`) from HTTP, then dispatch.
async fn dispatch(
	mut stream: TcpStream,
	addr: SocketAddr,
	shared: Arc<Shared>,
	http_ctx: Arc<HttpContext>,
) -> io::Result<()> {
	// Enable TCP keepalive to detect half-open connections.
	let sock_ref = socket2::SockRef::from(&stream);
	let keepalive = socket2::TcpKeepalive::new().with_time(std::time::Duration::from_secs(10));
	sock_ref.set_tcp_keepalive(&keepalive).ok();

	let mut first = [0u8; 1];
	tokio::time::timeout(AUTH_TIMEOUT, stream.read_exact(&mut first))
		.await
		.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "intake greeting timed out"))??;
	let peeked = PeekedStream::new(first[0], stream);

	match first[0] {
		0x16 => {
			let identity = load_identity(Arc::clone(&shared)).await?;
			let acceptor = tokio_rustls::TlsAcceptor::from(identity.server_config());
			let stream = tokio::time::timeout(AUTH_TIMEOUT, acceptor.accept(peeked))
				.await
				.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "registration TLS handshake timed out"))??;
			let material = stream
				.get_ref()
				.1
				.export_keying_material([0; transport::PAIRING_BYTES], transport::PAIRING_LABEL, None)
				.map_err(io::Error::other)?;
			let tls = TlsSession { identity, verification: transport::pairing_code(material) };
			handle_connection(stream, addr, shared, Some(tls)).await
		},
		b'{' if addr.ip().is_loopback() => handle_connection(peeked, addr, shared, None).await,
		b'{' => Err(io::Error::new(io::ErrorKind::PermissionDenied, "network registration requires TLS")),
		_ => http::handle(peeked, &http_ctx, addr).await,
	}
}

async fn load_identity(shared: Arc<Shared>) -> io::Result<Arc<Identity>> {
	tokio::task::spawn_blocking(move || {
		/* Hold initialization across cancellation so two handshakes cannot create different roots. */
		let mut identity = shared.identity.lock().expect("transport identity lock poisoned");
		if let Some(identity) = &*identity {
			return Ok(Arc::clone(identity));
		}
		let loaded = Identity::load(&shared.pki_dir, transport::HOST_NAME).map_err(io::Error::other)?;
		*identity = Some(Arc::clone(&loaded));
		Ok(loaded)
	})
	.await
	.map_err(io::Error::other)?
}

struct TlsSession {
	identity: Arc<Identity>,
	verification: String,
}

/// Handle a single NDJSON client connection.
async fn handle_connection<S>(
	stream: S,
	addr: SocketAddr,
	shared: Arc<Shared>,
	tls: Option<TlsSession>,
) -> io::Result<()>
where
	S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
	let (reader, writer) = tokio::io::split(stream);
	let mut reader = BufReader::new(reader);

	// The writer lives in its own task so the operator can reach the client at
	// any time, not only in reply to a request.
	let (out, outbox) = mpsc::channel::<Response>(OUTBOX_DEPTH);
	let (closing, mut closing_rx) = watch::channel(ConnectionState::Open);
	let writer = tokio::spawn(write_outbox(writer, outbox, closing_rx.clone(), closing.clone()));

	let auth_msg: Auth = match tokio::time::timeout(AUTH_TIMEOUT, recv(&mut reader))
		.await
		.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "authentication frame timed out"))??
	{
		Some(a) => a,
		None => return Ok(()),
	};
	if let Some(label) = auth_msg.pairing {
		if !auth_msg.token.is_empty() || auth_msg.request_token.is_some() {
			reply(&out, Response::terminal_error("pairing must not carry a credential or token request")).await?;
			return Ok(());
		}
		let Some(tls) = &tls else {
			reply(&out, Response::terminal_error("pairing requires TLS")).await?;
			return Ok(());
		};
		let result = pair_host(&shared, &label, addr, &tls.verification, &mut reader, &out).await;
		drop(out);
		let _ = writer.await;
		return result;
	}
	let credential = match auth_msg.request_token {
		Some(request) => {
			if !auth_msg.token.is_empty() {
				let message = "present a token or request one, not both".to_owned();
				reply(&out, Response::error(message)).await?;
				return Ok(());
			}
			match request_access(&shared, request, addr, &mut reader, &out).await? {
				Some(issued) => {
					reply(&out, Response::Token { token: issued.token.clone() }).await?;
					Credential::Token(issued.id)
				},
				None => return Ok(()),
			}
		},
		None => {
			let Some(credential) = authenticate(&shared, &auth_msg.token, addr) else {
				reply(&out, Response::error("Authentication failed")).await?;
				eprintln!("[auth]   {} rejected", addr.ip());
				return Ok(());
			};
			reply(&out, Response::Ok).await?;
			credential
		},
	};

	let connection = shared.registrations.lock().await.connection_id();
	let grant = credential.issued();
	shared.connections.add(connection, grant.clone(), out.clone(), closing.clone());
	let guard = ConnectionGuard { connections: Arc::clone(&shared.connections), connection };
	/* Authentication can race revocation before this connection enters the map. */
	if grant.as_ref().is_some_and(|id| !shared.access.has_token(id)) {
		closing.send_replace(ConnectionState::Closing { notice: Some(Response::GrantRevoked) });
	}

	let mut reader = RequestReader { reader, frame: Vec::new(), deferred: VecDeque::new() };
	let processing = async {
		loop {
			let request: Request = match reader.next().await {
				Ok(Some(r)) => r,
				Ok(None) => break,
				Err(error) => return Err(error),
			};

			let upstream = match (&request, &tls) {
				(Request::RegisterTls { root, .. }, Some(tls)) => match tls.identity.mutual_client_config(root) {
					Ok(client) => Some(UpstreamTls { root: root.clone(), client }),
					Err(error) => {
						reply(&out, Response::terminal_error(format!("invalid leaf TLS root: {error:#}"))).await?;
						continue;
					},
				},
				(Request::RegisterTls { .. }, None) | (Request::Register { .. }, Some(_)) => {
					reply(
						&out,
						Response::terminal_error(
							"network claims require TLS on both registration and application paths",
						),
					)
					.await?;
					continue;
				},
				_ => None,
			};
			match request {
				Request::Register { domain, target, force } | Request::RegisterTls { domain, target, force, .. } => {
					let domain = validate::normalize_bare_domain(&domain, &shared.tld);
					if let Err(message) = validate::validate_domain(&domain, &shared.tld) {
						if reply(&out, Response::terminal_error(message)).await.is_err() {
							break;
						}
						continue;
					}
					if let Err(message) = validate::validate_target(&target) {
						if reply(&out, Response::terminal_error(message)).await.is_err() {
							break;
						}
						continue;
					}

					let fqdn = format!("{domain}.{}", shared.tld);
					// The dashboard's own name is never registrable, so refuse it
					// before asking the operator for a grant.
					let reserved = [format!("testd.{}", shared.tld)];
					if reserved.iter().any(|name| name == &fqdn) {
						let message = format!("{fqdn} is a reserved name.");
						if reply(&out, Response::terminal_error(message)).await.is_err() {
							break;
						}
						continue;
					}
					// A leaf on the operator secret is trusted; an issued token only
					// covers the names the operator granted it.
					if let Some(grant) = &grant
						&& !shared.access.allows(grant, &fqdn)
						&& !request_name_grant(&shared, grant, &fqdn, addr, &mut reader, &out).await?
					{
						continue;
					}
					// Bare port → prepend client IP. Full host:port → use as-is.
					let target = if target.contains(':') {
						target
					} else {
						SocketAddr::new(addr.ip(), target.parse().expect("validated port")).to_string()
					};
					if !shared.allow_remote_targets && !addr.ip().is_loopback() && !target_is_client(&target, addr.ip())
					{
						let message = format!(
							"remote leaves may only target their own address: {target}; \
							 set [host] allow_remote_targets = true to permit explicit targets"
						);
						if reply(&out, Response::terminal_error(message)).await.is_err() {
							break;
						}
						continue;
					}
					let registration = Registration { domain: fqdn.clone(), target: target.clone() };

					let reserved: Vec<&str> = reserved.iter().map(String::as_str).collect();
					let (result, change, detached) = {
						#[cfg(test)]
						if let Some(checked) = shared.registration_checked.lock().unwrap().take() {
							checked.send(()).unwrap();
						}
						let mut regs = shared.registrations.lock().await;
						/* Approval may have been withdrawn while this request waited for the registry. */
						if grant.as_ref().is_some_and(|grant| !shared.access.allows(grant, &fqdn)) {
							drop(regs);
							reply(&out, Response::terminal_error(format!("the grant for {fqdn} was deleted"))).await?;
							continue;
						}
						let before = regs.claim(&fqdn).cloned();
						let mut transition = claim_transition(&shared).await;
						let result =
							regs.register_with_transport(connection, registration.clone(), upstream, force, &reserved);
						let after = regs.claim(&fqdn).cloned();
						let (change, detached) =
							begin_changed_share_claim(transition.as_mut(), &mut regs, before.as_ref(), after.as_ref());
						(result, change, detached)
					};
					let cleanup = finish_changed_shares(&shared, detached).await;

					match result {
						Err(RegisterError::Duplicate { owner }) => {
							let message = format!(
								"{fqdn} is already registered by connection {owner}. Use --force to take it over."
							);
							eprintln!("[warn]   {message}");
							if reply(&out, Response::terminal_error(message)).await.is_err() {
								break;
							}
						},
						Err(RegisterError::Reserved) => {
							let message = format!("{fqdn} is a reserved name.");
							if reply(&out, Response::terminal_error(message)).await.is_err() {
								break;
							}
						},
						Ok(registered) => {
							ensure_resolver_for(&shared, addr).await;
							if let Err(e) = apply_with_change(&shared, change.clone()).await {
								let mut regs = shared.registrations.lock().await;
								let previous = registered
									.displaced
									.filter(|claim| shared.connections.contains(claim.connection));
								regs.restore(connection, &fqdn, previous);
								drop(regs);
								let (message, restored) = rollback_error(&shared, e, change.clone()).await;
								if !restored {
									stop_unverified_share(&shared, &fqdn).await;
								}
								let _ = finish_share_claim_change(change).await;
								eprintln!("[error]  {fqdn} apply failed: {message}");
								if reply(&out, Response::error(message)).await.is_err() {
									break;
								}
							} else {
								if validate::is_single_label(&domain) {
									eprintln!("[warn]   {fqdn}: {}", validate::SINGLE_LABEL_HINT);
								}
								eprintln!("[reg]    {fqdn} -> {target}");
								let reconciled = finish_share_claim_change(change).await;
								let response = match cleanup.and(reconciled) {
									Ok(()) => Response::Ok,
									Err(error) => Response::AppliedWithError {
										message: format!("{fqdn} registered, but {error}"),
									},
								};
								if reply(&out, response).await.is_err() {
									break;
								}
							}
						},
					}
				},
				Request::Deregister { domain } => {
					let domain = validate::normalize_bare_domain(&domain, &shared.tld);
					if let Err(message) = validate::validate_domain(&domain, &shared.tld) {
						if reply(&out, Response::error(message)).await.is_err() {
							break;
						}
						continue;
					}

					let fqdn = format!("{domain}.{}", shared.tld);
					let (removed, change, detached) = {
						let mut regs = shared.registrations.lock().await;
						let before = regs.claim(&fqdn).cloned();
						let mut transition = claim_transition(&shared).await;
						let removed = regs
							.deregister(connection, &fqdn)
							.map(|_| before.clone().expect("a successfully removed claim existed"));
						let after = regs.claim(&fqdn).cloned();
						let (change, detached) =
							begin_changed_share_claim(transition.as_mut(), &mut regs, before.as_ref(), after.as_ref());
						(removed, change, detached)
					};
					let cleanup = finish_changed_shares(&shared, detached).await;

					let error = match removed {
						Ok(claim) => match apply_with_change(&shared, change.clone()).await {
							Ok(()) => {
								eprintln!("[dereg]  {fqdn}");
								let reconciled = finish_share_claim_change(change).await;
								let response = match cleanup.and(reconciled) {
									Ok(()) => Response::Ok,
									Err(error) => Response::AppliedWithError {
										message: format!("{fqdn} deregistered, but {error}"),
									},
								};
								if reply(&out, response).await.is_err() {
									break;
								}
								continue;
							},
							Err(e) => {
								// Restore the entry if it could not be unpublished.
								let mut regs = shared.registrations.lock().await;
								if shared.connections.contains(connection) {
									let _ = regs.register_with_transport(
										connection,
										claim.registration.clone(),
										claim.transport.clone(),
										false,
										&[],
									);
								}
								drop(regs);
								let (message, restored) = rollback_error(&shared, e, change.clone()).await;
								if !restored {
									stop_unverified_share(&shared, &fqdn).await;
								}
								let _ = finish_share_claim_change(change).await;
								eprintln!("[error]  {fqdn} apply failed: {message}");
								message
							},
						},
						Err(DeregisterError::NotFound) => format!("{fqdn} is not registered."),
						Err(DeregisterError::NotOwner { owner }) => format!("{fqdn} is owned by connection {owner}."),
					};

					if reply(&out, Response::error(error)).await.is_err() {
						break;
					}
				},
			}
		}
		Ok::<_, io::Error>(())
	};
	let result = tokio::select! {
		biased;
		_ = wait_for_close(&mut closing_rx) => Ok(()),
		result = processing => result,
	};

	// Removing the connection and its claims is atomic with a failed takeover's rollback.
	let (evicted, detached) = {
		let mut registrations = shared.registrations.lock().await;
		let mut transition = claim_transition(&shared).await;
		drop(guard);
		let evicted = registrations.release(connection);
		let detached = transition
			.as_mut()
			.map(|transition| transition.detach_connections(&mut registrations, &[connection]))
			.unwrap_or_default();
		(evicted, detached)
	};
	let _ = finish_changed_shares(&shared, detached).await;
	if !evicted.is_empty() {
		for registration in &evicted {
			eprintln!("[evict]  {}", registration.domain);
		}
		if let Err(e) = apply(&shared).await {
			eprintln!("[error]  apply failed during eviction: {e}");
		}
	}

	drop(out);
	let _ = writer.await;
	result
}

async fn wait_for_close(closing: &mut watch::Receiver<ConnectionState>) -> Option<Response> {
	loop {
		if let ConnectionState::Closing { notice } = closing.borrow_and_update().clone() {
			return notice;
		}
		if closing.changed().await.is_err() {
			/* Ordinary disconnects drain queued replies when the sender is dropped. */
			std::future::pending::<()>().await;
		}
	}
}

async fn write_outbox<W: AsyncWrite + Unpin>(
	mut writer: W,
	mut outbox: mpsc::Receiver<Response>,
	mut closing: watch::Receiver<ConnectionState>,
	closing_signal: watch::Sender<ConnectionState>,
) {
	let notice = tokio::select! {
		biased;
		notice = wait_for_close(&mut closing) => notice,
		_ = async {
			while let Some(response) = outbox.recv().await {
				if !matches!(tokio::time::timeout(WRITE_TIMEOUT, send(&mut writer, &response)).await, Ok(Ok(()))) {
					break;
				}
			}
		} => {
			closing_signal.send_replace(ConnectionState::Closing { notice: None });
			None
		},
	};
	/* Closure delivery is best effort; dropping both halves is unconditional. */
	let _ = tokio::time::timeout(CLOSE_TIMEOUT, async {
		if let Some(notice) = notice {
			let _ = send(&mut writer, &notice).await;
		}
		let _ = writer.shutdown().await;
	})
	.await;
}

/// Queue one reply without letting a stalled peer trap its reader task.
async fn reply(out: &mpsc::Sender<Response>, response: Response) -> io::Result<()> {
	tokio::time::timeout(WRITE_TIMEOUT, out.send(response))
		.await
		.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "reply queue stalled"))?
		.map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "connection closed"))
}

/// The resolver is only needed when a client registers from another machine.
/// Configure it once, lazily, and fail open: the claim is still accepted even
/// if the privileged step is refused.
async fn ensure_resolver_for(shared: &Shared, addr: SocketAddr) {
	if addr.ip().is_loopback() || shared.resolver_checked.swap(true, Ordering::SeqCst) {
		return;
	}
	let resolver = Arc::clone(&shared.resolver);
	match tokio::task::spawn_blocking(move || resolver.ensure_os()).await {
		Ok(Ok(())) => {},
		Ok(Err(e)) => eprintln!("[warn]   resolver setup failed: {e}; run `testd doctor --fix` on the host"),
		Err(e) => eprintln!("[warn]   resolver setup task failed: {e}"),
	}
}

/// Whether an explicit target points at the leaf's own address.
fn target_is_client(target: &str, client: std::net::IpAddr) -> bool {
	target.parse::<SocketAddr>().is_ok_and(|addr| addr.ip() == client)
}

/// A connection is admitted with the operator secret, an issued token, or the
/// dashboard's per-run token when it comes from loopback.
fn authenticate(shared: &Shared, token: &str, addr: SocketAddr) -> Option<Credential> {
	if token.is_empty() {
		return None;
	}
	if addr.ip().is_loopback() && shared.local_token.as_deref() == Some(token) {
		eprintln!("[auth]   {} (local)", addr.ip());
		return Some(Credential::Local);
	}
	if let Some(issued) = shared.access.authenticate(token) {
		eprintln!(
			"[auth]   {} (token {}: {})",
			addr.ip(),
			issued.id,
			access_label(&issued.label, issued.names.as_deref())
		);
		return Some(Credential::Token(issued.id));
	}
	if shared.secret.is_empty() {
		eprintln!("[warn]   no operator secret configured; run `testd init` on the host");
		return None;
	}
	if token == shared.secret {
		eprintln!("[auth]   {}", addr.ip());
		return Some(Credential::Secret);
	}
	None
}

async fn pair_host<R: AsyncBufRead + Unpin>(
	shared: &Shared,
	label: &str,
	addr: SocketAddr,
	verification: &str,
	reader: &mut R,
	out: &mpsc::Sender<Response>,
) -> io::Result<()> {
	let (id, mut decision) = match shared.access.request_pairing(label, &addr.to_string(), verification.to_owned()) {
		Ok(waiting) => waiting,
		Err(message) => return reply(out, Response::terminal_error(message)).await,
	};
	let _cancel = PendingGuard { access: Arc::clone(&shared.access), id: id.clone() };
	reply(out, Response::Pending { id }).await?;
	let mut byte = [0; 1];
	let response = tokio::select! {
		decision = &mut decision => match decision {
			Ok(Decision::Paired) => Response::Paired,
			_ => Response::terminal_error("pairing was denied or cancelled"),
		},
		_ = reader.read(&mut byte) => return Ok(()),
		_ = tokio::time::sleep(crate::access::REQUEST_TTL) => Response::terminal_error("pairing expired"),
	};
	reply(out, response).await
}

/// Wait for an operator to decide a token request on this connection. Returns
/// the approved token, or `None` after telling the client why it got nothing.
async fn request_access<R>(
	shared: &Shared,
	request: TokenRequest,
	addr: SocketAddr,
	reader: &mut R,
	out: &mpsc::Sender<Response>,
) -> io::Result<Option<IssuedToken>>
where
	R: AsyncBufRead + Unpin,
{
	// The operator approves names, so they are normalized the same way the
	// registration path normalizes them.
	let names: Vec<String> = request
		.names
		.iter()
		.map(|domain| validate::normalize_bare_domain(domain, &shared.tld))
		.filter(|domain| validate::validate_domain(domain, &shared.tld).is_ok())
		.map(|domain| format!("{domain}.{}", shared.tld))
		.collect();
	let label = access_label(&request.label, Some(&names));
	let (id, mut decision) = match shared.access.request(&request.label, &addr.to_string(), names) {
		Ok(waiting) => waiting,
		Err(message) => {
			reply(out, Response::error(message)).await?;
			return Ok(None);
		},
	};
	// Any exit from this function, including a dropped connection, clears the
	// request from the operator's queue.
	let _cancel = PendingGuard { access: Arc::clone(&shared.access), id: id.clone() };
	eprintln!("[auth]   {} requested access ({id}): {label}", addr.ip());
	reply(out, Response::Pending { id: id.clone() }).await?;

	let mut byte = [0u8; 1];
	let outcome = tokio::select! {
		decision = &mut decision => Waited::Decided(decision.ok()),
		read = reader.read(&mut byte) => {
			let _ = read;
			Waited::Closed
		},
		_ = tokio::time::sleep(crate::access::REQUEST_TTL) => Waited::Expired,
	};

	match outcome {
		Waited::Decided(Some(Decision::Approve(issued))) => {
			eprintln!("[auth]   {} approved ({id})", addr.ip());
			Ok(Some(issued))
		},
		Waited::Decided(Some(Decision::ApproveName | Decision::Paired)) => {
			let message = "the operator approved a name grant, not a token".to_owned();
			reply(out, Response::error(message)).await?;
			Ok(None)
		},
		Waited::Decided(Some(Decision::Deny)) => {
			let message = "the operator denied this access request".to_owned();
			reply(out, Response::error(message)).await?;
			eprintln!("[auth]   {} denied ({id})", addr.ip());
			Ok(None)
		},
		Waited::Decided(None) => {
			let message = "the access request is no longer pending".to_owned();
			let _ = reply(out, Response::error(message)).await;
			Ok(None)
		},
		Waited::Closed => Ok(None),
		Waited::Expired => {
			let message = "the access request expired without approval".to_owned();
			let _ = reply(out, Response::error(message)).await;
			eprintln!("[auth]   {} request expired ({id})", addr.ip());
			Ok(None)
		},
	}
}

/// Ask the operator for a grant on one name and wait for the answer. The
/// register request that triggered it stays open until the operator decides or
/// the request expires.
async fn request_name_grant<R: AsyncBufRead + Unpin>(
	shared: &Shared,
	grant: &str,
	fqdn: &str,
	addr: SocketAddr,
	reader: &mut RequestReader<R>,
	out: &mpsc::Sender<Response>,
) -> io::Result<bool> {
	let (id, mut decision) = match shared.access.request_name(grant, fqdn, &addr.to_string()) {
		Ok(waiting) => waiting,
		Err(message) => {
			reply(out, Response::error(message)).await?;
			return Ok(false);
		},
	};
	let _cancel = PendingGuard { access: Arc::clone(&shared.access), id: id.clone() };
	eprintln!("[auth]   {} wants {fqdn} ({id})", addr.ip());
	reply(out, Response::PendingGrant { id: id.clone(), domain: fqdn.to_owned() }).await?;

	let outcome = tokio::select! {
		decision = &mut decision => decision.ok(),
		closed = reader.buffer_until_disconnect() => {
			closed?;
			return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "leaf disconnected during grant approval"));
		},
		_ = tokio::time::sleep(crate::access::REQUEST_TTL) => None,
	};
	match outcome {
		Some(Decision::ApproveName) => {
			eprintln!("[auth]   {} approved {fqdn} ({id})", addr.ip());
			Ok(true)
		},
		Some(Decision::Deny) => {
			let message = "the operator denied the grant for this name".to_owned();
			reply(out, Response::terminal_error(message)).await?;
			eprintln!("[auth]   {} denied {fqdn} ({id})", addr.ip());
			Ok(false)
		},
		Some(Decision::Approve(_) | Decision::Paired) => {
			let message = "the operator approved a different request, not a name grant".to_owned();
			reply(out, Response::error(message)).await?;
			Ok(false)
		},
		None => {
			let message = "the grant request expired without approval".to_owned();
			reply(out, Response::error(message)).await?;
			eprintln!("[auth]   {} grant request for {fqdn} expired ({id})", addr.ip());
			Ok(false)
		},
	}
}

/// Partial frames survive cancellation when a grant decision interrupts reads.
struct RequestReader<R> {
	reader: R,
	frame: Vec<u8>,
	deferred: VecDeque<Request>,
}

impl<R: AsyncBufRead + Unpin> RequestReader<R> {
	async fn next(&mut self) -> io::Result<Option<Request>> {
		match self.deferred.pop_front() {
			Some(request) => Ok(Some(request)),
			None => self.read().await,
		}
	}

	async fn read(&mut self) -> io::Result<Option<Request>> {
		/* Idle leaves keep their claims; started frames have a deadline. */
		if self.frame.is_empty() && self.reader.fill_buf().await?.is_empty() {
			return Ok(None);
		}
		let complete = tokio::time::timeout(
			AUTH_TIMEOUT,
			testd_core::protocol::read_line_into(
				&mut self.reader,
				&mut self.frame,
				testd_core::protocol::MAX_FRAME_BYTES,
			),
		)
		.await
		.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "request frame timed out"))??;
		if !complete {
			return Ok(None);
		}
		let request = serde_json::from_slice(&self.frame).map_err(io::Error::other)?;
		self.frame.clear();
		Ok(Some(request))
	}

	/// Detect EOF during approval while preserving a bounded pipeline.
	async fn buffer_until_disconnect(&mut self) -> io::Result<()> {
		while let Some(request) = self.read().await? {
			if self.deferred.len() >= OUTBOX_DEPTH {
				return Err(io::Error::new(io::ErrorKind::InvalidData, "too many pipelined requests"));
			}
			self.deferred.push_back(request);
		}
		Ok(())
	}
}

enum Waited {
	Decided(Option<Decision>),
	Closed,
	Expired,
}

struct PendingGuard {
	access: Arc<Access>,
	id: String,
}

impl Drop for PendingGuard {
	fn drop(&mut self) {
		self.access.cancel(&self.id);
	}
}

async fn rollback_error(
	shared: &Shared,
	error: io::Error,
	change: Option<crate::share::ClaimChange>,
) -> (String, bool) {
	match apply_with_change(shared, change).await {
		Ok(()) => (error.to_string(), true),
		Err(rollback) => (format!("{error}; rollback could not update the resolver: {rollback}"), false),
	}
}

struct ApplyContext {
	registrations: Registrations,
	resolver: Arc<dyn Resolver>,
	lock: Arc<Mutex<()>>,
	shares: Option<Arc<crate::share::Shares>>,
	events: broadcast::Sender<EngineEvent>,
	answer_addr: std::net::Ipv4Addr,
}

/// Continue an in-flight resolver update even if its requesting connection closes.
async fn apply_with_change(shared: &Shared, change: Option<crate::share::ClaimChange>) -> io::Result<()> {
	let context = ApplyContext {
		registrations: shared.registrations.clone(),
		resolver: Arc::clone(&shared.resolver),
		lock: Arc::clone(&shared.apply_lock),
		shares: shared.shares.clone(),
		events: shared.event_tx.clone(),
		answer_addr: shared.answer_addr,
	};
	tokio::spawn(async move {
		let _change = change;
		apply_snapshot(context).await
	})
	.await
	.map_err(io::Error::other)?
}

async fn apply(shared: &Shared) -> io::Result<()> {
	apply_with_change(shared, None).await
}

async fn apply_snapshot(context: ApplyContext) -> io::Result<()> {
	let _apply = context.lock.lock().await;
	let (regs, claims) = {
		let registry = context.registrations.lock().await;
		(registry.snapshot(), registry.claims())
	};
	let names: Vec<crate::resolver::ResolvedName> = regs
		.iter()
		.map(|reg| crate::resolver::ResolvedName::new(reg.domain.clone(), context.answer_addr))
		.collect();
	let resolver = Arc::clone(&context.resolver);
	tokio::task::spawn_blocking(move || resolver.apply(&names))
		.await
		.map_err(io::Error::other)??;
	if let Some(shares) = &context.shares {
		let detached = {
			let mut registry = context.registrations.lock().await;
			let mut transition = shares.claim_transition().await;
			transition.detach_unapplied(&mut registry, &claims)
		};
		if let Err(error) = shares.finish_detached(detached).await {
			eprintln!("[error]  share cleanup failed after resolver update: {error:#}");
		}
	}
	let _ = context.events.send(EngineEvent::Changed(regs));
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	struct NoopResolver;

	impl Resolver for NoopResolver {
		fn apply(&self, _names: &[crate::resolver::ResolvedName]) -> io::Result<()> {
			Ok(())
		}

		fn cleanup(&self) -> io::Result<()> {
			Ok(())
		}
	}

	#[tokio::test]
	async fn registration_rechecks_grants_after_waiting_for_the_registry() {
		use testd_core::protocol::{recv, send};
		use tokio::io::{AsyncReadExt, BufReader};

		tokio::time::timeout(Duration::from_secs(5), async {
			let dir = tempfile::tempdir().unwrap();
			let access = Arc::new(Access::load(dir.path().join("tokens.json")).unwrap());
			let (request, _decision) = access
				.request("fixture", "127.0.0.1:1", vec!["app.test".into(), "ready.test".into()])
				.unwrap();
			let crate::access::Approved::Token(token) = access.approve(&request).unwrap() else {
				panic!("expected a token")
			};
			let shared = Arc::new(Shared {
				tld: "test".into(),
				registrations: Arc::new(Mutex::new(Registry::default())),
				resolver: Arc::new(NoopResolver),
				apply_lock: Arc::new(Mutex::new(())),
				access: Arc::clone(&access),
				shares: None,
				event_tx: broadcast::channel(16).0,
				resolver_checked: AtomicBool::new(true),
				answer_addr: std::net::Ipv4Addr::LOCALHOST,
				secret: String::new(),
				allow_remote_targets: false,
				local_token: None,
				pki_dir: dir.path().join("pki"),
				identity: StdMutex::new(None),
				connections: Arc::new(Connections::default()),
				registration_checked: StdMutex::new(None),
			});
			let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
			let address = listener.local_addr().unwrap();
			let serving = Arc::clone(&shared);
			let _server = crate::task::BackgroundTask(tokio::spawn(async move {
				let (mut stream, peer) = listener.accept().await.unwrap();
				let mut first = [0];
				stream.read_exact(&mut first).await.unwrap();
				handle_connection(PeekedStream::new(first[0], stream), peer, serving, None)
					.await
					.unwrap();
			}));
			let (reader, mut writer) = TcpStream::connect(address).await.unwrap().into_split();
			let mut reader = BufReader::new(reader);
			send(&mut writer, &Auth { token: token.token, ..Auth::default() })
				.await
				.unwrap();
			assert!(matches!(recv::<_, Response>(&mut reader).await.unwrap(), Some(Response::Ok)));
			let register =
				|domain: &str| Request::Register { domain: domain.into(), target: "3000".into(), force: false };
			/* Complete connection setup before holding the registry lock. */
			send(&mut writer, &register("ready")).await.unwrap();
			assert!(matches!(recv::<_, Response>(&mut reader).await.unwrap(), Some(Response::Ok)));
			let registry = shared.registrations.lock().await;
			let (checked, waiting) = oneshot::channel();
			*shared.registration_checked.lock().unwrap() = Some(checked);
			send(&mut writer, &register("app")).await.unwrap();
			waiting.await.unwrap();
			access.remove_name(&token.id, "app.test", &[]).unwrap();
			drop(registry);
			let response = recv::<_, Response>(&mut reader).await.unwrap();
			assert!(matches!(
				response,
				Some(Response::Error { disposition: Some(testd_core::protocol::ErrorDisposition::Terminal), .. })
			));
			assert!(!shared.registrations.lock().await.contains("app.test"));
		})
		.await
		.expect("registration grant race timed out");
	}

	#[tokio::test]
	async fn a_specific_network_intake_keeps_a_loopback_dashboard_and_local_auth() {
		let dir = tempfile::tempdir().unwrap();
		/* macOS does not provide secondary IPv4 loopback addresses without interface aliases. */
		let intake_addr = if cfg!(target_os = "macos") { "[::1]:0" } else { "127.0.0.2:0" };
		let intake = TcpListener::bind(intake_addr).await.unwrap();
		let port = intake.local_addr().unwrap().port();
		let mut config: HostConfig = serde_json::from_str("{}").unwrap();
		/* Substitute a loopback intake socket while exercising the configured remote-bind path. */
		config.listen_addr = SocketAddr::from(([192, 0, 2, 1], port));
		config.pki_dir = dir.path().join("pki");
		let local = config.local_client_addr();
		let access = Arc::new(Access::load(dir.path().join("tokens.json")).unwrap());
		let mut engine = HostEngine::new(config, Arc::new(NoopResolver), access);
		engine.set_local_token("local-token".into());
		engine.set_dashboard_token("dashboard-token".into());
		let server = crate::task::BackgroundTask(tokio::spawn(async move { engine.serve(intake).await.unwrap() }));
		tokio::time::timeout(Duration::from_secs(3), async {
			let mut stream = loop {
				match TcpStream::connect(local).await {
					Ok(stream) => break stream,
					Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
				}
			};
			stream
				.write_all(
					b"GET /api/status HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer dashboard-token\r\n\r\n",
				)
				.await
				.unwrap();
			let mut response = String::new();
			stream.read_to_string(&mut response).await.unwrap();
			assert!(response.starts_with("HTTP/1.1 200"), "{response}");
			let mut stream = BufReader::new(TcpStream::connect(local).await.unwrap());
			send(stream.get_mut(), &Auth { token: "local-token".into(), ..Auth::default() })
				.await
				.unwrap();
			assert!(matches!(recv::<_, Response>(&mut stream).await.unwrap(), Some(Response::Ok)));
		})
		.await
		.expect("loopback dashboard and authentication must remain available");
		drop(server);
	}

	#[test]
	fn remote_targets_must_match_the_leaf_address() {
		let client = "192.168.1.9".parse().unwrap();
		assert!(target_is_client("192.168.1.9:3000", client));
		assert!(!target_is_client("127.0.0.1:3000", client));
		assert!(!target_is_client("localhost:3000", client));
	}

	#[test]
	fn deleting_a_grant_cannot_withdraw_a_new_owners_claim() {
		let mut registrations = Registry::default();
		let first = registrations.connection_id();
		let second = registrations.connection_id();
		let registration = |target: &str| Registration { domain: "app.test".into(), target: target.into() };
		registrations
			.register(first, registration("127.0.0.1:3000"), false, &[])
			.unwrap();
		let previous = registrations.claim("app.test").unwrap().clone();
		registrations
			.register(second, registration("127.0.0.1:4000"), true, &[])
			.unwrap();

		assert!(!withdraw_granted_claim(&mut registrations, &previous));
		assert_eq!(registrations.claim("app.test").unwrap().connection, second);
		let current = registrations.claim("app.test").unwrap().clone();
		assert!(withdraw_granted_claim(&mut registrations, &current));
		assert!(registrations.claim("app.test").is_none());
	}

	#[tokio::test]
	async fn closure_preempts_a_full_reply_queue_and_a_blocked_writer() {
		let (writer, _unread_peer) = tokio::io::duplex(1);
		let (out, outbox) = mpsc::channel(OUTBOX_DEPTH);
		let (closing, receiver) = watch::channel(ConnectionState::Open);
		for _ in 0..OUTBOX_DEPTH {
			out.try_send(Response::Ok).unwrap();
		}
		let task = tokio::spawn(write_outbox(writer, outbox, receiver, closing.clone()));
		tokio::task::yield_now().await;
		closing.send_replace(ConnectionState::Closing { notice: Some(Response::GrantRevoked) });
		tokio::time::timeout(CLOSE_TIMEOUT + Duration::from_secs(1), task)
			.await
			.expect("closure bypasses the reply queue")
			.unwrap();
		assert!(out.is_closed());
	}
}
