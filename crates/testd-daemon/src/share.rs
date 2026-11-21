//! Public and tailnet sharing: one tunnel per registered name, owned by the daemon.
//!
//! The tunnel has to outlive the terminal that asked for it, so the daemon
//! starts and stops the provider. A tunnel is bound to the owning connection
//! and target; a changed or removed claim cancels pending starts and stops it.
//!
//! Providers are spawned, never linked and never called through an SDK. They
//! differ in what they expose (public internet or tailnet), what target they
//! accept, and how their public URL becomes known, so each one owns those three
//! answers:
//!
//! | Provider | Exposure | Target | URL |
//! |---|---|---|---|
//! | `cloudflared` | public | any reachable target | printed by the provider |
//! | `ngrok` | public | any reachable target | printed by the provider |
//! | `tailscale` | tailnet | loopback only | `tailscale status --json` |

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, mpsc, watch};

use crate::engine::Registrations;
use crate::registry::{Claim, Registry};

/// Provider used when the caller does not choose one.
pub const DEFAULT_PROVIDER: &str = "cloudflared";
/// How long a provider may take to report its public URL.
const URL_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the startup sweep waits for `tailscale serve status`.
const SWEEP_TIMEOUT: Duration = Duration::from_secs(5);
/// Detect a spawned provider's exit without holding the share lock while waiting.
const PROVIDER_EXIT_CHECK_INTERVAL: Duration = Duration::from_secs(1);
/// Bound waits for provider termination and pending-start cleanup.
const PROVIDER_STOP_TIMEOUT: Duration = Duration::from_secs(10);
const SHARE_CLEANUP_TIMEOUT: Duration = Duration::from_secs(60);

/// Where a share is reachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Exposure {
	/// Anyone with the URL, like `cloudflared` and `ngrok`.
	Public,
	/// Devices on the tailnet only, like `tailscale serve`.
	Tailnet,
}

impl Exposure {
	pub fn label(self) -> &'static str {
		match self {
			Exposure::Public => "public",
			Exposure::Tailnet => "tailnet",
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
	Cloudflared,
	Tailscale,
	Ngrok,
}

impl Provider {
	pub fn parse(value: &str) -> Option<Self> {
		match value.trim().to_ascii_lowercase().as_str() {
			"cloudflared" | "cloudflare" => Some(Provider::Cloudflared),
			"tailscale" | "ts" => Some(Provider::Tailscale),
			"ngrok" => Some(Provider::Ngrok),
			_ => None,
		}
	}

	pub fn name(self) -> &'static str {
		match self {
			Provider::Cloudflared => "cloudflared",
			Provider::Tailscale => "tailscale",
			Provider::Ngrok => "ngrok",
		}
	}

	pub fn exposure(self) -> Exposure {
		match self {
			Provider::Cloudflared | Provider::Ngrok => Exposure::Public,
			Provider::Tailscale => Exposure::Tailnet,
		}
	}

	/// Binary invoked for this provider.
	pub fn binary(self) -> &'static str {
		self.name()
	}

	/// Arguments that start the share, given the address it proxies to.
	fn start_args(self, upstream: &str) -> Vec<String> {
		match self {
			Provider::Cloudflared => {
				vec!["tunnel".into(), "--no-autoupdate".into(), "--url".into(), format!("http://{upstream}")]
			},
			// `ngrok http` takes a host:port, not a URL.
			Provider::Ngrok => vec!["http".into(), upstream.to_owned()],
			// `serve` writes config and returns; there is no long-running child.
			Provider::Tailscale => vec!["serve".into(), "--bg".into(), format!("http://{upstream}")],
		}
	}

	/// The public URL in a provider log line.
	fn url_from_line(self, line: &str) -> Option<String> {
		// Stop at whitespace, a quote, a pipe, or the first path segment; providers
		// print URLs inside tables, banners, and JSON log records.
		let rest = &line[line.find("https://")? + "https://".len()..];
		let host = rest
			.split(|c: char| c.is_whitespace() || c == '|' || c == '/' || c == '"' || c == '\\')
			.next()?;
		let known = match self {
			Provider::Cloudflared => host.ends_with(".trycloudflare.com") && host != "api.trycloudflare.com",
			Provider::Ngrok => {
				host.ends_with(".ngrok-free.app")
					|| host.ends_with(".ngrok-free.dev")
					|| host.ends_with(".ngrok.io")
					|| host.ends_with(".ngrok.app")
			},
			Provider::Tailscale => host.ends_with(".ts.net"),
		};
		known.then(|| format!("https://{host}"))
	}
}

/// One active share, as the CLI and the tray see it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ShareView {
	pub name: String,
	pub provider: String,
	pub exposure: Exposure,
	pub target: String,
	pub url: String,
}

/// The local edge that routes a provider's hostname to its authorizing claim.
#[derive(Clone)]
pub struct Front {
	/// Plain HTTP listener of the edge.
	pub plain_addr: SocketAddr,
}

/// The daemon's set of tunnels.
#[derive(Clone)]
pub struct Shares {
	default: Result<Provider, String>,
	/// Overrides the binary for the selected provider; tests and odd installs.
	binary: Option<PathBuf>,
	/// The local edge, for providers that cannot reach the target themselves.
	front: Option<Front>,
	registrations: Registrations,
	state: Arc<Mutex<ShareState>>,
	/// Serializes Tailscale status inspection, startup, and cleanup.
	tailscale_actions: Arc<Mutex<()>>,
	/// Any change to the active set, for live frontends.
	revision: watch::Sender<u64>,
}

#[derive(Default)]
struct ShareState {
	entries: BTreeMap<String, ShareLifecycle>,
	claim_changes: BTreeMap<String, Vec<Arc<()>>>,
	is_shutting_down: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ClaimBinding {
	domain: String,
	connection: u64,
	target: String,
	root: Option<Vec<u8>>,
}

impl ClaimBinding {
	fn from_claim(claim: &Claim) -> Self {
		Self {
			domain: claim.registration.domain.clone(),
			connection: claim.connection,
			target: claim.registration.target.clone(),
			root: claim.transport.as_ref().map(|transport| transport.root.clone()),
		}
	}
}

#[derive(Clone)]
enum PendingResult {
	Running,
	Cleaned,
	Unverified(String),
}

struct Pending {
	identity: Arc<()>,
	binding: ClaimBinding,
	provider: Provider,
	cancel: watch::Sender<bool>,
	finished: watch::Sender<PendingResult>,
}

enum ShareLifecycle {
	Starting(Pending),
	Active(Active),
	/* Resources remain owned by startup or cleanup tasks until they confirm release. */
	Stopping(Pending),
	/* Keep the name and provider reserved when external cleanup cannot be verified. */
	CleanupUnverified(Pending),
}

impl ShareLifecycle {
	fn pending(&self) -> Option<&Pending> {
		match self {
			Self::Starting(pending) | Self::Stopping(pending) | Self::CleanupUnverified(pending) => Some(pending),
			Self::Active(_) => None,
		}
	}

	fn into_pending(self) -> Option<Pending> {
		match self {
			Self::Starting(pending) | Self::Stopping(pending) | Self::CleanupUnverified(pending) => Some(pending),
			Self::Active(_) => None,
		}
	}

	fn binding(&self) -> &ClaimBinding {
		match self {
			Self::Active(share) => &share.binding,
			Self::Starting(pending) | Self::Stopping(pending) | Self::CleanupUnverified(pending) => &pending.binding,
		}
	}

