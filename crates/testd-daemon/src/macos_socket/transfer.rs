//! Transfer one launchd-owned file descriptor through a private Unix socket.

use std::io;
use std::mem::{size_of, zeroed};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::ptr;

unsafe extern "C" {
	fn launch_activate_socket(name: *const libc::c_char, descriptors: *mut *mut libc::c_int, count: *mut usize) -> i32;
}

pub(super) fn activate(name: &str) -> io::Result<OwnedFd> {
	let name = std::ffi::CString::new(name)?;
	let mut descriptors = ptr::null_mut();
	let mut count = 0;
	/* SAFETY: launchd fills the out parameters and returns malloc-owned storage on success. */
	let error = unsafe { launch_activate_socket(name.as_ptr(), &mut descriptors, &mut count) };
	if error != 0 {
		return Err(io::Error::from_raw_os_error(error));
	}
	/* SAFETY: a successful activation returns count live, caller-owned descriptors. */
	let mut sockets: Vec<_> = unsafe {
		let values = if count == 0 { &[] } else { std::slice::from_raw_parts(descriptors, count) };
		values.iter().map(|&fd| OwnedFd::from_raw_fd(fd)).collect()
	};
	/* SAFETY: launch_activate_socket documents freeing its returned array with free. */
	unsafe { libc::free(descriptors.cast()) };
	if sockets.len() != 1 {
		return Err(io::Error::other("the launchd job must provide exactly one HTTPS socket"));
	}
	Ok(sockets.pop().unwrap())
}

fn wait(channel: &UnixStream, events: libc::c_short) -> io::Result<()> {
	let deadline = std::time::Instant::now() + super::TIMEOUT;
	loop {
		let remaining = deadline.saturating_duration_since(std::time::Instant::now());
		let mut descriptor = libc::pollfd { fd: channel.as_raw_fd(), events, revents: 0 };
		/* SAFETY: poll receives one initialized descriptor and a bounded timeout. */
		let result = unsafe { libc::poll(&mut descriptor, 1, remaining.as_millis() as i32) };
		if result > 0 {
			return Ok(());
		}
		if result == 0 {
			return Err(io::Error::new(io::ErrorKind::TimedOut, "socket transfer timed out"));
		}
		let error = io::Error::last_os_error();
		if error.kind() != io::ErrorKind::Interrupted {
			return Err(error);
		}
	}
}

pub(super) fn send(channel: &UnixStream, descriptor: &OwnedFd) -> io::Result<()> {
	wait(channel, libc::POLLOUT)?;
	let mut payload = [1u8];
	let mut vector = libc::iovec { iov_base: payload.as_mut_ptr().cast(), iov_len: payload.len() };
	let mut control = [0usize; 8];
	/* SAFETY: zeroed msghdr has null optional fields; the supplied buffers live through sendmsg. */
	unsafe {
		let mut message: libc::msghdr = zeroed();
		message.msg_iov = &mut vector;
		message.msg_iovlen = 1;
		message.msg_control = control.as_mut_ptr().cast();
		message.msg_controllen = libc::CMSG_SPACE(size_of::<RawFd>() as _) as _;
		let header = libc::CMSG_FIRSTHDR(&message);
		(*header).cmsg_level = libc::SOL_SOCKET;
		(*header).cmsg_type = libc::SCM_RIGHTS;
		(*header).cmsg_len = libc::CMSG_LEN(size_of::<RawFd>() as _) as _;
		ptr::write_unaligned(libc::CMSG_DATA(header).cast::<RawFd>(), descriptor.as_raw_fd());
		if libc::sendmsg(channel.as_raw_fd(), &message, libc::MSG_DONTWAIT) != 1 {
			return Err(io::Error::last_os_error());
		}
	}
	Ok(())
}

pub(super) fn receive(channel: &UnixStream) -> io::Result<OwnedFd> {
	wait(channel, libc::POLLIN)?;
	let mut payload = [0u8];
	let mut vector = libc::iovec { iov_base: payload.as_mut_ptr().cast(), iov_len: payload.len() };
	let mut control = [0usize; 8];
	let mut descriptors = Vec::new();
	/* SAFETY: recvmsg writes into initialized, aligned buffers. CMSG helpers bound each header to that buffer. */
	unsafe {
		let mut message: libc::msghdr = zeroed();
		message.msg_iov = &mut vector;
		message.msg_iovlen = 1;
		message.msg_control = control.as_mut_ptr().cast();
		message.msg_controllen = size_of_val(&control) as _;
		let received = libc::recvmsg(channel.as_raw_fd(), &mut message, libc::MSG_DONTWAIT);
		if received < 0 {
			return Err(io::Error::last_os_error());
		}
		let mut header = libc::CMSG_FIRSTHDR(&message);
		while !header.is_null() {
			if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_RIGHTS {
				let length = (*header).cmsg_len as usize - libc::CMSG_LEN(0) as usize;
				for index in 0..length / size_of::<RawFd>() {
					let fd = ptr::read_unaligned(libc::CMSG_DATA(header).cast::<RawFd>().add(index));
					descriptors.push(OwnedFd::from_raw_fd(fd));
				}
			}
			header = libc::CMSG_NXTHDR(&message, header);
		}
		if received != 1 || payload != [1] || message.msg_flags & libc::MSG_CTRUNC != 0 || descriptors.len() != 1 {
			return Err(io::Error::other("invalid socket-transfer message"));
		}
		let descriptor = descriptors.pop().unwrap();
		if libc::fcntl(descriptor.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) == -1 {
			return Err(io::Error::last_os_error());
		}
		Ok(descriptor)
	}
}
