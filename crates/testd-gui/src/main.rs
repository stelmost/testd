#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod activation;
mod browser;
mod host;
mod platform;
mod private;
mod tray;

use anyhow::{Context, bail};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::platform::run_return::EventLoopExtRunReturn;
use testd_daemon::control::Dispatch;
use testd_daemon::health::Action;
use tokio::sync::{mpsc, watch};
use tray_icon::menu::MenuEvent;

use activation::Activation;
use host::Command;

pub enum UiEvent {
	State(tray::State),
	Error(String),
	Menu(MenuEvent, Option<bool>),
	Stopped(Result<(), String>),
}

fn main() {
	#[cfg(target_os = "macos")]
	if let Some(result) = testd_daemon::macos_socket::run_helper() {
		if let Err(error) = result {
			eprintln!("socket activation failed: {error}");
			std::process::exit(1);
		}
		return;
	}
	if let Err(error) = run() {
		display_error(&format!("{error:#}"));
		std::process::exit(1);
	}
}

fn initial_activation(arguments: impl Iterator<Item = std::ffi::OsString>) -> anyhow::Result<Activation> {
	let arguments: Vec<_> = arguments.collect();
	if arguments.is_empty() {
		return Ok(Activation::ContinueSetup);
	}
	if arguments.len() != 1 {
		bail!("expected no arguments or one supported testd:// action");
	}
	/* Normalize OS-delivered URIs like the old deep-link backend without echoing parser inputs. */
	let url = arguments[0].to_str().and_then(|value| url::Url::parse(value).ok());
	match url
		.as_ref()
		.filter(|url| url.scheme() == "testd")
		.and_then(|url| Action::parse(url.as_str()))
	{
		Some(Action::SetupResolver) => Ok(Activation::RepairResolver),
		Some(Action::TrustRoot) => Ok(Activation::TrustRoot),
		None => bail!("unsupported testd URL action"),
	}
}

fn run() -> anyhow::Result<()> {
	let action = initial_activation(std::env::args_os().skip(1))?;
	let repair = match action {
		Activation::ContinueSetup => None,
		Activation::RepairResolver => Some(Action::SetupResolver),
		Activation::TrustRoot => Some(Action::TrustRoot),
	};
	if let Some(action) = repair {
		match testd_daemon::control::send_action(action.url())? {
			Dispatch::Handled(Ok(())) => return Ok(()),
			Dispatch::Handled(Err(error)) => bail!("{error}"),
			Dispatch::NotRunning => {},
		}
	}
	let runtime = tokio::runtime::Runtime::new().context("cannot create the desktop worker runtime")?;
	let (commands, receive) = mpsc::channel(32);
	let (stop, shutdown) = watch::channel(false);
	let Some(guard) =
		activation::acquire(&testd_core::config::config_dir(), action, runtime.handle(), commands.clone())?
	else {
		return Ok(());
	};
	if repair.is_some() {
		commands.try_send(Command::Activate(action))?;
	}
	run_desktop(runtime, commands, receive, stop, shutdown, &guard)
}

fn run_desktop(
	runtime: tokio::runtime::Runtime,
	commands: mpsc::Sender<Command>,
	receive: mpsc::Receiver<Command>,
	stop: watch::Sender<bool>,
	shutdown: watch::Receiver<bool>,
	guard: &activation::Guard,
) -> anyhow::Result<()> {
	#[cfg(target_os = "linux")]
	gtk::init().context("cannot initialize GTK; run the tray in a desktop session, or use testd host")?;
	let mut event_loop = EventLoopBuilder::<UiEvent>::with_user_event().build();
	#[cfg(target_os = "macos")]
	{
		use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
		event_loop.set_activation_policy(ActivationPolicy::Accessory);
		event_loop.set_dock_visibility(false);
	}
	let proxy = event_loop.create_proxy();
	MenuEvent::set_event_handler(Some({
		let proxy = proxy.clone();
		move |event: MenuEvent| {
			let preference = if event.id.as_ref() == "autostart" { tray::clicked_login_preference() } else { None };
			let _ = proxy.send_event(UiEvent::Menu(event, preference));
		}
	}));
	let mut receive = Some(receive);
	let mut tray = None;
	let mut worker = None;
	let mut failure = None;
	trace(&format!(
		"start {} {} at {}",
		env!("CARGO_PKG_NAME"),
		env!("CARGO_PKG_VERSION"),
		std::env::current_exe()?.display()
	));
	/* Returning permits owned cleanup before the runtime, guard, and native objects are dropped. */
	let exit = event_loop.run_return(|event, _, control| {
		*control = ControlFlow::Wait;
		match event {
			Event::NewEvents(StartCause::Init) => match tray::Tray::new() {
				Ok(created) => {
					tray = Some(created);
					worker = Some(runtime.spawn(host::run(receive.take().unwrap(), shutdown.clone(), proxy.clone())));
				},
				Err(error) => {
					failure = Some(format!("cannot create the native tray: {error:#}"));
					*control = ControlFlow::Exit;
				},
			},
			Event::UserEvent(UiEvent::State(state)) => {
				if let Some(tray) = &mut tray
					&& let Err(error) = tray.update(state)
				{
					failure = Some(format!("cannot update the native tray: {error:#}"));
					let _ = stop.send(true);
				}
			},
			Event::UserEvent(UiEvent::Menu(event, preference)) => {
				handle_menu(event.id.as_ref(), preference, &commands, &stop)
			},
			Event::Opened { urls } => handle_urls(urls, &commands),
			Event::UserEvent(UiEvent::Error(message)) => display_error(&message),
			Event::UserEvent(UiEvent::Stopped(result)) => {
				if let Err(error) = result {
					failure = Some(error);
				}
				*control = ControlFlow::Exit;
			},
			Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
				let _ = stop.send(true);
			},
			Event::LoopDestroyed => {
				/* AppKit termination can exit without returning through Rust destructors. */
				let _ = stop.send(true);
				if let Err(error) = guard.cleanup() {
					eprintln!("[warn] tray termination cleanup failed: {error:#}");
				}
			},
			_ => {},
		}
	});
	let _ = stop.send(true);
	if let Some(worker) = worker {
		runtime.block_on(worker)?.map_err(anyhow::Error::msg)?;
	}
	drop(tray);
	if let Some(error) = failure {
		bail!("{error}");
	}
	anyhow::ensure!(exit == 0, "the desktop event loop stopped with code {exit}");
	trace("exited");
	Ok(())
}