	fn provider(&self) -> Provider {
		match self {
			Self::Active(share) => share.provider,
			Self::Starting(pending) | Self::Stopping(pending) | Self::CleanupUnverified(pending) => pending.provider,
		}
	}
}

#[derive(Default)]
struct StopBatch {
	stopping: Vec<(String, StoppingShare)>,
	waiting: Vec<watch::Receiver<PendingResult>>,
	absence_checks: Vec<(String, Arc<()>)>,
	errors: Vec<String>,
}

impl StopBatch {
	fn is_empty(&self) -> bool {
		self.stopping.is_empty()
			&& self.waiting.is_empty()
			&& self.absence_checks.is_empty()
			&& self.errors.is_empty()
	}
}

#[derive(Default)]
pub(crate) struct DetachedShares {
	task: Option<tokio::task::JoinHandle<Result<()>>>,
}

pub(crate) struct ClaimTransition<'a> {
	shares: &'a Shares,
	state: tokio::sync::MutexGuard<'a, ShareState>,
}

#[derive(Clone)]
pub(crate) struct ClaimChange {
	inner: Arc<ClaimChangeInner>,
}

struct ClaimChangeInner {
	shares: Shares,
	domain: String,
	identity: Arc<()>,
	finished: AtomicBool,
}

impl ClaimChange {
	pub(crate) async fn finish(self) -> Result<()> {
		let result = self
			.inner
			.shares
			.end_claim_change(&self.inner.domain, &self.inner.identity)
			.await;
		self.inner.finished.store(true, Ordering::SeqCst);
		result
	}
}

impl Drop for ClaimChangeInner {
	fn drop(&mut self) {
		if self.finished.load(Ordering::SeqCst) {
			return;
		}
		let shares = self.shares.clone();
		let domain = self.domain.clone();
		let identity = Arc::clone(&self.identity);
		if let Ok(runtime) = tokio::runtime::Handle::try_current() {
			runtime.spawn(async move {
				if let Err(error) = shares.end_claim_change(&domain, &identity).await {
					eprintln!("[error]  could not reconcile an interrupted claim change: {error:#}");
				}
			});
		} else {
			eprintln!("[error]  interrupted claim change for {domain} could not be reconciled");
		}
	}
}

impl ClaimTransition<'_> {
	pub(crate) fn begin_change(&mut self, registry: &mut Registry, domain: &str) -> (ClaimChange, DetachedShares) {
		let identity = Arc::new(());
		self.state
			.claim_changes
			.entry(domain.to_owned())
			.or_default()
			.push(Arc::clone(&identity));
		let names: Vec<String> = self
			.state
			.entries
			.iter()
			.filter(|(_, entry)| entry.pending().is_some() && entry.binding().domain == domain)
			.map(|(name, _)| name.clone())
			.collect();
		let mut batch = StopBatch::default();
		for name in names {
			Shares::detach_name(&mut self.state, registry, &name, &mut batch);
		}
		let change = ClaimChange {
			inner: Arc::new(ClaimChangeInner {
				shares: self.shares.clone(),
				domain: domain.to_owned(),
				identity,
				finished: AtomicBool::new(false),
			}),
		};
		(change, self.shares.queue_cleanup(batch))
	}

	pub(crate) fn detach_claims(&mut self, registry: &mut Registry, claims: &[Claim]) -> DetachedShares {
		let bindings: Vec<ClaimBinding> = claims.iter().map(ClaimBinding::from_claim).collect();
		self.detach_matching(registry, |binding| bindings.contains(binding))
	}

	pub(crate) fn detach_connections(&mut self, registry: &mut Registry, connections: &[u64]) -> DetachedShares {
		self.detach_matching(registry, |binding| connections.contains(&binding.connection))
	}

	pub(crate) fn detach_domain(&mut self, registry: &mut Registry, domain: &str) -> DetachedShares {
		self.detach_matching(registry, |binding| binding.domain == domain)
	}

	pub(crate) fn detach_unapplied(&mut self, registry: &mut Registry, claims: &[Claim]) -> DetachedShares {
		let bindings: Vec<ClaimBinding> = claims.iter().map(ClaimBinding::from_claim).collect();
		self.detach_matching(registry, |binding| !bindings.contains(binding))
	}

	fn detach_matching(&mut self, registry: &mut Registry, matches: impl Fn(&ClaimBinding) -> bool) -> DetachedShares {
		let names: Vec<String> = self
			.state
			.entries
			.iter()
			.filter(|(_, entry)| matches(entry.binding()))
			.map(|(name, _)| name.clone())
			.collect();
		let mut batch = StopBatch::default();
		for name in names {
			Shares::detach_name(&mut self.state, registry, &name, &mut batch);
		}
		self.shares.queue_cleanup(batch)
	}
}

#[derive(Debug)]
struct CleanupUnverified(String);

impl std::fmt::Display for CleanupUnverified {
	fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		formatter.write_str(&self.0)
	}
}

impl std::error::Error for CleanupUnverified {}

enum PublishFailure {
	ClaimChanged,
	AliasConflict,
}

struct ShareStart {
	name: String,
	provider: Provider,
	owner: ClaimBinding,
	identity: Arc<()>,
	cancellation: watch::Receiver<bool>,
}

struct Active {
	/// Distinguishes a stopped share from a replacement with the same name.
	identity: Arc<()>,
	binding: ClaimBinding,
	provider: Provider,
	url: String,
	resource: ProviderResource,
}

enum ProviderResource {
	Process { child: Box<Child>, alias: String },
	Tailscale { alias: String },
}

impl ProviderResource {
	fn alias(&self) -> &str {
		match self {
			Self::Process { alias, .. } | Self::Tailscale { alias } => alias,
		}
	}
}

struct StoppingShare {
	identity: Arc<()>,
	provider: Provider,
	resource: ProviderResource,
}

impl Active {
	fn into_stopping(self) -> StoppingShare {
		StoppingShare { identity: self.identity, provider: self.provider, resource: self.resource }
	}
}

impl Shares {
	/// Default provider from `TESTD_TUNNEL_PROVIDER`, binary from `TESTD_TUNNEL`.
	pub fn from_env(front: Option<Front>, registrations: Registrations) -> Self {
		let binary = std::env::var_os("TESTD_TUNNEL")
			.filter(|value| !value.is_empty())
			.map(PathBuf::from);
		let mut shares = Self::new(Provider::Cloudflared, binary, front, registrations);
		/* Defer invalid-default errors so an explicit provider can still take precedence. */
		shares.default = match std::env::var("TESTD_TUNNEL_PROVIDER") {
			Ok(value) => Provider::parse(&value).ok_or_else(|| "invalid TESTD_TUNNEL_PROVIDER".to_owned()),
			Err(std::env::VarError::NotPresent) => Ok(Provider::Cloudflared),
			Err(std::env::VarError::NotUnicode(_)) => Err("TESTD_TUNNEL_PROVIDER is not valid Unicode".to_owned()),
		};
		shares
	}

	pub fn new(default: Provider, binary: Option<PathBuf>, front: Option<Front>, registrations: Registrations) -> Self {
		Self {
			default: Ok(default),
			binary,
			front,
			registrations,
			state: Arc::new(Mutex::new(ShareState::default())),
			tailscale_actions: Arc::new(Mutex::new(())),
			revision: watch::channel(0).0,
		}
	}

	pub(crate) fn registrations(&self) -> &Registrations {
		&self.registrations
	}

	/// Any change to the active set, for live frontends.
	pub fn state_changes(&self) -> watch::Receiver<u64> {
		self.revision.subscribe()
	}

	fn bump(&self) {
		self.revision.send_modify(|revision| *revision = revision.wrapping_add(1));
	}

