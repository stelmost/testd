//! Native launchd socket activation without changing DNS, trust, or login items.

#![cfg(target_os = "macos")]

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

struct Child(std::process::Child);

impl Drop for Child {
	fn drop(&mut self) {
		if self.0.try_wait().ok().flatten().is_none() {
			self.0.kill().unwrap();
			self.0.wait().unwrap();
		}
	}
}

#[test]
fn default_https_activates_an_exact_loopback_listener_and_releases_it_on_exit() {
	let uid = Command::new("/usr/bin/id").arg("-u").output().unwrap();
	let domain = format!("gui/{}", String::from_utf8(uid.stdout).unwrap().trim());
	let session = Command::new("/bin/launchctl").args(["print", &domain]).output().unwrap();
	assert!(session.status.success(), "native socket activation requires a logged-in macOS GUI session");
	let port = (800..1024)
		.find(|port| {
			let address = (Ipv4Addr::UNSPECIFIED, *port);
			TcpListener::bind(address).is_ok()
		})
		.expect("an unused privileged fixture port");
	let fixture = tempfile::tempdir().unwrap();
	let home = fixture.path();
	let candidate = env!("CARGO_BIN_EXE_testd");
	let initialized = Command::new(candidate)
		.args(["init", "--caddyfile", ""])
		.env("HOME", home)
		.env_remove("TESTD_HOST_TLD")
		.output()
		.unwrap();
	assert!(initialized.status.success(), "{}", String::from_utf8_lossy(&initialized.stderr));
	let intake = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
	let intake_address = intake.local_addr().unwrap();
	drop(intake);
	let path = home.join(".config/testd/config.toml");
	let config = std::fs::read_to_string(&path).unwrap();
	let config = config.replace(
		"[host]\n",
		&format!(
			"[host]\nlisten_addr = '{intake_address}'\nhttp_addr = '127.0.0.1:0'\nedge_addr = '127.0.0.1:{port}'\n"
		),
	);
	std::fs::write(&path, config).unwrap();
	let child = Command::new(candidate)
		.arg("host")
		.env("HOME", home)
		.env("TESTD_TUNNEL", home)
		.env_remove("TESTD_HOST_TLD")
		.stdin(Stdio::null())
		.stdout(Stdio::null())
		.stderr(Stdio::piped())
		.spawn()
		.unwrap();
	let mut child = Child(child);
	let errors = child.0.stderr.take().unwrap();
	let (sender, receiver) = mpsc::channel();
	std::thread::spawn(move || {
		for line in BufReader::new(errors).lines() {
			if sender.send(line.unwrap()).is_err() {
				break;
			}
		}
	});
	let mut output = String::new();
	loop {
		let line = receiver.recv_timeout(Duration::from_secs(10)).expect(&output);
		output.push_str(&line);
		output.push('\n');
		if line.starts_with("[listen]") {
			break;
		}
	}
	assert_https(home, port);
	child.0.kill().unwrap();
	child.0.wait().unwrap();
	assert!(std::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_err());
}

fn assert_https(home: &std::path::Path, port: u16) {
	use rustls::pki_types::pem::PemObject;

	let root = std::fs::read(home.join(".config/testd/pki/root.crt")).unwrap();
	let mut store = rustls::RootCertStore::empty();
	store
		.add(rustls::pki_types::CertificateDer::from_pem_slice(&root).unwrap())
		.unwrap();
	let tls = rustls::ClientConfig::builder()
		.with_root_certificates(store)
		.with_no_client_auth();
	let name = rustls::pki_types::ServerName::try_from("testd.test").unwrap();
	let connection = rustls::ClientConnection::new(std::sync::Arc::new(tls), name).unwrap();
	let socket = std::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
	socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
	socket.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
	let mut stream = rustls::StreamOwned::new(connection, socket);
	stream
		.write_all(b"GET / HTTP/1.1\r\nHost: testd.test\r\nConnection: close\r\n\r\n")
		.unwrap();
	let mut response = String::new();
	BufReader::new(&mut stream).read_line(&mut response).unwrap();
	assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
}