fn handle_menu(id: &str, preference: Option<bool>, commands: &mpsc::Sender<Command>, stop: &watch::Sender<bool>) {
	trace(&format!("menu event: {id}"));
	if let Some(command) = Command::from_menu(id, preference) {
		if matches!(command, Command::Quit) {
			let _ = stop.send(true);
		} else if let Err(error) = commands.try_send(command) {
			display_error(&format!("desktop action unavailable: {error}"));
		}
	}
}

fn handle_urls(urls: impl IntoIterator<Item = url::Url>, commands: &mpsc::Sender<Command>) {
	for url in urls {
		let Some(action) = Action::parse(url.as_str()) else {
			display_error("unsupported testd URL action");
			return;
		};
		if let Err(error) = commands.try_send(Command::Repair(action)) {
			display_error(&error.to_string());
			return;
		}
	}
}

fn display_error(error: &str) {
	let message = format!(
		"{error}\n\nConfiguration: {}\n\n\
		Fix the reported problem, then retry or exit and restart Testd.\n\
		If a port is busy, stop the other daemon. On Linux, grant the executable CAP_NET_BIND_SERVICE \
		or set [host] edge_addr to '127.0.0.1:8443'.",
		testd_core::config::config_path().display(),
	);
	eprintln!("[error] {message}");
	trace(&message);
	let directory = testd_core::config::config_dir();
	let log = directory.join("last-error.txt");
	let saved = (|| -> std::io::Result<()> {
		private::directory(&directory)?;
		if log.try_exists()? {
			private::open(&log)?;
		}
		testd_core::config::write_private(&log, message.as_bytes())
	})();
	if let Err(error) = saved {
		eprintln!("[warn] could not retain the desktop diagnostic: {error}");
	}
	if let Err(error) = platform::show_error(&message) {
		eprintln!("[warn] could not display the desktop diagnostic: {error:#}");
	}
}

pub(crate) fn trace(message: &str) {
	let Some(path) = std::env::var_os("TESTD_TRAY_TRACE") else {
		return;
	};
	let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) else {
		return;
	};
	if let Err(error) = private::append_log(std::path::Path::new(&path), &format!("{:.3} {message}", now.as_secs_f64()))
	{
		eprintln!("[warn] private tray trace unavailable: {error}");
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn url_arguments_accept_only_the_two_existing_actions_without_echoing_untrusted_input() {
		assert!(matches!(initial_activation(std::iter::empty()).unwrap(), Activation::ContinueSetup));
		for (url, expected) in [
			("TESTD://fix/resolver", Activation::RepairResolver),
			("testd://fix/resolver", Activation::RepairResolver),
			("testd://fix/certificate", Activation::TrustRoot),
		] {
			let action = initial_activation([url.into()].into_iter()).unwrap();
			assert_eq!(std::mem::discriminant(&action), std::mem::discriminant(&expected));
		}
		for url in ["testd://run/sh", "testd://fix/resolver?token=private", "https://foreign.example/private"] {
			let error = initial_activation([url.into()].into_iter()).unwrap_err().to_string();
			assert!(!error.contains("private"));
		}
	}
}