	/// Start a share bound to the current registered connection and target.
	pub async fn start(&self, name: &str, domain: &str, provider: Option<Provider>) -> Result<String> {
		if name.is_empty()
			|| !domain
				.strip_prefix(name)
				.is_some_and(|suffix| suffix.starts_with('.') && suffix.len() > 1)
		{
			bail!("share name does not match its registered domain");
		}
		let provider = self.checked_provider(provider)?;
		let registry = self.registrations.lock().await;
		let claim = registry
			.claim(domain)
			.ok_or_else(|| anyhow!("{name} is not registered on this host"))?;
		let start = self.reserve(name, provider, ClaimBinding::from_claim(claim)).await?;
		drop(registry);
		self.run_start(start).await
	}

	fn checked_provider(&self, provider: Option<Provider>) -> Result<Provider> {
		let provider = match provider {
			Some(provider) => provider,
			None => self.default.clone().map_err(anyhow::Error::msg)?,
		};
		if self.front.is_none() {
			bail!("{} needs the plain HTTP routing listener, and this daemon has none", provider.name());
		}
		Ok(provider)
	}

	async fn reserve(&self, name: &str, provider: Provider, binding: ClaimBinding) -> Result<ShareStart> {
		let mut state = self.state.lock().await;
		if state.is_shutting_down {
			bail!("the share manager is shutting down");
		}
		if state.claim_changes.contains_key(&binding.domain) {
			bail!("{} is being registered; wait for its result before sharing", binding.domain);
		}
		match state.entries.get(name) {
			Some(ShareLifecycle::Active(existing)) => {
				bail!("{name} is already shared at {} ({})", existing.url, existing.provider.name());
			},
			Some(_) => bail!("{name} is already starting or stopping a share"),
			None => {},
		}
		if state.entries.values().any(|entry| entry.binding().domain == binding.domain) {
			bail!("{} already has a share starting, running, or stopping", binding.domain);
		}
		if provider == Provider::Tailscale && state.entries.values().any(|entry| entry.provider() == provider) {
			bail!("tailscale serve is already owned by another share");
		}
		let identity = Arc::new(());
		let (cancel, cancellation) = watch::channel(false);
		let (finished, _) = watch::channel(PendingResult::Running);
		state.entries.insert(
			name.to_owned(),
			ShareLifecycle::Starting(Pending {
				identity: Arc::clone(&identity),
				binding: binding.clone(),
				provider,
				cancel,
				finished,
			}),
		);
		Ok(ShareStart { name: name.to_owned(), provider, owner: binding, identity, cancellation })
	}

	async fn run_start(&self, start: ShareStart) -> Result<String> {
		let name = start.name.clone();
		let identity = Arc::clone(&start.identity);
		let shares = self.clone();
		match tokio::spawn(async move {
			let result = shares.finish_start(start).await;
			if let Err(error) = &result {
				eprintln!("[share]  start did not complete: {error:#}");
			}
			result
		})
		.await
		{
			Ok(result) => result,
			Err(error) => {
				self.abandon_pending(&name, &identity, &error.to_string()).await;
				Err(error).context("share start task failed; provider cleanup is unverified")
			},
		}
	}

	async fn finish_start(&self, start: ShareStart) -> Result<String> {
		let binary = self.binary_for(start.provider);
		let mut cancellation = start.cancellation.clone();
		let spawned = self.spawn(&start, &binary, &mut cancellation).await;
		let mut share = match spawned {
			Ok(share) => share,
			Err(error) => {
				if error.is::<CleanupUnverified>() {
					self.abandon_pending(&start.name, &start.identity, &error.to_string()).await;
				} else {
					self.finish_pending(&start.name, &start.identity).await;
				}
				return Err(error);
			},
		};
		let startup = match &mut share.resource {
			ProviderResource::Process { child, .. } => child.try_wait().context("check the provider after startup"),
			ProviderResource::Tailscale { .. } => Ok(None),
		};
		match startup {
			Ok(Some(status)) => {
				self.finish_pending(&start.name, &start.identity).await;
				bail!("{} exited during startup with {status}", start.provider.name());
			},
			Err(error) => {
				self.finish_release(&start.name, share.into_stopping()).await?;
				return Err(error);
			},
			Ok(None) => {},
		}

		let mut registry = self.registrations.lock().await;
		let published = self.publish(&start.name, share, &mut registry).await;
		drop(registry);
		match published {
			Ok(url) => Ok(url),
			Err((PublishFailure::ClaimChanged, share)) => {
				self.finish_release(&start.name, share.into_stopping()).await?;
				bail!("the claim or pending share for {} changed during startup", start.name);
			},
			Err((PublishFailure::AliasConflict, _share)) => {
				let error = "the provider alias changed during startup; inspect the provider state";
				self.abandon_pending(&start.name, &start.identity, error).await;
				Err(CleanupUnverified(error.to_owned()).into())
			},
		}
	}

	async fn publish(
		&self,
		name: &str,
		share: Active,
		registry: &mut Registry,
	) -> std::result::Result<String, (PublishFailure, Box<Active>)> {
		let valid = registry
			.claim(&share.binding.domain)
			.is_some_and(|claim| ClaimBinding::from_claim(claim) == share.binding);
		let mut state = self.state.lock().await;
		let may_publish = matches!(state.entries.get(name), Some(ShareLifecycle::Starting(pending))
			if Arc::ptr_eq(&pending.identity, &share.identity) && !*pending.cancel.borrow());
		if !valid || !may_publish || state.is_shutting_down {
			return Err((PublishFailure::ClaimChanged, Box::new(share)));
		}
		let alias = share.resource.alias();
		if registry.contains(alias) {
			return Err((PublishFailure::AliasConflict, Box::new(share)));
		}
		registry.set_claim_alias(alias, &share.binding.domain);
		let url = share.url.clone();
		let child_identity = match &share.resource {
			ProviderResource::Process { .. } => Some(Arc::clone(&share.identity)),
			ProviderResource::Tailscale { .. } => None,
		};
		let previous = state.entries.insert(name.to_owned(), ShareLifecycle::Active(share));
		let Some(ShareLifecycle::Starting(pending)) = previous else { unreachable!("checked above") };
		pending.finished.send_replace(PendingResult::Cleaned);
		drop(state);
		self.bump();
		if let Some(identity) = child_identity {
			self.watch_provider(name.to_owned(), identity);
		}
		Ok(url)
	}

	async fn finish_pending(&self, name: &str, identity: &Arc<()>) {
		let mut state = self.state.lock().await;
		if state
			.entries
			.get(name)
			.and_then(ShareLifecycle::pending)
			.is_some_and(|pending| Arc::ptr_eq(&pending.identity, identity))
		{
			let pending = state
				.entries
				.remove(name)
				.and_then(ShareLifecycle::into_pending)
				.expect("checked above");
			pending.finished.send_replace(PendingResult::Cleaned);
		}
	}

	async fn abandon_pending(&self, name: &str, identity: &Arc<()>, error: &str) {
		let mut state = self.state.lock().await;
		if state
			.entries
			.get(name)
			.and_then(ShareLifecycle::pending)
			.is_some_and(|pending| Arc::ptr_eq(&pending.identity, identity))
		{
			let pending = state
				.entries
				.remove(name)
				.and_then(ShareLifecycle::into_pending)
				.expect("checked above");
			pending.cancel.send_replace(true);
			pending.finished.send_replace(PendingResult::Unverified(error.to_owned()));
			state
				.entries
				.insert(name.to_owned(), ShareLifecycle::CleanupUnverified(pending));
		}
	}

