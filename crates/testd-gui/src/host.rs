/*! The async desktop owner. Only data and commands cross the native UI boundary. */

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use tao::event_loop::EventLoopProxy;
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::{JoinHandle, JoinSet};

use testd_daemon::daemon::Daemon;
use testd_daemon::engine::EngineEvent;
use testd_daemon::health::{Action, Health};

use crate::UiEvent;
use crate::activation::Activation;
use crate::browser::Handoff;
use crate::tray::{Phase, State, summarize_launches};

#[derive(Debug)]
pub enum Command {
	Activate(Activation),
	OpenPage,
	EnableAutostart,
	DisableAutostart,
	Repair(Action),
	OpenDomain(String),
	StopDomain(String),
	StartLaunch(String),
	StopLaunch(String),
	OpenShare(String),
	StopShare(String),
	Quit,
}

impl Command {
	pub fn from_menu(id: &str, requested_login: Option<bool>) -> Option<Self> {
		match id {
			"autostart" => Some(if requested_login? { Self::EnableAutostart } else { Self::DisableAutostart }),
			"quit" => Some(Self::Quit),
			"dashboard-open" => Some(Self::OpenPage),
			"setup-dns" => Some(Self::Repair(Action::SetupResolver)),
			"trust-root" => Some(Self::Repair(Action::TrustRoot)),
			_ => {
				let (action, value) = id.split_once(':')?;
				if value.is_empty() {
					return None;
				}
				Some(match action {
					"domain-open" => Self::OpenDomain(value.into()),
					"domain-stop" => Self::StopDomain(value.into()),
					"launch-start" => Self::StartLaunch(value.into()),
					"launch-stop" => Self::StopLaunch(value.into()),
					"share-open" => Self::OpenShare(value.into()),
					"share-stop" => Self::StopShare(value.into()),
					_ => return None,
				})
			},
		}
	}
}

struct Running {
	daemon: Arc<Daemon>,
	page: Handoff,
	engine: JoinHandle<std::io::Result<()>>,
	events: broadcast::Receiver<EngineEvent>,
	health: watch::Receiver<Health>,
	requests: watch::Receiver<Vec<testd_daemon::access::PendingView>>,
}

enum Host {
	Setup { server: testd_daemon::setup::Setup, page: Handoff },
	Running(Running),
	Failed,
}

enum Wake {
	Command(Option<Command>),
	Finish(testd_daemon::setup::Finish),
	Refresh,
	Failed(String),
}

impl Host {
	async fn initial() -> anyhow::Result<Self> {
		if !testd_core::config::config_exists() || testd_core::config::has_pending_config() {
			let server = testd_daemon::setup::start(&tokio::runtime::Handle::current())?;
			let page = Handoff::new(&testd_core::config::config_dir(), &server.url)?;
			Ok(Self::Setup { server, page })
		} else {
			Self::start().await
		}
	}

	async fn start() -> anyhow::Result<Self> {
		let daemon = Arc::new(Daemon::new_gui().await.map_err(|error| anyhow::anyhow!(error.to_string()))?);
		let result = async {
			let listener = daemon.engine.bind().await?;
			let page = Handoff::new(&testd_core::config::config_dir(), &daemon.engine.local_dashboard_url())?;
			let events = daemon.engine.subscribe();
			let health = daemon.health_changes();
			let requests = daemon.request_changes();
			let engine = Arc::clone(&daemon.engine);
			let engine = tokio::spawn(async move { engine.serve(listener).await });
			Ok::<_, anyhow::Error>(Self::Running(Running {
				daemon: Arc::clone(&daemon),
				page,
				engine,
				events,
				health,
				requests,
			}))
		}
		.await;
		if let Err(error) = &result
			&& let Err(cleanup) = daemon.shutdown().await
		{
			bail!("{error:#}; startup cleanup failed: {cleanup}");
		}
		result
	}

	fn phase(&self) -> Phase {
		match self {
			Self::Setup { .. } => Phase::Setup,
			Self::Running(_) => Phase::Running,
			Self::Failed => Phase::Failed,
		}
	}

	fn open_page(&self, tasks: &mut JoinSet<anyhow::Result<()>>) {
		let page = match self {
			Self::Setup { page, .. } => page,
			Self::Running(host) => &host.page,
			Self::Failed => return,
		};
		let url = page.file_url();
		tasks.spawn(async move { tokio::task::spawn_blocking(move || crate::browser::open(&url)).await? });
	}

