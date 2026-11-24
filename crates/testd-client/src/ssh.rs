/*! Supervise a dev server over an authenticated OpenSSH command channel. */

use std::io::{self, Stderr};
use std::net::{IpAddr, Ipv4Addr, TcpListener};
use std::num::NonZeroU16;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncWrite, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior, timeout, timeout_at};

use testd_core::protocol::{self, MAX_FRAME_BYTES};

use crate::{listeners, process::ProcessTree};

mod bootstrap;
pub use bootstrap::{HELPER_VERSION, InstallRequest, bootstrap_command};

const VERSION: u16 = 1;
const IO_TIMEOUT: Duration = Duration::from_secs(3);
const HEARTBEAT: Duration = Duration::from_secs(4);
const LEASE: Duration = Duration::from_secs(20);
pub const STOP_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteCommand {
	pub program: String,
	#[serde(default)]
	pub args: Vec<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub cwd: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct App {
	pub port: NonZeroU16,
	pub command: RemoteCommand,
}

impl App {
	pub fn validate(&self) -> Result<()> {
		let command = &self.command;
		ensure!(!command.program.trim().is_empty(), "enter a remote command");
		let args = std::iter::once(&command.program).chain(&command.args).chain(&command.cwd);
		ensure!(!args.into_iter().any(|arg| arg.contains('\0')), "the remote command contains a NUL byte");
		let frame = serde_json::to_vec(&Request::Start { app: self.clone() })?;
		ensure!(frame.len() < MAX_FRAME_BYTES, "the remote command is too long");
		Ok(())
	}
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
enum Request {
	Install,
	Start { app: App },
	Ping,
	Stop,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
enum Reply {
	Install { version: Option<String> },
	Hello { version: u16 },
	Ready,
	Exited { code: Option<i32> },
	Stopped,
	Failed { message: String },
}

pub(crate) enum End {
	Stopped,
	Exited(Option<i32>),
}

/* Keep a partial frame when a heartbeat, stop, or child exit wins select!. */
struct Inbox<R> {
	reader: BufReader<R>,
	frame: Vec<u8>,
}

impl<R: AsyncRead + Unpin> Inbox<R> {
	fn new(input: R) -> Self {
		Self { reader: BufReader::new(input), frame: Vec::new() }
	}

	async fn receive<T: DeserializeOwned>(&mut self) -> Result<Option<T>> {
		if !protocol::read_line_into(&mut self.reader, &mut self.frame, MAX_FRAME_BYTES).await? {
			return Ok(None);
		}
		let result = serde_json::from_slice(&self.frame);
		self.frame.clear();
		Ok(Some(result?))
	}
}

async fn send<W: AsyncWrite + Unpin, T: Serialize>(output: &mut W, message: &T) -> Result<()> {
	timeout(IO_TIMEOUT, protocol::send(output, message))
		.await
		.context("SSH control write timed out")??;
	Ok(())
}

pub(crate) struct Session {
	stop: oneshot::Sender<()>,
	task: JoinHandle<Result<End>>,
}

impl Session {
	pub(crate) fn start(
		child: &mut Child,
		app: App,
		installation: mpsc::Sender<InstallRequest>,
		deadline: Instant,
	) -> (Self, oneshot::Receiver<()>) {
		let input = child.stdout.take().expect("managed SSH stdout");
		let output = child.stdin.take().expect("managed SSH stdin");
		let (stop, stopped) = oneshot::channel();
		let (ready, readiness) = oneshot::channel();
		let task = tokio::spawn(host_session(input, output, app, installation, deadline, stopped, ready));
		(Self { stop, task }, readiness)
	}

	pub(crate) async fn finished(&mut self) -> Result<End> {
		(&mut self.task).await.context("SSH control task failed")?
	}

	pub(crate) async fn stop(self) -> Result<End> {
		let _ = self.stop.send(());
		self.task.await.context("SSH control task failed")?
	}
}

async fn host_session(
	input: impl AsyncRead + Unpin,
	mut output: impl AsyncWrite + Unpin,
	app: App,
	installation: mpsc::Sender<InstallRequest>,
	deadline: Instant,
	mut stopped: oneshot::Receiver<()>,
	ready: oneshot::Sender<()>,
) -> Result<End> {
	let mut inbox = Inbox::new(input);
	tokio::select! {
		biased;
		_ = &mut stopped => return Ok(End::Stopped),
		result = timeout_at(deadline, prepare_helper(&mut inbox, &mut output, installation)) => {
			result.context("preparing testd on the SSH host timed out")??;
		},
	};
	send(&mut output, &Request::Start { app })
		.await
		.context("could not start the remote command")?;
	drive_app(&mut inbox, &mut output, deadline, stopped, ready).await
}

async fn prepare_helper<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
	inbox: &mut Inbox<R>,
	output: &mut W,
	installation: mpsc::Sender<InstallRequest>,
) -> Result<()> {
	let mut hello = inbox.receive().await.context("invalid SSH helper greeting")?;
	if let Some(Reply::Install { version }) = hello {
		let (respond, answer) = oneshot::channel();
		installation
			.send(InstallRequest { version, respond })
			.await
			.context("the SSH installation prompt is unavailable")?;
		ensure!(answer.await.unwrap_or(false), "remote testd installation was cancelled");
		send(output, &Request::Install).await?;
		hello = inbox
			.receive()
			.await
			.context("invalid SSH helper greeting after installation")?;
	}
	match hello {
		Some(Reply::Hello { version: VERSION }) => Ok(()),
		Some(Reply::Hello { version }) => bail!("SSH host uses unsupported testd protocol version {version}"),
		Some(Reply::Failed { message }) => bail!("SSH host: {message}"),
		_ => bail!("testd did not start on the SSH host; check the SSH app logs"),
	}
}

async fn drive_app<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
	inbox: &mut Inbox<R>,
	output: &mut W,
	deadline: Instant,
	mut stopped: oneshot::Receiver<()>,
	ready: oneshot::Sender<()>,
) -> Result<End> {
	let mut ready = Some(ready);
	let mut heartbeat = tokio::time::interval(HEARTBEAT);
	heartbeat.set_missed_tick_behavior(MissedTickBehavior::Skip);
	loop {
		tokio::select! {
			biased;
			_ = &mut stopped => return stop_app(inbox, output).await,
			reply = inbox.receive() => match reply? {
				Some(Reply::Ready) => {
					if let Some(ready) = ready.take() {
						let _ = ready.send(());
					}
				},
				reply => return terminal_reply(reply),
			},
			_ = tokio::time::sleep_until(deadline), if ready.is_some() => {
				stop_app(inbox, output).await?;
				bail!("the remote command did not listen on the app port");
			},
			_ = heartbeat.tick() => send(output, &Request::Ping).await?,
		}
	}
}

async fn stop_app<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(inbox: &mut Inbox<R>, output: &mut W) -> Result<End> {
	send(output, &Request::Stop)
		.await
		.context("remote shutdown could not be confirmed")?;
	timeout(STOP_TIMEOUT, wait_for_exit(inbox))
		.await
		.context("remote shutdown could not be confirmed in time")?
}

async fn wait_for_exit<R: AsyncRead + Unpin>(inbox: &mut Inbox<R>) -> Result<End> {
	loop {
		let reply = inbox.receive().await?;
		if !matches!(reply, Some(Reply::Ready)) {
			return terminal_reply(reply);
		}
	}
}

fn terminal_reply(reply: Option<Reply>) -> Result<End> {
	match reply {
		Some(Reply::Exited { code }) => Ok(End::Exited(code)),
		Some(Reply::Stopped) => Ok(End::Stopped),
		Some(Reply::Failed { message }) => bail!("SSH host: {message}"),
		Some(_) => bail!("unexpected reply from the SSH host"),
		None => bail!("SSH connection closed before remote shutdown could be confirmed"),
	}
}

/// The remote command runs as the SSH user and owns a single process tree.
pub async fn serve(input: impl AsyncRead + Unpin, mut output: impl AsyncWrite + Unpin) -> Result<()> {
	let mut inbox = Inbox::new(input);
	send(&mut output, &Reply::Hello { version: VERSION }).await?;
	let start = timeout(LEASE, inbox.receive())
		.await
		.context("waiting for a remote start command timed out")??;
	let app = match start {
		Some(Request::Start { app }) => app,
		Some(Request::Stop) | None => return Ok(()),
		Some(Request::Ping | Request::Install) => bail!("expected a remote start command"),
	};
	let result = run_app(app, &mut inbox, &mut output).await;
	match result {
		Ok(End::Exited(code)) => send(&mut output, &Reply::Exited { code }).await,
		Ok(End::Stopped) => send(&mut output, &Reply::Stopped).await,
		Err(error) => {
			let message = format!("{error:#}");
			let _ = send(&mut output, &Reply::Failed { message }).await;
			Err(error)
		},
	}
}

async fn run_app<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
	app: App,
	inbox: &mut Inbox<R>,
	output: &mut W,
) -> Result<End> {
	app.validate()?;
	drop(TcpListener::bind((Ipv4Addr::LOCALHOST, app.port.get())).context("remote loopback app port is unavailable")?);
	let shutdown = crate::prepare_shutdown_signal()?;
	let mut command = Command::new(&app.command.program);
	command
		.args(&app.command.args)
		.stdin(Stdio::null())
		.stdout(log_output(io::stderr())?)
		.stderr(Stdio::inherit());
	if let Some(cwd) = app.command.cwd.filter(|cwd| !cwd.is_empty()) {
		command.current_dir(cwd);
	}
	let (mut child, mut tree) =
		ProcessTree::spawn(&mut command).with_context(|| format!("could not start {}", app.command.program))?;
	let result = tokio::select! {
		signal = shutdown => signal.map(|()| End::Stopped).map_err(Into::into),
		result = watch_app(&mut child, app.port, inbox, output) => result,
	};
	tree.stop(&mut child).await.context("could not stop the remote command")?;
	result
}