	async fn finish_release(&self, name: &str, share: StoppingShare) -> Result<()> {
		let identity = Arc::clone(&share.identity);
		match self.release(share).await {
			Ok(()) => {
				self.finish_pending(name, &identity).await;
				Ok(())
			},
			Err(error) => {
				self.abandon_pending(name, &identity, &error.to_string()).await;
				Err(CleanupUnverified(format!("could not stop share {name}: {error:#}")).into())
			},
		}
	}

	/// Remove an exited child without confusing it with a replacement share.
	fn watch_provider(&self, name: String, identity: Arc<()>) {
		let state = Arc::downgrade(&self.state);
		let registrations = Arc::downgrade(&self.registrations);
		let revision = self.revision.clone();
		tokio::spawn(async move {
			let mut reported_failure = false;
			loop {
				tokio::time::sleep(PROVIDER_EXIT_CHECK_INTERVAL).await;
				let Some(state) = state.upgrade() else { return };
				let Some(registrations) = registrations.upgrade() else { return };
				let mut registry = registrations.lock().await;
				let mut shares = state.lock().await;
				let Some(ShareLifecycle::Active(share)) = shares.entries.get_mut(&name) else { return };
				if !Arc::ptr_eq(&share.identity, &identity) {
					return;
				}
				let ProviderResource::Process { child, .. } = &mut share.resource else { return };
				match child.try_wait() {
					Ok(None) => reported_failure = false,
					Ok(Some(status)) => {
						let provider = share.provider.name();
						/* The child is reaped; explicit stop's alias ownership checks still apply. */
						let mut batch = StopBatch::default();
						Self::detach_name(&mut shares, &mut registry, &name, &mut batch);
						if batch.errors.is_empty() {
							let pending = shares
								.entries
								.remove(&name)
								.and_then(ShareLifecycle::into_pending)
								.expect("an exited share was detached");
							pending.finished.send_replace(PendingResult::Cleaned);
						}
						drop(shares);
						drop(registry);
						for error in batch.errors {
							eprintln!("[error]  share cleanup failed: {error}");
						}
						eprintln!("[share]  {name} closed: {provider} exited with {status}");
						revision.send_modify(|value| *value = value.wrapping_add(1));
						return;
					},
					Err(error) => {
						if !reported_failure {
							eprintln!("[warn]   could not check the provider for {name}: {error}");
							reported_failure = true;
						}
					},
				}
			}
		});
	}

	/// Stop a live or pending share and report provider cleanup failures.
	pub async fn stop(&self, name: &str) -> Result<bool> {
		let mut registry = self.registrations.lock().await;
		let mut state = self.state.lock().await;
		let mut batch = StopBatch::default();
		Self::detach_name(&mut state, &mut registry, name, &mut batch);
		drop(state);
		drop(registry);
		let found = !batch.is_empty();
		let detached = self.queue_cleanup(batch);
		self.finish_detached(detached).await?;
		Ok(found)
	}

	pub async fn list(&self) -> Vec<ShareView> {
		self.state
			.lock()
			.await
			.entries
			.iter()
			.filter_map(|(name, entry)| match entry {
				ShareLifecycle::Active(share) => Some((name, share)),
				_ => None,
			})
			.map(|(name, share)| ShareView {
				name: name.clone(),
				provider: share.provider.name().to_owned(),
				exposure: share.provider.exposure(),
				target: share.binding.target.clone(),
				url: share.url.clone(),
			})
			.collect()
	}

	fn detach_name(state: &mut ShareState, registry: &mut Registry, name: &str, detached: &mut StopBatch) {
		let Some(entry) = state.entries.remove(name) else { return };
		let next = match entry {
			ShareLifecycle::Active(share) => {
				let (cancel, _) = watch::channel(true);
				let (finished, _) = watch::channel(PendingResult::Running);
				let alias = share.resource.alias();
				let owns_alias = registry.route(alias).is_some_and(|claim| {
					claim.connection == share.binding.connection
						&& claim.registration.target == share.binding.target
						&& claim.transport.as_ref().map(|tls| &tls.root) == share.binding.root.as_ref()
				});
				let alias_error = if owns_alias && registry.remove_alias(alias) {
					None
				} else {
					Some(format!("provider alias {alias} changed; leaving provider state untouched"))
				};
				let pending = Pending {
					identity: Arc::clone(&share.identity),
					binding: share.binding.clone(),
					provider: share.provider,
					cancel,
					finished,
				};
				match alias_error {
					Some(error) => {
						pending.finished.send_replace(PendingResult::Unverified(error.clone()));
						detached.errors.push(error);
						ShareLifecycle::CleanupUnverified(pending)
					},
					None => {
						detached.stopping.push((name.to_owned(), share.into_stopping()));
						ShareLifecycle::Stopping(pending)
					},
				}
			},
			ShareLifecycle::Starting(pending) => {
				pending.cancel.send_replace(true);
				detached.waiting.push(pending.finished.subscribe());
				ShareLifecycle::Stopping(pending)
			},
			ShareLifecycle::Stopping(pending) => {
				detached.waiting.push(pending.finished.subscribe());
				ShareLifecycle::Stopping(pending)
			},
			ShareLifecycle::CleanupUnverified(pending) => {
				if pending.provider == Provider::Tailscale {
					detached.absence_checks.push((name.to_owned(), Arc::clone(&pending.identity)));
				} else {
					detached.waiting.push(pending.finished.subscribe());
				}
				ShareLifecycle::CleanupUnverified(pending)
			},
		};
		state.entries.insert(name.to_owned(), next);
	}