	async fn next(
		&mut self,
		commands: &mut mpsc::Receiver<Command>,
		stop: &mut watch::Receiver<bool>,
		ticker: &mut tokio::time::Interval,
	) -> Wake {
		match self {
			Self::Setup { server, .. } => tokio::select! {
				biased;
				_ = stop.changed() => Wake::Command(Some(Command::Quit)),
				command = commands.recv() => Wake::Command(command),
				finish = server.next_finish() => match finish {
					Ok(reply) => Wake::Finish(reply),
					Err(error) => Wake::Failed(error),
				},
				_ = ticker.tick() => Wake::Refresh,
			},
			Self::Running(host) => tokio::select! {
				biased;
				_ = stop.changed() => Wake::Command(Some(Command::Quit)),
				command = commands.recv() => Wake::Command(command),
				result = &mut host.engine => Wake::Failed(match result {
					Ok(Ok(())) => "the host engine stopped unexpectedly".into(),
					Ok(Err(error)) => format!("host engine failed: {error}"),
					Err(error) => format!("host engine task failed: {error}"),
				}),
				_ = host.events.recv() => Wake::Refresh,
				_ = host.health.changed() => Wake::Refresh,
				_ = host.requests.changed() => Wake::Refresh,
				_ = ticker.tick() => Wake::Refresh,
			},
			Self::Failed => tokio::select! {
				biased;
				_ = stop.changed() => Wake::Command(Some(Command::Quit)),
				command = commands.recv() => Wake::Command(command),
				_ = ticker.tick() => Wake::Refresh,
			},
		}
	}

	async fn snapshot(&self, autostart: Option<bool>) -> State {
		let mut state = State::new(self.phase());
		if let Self::Running(host) = self {
			state.regs = host.daemon.engine.registrations().lock().await.snapshot();
			state.health = host.daemon.health();
			state.autostart = autostart;
			state.shares = host.daemon.shares().list().await;
			state.launches = summarize_launches(&host.daemon.launches().list());
			state.tld = host.daemon.tld().to_owned();
			state.has_request = !host.daemon.pending_requests().is_empty();
		}
		state
	}

	async fn shutdown(&mut self) -> Result<(), String> {
		match self {
			Self::Running(host) => {
				let result = host.daemon.shutdown().await;
				host.engine.abort();
				result
			},
			Self::Setup { server, .. } => server.shutdown().await.map_err(|error| error.to_string()),
			Self::Failed => Ok(()),
		}
	}
}

pub async fn run(
	mut commands: mpsc::Receiver<Command>,
	mut stop: watch::Receiver<bool>,
	proxy: EventLoopProxy<UiEvent>,
) -> Result<(), String> {
	if *stop.borrow() {
		let _ = proxy.send_event(UiEvent::Stopped(Ok(())));
		return Ok(());
	}
	let mut host = match Host::initial().await {
		Ok(host) => host,
		Err(error) => {
			notify(&proxy, error);
			Host::Failed
		},
	};
	let mut tasks = JoinSet::new();
	if *stop.borrow() {
		return stop_desktop(host, tasks, &proxy).await;
	}
	let mut autostart = initialize_desktop(&host, &mut tasks, &proxy).await;
	let mut pending = Vec::new();
	let mut shown = host.snapshot(autostart).await;
	let _ = proxy.send_event(UiEvent::State(shown.clone()));
	let mut ticker = tokio::time::interval(Duration::from_secs(1));
	let mut last_login_query = std::time::Instant::now();
	loop {
		while let Some(result) = tasks.try_join_next() {
			match result {
				Ok(Ok(())) => {},
				Ok(Err(error)) => notify(&proxy, error),
				Err(error) => notify(&proxy, error.into()),
			}
		}
		match host.next(&mut commands, &mut stop, &mut ticker).await {
			Wake::Command(None | Some(Command::Quit)) => break,
			Wake::Command(Some(command)) => {
				/* Popup callbacks finish before an action can re-enter native desktop services. */
				tokio::time::sleep(Duration::from_millis(150)).await;
				if tasks.len() >= 16 {
					notify(&proxy, anyhow::anyhow!("desktop operations are busy; retry shortly"));
					continue;
				}
				if let Err(error) = dispatch(command, &host, &mut tasks, &mut pending, &mut autostart).await {
					notify(&proxy, error);
				}
			},
			Wake::Finish(finish) => {
				shown = State::new(Phase::Starting);
				let _ = proxy.send_event(UiEvent::State(shown.clone()));
				autostart = complete_setup(&mut host, finish, &mut pending, &mut tasks, &proxy, &stop).await;
			},
			Wake::Failed(error) => {
				let error = match host.shutdown().await {
					Ok(()) => error,
					Err(cleanup) => format!("{error}; cleanup: {cleanup}"),
				};
				host = Host::Failed;
				notify(&proxy, anyhow::anyhow!(error));
			},
			Wake::Refresh => {
				if last_login_query.elapsed() >= Duration::from_secs(5) && matches!(host, Host::Running(_)) {
					last_login_query = std::time::Instant::now();
					autostart = query_login(autostart, &proxy).await;
				}
			},
		}
		let state = host.snapshot(autostart).await;
		if state != shown {
			/* Native popup mouse-up must finish before replacing menu handles. */
			tokio::time::sleep(Duration::from_millis(150)).await;
			let _ = proxy.send_event(UiEvent::State(state.clone()));
			shown = state;
		}
	}
	commands.close();
	stop_desktop(host, tasks, &proxy).await
}