async fn watch_app<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
	child: &mut Child,
	port: NonZeroU16,
	inbox: &mut Inbox<R>,
	output: &mut W,
) -> Result<End> {
	let listener = wait_for_port(child.id().expect("new SSH app child"), port.get());
	tokio::pin!(listener);
	let mut announced = false;
	let mut lease = Instant::now() + LEASE;
	loop {
		tokio::select! {
			status = child.wait() => return Ok(End::Exited(status?.code())),
			request = timeout_at(lease, inbox.receive()) => match request.context("SSH control heartbeat expired")?? {
				Some(Request::Ping) => lease = Instant::now() + LEASE,
				Some(Request::Stop) | None => return Ok(End::Stopped),
				Some(Request::Start { .. } | Request::Install) => bail!("the SSH session already owns an app"),
			},
			result = &mut listener, if !announced => {
				result?;
				send(output, &Reply::Ready).await?;
				announced = true;
			},
		}
	}
}

async fn wait_for_port(pid: u32, port: u16) -> Result<()> {
	loop {
		let sockets = tokio::task::spawn_blocking(move || listeners::discover(pid))
			.await?
			.map_err(anyhow::Error::msg)?;
		if sockets.iter().any(|socket| {
			socket.port == port
				&& (socket.address.is_unspecified() || socket.address == IpAddr::V4(Ipv4Addr::LOCALHOST))
		}) {
			return Ok(());
		}
		tokio::time::sleep(Duration::from_millis(200)).await;
	}
}

