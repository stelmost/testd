/*! Early owner-authenticated activation, including before the daemon exists. */

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::{JoinHandle, JoinSet};

use crate::host::Command;

const LIMIT: u64 = 4096;
const TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Activation {
	ContinueSetup,
	RepairResolver,
	TrustRoot,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Descriptor {
	address: SocketAddr,
	token: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Request {
	token: String,
	action: Activation,
}

#[derive(Deserialize, Serialize)]
struct Response {
	accepted: bool,
}

pub struct Guard {
	path: PathBuf,
	file: File,
	_task: JoinHandle<()>,
	_lock: File,
}

impl Drop for Guard {
	fn drop(&mut self) {
		self._task.abort();
		if let Err(error) = crate::private::remove_owned(&self.path, &self.file) {
			eprintln!("[warn] tray activation cleanup failed: {error}");
		}
	}
}

/// `None` means the request was delegated to an existing tray, not executed here.
pub fn acquire(
	directory: &Path,
	action: Activation,
	runtime: &tokio::runtime::Handle,
	commands: mpsc::Sender<Command>,
) -> anyhow::Result<Option<Guard>> {
	crate::private::directory(directory)?;
	let lock_path = directory.join("tray.lock");
	let lock = match testd_core::private_file::create_new(&lock_path) {
		Ok(file) => file,
		Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => crate::private::open(&lock_path)?,
		Err(error) => return Err(error.into()),
	};
	let path = directory.join("tray-control.json");
	match lock.try_lock_exclusive() {
		Ok(()) => {},
		Err(error) if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {
			delegate(&path, action)?;
			return Ok(None);
		},
		Err(error) => return Err(error).context("cannot acquire the tray instance lock"),
	}
	/* A free lock permits replacement of a valid stale descriptor, not arbitrary local files. */
	if path.try_exists()? {
		let _: Descriptor = serde_json::from_slice(&crate::private::read(&path)?)
			.context("invalid existing tray activation state; left untouched")?;
	}
	let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
	listener.set_nonblocking(true)?;
	let descriptor =
		Descriptor { address: listener.local_addr()?, token: testd_core::config::generate_operator_secret()? };
	testd_core::config::write_private(&path, &serde_json::to_vec(&descriptor)?)?;
	let file = crate::private::open(&path)?;
	let _entered = runtime.enter();
	let listener = TcpListener::from_std(listener)?;
	let task = runtime.spawn(serve(listener, descriptor.token, commands));
	Ok(Some(Guard { path, file, _task: task, _lock: lock }))
}

fn delegate(path: &Path, action: Activation) -> anyhow::Result<()> {
	let deadline = Instant::now() + TIMEOUT;
	loop {
		match send(path, action) {
			Ok(()) => return Ok(()),
			Err(error) if Instant::now() >= deadline => {
				return Err(error).context("the existing tray is starting or unavailable; it was not replaced");
			},
			Err(_) => std::thread::sleep(Duration::from_millis(50)),
		}
	}
}

fn send(path: &Path, action: Activation) -> anyhow::Result<()> {
	let descriptor: Descriptor = serde_json::from_slice(&crate::private::read(path)?)?;
	if !descriptor.address.ip().is_loopback() {
		bail!("tray activation must stay on loopback");
	}
	let mut stream = TcpStream::connect_timeout(&descriptor.address, TIMEOUT)?;
	stream.set_read_timeout(Some(TIMEOUT))?;
	stream.set_write_timeout(Some(TIMEOUT))?;
	stream.write_all(&serde_json::to_vec(&Request { token: descriptor.token, action })?)?;
	stream.write_all(b"\n")?;
	let mut response = Vec::new();
	BufReader::new(std::io::Read::take(stream, LIMIT + 1)).read_until(b'\n', &mut response)?;
	if response.len() as u64 > LIMIT || !response.ends_with(b"\n") {
		bail!("invalid tray activation response");
	}
	if !serde_json::from_slice::<Response>(&response)?.accepted {
		bail!("the tray refused activation; it may be stopping or busy");
	}
	Ok(())
}

async fn serve(listener: TcpListener, token: String, commands: mpsc::Sender<Command>) {
	let mut clients = JoinSet::new();
	loop {
		tokio::select! {
			accepted = listener.accept() => match accepted {
				Ok((stream, peer)) if peer.ip().is_loopback() && clients.len() < 16 => {
					let token = token.clone();
					let commands = commands.clone();
					clients.spawn(async move {
						let _ = tokio::time::timeout(TIMEOUT, handle(stream, &token, commands)).await;
					});
				},
				Ok(_) => {},
				Err(error) => {
					eprintln!("[error] tray activation listener failed: {error}");
					return;
				},
			},
			_ = clients.join_next(), if !clients.is_empty() => {},
		}
	}
}

async fn handle(stream: tokio::net::TcpStream, token: &str, commands: mpsc::Sender<Command>) -> std::io::Result<()> {
	let (reader, mut writer) = stream.into_split();
	let mut line = Vec::new();
	tokio::io::BufReader::new(reader)
		.take(LIMIT + 1)
		.read_until(b'\n', &mut line)
		.await?;
	let accepted = if line.len() as u64 <= LIMIT && line.ends_with(b"\n") {
		serde_json::from_slice::<Request>(&line).ok().is_some_and(|request| {
			bool::from(token.as_bytes().ct_eq(request.token.as_bytes()))
				&& commands.try_send(Command::Activate(request.action)).is_ok()
		})
	} else {
		false
	};
	let mut response = serde_json::to_vec(&Response { accepted })?;
	response.push(b'\n');
	writer.write_all(&response).await
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
	async fn duplicate_start_delegates_without_taking_over_and_cleanup_preserves_the_lock_file() {
		let home = tempfile::tempdir().unwrap();
		let (commands, mut receive) = mpsc::channel(4);
		let runtime = tokio::runtime::Handle::current();
		let guard = acquire(home.path(), Activation::ContinueSetup, &runtime, commands.clone())
			.unwrap()
			.unwrap();
		assert!(
			acquire(home.path(), Activation::TrustRoot, &runtime, commands)
				.unwrap()
				.is_none()
		);
		assert!(matches!(receive.recv().await, Some(Command::Activate(Activation::TrustRoot))));
		drop(guard);
		assert!(!home.path().join("tray-control.json").exists());
		assert!(home.path().join("tray.lock").exists());
	}

	#[tokio::test]
	async fn activation_rejects_wrong_tokens_and_unknown_commands() {
		let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
		let address = listener.local_addr().unwrap();
		let (commands, mut receive) = mpsc::channel(4);
		for request in [
			b"{\"token\":\"wrong\",\"action\":\"trust_root\"}\n".as_slice(),
			b"{\"token\":\"owner\",\"action\":\"execute\"}\n".as_slice(),
		] {
			let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
			let (server, _) = listener.accept().await.unwrap();
			let task = tokio::spawn(handle(server, "owner", commands.clone()));
			client.write_all(request).await.unwrap();
			let mut result = Vec::new();
			client.read_to_end(&mut result).await.unwrap();
			assert!(!serde_json::from_slice::<Response>(&result).unwrap().accepted);
			task.await.unwrap().unwrap();
		}
		assert!(receive.try_recv().is_err());
	}
}