async fn initialize_desktop(
	host: &Host,
	tasks: &mut JoinSet<anyhow::Result<()>>,
	proxy: &EventLoopProxy<UiEvent>,
) -> Option<bool> {
	if matches!(host, Host::Setup { .. }) {
		host.open_page(tasks);
	}
	let autostart = if matches!(host, Host::Running(_)) { initialize_login(proxy).await } else { None };
	match tokio::task::spawn_blocking(crate::platform::register_scheme).await {
		Ok(Ok(())) => {},
		Ok(Err(error)) => notify(proxy, error),
		Err(error) => notify(proxy, error.into()),
	}
	autostart
}

async fn complete_setup(
	host: &mut Host,
	finish: testd_daemon::setup::Finish,
	pending: &mut Vec<Action>,
	tasks: &mut JoinSet<anyhow::Result<()>>,
	proxy: &EventLoopProxy<UiEvent>,
	stop: &watch::Receiver<bool>,
) -> Option<bool> {
	match Host::start().await {
		Ok(started) => {
			let Host::Running(running) = &started else { unreachable!() };
			if let Err(error) = finish.complete(Ok(running.daemon.engine.local_dashboard_url())).await {
				notify(proxy, error.into());
			}
			*host = started;
			if *stop.borrow() {
				pending.clear();
				return None;
			}
			for action in pending.drain(..) {
				dispatch_repair(host, action, tasks);
			}
			initialize_login(proxy).await
		},
		Err(error) => {
			if let Err(delivery) = finish.complete(Err(format!("{error:#}"))).await {
				notify(proxy, delivery.into());
			}
			None
		},
	}
}

async fn query_login(previous: Option<bool>, proxy: &EventLoopProxy<UiEvent>) -> Option<bool> {
	match tokio::task::spawn_blocking(crate::platform::autostart_enabled).await {
		Ok(Ok(enabled)) => Some(enabled),
		Ok(Err(error)) => {
			if previous.is_some() {
				notify(proxy, error);
			}
			None
		},
		Err(error) => {
			notify(proxy, error.into());
			None
		},
	}
}

async fn stop_desktop(
	mut host: Host,
	mut tasks: JoinSet<anyhow::Result<()>>,
	proxy: &EventLoopProxy<UiEvent>,
) -> Result<(), String> {
	let _ = proxy.send_event(UiEvent::State(State::new(Phase::Stopping)));
	while let Some(result) = tasks.join_next().await {
		match result {
			Ok(Ok(())) => {},
			Ok(Err(error)) => notify(proxy, error),
			Err(error) => notify(proxy, error.into()),
		}
	}
	let result = host.shutdown().await;
	drop(host);
	let _ = proxy.send_event(UiEvent::Stopped(result.clone()));
	result
}

async fn initialize_login(proxy: &EventLoopProxy<UiEvent>) -> Option<bool> {
	match tokio::task::spawn_blocking(crate::platform::autostart_enabled).await {
		Ok(Ok(enabled)) => Some(enabled),
		Ok(Err(error)) => {
			notify(proxy, error);
			None
		},
		Err(error) => {
			notify(proxy, error.into());
			None
		},
	}
}

fn notify(proxy: &EventLoopProxy<UiEvent>, error: anyhow::Error) {
	let message = format!("{error:#}");
	crate::trace(&message);
	if proxy.send_event(UiEvent::Error(message.clone())).is_err() {
		eprintln!("[error] {message}");
	}
}