	/// Acquire after the registry lock and before changing any claim.
	pub(crate) async fn claim_transition(&self) -> ClaimTransition<'_> {
		ClaimTransition { shares: self, state: self.state.lock().await }
	}

	fn queue_cleanup(&self, batch: StopBatch) -> DetachedShares {
		if batch.is_empty() {
			return DetachedShares::default();
		}
		let shares = self.clone();
		DetachedShares {
			task: Some(tokio::spawn(async move {
				let result = shares.finish_stop_batch(batch).await;
				if let Err(error) = &result {
					eprintln!("[error]  share cleanup failed: {error:#}");
				}
				result
			})),
		}
	}

	pub(crate) async fn finish_detached(&self, detached: DetachedShares) -> Result<()> {
		match detached.task {
			Some(task) => task.await.context("share cleanup task failed")?,
			None => Ok(()),
		}
	}

	async fn finish_stop_batch(&self, detached: StopBatch) -> Result<()> {
		if !detached.stopping.is_empty() || !detached.errors.is_empty() {
			self.bump();
		}
		let mut errors = detached.errors;
		for (name, share) in detached.stopping {
			if let Err(error) = self.finish_release(&name, share).await {
				errors.push(error.to_string());
			}
			self.bump();
		}
		for (name, identity) in detached.absence_checks {
			if let Err(error) = self.finish_tailscale_absence_check(&name, &identity).await {
				errors.push(error.to_string());
			}
		}
		for mut finished in detached.waiting {
			loop {
				let result = finished.borrow_and_update().clone();
				match result {
					PendingResult::Cleaned => break,
					PendingResult::Unverified(error) => {
						errors.push(error);
						break;
					},
					PendingResult::Running => {},
				}
				match tokio::time::timeout(SHARE_CLEANUP_TIMEOUT, finished.changed()).await {
					Ok(Ok(())) => {},
					_ => {
						errors.push("a pending share ended without confirmed cleanup".to_owned());
						break;
					},
				}
			}
		}
		if !errors.is_empty() {
			bail!("{}", errors.join("; "));
		}
		Ok(())
	}

	async fn finish_tailscale_absence_check(&self, name: &str, identity: &Arc<()>) -> Result<()> {
		let _guard = self.tailscale_actions.lock().await;
		{
			let state = self.state.lock().await;
			if !matches!(state.entries.get(name), Some(ShareLifecycle::CleanupUnverified(pending))
				if Arc::ptr_eq(&pending.identity, identity))
			{
				return Ok(());
			}
		}
		/* An operator may have removed the route after a failed cleanup; never delete it on a retry. */
		let binary = self.binary_for(Provider::Tailscale);
		let status = run_briefly(&binary, &["serve".into(), "status".into(), "--json".into()], SWEEP_TIMEOUT)
			.await?
			.context("tailscale serve status timed out while verifying prior cleanup")?;
		if !status.status.success() || !is_443_available(&String::from_utf8_lossy(&status.stdout)) {
			bail!("share {name} cleanup remains unverified; provider state was left untouched");
		}
		self.finish_pending(name, identity).await;
		self.bump();
		Ok(())
	}

	async fn end_claim_change(&self, domain: &str, identity: &Arc<()>) -> Result<()> {
		let mut registry = self.registrations.lock().await;
		let mut state = self.state.lock().await;
		if let Some(changes) = state.claim_changes.get_mut(domain) {
			changes.retain(|change| !Arc::ptr_eq(change, identity));
			if changes.is_empty() {
				state.claim_changes.remove(domain);
			}
		}
		let mut batch = StopBatch::default();
		if !state.claim_changes.contains_key(domain) {
			let current = registry.claim(domain).map(ClaimBinding::from_claim);
			let names: Vec<String> = state
				.entries
				.iter()
				.filter(|(_, entry)| entry.binding().domain == domain && Some(entry.binding()) != current.as_ref())
				.map(|(name, _)| name.clone())
				.collect();
			for name in names {
				Self::detach_name(&mut state, &mut registry, &name, &mut batch);
			}
		}
		drop(state);
		drop(registry);
		let detached = self.queue_cleanup(batch);
		self.finish_detached(detached).await
	}

	/// Stop shares whose owning connection or target no longer matches the live claim.
	pub(crate) async fn reconcile(&self) -> Result<()> {
		let mut registry = self.registrations.lock().await;
		let valid = |binding: &ClaimBinding| {
			registry
				.claim(&binding.domain)
				.is_some_and(|claim| ClaimBinding::from_claim(claim) == *binding)
		};
		let mut state = self.state.lock().await;
		let stale: Vec<String> = state
			.entries
			.iter()
			.filter(|(_, entry)| !state.claim_changes.contains_key(&entry.binding().domain) && !valid(entry.binding()))
			.map(|(name, _)| name.clone())
			.collect();
		let mut batch = StopBatch::default();
		for name in stale {
			Self::detach_name(&mut state, &mut registry, &name, &mut batch);
		}
		drop(state);
		let detached = self.queue_cleanup(batch);
		drop(registry);
		self.finish_detached(detached).await
	}

	/** Report matching persistent serve entries without assuming they belong to testd. */
	pub async fn warn_unowned_serves(&self) {
		let Some(front) = &self.front else { return };
		let _guard = self.tailscale_actions.lock().await;
		/* Do not report a live share as an orphan while starting or stopping it. */
		{
			let state = self.state.lock().await;
			if !state.entries.is_empty() {
				return;
			}
		}

		let binary = self.binary_for(Provider::Tailscale);
		let upstream = format!("http://{}", front.plain_addr);
		let args = vec!["serve".to_owned(), "status".to_owned(), "--json".to_owned()];
		let Ok(Some(output)) = run_briefly(&binary, &args, SWEEP_TIMEOUT).await else { return };
		if !output.status.success() {
			return;
		}
		for port in serve_ports_pointing_at(&String::from_utf8_lossy(&output.stdout), &upstream) {
			eprintln!(
				"[warn]   tailscale serve :{port} points at {upstream}, but ownership is unknown; \
				 left untouched. Inspect `{} serve status` before sharing",
				binary.display()
			);
		}
	}

	/// Stop everything, including pending starts, for an orderly shutdown.
	pub async fn shutdown(&self) -> Result<()> {
		let mut registry = self.registrations.lock().await;
		let mut state = self.state.lock().await;
		state.is_shutting_down = true;
		let names: Vec<String> = state.entries.keys().cloned().collect();
		let mut batch = StopBatch::default();
		for name in names {
			Self::detach_name(&mut state, &mut registry, &name, &mut batch);
		}
		drop(state);
		drop(registry);
		let detached = self.queue_cleanup(batch);
		self.finish_detached(detached).await
	}

	fn binary_for(&self, provider: Provider) -> PathBuf {
		self.binary.clone().unwrap_or_else(|| PathBuf::from(provider.binary()))
	}

	async fn spawn(
		&self,
		start: &ShareStart,
		binary: &Path,
		cancellation: &mut watch::Receiver<bool>,
	) -> Result<Active> {
		let ShareStart { name, provider, .. } = start;
		if *provider == Provider::Tailscale {
			return self.spawn_tailscale(start, binary, cancellation).await;
		}
		if *cancellation.borrow() {
			bail!("share start for {name} was cancelled");
		}
		/* Never let a public provider outlive the claim by pointing it at the raw target. */
		let front = self.front.as_ref().context("public shares require the local routing edge")?;
		let upstream = front.plain_addr.to_string();
		let args = provider.start_args(&upstream);
		let mut child = spawn_with_retry(binary, &args, true)
			.await
			.with_context(|| format!("start {} (install it, or set TESTD_TUNNEL to its path)", binary.display()))?;
		let result = tokio::select! {
			biased;
			_ = wait_for_cancellation(cancellation) => Err(anyhow!("share start for {name} was cancelled")),
			url = self.read_url(&mut child, *provider, binary) => url,
		};
		let url = match result {
			Ok(url) => url,
			Err(error) => {
				if let Err(cleanup) = stop_child(&mut child).await {
					return Err(CleanupUnverified(format!("{error}; provider cleanup failed: {cleanup}")).into());
				}
				return Err(error);
			},
		};
		let alias = url.strip_prefix("https://").context("provider returned a non-HTTPS URL")?.to_owned();
		Ok(Active {
			identity: Arc::clone(&start.identity),
			binding: start.owner.clone(),
			provider: *provider,
			url,
			resource: ProviderResource::Process { child: Box::new(child), alias },
		})
	}

	/// Configure persistent Serve state without overwriting an existing HTTPS route.
	async fn spawn_tailscale(
		&self,
		start: &ShareStart,
		binary: &Path,
		cancellation: &mut watch::Receiver<bool>,
	) -> Result<Active> {
		let ShareStart { name, .. } = start;
		/* Keep startup inspection and serve changes ordered. */
		let _guard = tokio::select! {
			biased;
			_ = wait_for_cancellation(cancellation) => bail!("share start for {name} was cancelled"),
			guard = self.tailscale_actions.lock() => guard,
		};
		let front = self
			.front
			.as_ref()
			.context("the built-in edge is not running, so the provider has no local front")?
			.clone();
		let host = tokio::select! {
			biased;
			_ = wait_for_cancellation(cancellation) => bail!("share start for {name} was cancelled"),
			result = self.tailscale_host(binary) => result?,
		};
		let status_args = ["serve".to_owned(), "status".to_owned(), "--json".to_owned()];
		let status = tokio::select! {
			biased;
			_ = wait_for_cancellation(cancellation) => bail!("share start for {name} was cancelled"),
			result = run_briefly(binary, &status_args, SWEEP_TIMEOUT) => {
				match result {
					Ok(Some(output)) if output.status.success() => String::from_utf8_lossy(&output.stdout).into_owned(),
					Ok(Some(output)) => bail!("tailscale serve status failed with {}", output.status),
					Ok(None) => bail!("tailscale serve status timed out"),
					Err(e) => bail!("could not inspect tailscale serve: {e}"),
				}
			},
		};
		if !is_443_available(&status) {
			bail!("tailscale serve already has configuration on :443; refusing to overwrite it");
		}
		{
			let registry = self.registrations.lock().await;
			if *cancellation.borrow()
				|| !registry
					.claim(&start.owner.domain)
					.is_some_and(|claim| ClaimBinding::from_claim(claim) == start.owner)
			{
				bail!("share start for {name} was cancelled");
			}
			if registry.contains(&host) {
				bail!("the provider alias {host} is already in use");
			}
		}

		let args = Provider::Tailscale.start_args(&front.plain_addr.to_string());
		let started = tokio::select! {
			biased;
			_ = wait_for_cancellation(cancellation) => Err(anyhow!("share start was cancelled")),
			result = run_with_retry(binary, &args) => result.and_then(|output| {
				if output.status.success() { Ok(()) } else { Err(anyhow!("provider exited with {}", output.status)) }
			}),
		};
		if let Err(error) = started {
			return Err(CleanupUnverified(format!(
				"tailscale serve for {name} may have changed despite {error}; inspect its state"
			))
			.into());
		}

		Ok(Active {
			identity: Arc::clone(&start.identity),
			binding: start.owner.clone(),
			provider: Provider::Tailscale,
			url: format!("https://{host}"),
			resource: ProviderResource::Tailscale { alias: host },
		})
	}

	/// First URL a provider prints, from either stream.
	///
	/// The readers keep draining after the URL, because a provider that blocks on
	/// a full pipe would stop serving.
	async fn read_url(&self, child: &mut Child, provider: Provider, binary: &Path) -> Result<String> {
		let (tx, mut rx) = mpsc::channel::<String>(4);

		let drain = |stream: Option<Box<dyn tokio::io::AsyncRead + Unpin + Send>>| {
			let Some(stream) = stream else { return };
			let tx = tx.clone();
			tokio::spawn(async move {
				let mut lines = BufReader::new(stream).lines();
				let mut sent = false;
				while let Ok(Some(line)) = lines.next_line().await {
					if !sent && let Some(url) = provider.url_from_line(&line) {
						sent = true;
						let _ = tx.send(url).await;
					}
				}
			});
		};
		drain(
			child
				.stdout
				.take()
				.map(|out| Box::new(out) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
		);
		drain(
			child
				.stderr
				.take()
				.map(|err| Box::new(err) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
		);
		drop(tx);

		match tokio::time::timeout(URL_TIMEOUT, rx.recv()).await {
			Ok(Some(url)) => Ok(url),
			Ok(None) => bail!("{} exited before printing a URL", binary.display()),
			Err(_) => bail!("{} printed no URL in {}s", binary.display(), URL_TIMEOUT.as_secs()),
		}
	}

	/// This node's stable tailnet name, from `tailscale status --json`.
	async fn tailscale_host(&self, binary: &Path) -> Result<String> {
		let args = vec!["status".to_owned(), "--json".to_owned()];
		let output = run_with_retry(binary, &args)
			.await
			.with_context(|| format!("run {} status --json", binary.display()))?;
		if !output.status.success() {
			bail!("{} status --json failed with {}", binary.display(), output.status);
		}
		status_host(&String::from_utf8_lossy(&output.stdout))
			.ok_or_else(|| anyhow!("no tailnet DNS name in {} status --json", binary.display()))
	}

	/// Kill a child, or run the provider's cleanup command.
	async fn release(&self, share: StoppingShare) -> Result<()> {
		let alias = match share.resource {
			ProviderResource::Process { mut child, .. } => {
				return stop_child(&mut child).await.context("stop provider process");
			},
			ProviderResource::Tailscale { alias } => alias,
		};
		let _guard = self.tailscale_actions.lock().await;
		let binary = self.binary_for(share.provider);
		let front = self.front.as_ref().context("the tailscale front is missing during cleanup")?;
		let upstream = format!("http://{}", front.plain_addr);
		let status_args = ["serve".to_owned(), "status".to_owned(), "--json".to_owned()];
		let status = match run_briefly(&binary, &status_args, SWEEP_TIMEOUT).await {
			Ok(Some(output)) if output.status.success() => String::from_utf8_lossy(&output.stdout).into_owned(),
			_ => bail!("could not inspect tailscale serve; leaving :443 alone"),
		};
		let config: serde_json::Value =
			serde_json::from_str(&status).context("tailscale serve returned malformed status; leaving :443 alone")?;
		if serve_config_has_free_443(&config) {
			return Ok(());
		}
		if !owns_private_serve_443(&config, &alias, &upstream) {
			bail!("tailscale serve :443 changed or is not private; leaving it in place");
		}
		/* Scope cleanup to this serve port, leaving unrelated entries alone. */
		let args = ["serve".to_owned(), "--https=443".to_owned(), "off".to_owned()];
		let output = run_with_retry(&binary, &args).await?;
		if !output.status.success() {
			bail!("{} {} exited with {}; inspect the provider state", binary.display(), args.join(" "), output.status);
		}
		/* A successful command does not establish that persistent exposure was removed. */
		let removal = run_briefly(&binary, &status_args, SWEEP_TIMEOUT).await?;
		if !removal.is_some_and(|output| {
			output.status.success() && is_443_available(&String::from_utf8_lossy(&output.stdout))
		}) {
			bail!("tailscale serve :443 removal is unverified; inspect the provider state");
		}
		Ok(())
	}
}

/// This node's tailnet name, from `tailscale status --json`.
pub fn status_host(status: &str) -> Option<String> {
	let value: serde_json::Value = serde_json::from_str(status).ok()?;
	let name = value.get("Self")?.get("DNSName")?.as_str()?.trim_end_matches('.');
	if name.is_empty() { None } else { Some(name.to_owned()) }
}

/** Find exact edge routes; a matching target alone does not establish ownership. */
fn serve_ports_pointing_at(status: &str, upstream: &str) -> Vec<u16> {
	let Ok(value) = serde_json::from_str::<serde_json::Value>(status) else { return Vec::new() };
	let Some(web) = value.get("Web").and_then(|web| web.as_object()) else { return Vec::new() };
	let mut ports: Vec<u16> = Vec::new();
	for (host_port, entry) in web {
		let Some(handlers) = entry.get("Handlers").and_then(|handlers| handlers.as_object()) else {
			continue;
		};
		if handlers.len() != 1 {
			continue;
		}
		let proxy = handlers
			.get("/")
			.and_then(|handler| handler.get("Proxy"))
			.and_then(|proxy| proxy.as_str());
		if proxy != Some(upstream) {
			continue;
		}
		if let Some(port) = host_port.rsplit(':').next().and_then(|port| port.parse::<u16>().ok())
			&& !ports.contains(&port)
		{
			ports.push(port);
		}
	}
	ports.sort_unstable();
	ports
}

/** Do not adopt any existing :443 route, including an identical edge target. */
fn is_443_available(status: &str) -> bool {
	serde_json::from_str::<serde_json::Value>(status).is_ok_and(|value| serve_config_has_free_443(&value))
}

/** Remove only the expected private route from a copy; any other :443 state prevents cleanup. */
fn owns_private_serve_443(config: &serde_json::Value, alias: &str, upstream: &str) -> bool {
	use serde_json::{Value, json};

	let mut remaining = config.clone();
	let route = format!("{alias}:443");
	let Some(web) = remaining.get_mut("Web").and_then(Value::as_object_mut) else { return false };
	if web.remove(&route) != Some(json!({"Handlers": {"/": {"Proxy": upstream}}})) {
		return false;
	}
	let Some(tcp) = remaining.get_mut("TCP").and_then(Value::as_object_mut) else { return false };
	if tcp.remove("443") != Some(json!({"HTTPS": true})) {
		return false;
	}
	if let Some(funnel) = remaining.get_mut("AllowFunnel")
		&& !funnel.is_null()
	{
		let Some(entries) = funnel.as_object_mut() else { return false };
		if !matches!(entries.remove(&route), None | Some(Value::Bool(false))) {
			return false;
		}
	}
	serve_config_has_free_443(&remaining)
}

fn serve_config_has_free_443(value: &serde_json::Value) -> bool {
	if value.is_null() {
		return true;
	}
	let Some(config) = value.as_object() else { return false };
	for field in ["TCP", "Web", "AllowFunnel"] {
		let Some(value) = config.get(field) else { continue };
		if value.is_null() {
			continue;
		}
		let Some(entries) = value.as_object() else { return false };
		for key in entries.keys() {
			let port = if field == "TCP" { Some(key.as_str()) } else { key.rsplit_once(':').map(|(_, port)| port) };
			if port.and_then(|port| port.parse::<u16>().ok()).is_none_or(|port| port == 443) {
				return false;
			}
		}
	}
	for field in ["Foreground", "Services"] {
		let Some(value) = config.get(field) else { continue };
		if value.is_null() {
			continue;
		}
		let Some(entries) = value.as_object() else { return false };
		if !entries.values().all(serve_config_has_free_443) {
			return false;
		}
	}
	true
}

async fn stop_child(child: &mut Child) -> Result<()> {
	if child.try_wait()?.is_none() {
		tokio::time::timeout(PROVIDER_STOP_TIMEOUT, child.kill())
			.await
			.context("provider termination timed out")??;
	}
	Ok(())
}

async fn wait_for_cancellation(cancellation: &mut watch::Receiver<bool>) {
	loop {
		if *cancellation.borrow_and_update() || cancellation.changed().await.is_err() {
			return;
		}
	}
}

/// How often a spawn is retried when the kernel reports the binary as busy.
const BUSY_RETRIES: usize = 5;
/// `ETXTBSY`: the file is open for writing elsewhere, usually a binary that was
/// just installed. It clears within milliseconds.
const ETXTBSY: i32 = 26;

/// Start a provider, retrying while the kernel reports the binary as busy.
async fn spawn_with_retry(binary: &Path, args: &[String], piped: bool) -> Result<Child> {
	let mut attempts = 0;
	loop {
		let mut command = Command::new(binary);
		command.args(args);
		if piped {
			command.stdout(Stdio::piped()).stderr(Stdio::piped());
		}
		command.kill_on_drop(true);
		match command.spawn() {
			Ok(child) => return Ok(child),
			Err(e) if e.raw_os_error() == Some(ETXTBSY) && attempts < BUSY_RETRIES => {
				attempts += 1;
				tokio::time::sleep(Duration::from_millis(20)).await;
			},
			Err(e) => return Err(e.into()),
		}
	}
}

/// Run a provider command with a deadline. `None` means it was still running when
/// the deadline passed; dropping the child kills it.
async fn run_briefly(binary: &Path, args: &[String], limit: Duration) -> Result<Option<std::process::Output>> {
	let child = spawn_with_retry(binary, args, true).await?;
	match tokio::time::timeout(limit, child.wait_with_output()).await {
		Ok(result) => Ok(Some(result?)),
		Err(_) => Ok(None),
	}
}

/// Run a provider command with a deadline and the same busy retry.
async fn run_with_retry(binary: &Path, args: &[String]) -> Result<std::process::Output> {
	run_briefly(binary, args, URL_TIMEOUT)
		.await?
		.ok_or_else(|| anyhow!("{} {} timed out", binary.display(), args.join(" ")))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn invalid_environment_does_not_fall_back_or_override_an_explicit_provider() {
		const FIXTURE: &str = "TESTD_INVALID_PROVIDER_FIXTURE";
		if std::env::var_os(FIXTURE).is_some() {
			let shares = Shares::from_env(
				Some(Front { plain_addr: "127.0.0.1:7740".parse().unwrap() }),
				Arc::new(Mutex::new(Registry::default())),
			);
			let error = shares.checked_provider(None).unwrap_err().to_string();
			assert!(error.contains("TESTD_TUNNEL_PROVIDER"), "{error}");
			assert_eq!(shares.checked_provider(Some(Provider::Cloudflared)).unwrap(), Provider::Cloudflared);
			return;
		}
		let check = |value: std::ffi::OsString| {
			let output = std::process::Command::new(std::env::current_exe().unwrap())
				.args([
					"--exact",
					"share::tests::invalid_environment_does_not_fall_back_or_override_an_explicit_provider",
				])
				.env(FIXTURE, "1")
				.env("TESTD_TUNNEL_PROVIDER", value)
				.output()
				.unwrap();
			assert!(
				output.status.success(),
				"{}\n{}",
				String::from_utf8_lossy(&output.stdout),
				String::from_utf8_lossy(&output.stderr)
			);
		};
		check("invalid-provider".into());
		check("".into());
		#[cfg(unix)]
		{
			use std::os::unix::ffi::OsStringExt;
			check(std::ffi::OsString::from_vec(vec![0xff]));
		}
	}

	fn manager() -> Shares {
		Shares::new(
			Provider::Tailscale,
			None,
			Some(Front { plain_addr: "127.0.0.1:7740".parse().unwrap() }),
			Arc::new(Mutex::new(Registry::default())),
		)
	}

	async fn candidate(shares: &Shares) -> Active {
		let mut registry = shares.registrations.lock().await;
		let connection = registry.connection_id();
		registry
			.register(
				connection,
				testd_core::Registration { domain: "app.test".into(), target: "127.0.0.1:3000".into() },
				true,
				&[],
			)
			.unwrap();
		let binding = ClaimBinding::from_claim(registry.claim("app.test").unwrap());
		let start = shares.reserve("app", Provider::Tailscale, binding).await.unwrap();
		Active {
			identity: start.identity,
			binding: start.owner,
			provider: start.provider,
			url: "https://node.tailnet.ts.net".into(),
			resource: ProviderResource::Tailscale { alias: "node.tailnet.ts.net".into() },
		}
	}

	#[tokio::test]
	async fn a_share_transfers_from_starting_to_active_to_stopping() {
		let shares = manager();
		let share = candidate(&shares).await;
		let identity = Arc::clone(&share.identity);
		assert!(matches!(shares.state.lock().await.entries.get("app"), Some(ShareLifecycle::Starting(_))));
		assert!(shares.list().await.is_empty());

		let mut registry = shares.registrations.lock().await;
		assert!(shares.publish("app", share, &mut registry).await.is_ok());
		assert!(registry.contains("node.tailnet.ts.net"));
		assert!(matches!(shares.state.lock().await.entries.get("app"), Some(ShareLifecycle::Active(_))));
		let view = shares.list().await.remove(0);
		assert_eq!(view.url, "https://node.tailnet.ts.net");

		let mut state = shares.state.lock().await;
		let mut batch = StopBatch::default();
		Shares::detach_name(&mut state, &mut registry, "app", &mut batch);
		assert!(matches!(state.entries.get("app"), Some(ShareLifecycle::Stopping(_))));
		assert!(!registry.contains("node.tailnet.ts.net"));
		let (_, stopping) = batch.stopping.pop().unwrap();
		assert!(Arc::ptr_eq(&stopping.identity, &identity));
		assert_eq!(stopping.resource.alias(), "node.tailnet.ts.net");
		Shares::detach_name(&mut state, &mut registry, "app", &mut batch);
		assert!(batch.stopping.is_empty(), "only one task owns provider cleanup");
		drop(state);
		drop(registry);
		assert!(shares.list().await.is_empty());

		/* Simulate confirmed provider release without executing a real provider. */
		shares.finish_pending("app", &identity).await;
		assert!(shares.state.lock().await.entries.is_empty());
		assert!(matches!(*batch.waiting[0].borrow(), PendingResult::Cleaned));
	}

	#[tokio::test]
	async fn a_cancelled_start_cannot_publish_and_returns_its_owned_resource() {
		let shares = manager();
		let share = candidate(&shares).await;
		let identity = Arc::clone(&share.identity);
		let mut registry = shares.registrations.lock().await;
		let mut batch = StopBatch::default();
		Shares::detach_name(&mut *shares.state.lock().await, &mut registry, "app", &mut batch);
		let Err((PublishFailure::ClaimChanged, unpublished)) = shares.publish("app", share, &mut registry).await else {
			panic!("a stopping share must not publish");
		};
		assert!(Arc::ptr_eq(&unpublished.identity, &identity));
		assert!(!registry.contains("node.tailnet.ts.net"));
		drop(registry);
		shares.finish_pending("app", &unpublished.into_stopping().identity).await;
		assert!(matches!(*batch.waiting[0].borrow(), PendingResult::Cleaned));
	}

	#[tokio::test]
	async fn unverified_cleanup_stays_quarantined_and_cannot_publish() {
		let shares = manager();
		let share = candidate(&shares).await;
		shares.abandon_pending("app", &share.identity, "cleanup failed").await;
		let mut registry = shares.registrations.lock().await;
		let mut batch = StopBatch::default();
		Shares::detach_name(&mut *shares.state.lock().await, &mut registry, "app", &mut batch);
		assert!(matches!(shares.state.lock().await.entries.get("app"), Some(ShareLifecycle::CleanupUnverified(_))));
		assert_eq!(batch.absence_checks[0].0, "app");
		assert!(Arc::ptr_eq(&batch.absence_checks[0].1, &share.identity));
		let binding = share.binding.clone();
		assert!(matches!(shares.publish("app", share, &mut registry).await, Err((PublishFailure::ClaimChanged, _))));
		drop(registry);
		assert!(shares.reserve("app", Provider::Tailscale, binding.clone()).await.is_err());
		let other = ClaimBinding { domain: "other.test".into(), ..binding };
		assert!(shares.reserve("other", Provider::Tailscale, other).await.is_err());
	}

	#[tokio::test]
	async fn stale_completion_and_failure_cannot_change_a_replacement() {
		let shares = manager();
		let old = candidate(&shares).await;
		shares.finish_pending("app", &old.identity).await;
		let replacement = candidate(&shares).await;
		shares.finish_pending("app", &old.identity).await;
		shares.abandon_pending("app", &old.identity, "stale failure").await;
		let state = shares.state.lock().await;
		let Some(ShareLifecycle::Starting(pending)) = state.entries.get("app") else {
			panic!("the replacement must remain starting");
		};
		assert!(Arc::ptr_eq(&pending.identity, &replacement.identity));
		assert!(!*pending.cancel.borrow());
	}

	#[test]
	fn reads_the_cloudflared_tunnel_url() {
		let line = "2026-10-30T10:00:00Z INF |  https://brave-otter-harbor.trycloudflare.com  |";
		assert_eq!(
			Provider::Cloudflared.url_from_line(line).as_deref(),
			Some("https://brave-otter-harbor.trycloudflare.com")
		);
		assert!(
			Provider::Cloudflared
				.url_from_line("INF https://api.trycloudflare.com docs")
				.is_none()
		);
		assert!(Provider::Cloudflared.url_from_line("INF connecting...").is_none());
	}

	#[test]
	fn reads_the_ngrok_tunnel_url_from_a_log_record() {
		let line = r#"t=2026-10-30T10:00:00+0000 lvl=info msg="started tunnel" obj=tunnels name=command_line addr=127.0.0.1:3000 url=https://1a2b-3c4d.ngrok-free.app"#;
		assert_eq!(Provider::Ngrok.url_from_line(line).as_deref(), Some("https://1a2b-3c4d.ngrok-free.app"));
		assert!(Provider::Ngrok.url_from_line("https://example.com").is_none());
	}

	#[test]
	fn reads_the_tailnet_name() {
		let status = r#"{"Self":{"DNSName":"node-a.tailnet.ts.net.","HostName":"node-a"}}"#;
		assert_eq!(status_host(status).as_deref(), Some("node-a.tailnet.ts.net"));
		assert!(status_host(r#"{"Self":{}}"#).is_none());
		assert!(status_host("not json").is_none());
	}

	#[test]
	fn finds_only_the_serve_entries_that_point_at_this_edge() {
		let status = r#"{"Web":{
			"node-a.tailnet.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:7740"}}},
			"node-a.tailnet.ts.net:8443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:9000"}}},
			"node-a.tailnet.ts.net:10000":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:7740"}}}
		}}"#;
		assert_eq!(serve_ports_pointing_at(status, "http://127.0.0.1:7740"), vec![443, 10000]);
		assert!(serve_ports_pointing_at(status, "http://127.0.0.1:7741").is_empty());
		assert!(serve_ports_pointing_at("", "http://127.0.0.1:7740").is_empty());
		assert!(serve_ports_pointing_at(r#"{"Web":{}}"#, "http://127.0.0.1:7740").is_empty());
	}

	#[test]
	fn every_existing_or_ambiguous_handler_on_443_blocks_the_share() {
		for status in [
			r#"{"Web":{"node.tail.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:7740"}}}}}"#,
			r#"{"Web":{"node.tail.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:9000"}}}}}"#,
			r#"{"TCP":{"443":{"TCPForward":"127.0.0.1:9000"}}}"#,
			r#"{"Foreground":{"other":{"TCP":{"443":{"HTTPS":true}}}}}"#,
			r#"{"Services":{"other":{"Web":{"node.tail.ts.net:443":{}}}}}"#,
			r#"{"AllowFunnel":{"node.tail.ts.net:443":true}}"#,
			r#"{"TCP":"unrecognized"}"#,
			r#"{"Web":{"bad-key":{}}}"#,
			"not json",
		] {
			assert!(!is_443_available(status), "{status}");
		}
		for status in ["null", r#"{}"#, r#"{"Web":{}}"#, r#"{"TCP":{"8443":{}}}"#] {
			assert!(is_443_available(status), "{status}");
		}
	}

	#[test]
	fn providers_expose_where_they_reach() {
		assert_eq!(Provider::Cloudflared.exposure(), Exposure::Public);
		assert_eq!(Provider::Ngrok.exposure(), Exposure::Public);
		assert_eq!(Provider::Tailscale.exposure(), Exposure::Tailnet);
		assert_eq!(Provider::parse("TS"), Some(Provider::Tailscale));
		assert_eq!(Provider::parse("cloudflare"), Some(Provider::Cloudflared));
		assert_eq!(Provider::parse("mystery"), None);
	}
}
