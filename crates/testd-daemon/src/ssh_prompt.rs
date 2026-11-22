//! Use the desktop dashboard for prompts from the OpenSSH child process.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use testd_core::{config::generate_operator_secret, protocol};
use tokio::io::AsyncReadExt;
use tokio::sync::{oneshot, watch};
use tokio::time::timeout;

use crate::task::BackgroundTask;

const ENV: &str = "TESTD_SSH_ASKPASS";
const ANSWER_LIMIT: usize = 4096;
const PROMPT_TIMEOUT: Duration = Duration::from_secs(110);

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
	Response,
	Notice,
}

#[derive(Debug, Clone, Serialize)]
pub struct Prompt {
	id: String,
	message: String,
	kind: Kind,
}

#[derive(Deserialize, Serialize)]
struct Request {
	token: String,
	message: String,
	kind: Kind,
}

struct Pending {
	view: Prompt,
	reply: oneshot::Sender<Option<String>>,
}

#[derive(Default)]
struct State {
	pending: Option<Pending>,
	cancelled: bool,
}

pub(crate) struct Prompts {
	state: Mutex<State>,
	revision: watch::Sender<u64>,
}

impl Prompts {
	pub(crate) fn view(&self) -> Option<Prompt> {
		self.state
			.lock()
			.expect("SSH prompts poisoned")
			.pending
			.as_ref()
			.map(|pending| pending.view.clone())
	}

	pub(crate) fn answer(&self, id: &str, value: Option<String>) -> Result<(), String> {
		if value
			.as_ref()
			.is_some_and(|value| value.len() > ANSWER_LIMIT || value.contains(['\r', '\n', '\0']))
		{
			return Err("invalid SSH response".into());
		}
		let mut state = self.state.lock().expect("SSH prompts poisoned");
		if state.pending.as_ref().is_none_or(|pending| pending.view.id != id) {
			return Err("this SSH prompt has expired".into());
		}
		let pending = state.pending.take().expect("matched SSH prompt");
		state.cancelled = value.is_none();
		self.changed();
		pending.reply.send(value).map_err(|_| "this SSH prompt has expired".into())
	}

	fn changed(&self) {
		self.revision.send_modify(|revision| *revision = revision.wrapping_add(1));
	}
}

pub(crate) struct Askpass {
	pub(crate) prompts: Arc<Prompts>,
	pub(crate) env: Vec<(String, String)>,
	task: BackgroundTask,
}

impl Drop for Askpass {
	fn drop(&mut self) {
		self.task.0.abort();
		let mut state = self.prompts.state.lock().expect("SSH prompts poisoned");
		state.cancelled = true;
		state.pending.take();
		self.prompts.changed();
	}
}

impl Askpass {
	pub(crate) fn start(revision: watch::Sender<u64>) -> anyhow::Result<Self> {
		let socket = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
		socket.set_nonblocking(true)?;
		let address = socket.local_addr()?;
		let socket = tokio::net::TcpListener::from_std(socket)?;
		let token = generate_operator_secret()?;
		let executable = askpass_executable()?
			.into_os_string()
			.into_string()
			.map_err(|_| anyhow::anyhow!("the testd executable path is not valid UTF-8"))?;
		let display = std::env::var("DISPLAY").unwrap_or_else(|_| "testd".into());
		let env = vec![
			("SSH_ASKPASS".into(), executable),
			("SSH_ASKPASS_REQUIRE".into(), "force".into()),
			("DISPLAY".into(), display),
			(ENV.into(), format!("{address} {token}")),
		];
		let prompts = Arc::new(Prompts { state: Mutex::default(), revision });
		let state = Arc::clone(&prompts);
		let task = tokio::spawn(async move {
			while let Ok((mut stream, _)) = socket.accept().await {
				if let Err(error) = respond(&state, &token, &mut stream).await
					&& !matches!(error.kind(), std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset)
				{
					eprintln!("[testd] [warn] SSH prompt connection: {error}");
				}
			}
		});
		Ok(Self { prompts, env, task: BackgroundTask(task) })
	}
}

async fn respond(prompts: &Prompts, token: &str, stream: &mut tokio::net::TcpStream) -> std::io::Result<()> {
	use subtle::ConstantTimeEq;

	let (reader, mut writer) = stream.split();
	let mut reader = tokio::io::BufReader::new(reader);
	let Ok(Ok(Some(request))) = timeout(Duration::from_secs(2), protocol::recv::<_, Request>(&mut reader)).await else {
		return Ok(());
	};
	if !bool::from(token.as_bytes().ct_eq(request.token.as_bytes())) || request.message.len() > ANSWER_LIMIT {
		return Ok(());
	}

	let (reply, received) = oneshot::channel();
	let view = Prompt { id: generate_operator_secret()?, message: request.message, kind: request.kind };
	let accepts_prompt = {
		let mut state = prompts.state.lock().expect("SSH prompts poisoned");
		if !state.cancelled {
			state.pending = Some(Pending { view, reply });
		}
		!state.cancelled
	};
	if !accepts_prompt {
		return protocol::send(&mut writer, &Option::<String>::None).await;
	}
	prompts.changed();

	let value = tokio::select! {
		response = timeout(PROMPT_TIMEOUT, received) => response.ok().and_then(Result::ok).flatten(),
		_ = reader.read_u8() => None,
	};
	{
		let mut state = prompts.state.lock().expect("SSH prompts poisoned");
		state.pending.take();
		state.cancelled |= value.is_none();
	}
	prompts.changed();
	protocol::send(&mut writer, &value).await
}

