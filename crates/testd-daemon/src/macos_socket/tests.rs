use std::io::Write;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::os::fd::AsRawFd;

use super::*;

#[test]
fn activation_is_limited_to_privileged_loopback_addresses() {
	for address in ["127.0.0.1:443", "[::1]:443", "127.0.0.1:80"] {
		assert!(eligible(address.parse().unwrap()), "{address}");
	}
	for address in ["0.0.0.0:443", "[::]:443", "192.0.2.1:443", "127.0.0.1:0", "127.0.0.1:8443"] {
		assert!(!eligible(address.parse().unwrap()), "{address}");
	}
}

#[test]
fn launchd_job_keeps_the_exact_address_and_escapes_paths() {
	let directory = tempfile::tempdir().unwrap();
	let path = directory.path().join("job.plist");
	let channel = directory.path().join("channel & ' local");
	let log = directory.path().join("helper.log");
	for address in [SocketAddr::from((Ipv4Addr::LOCALHOST, 443)), SocketAddr::from((Ipv6Addr::LOCALHOST, 443))] {
		write_job(&path, "dev.testd.fixture", address, &channel, &log).unwrap();
		let job = plist::Value::from_file(&path).unwrap();
		let job = job.as_dictionary().unwrap();
		let socket = job["Sockets"].as_dictionary().unwrap()[SOCKET_NAME].as_dictionary().unwrap();
		assert_eq!(socket["SockNodeName"].as_string().unwrap(), address.ip().to_string());
		assert_eq!(socket["SockServiceName"].as_string(), Some("443"));
		assert_eq!(job["ProgramArguments"].as_array().unwrap()[2].as_string(), channel.to_str());
		assert_eq!(job["RunAtLoad"].as_boolean(), Some(true));
		assert!(!job.contains_key("KeepAlive"));
		assert!(!job.contains_key("UserName"));
	}
}

#[test]
fn transferred_listener_survives_its_sender_and_is_not_inherited() {
	let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
	let address = listener.local_addr().unwrap();
	let (sender, receiver) = UnixStream::pair().unwrap();
	let descriptor = OwnedFd::from(listener);
	transfer::send(&sender, &descriptor).unwrap();
	drop(descriptor);
	drop(sender);
	let descriptor = transfer::receive(&receiver).unwrap();
	/* SAFETY: F_GETFD only inspects flags of the live, owned descriptor. */
	assert_ne!(unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC, 0);
	let received = TcpListener::from(descriptor);
	assert_eq!(received.local_addr().unwrap(), address);
	let _client = std::net::TcpStream::connect(address).unwrap();
	assert_eq!(received.accept().unwrap().1.ip(), Ipv4Addr::LOCALHOST);
}

#[test]
fn a_message_without_a_descriptor_is_refused() {
	let (mut sender, receiver) = UnixStream::pair().unwrap();
	sender.write_all(&[1]).unwrap();
	drop(sender);
	assert!(transfer::receive(&receiver).is_err());
}