#[cfg(unix)]
fn log_output(stderr: Stderr) -> io::Result<Stdio> {
	use std::os::fd::AsFd;
	stderr.as_fd().try_clone_to_owned().map(Stdio::from)
}

#[cfg(windows)]
fn log_output(stderr: Stderr) -> io::Result<Stdio> {
	use std::os::windows::io::AsHandle;
	stderr.as_handle().try_clone_to_owned().map(Stdio::from)
}

#[cfg(not(any(unix, windows)))]
fn log_output(_stderr: Stderr) -> io::Result<Stdio> {
	Ok(Stdio::null())
}

#[cfg(test)]
mod tests {
	use super::*;
	use tokio::io::duplex;

	fn app() -> App {
		App {
			port: NonZeroU16::new(3000).unwrap(),
			command: RemoteCommand {
				program: "program".into(),
				args: vec!["two words".into(), "$literal".into()],
				cwd: Some("path with spaces".into()),
			},
		}
	}

	#[tokio::test]
	async fn installation_waits_for_approval_before_starting_the_app() {
		let (host, leaf) = duplex(4096);
		let (input, output) = tokio::io::split(host);
		let (leaf_input, mut leaf_output) = tokio::io::split(leaf);
		let mut inbox = Inbox::new(leaf_input);
		let (_stop, stopped) = oneshot::channel();
		let (ready, _) = oneshot::channel();
		let (installation, mut requests) = mpsc::channel(1);
		let host =
			tokio::spawn(host_session(input, output, app(), installation, Instant::now() + IO_TIMEOUT, stopped, ready));
		send(&mut leaf_output, &Reply::Install { version: Some("0.0.1".into()) })
			.await
			.unwrap();
		let request = timeout(IO_TIMEOUT, requests.recv()).await.unwrap().unwrap();
		assert_eq!(request.version.as_deref(), Some("0.0.1"));
		assert!(timeout(Duration::from_millis(20), inbox.receive::<Request>()).await.is_err());
		request.respond.send(true).unwrap();
		assert!(matches!(timeout(IO_TIMEOUT, inbox.receive()).await.unwrap().unwrap(), Some(Request::Install)));
		send(&mut leaf_output, &Reply::Hello { version: VERSION }).await.unwrap();
		assert!(matches!(timeout(IO_TIMEOUT, inbox.receive()).await.unwrap().unwrap(), Some(Request::Start { .. })));
		send(&mut leaf_output, &Reply::Exited { code: Some(0) }).await.unwrap();
		assert!(matches!(timeout(IO_TIMEOUT, host).await.unwrap().unwrap().unwrap(), End::Exited(Some(0))));
	}