fn askpass_executable() -> std::io::Result<PathBuf> {
	#[cfg(target_os = "linux")]
	if let (Some(appdir), Some(appimage)) = (std::env::var_os("APPDIR"), std::env::var_os("APPIMAGE"))
		&& std::env::current_exe()?.starts_with(appdir)
	{
		return Ok(PathBuf::from(appimage));
	}
	std::env::current_exe()
}

/// OpenSSH invokes this executable with the prompt as its sole argument.
pub fn run_askpass() -> bool {
	let Ok(capability) = std::env::var(ENV) else { return false };
	match ask(&capability) {
		Ok(Some(answer)) => {
			let mut output = std::io::stdout().lock();
			if writeln!(output, "{answer}").is_err() {
				std::process::exit(1);
			}
			true
		},
		Ok(None) => std::process::exit(1),
		Err(error) => {
			eprintln!("testd: SSH prompt failed: {error:#}");
			std::process::exit(1);
		},
	}
}

fn ask(capability: &str) -> anyhow::Result<Option<String>> {
	use anyhow::{Context, ensure};

	let (address, token) = capability.split_once(' ').context("invalid SSH prompt address")?;
	let address: SocketAddr = address.parse()?;
	ensure!(address.ip().is_loopback(), "SSH prompt address must be local");
	let [message]: [_; 1] = std::env::args_os()
		.skip(1)
		.collect::<Vec<_>>()
		.try_into()
		.map_err(|_| anyhow::anyhow!("expected one SSH prompt"))?;
	let kind = match std::env::var("SSH_ASKPASS_PROMPT").as_deref() {
		Ok("none") => Kind::Notice,
		_ => Kind::Response,
	};
	let request = Request { token: token.to_owned(), message: message.to_string_lossy().into_owned(), kind };
	let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
	stream.set_read_timeout(Some(PROMPT_TIMEOUT + Duration::from_secs(5)))?;
	stream.set_write_timeout(Some(Duration::from_secs(2)))?;
	serde_json::to_writer(&stream, &request)?;
	stream.write_all(b"\n")?;

	/* JSON can escape a byte as six bytes. The broker bounds the decoded response. */
	let mut line = Vec::new();
	BufReader::new(stream)
		.take((ANSWER_LIMIT * 6 + 4) as u64)
		.read_until(b'\n', &mut line)?;
	Ok(serde_json::from_slice(&line)?)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn prompts_are_scoped_to_the_launch_and_cancelled_prompts_do_not_retry() {
		let (revision, mut changes) = watch::channel(0);
		let broker = Askpass::start(revision).unwrap();
		let (address, token) = broker
			.env
			.iter()
			.find(|(key, _)| key == ENV)
			.unwrap()
			.1
			.split_once(' ')
			.unwrap();

		let mut wrong_child = tokio::net::TcpStream::connect(address).await.unwrap();
		let mut request = Request { token: "wrong".into(), message: "SSH password:".into(), kind: Kind::Response };
		protocol::send(&mut wrong_child, &request).await.unwrap();
		assert_eq!(wrong_child.read_u8().await.unwrap_err().kind(), std::io::ErrorKind::UnexpectedEof);
		assert!(broker.prompts.view().is_none());

		request.token = token.into();
		let mut child = tokio::net::TcpStream::connect(address).await.unwrap();
		protocol::send(&mut child, &request).await.unwrap();
		timeout(Duration::from_secs(2), changes.changed()).await.unwrap().unwrap();
		let prompt = broker.prompts.view().unwrap();
		assert!(broker.prompts.answer("wrong-prompt", Some("answer".into())).is_err());
		assert!(broker.prompts.answer(&prompt.id, Some("invalid\nanswer".into())).is_err());
		broker.prompts.answer(&prompt.id, Some("fixture".into())).unwrap();
		let mut reader = tokio::io::BufReader::new(child);
		assert_eq!(protocol::recv::<_, Option<String>>(&mut reader).await.unwrap(), Some(Some("fixture".into())));

		let mut child = tokio::net::TcpStream::connect(address).await.unwrap();
		protocol::send(&mut child, &request).await.unwrap();
		let prompt = timeout(Duration::from_secs(2), async {
			loop {
				if let Some(current) = broker.prompts.view()
					&& current.id != prompt.id
				{
					break current;
				}
				changes.changed().await.unwrap();
			}
		})
		.await
		.unwrap();
		broker.prompts.answer(&prompt.id, None).unwrap();

		let mut reader = tokio::io::BufReader::new(child);
		assert_eq!(protocol::recv::<_, Option<String>>(&mut reader).await.unwrap(), Some(None));

		let mut retry = tokio::net::TcpStream::connect(address).await.unwrap();
		protocol::send(&mut retry, &request).await.unwrap();
		let mut reader = tokio::io::BufReader::new(retry);
		assert_eq!(protocol::recv::<_, Option<String>>(&mut reader).await.unwrap(), Some(None));
		assert!(broker.prompts.view().is_none());
	}
}