fn dispatch_repair(host: &Host, action: Action, tasks: &mut JoinSet<anyhow::Result<()>>) {
	if let Host::Running(host) = host {
		let daemon = Arc::clone(&host.daemon);
		tasks.spawn(async move { daemon.repair(action).await.map_err(anyhow::Error::msg) });
	}
}

async fn dispatch(
	command: Command,
	host: &Host,
	tasks: &mut JoinSet<anyhow::Result<()>>,
	pending: &mut Vec<Action>,
	autostart: &mut Option<bool>,
) -> anyhow::Result<()> {
	let command = match command {
		Command::Activate(Activation::ContinueSetup) => {
			if matches!(host, Host::Setup { .. }) {
				host.open_page(tasks);
			}
			return Ok(());
		},
		Command::Activate(Activation::RepairResolver) => Command::Repair(Action::SetupResolver),
		Command::Activate(Activation::TrustRoot) => Command::Repair(Action::TrustRoot),
		command => command,
	};
	if matches!(command, Command::OpenPage) {
		host.open_page(tasks);
		return Ok(());
	}
	if let Command::Repair(action) = command {
		if matches!(host, Host::Setup { .. }) {
			if !pending.contains(&action) {
				pending.push(action);
			}
		} else if matches!(host, Host::Running(_)) {
			dispatch_repair(host, action, tasks);
		} else {
			bail!("the host is unavailable; exit and restart Testd before repairing it");
		}
		return Ok(());
	}
	let Host::Running(host) = host else {
		bail!("the host is not running");
	};
	let daemon = &host.daemon;
	match command {
		Command::EnableAutostart => {
			*autostart = Some(tokio::task::spawn_blocking(crate::platform::enable_autostart).await??);
		},
		Command::DisableAutostart => {
			*autostart = Some(tokio::task::spawn_blocking(crate::platform::disable_autostart).await??);
		},
		Command::StopDomain(domain) => daemon.stop_domain(&domain).await.map_err(anyhow::Error::msg)?,
		Command::StartLaunch(id) => daemon.launches().start(&id).map_err(anyhow::Error::msg)?,
		Command::StopLaunch(id) => daemon.launches().stop(&id).map_err(anyhow::Error::msg)?,
		Command::StopShare(name) => {
			daemon.shares().stop(&name).await?;
		},
		Command::OpenDomain(domain) => {
			let url = daemon.engine.domain_url(&domain);
			tasks.spawn(async move { tokio::task::spawn_blocking(move || crate::platform::open_browser(&url)).await? });
		},
		Command::OpenShare(name) => {
			let url = daemon
				.shares()
				.list()
				.await
				.into_iter()
				.find(|share| share.name == name)
				.context("the share no longer exists")?
				.url;
			tasks.spawn(async move { tokio::task::spawn_blocking(move || crate::platform::open_browser(&url)).await? });
		},
		Command::Activate(_) | Command::Repair(_) | Command::OpenPage | Command::Quit => unreachable!(),
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn exit_has_priority_over_queued_desktop_actions() {
		let (commands, mut receive) = mpsc::channel(2);
		commands.try_send(Command::Repair(Action::TrustRoot)).unwrap();
		let (stop, mut shutdown) = watch::channel(false);
		stop.send(true).unwrap();
		let mut host = Host::Failed;
		let mut ticker = tokio::time::interval(Duration::from_secs(1));
		assert!(matches!(
			host.next(&mut receive, &mut shutdown, &mut ticker).await,
			Wake::Command(Some(Command::Quit))
		));
		assert!(matches!(receive.try_recv(), Ok(Command::Repair(Action::TrustRoot))));
	}

	#[test]
	fn login_actions_keep_the_captured_checkbox_intent() {
		assert!(matches!(Command::from_menu("autostart", Some(true)), Some(Command::EnableAutostart)));
		assert!(matches!(Command::from_menu("autostart", Some(false)), Some(Command::DisableAutostart)));
		assert!(Command::from_menu("autostart", None).is_none());
	}

	#[test]
	fn menu_dispatch_rejects_headers_empty_values_and_unknown_actions() {
		for id in ["domain:app.example.test", "launch:one", "quick-launches", "domain-stop:", "execute:sh"] {
			assert!(Command::from_menu(id, None).is_none());
		}
		assert!(matches!(Command::from_menu("domain-stop:app.example.test", None), Some(Command::StopDomain(_))));
		assert!(matches!(Command::from_menu("quit", None), Some(Command::Quit)));
	}
}