	#[tokio::test]
	async fn declining_installation_sends_no_remote_command() {
		let (host, leaf) = duplex(1024);
		let (input, output) = tokio::io::split(host);
		let (leaf_input, mut leaf_output) = tokio::io::split(leaf);
		let (_stop, stopped) = oneshot::channel();
		let (ready, _) = oneshot::channel();
		let (installation, mut requests) = mpsc::channel(1);
		let host =
			tokio::spawn(host_session(input, output, app(), installation, Instant::now() + IO_TIMEOUT, stopped, ready));
		send(&mut leaf_output, &Reply::Install { version: None }).await.unwrap();
		let request = timeout(IO_TIMEOUT, requests.recv()).await.unwrap().unwrap();
		assert!(request.version.is_none());
		request.respond.send(false).unwrap();
		let error = timeout(IO_TIMEOUT, host).await.unwrap().unwrap().err().unwrap();
		assert!(error.to_string().contains("cancelled"));
		assert!(Inbox::new(leaf_input).receive::<Request>().await.unwrap().is_none());
	}

	#[tokio::test]
	async fn stopping_while_installation_is_pending_expires_the_confirmation() {
		let (host, leaf) = duplex(1024);
		let (input, output) = tokio::io::split(host);
		let (leaf_input, mut leaf_output) = tokio::io::split(leaf);
		let (stop, stopped) = oneshot::channel();
		let (ready, _) = oneshot::channel();
		let (installation, mut requests) = mpsc::channel(1);
		let host =
			tokio::spawn(host_session(input, output, app(), installation, Instant::now() + IO_TIMEOUT, stopped, ready));
		send(&mut leaf_output, &Reply::Install { version: None }).await.unwrap();
		let mut request = timeout(IO_TIMEOUT, requests.recv()).await.unwrap().unwrap();
		stop.send(()).unwrap();
		assert!(matches!(timeout(IO_TIMEOUT, host).await.unwrap().unwrap().unwrap(), End::Stopped));
		timeout(IO_TIMEOUT, request.respond.closed()).await.unwrap();
		assert!(Inbox::new(leaf_input).receive::<Request>().await.unwrap().is_none());
	}

	#[tokio::test]
	async fn stop_waits_for_the_remote_app_to_exit() {
		let (host, leaf) = duplex(4096);
		let (input, output) = tokio::io::split(host);
		let (leaf_input, mut leaf_output) = tokio::io::split(leaf);
		let mut inbox = Inbox::new(leaf_input);
		let (stop, stopped) = oneshot::channel();
		let (ready, readiness) = oneshot::channel();
		let deadline = Instant::now() + IO_TIMEOUT;
		let (installation, _) = mpsc::channel(1);
		let mut host = tokio::spawn(host_session(input, output, app(), installation, deadline, stopped, ready));
		send(&mut leaf_output, &Reply::Hello { version: VERSION }).await.unwrap();
		let Some(Request::Start { app }) = timeout(IO_TIMEOUT, inbox.receive()).await.unwrap().unwrap() else {
			panic!("expected the saved command");
		};
		assert_eq!(app.command.args, ["two words", "$literal"]);
		assert_eq!(app.command.cwd.as_deref(), Some("path with spaces"));
		send(&mut leaf_output, &Reply::Ready).await.unwrap();
		timeout(IO_TIMEOUT, readiness).await.unwrap().unwrap();
		stop.send(()).unwrap();
		let stopped = async {
			while let Some(request) = inbox.receive().await.unwrap() {
				if matches!(request, Request::Stop) {
					return;
				}
			}
			panic!("SSH channel closed without sending Stop");
		};
		timeout(IO_TIMEOUT, stopped).await.unwrap();
		assert!(!host.is_finished());
		send(&mut leaf_output, &Reply::Stopped).await.unwrap();
		assert!(matches!(timeout(IO_TIMEOUT, &mut host).await.unwrap().unwrap().unwrap(), End::Stopped));
	}

	#[tokio::test]
	async fn cancelling_before_the_greeting_sends_no_command() {
		let (host, mut leaf) = duplex(1024);
		let (input, output) = tokio::io::split(host);
		let (stop, stopped) = oneshot::channel();
		let (ready, _) = oneshot::channel();
		let (installation, _) = mpsc::channel(1);
		stop.send(()).unwrap();
		let result =
			host_session(input, output, app(), installation, Instant::now() + IO_TIMEOUT, stopped, ready).await;
		assert!(matches!(result.unwrap(), End::Stopped));
		assert!(Inbox::new(&mut leaf).receive::<Request>().await.unwrap().is_none());
	}
}
