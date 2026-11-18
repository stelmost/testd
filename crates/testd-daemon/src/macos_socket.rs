//! Receive an exact loopback listener from launchd without elevating the daemon.

use std::io;
use std::net::{SocketAddr, TcpListener};
use std::os::fd::OwnedFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

mod transfer;

const HELPER_FLAG: &str = "--testd-socket-helper";
const SOCKET_NAME: &str = "HTTPS";
const TIMEOUT: Duration = Duration::from_secs(5);

/// Handle launchd's socket-transfer invocation before initializing CLI or GUI state.
pub fn run_helper() -> Option<io::Result<()>> {
	let mut arguments = std::env::args_os().skip(1);
	if arguments.next().as_deref() != Some(std::ffi::OsStr::new(HELPER_FLAG)) {
		return None;
	}
	Some((|| {
		let path = arguments
			.next()
			.ok_or_else(|| io::Error::other("missing socket-transfer path"))?;
		if arguments.next().is_some() {
			return Err(io::Error::other("unexpected socket-helper arguments"));
		}
		let listener = transfer::activate(SOCKET_NAME)?;
		let channel = UnixStream::connect(path)?;
		transfer::send(&channel, &listener)
	})())
}

pub(crate) fn bind(address: SocketAddr) -> io::Result<TcpListener> {
	match TcpListener::bind(address) {
		Err(error) if error.kind() == io::ErrorKind::PermissionDenied && eligible(address) => activate(address),
		result => result,
	}
}

fn eligible(address: SocketAddr) -> bool {
	address.ip().is_loopback() && (1..1024).contains(&address.port())
}

fn activate(address: SocketAddr) -> io::Result<TcpListener> {
	let directory = tempfile::Builder::new().prefix("testd-socket-").tempdir()?;
	let path = directory.path().join("transfer.sock");
	let channel = UnixListener::bind(&path)?;
	channel.set_nonblocking(true)?;
	let id = directory.path().file_name().unwrap().to_string_lossy();
	let label = format!("dev.testd.socket.{id}");
	/* SAFETY: geteuid has no arguments or memory preconditions. */
	let target = format!("gui/{}/{label}", unsafe { libc::geteuid() });
	let plist = directory.path().join("socket.plist");
	let log = directory.path().join("helper.log");
	write_job(&plist, &label, address, &path, &log)?;
	let job = Job { target, loaded: false };
	let mut job = job.load(&plist)?;
	let result = receive(&channel, address).map_err(|error| {
		let detail = std::fs::read_to_string(&log).unwrap_or_default();
		io::Error::new(error.kind(), format!("launchd listener transfer failed: {error}; {}", detail.trim()))
	});
	job.unload()?;
	result
}

fn write_job(path: &Path, label: &str, address: SocketAddr, channel: &Path, log: &Path) -> io::Result<()> {
	let executable = std::env::current_exe()?;
	let value = serde_json::json!({
		"Label": label,
		"ProgramArguments": [executable, HELPER_FLAG, channel],
		"RunAtLoad": true,
		"StandardErrorPath": log,
		"Sockets": {
			SOCKET_NAME: {
				"SockFamily": if address.is_ipv4() { "IPv4" } else { "IPv6" },
				"SockType": "stream",
				"SockNodeName": address.ip().to_string(),
				"SockServiceName": address.port().to_string(),
				"SockPassive": true
			}
		}
	});
	plist::to_file_xml(path, &value).map_err(io::Error::other)
}

fn receive(channel: &UnixListener, address: SocketAddr) -> io::Result<TcpListener> {
	let deadline = Instant::now() + TIMEOUT;
	let stream = loop {
		match channel.accept() {
			Ok((stream, _)) => break stream,
			Err(error) if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline => {
				std::thread::sleep(Duration::from_millis(10));
			},
			Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
				return Err(io::Error::new(io::ErrorKind::TimedOut, "launchd did not start the socket helper"));
			},
			Err(error) => return Err(io::Error::new(error.kind(), format!("accept helper channel: {error}"))),
		}
	};
	let descriptor: OwnedFd = transfer::receive(&stream)
		.map_err(|error| io::Error::new(error.kind(), format!("receive descriptor: {error}")))?;
	let listener = TcpListener::from(descriptor);
	let actual = listener
		.local_addr()
		.map_err(|error| io::Error::new(error.kind(), format!("inspect listener address: {error}")))?;
	let socket_type = socket2::SockRef::from(&listener)
		.r#type()
		.map_err(|error| io::Error::new(error.kind(), format!("inspect socket type: {error}")))?;
	if actual != address || socket_type != socket2::Type::STREAM {
		return Err(io::Error::other("launchd returned a different socket; refusing the listener"));
	}
	Ok(listener)
}

struct Job {
	target: String,
	loaded: bool,
}

impl Job {
	fn load(mut self, plist: &Path) -> io::Result<Self> {
		let domain = self.target.rsplit_once('/').unwrap().0;
		let output = Command::new("/bin/launchctl").args(["bootstrap", domain]).arg(plist).output()?;
		if !output.status.success() {
			return Err(io::Error::other(format!(
				"launchd could not create the loopback listener; use the logged-in desktop session: {}",
				String::from_utf8_lossy(&output.stderr).trim()
			)));
		}
		self.loaded = true;
		Ok(self)
	}

	fn unload(&mut self) -> io::Result<()> {
		let output = Command::new("/bin/launchctl").args(["bootout", &self.target]).output()?;
		if !output.status.success() {
			return Err(io::Error::other(format!(
				"could not unload socket helper {}; inspect before retrying: {}",
				self.target,
				String::from_utf8_lossy(&output.stderr).trim()
			)));
		}
		self.loaded = false;
		Ok(())
	}
}

impl Drop for Job {
	fn drop(&mut self) {
		if self.loaded
			&& let Err(error) = self.unload()
		{
			eprintln!("[error] {error}");
		}
	}
}

#[cfg(test)]
mod tests;
