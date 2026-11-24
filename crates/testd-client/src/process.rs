//! Own the whole process tree, including descendants whose parent has exited.

use std::io;
use std::time::Duration;
use tokio::process::{Child, Command};

pub(crate) enum Console {
	Inherit,
	Hidden,
}

pub(crate) struct ProcessTree {
	#[cfg(unix)]
	pid: u32,
	#[cfg(windows)]
	job: std::os::windows::io::OwnedHandle,
}

impl ProcessTree {
	pub(crate) fn spawn(command: &mut Command, console: Console) -> io::Result<(Child, Self)> {
		command.kill_on_drop(true);
		#[cfg(not(windows))]
		let _ = console;
		#[cfg(unix)]
		command.process_group(0);
		#[cfg(windows)]
		{
			use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CREATE_SUSPENDED};
			let console_flags = match console {
				Console::Inherit => 0,
				Console::Hidden => CREATE_NO_WINDOW,
			};
			command.creation_flags(CREATE_SUSPENDED | CREATE_NEW_PROCESS_GROUP | console_flags);
		}
		let child = command.spawn()?;
		#[cfg(unix)]
		let tree = Self { pid: child.id().expect("newly spawned child has a PID") };
		#[cfg(windows)]
		let tree = Self { job: windows_job(&child)? };
		#[cfg(not(any(unix, windows)))]
		let tree = Self {};
		Ok((child, tree))
	}

	pub(crate) async fn stop(&mut self, child: &mut Child) -> io::Result<()> {
		#[cfg(unix)]
		{
			if self.pid != 0 {
				signal_group(self.pid, libc::SIGTERM)?;
				let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
				signal_group(self.pid, libc::SIGKILL)?;
				self.pid = 0;
			}
		}
		#[cfg(windows)]
		{
			use std::os::windows::io::AsRawHandle;
			/* The job still owns descendants after the direct child's handle is reaped. */
			if unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job.as_raw_handle(), 1) } == 0 {
				return Err(io::Error::last_os_error());
			}
		}
		#[cfg(not(any(unix, windows)))]
		child.start_kill()?;
		tokio::time::timeout(Duration::from_secs(5), child.wait())
			.await
			.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "process tree did not stop"))??;
		#[cfg(windows)]
		tokio::time::timeout(Duration::from_secs(5), async {
			while active_processes(&self.job)? != 0 {
				tokio::time::sleep(Duration::from_millis(10)).await;
			}
			Ok::<_, io::Error>(())
		})
		.await
		.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "job descendants did not stop"))??;
		Ok(())
	}
}

#[cfg(unix)]
fn signal_group(pid: u32, signal: i32) -> io::Result<()> {
	if unsafe { libc::kill(-(pid as i32), signal) } == 0 {
		return Ok(());
	}
	let error = io::Error::last_os_error();
	if error.raw_os_error() == Some(libc::ESRCH) { Ok(()) } else { Err(error) }
}

#[cfg(unix)]
impl Drop for ProcessTree {
	fn drop(&mut self) {
		if self.pid != 0
			&& let Err(error) = signal_group(self.pid, libc::SIGKILL)
		{
			eprintln!("[testd] [error] could not clean up process group {}: {error}", self.pid);
		}
	}
}

#[cfg(windows)]
fn windows_job(child: &Child) -> io::Result<std::os::windows::io::OwnedHandle> {
	use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
	use windows_sys::Win32::System::JobObjects::*;

	/* Spawn suspended: the child cannot escape ownership by creating descendants before assignment. */
	let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
	if handle.is_null() {
		return Err(io::Error::last_os_error());
	}
	let job = unsafe { OwnedHandle::from_raw_handle(handle) };
	let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
	limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
	let configured = unsafe {
		SetInformationJobObject(
			job.as_raw_handle(),
			JobObjectExtendedLimitInformation,
			(&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
			std::mem::size_of_val(&limits) as u32,
		)
	};
	if configured == 0 {
		return Err(io::Error::last_os_error());
	}
	let assigned = unsafe { AssignProcessToJobObject(job.as_raw_handle(), child.raw_handle().unwrap()) };
	if assigned == 0 {
		return Err(io::Error::last_os_error());
	}
	resume_child(child.id().unwrap())?;
	Ok(job)
}

#[cfg(windows)]
fn active_processes(job: &std::os::windows::io::OwnedHandle) -> io::Result<u32> {
	use std::os::windows::io::AsRawHandle;
	use windows_sys::Win32::System::JobObjects::*;
	let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
	let read = unsafe {
		QueryInformationJobObject(
			job.as_raw_handle(),
			JobObjectBasicAccountingInformation,
			(&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
			std::mem::size_of_val(&accounting) as u32,
			std::ptr::null_mut(),
		)
	};
	if read == 0 { Err(io::Error::last_os_error()) } else { Ok(accounting.ActiveProcesses) }
}

#[cfg(windows)]
fn resume_child(pid: u32) -> io::Result<()> {
	use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
	use windows_sys::Win32::Foundation::{ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE};
	use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
	use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

	let handle = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
	if handle == INVALID_HANDLE_VALUE {
		return Err(io::Error::last_os_error());
	}
	let snapshot = unsafe { OwnedHandle::from_raw_handle(handle) };
	let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
	entry.dwSize = std::mem::size_of_val(&entry) as u32;
	let mut found = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) };
	while found != 0 {
		if entry.th32OwnerProcessID == pid {
			let handle = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
			if handle.is_null() {
				return Err(io::Error::last_os_error());
			}
			let thread = unsafe { OwnedHandle::from_raw_handle(handle) };
			if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
				return Err(io::Error::last_os_error());
			}
			return Ok(());
		}
		found = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) };
	}
	let error = io::Error::last_os_error();
	if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
		Err(io::Error::new(io::ErrorKind::NotFound, "the suspended child's thread was not found"))
	} else {
		Err(error)
	}
}

#[cfg(all(test, any(target_os = "linux", windows)))]
mod tests {
	use super::*;
	use std::process::Stdio;
	use tokio::io::{AsyncBufReadExt, BufReader};

	#[cfg(target_os = "linux")]
	#[tokio::test]
	async fn normal_parent_exit_does_not_leave_descendants_running() {
		let mut command = Command::new("sh");
		command.args(["-c", "sleep 30 & echo $!; exit 7"]).stdout(Stdio::piped());
		let (mut child, mut tree) = ProcessTree::spawn(&mut command, Console::Inherit).unwrap();
		let mut output = BufReader::new(child.stdout.take().unwrap());
		let mut line = String::new();
		tokio::time::timeout(Duration::from_secs(3), output.read_line(&mut line))
			.await
			.unwrap()
			.unwrap();
		let pid: u32 = line.trim().parse().unwrap();
		assert_eq!(child.wait().await.unwrap().code(), Some(7));
		assert!(child.id().is_none(), "cleanup cannot rely on the reaped child's PID");
		tree.stop(&mut child).await.unwrap();
		tokio::time::timeout(Duration::from_secs(3), async {
			loop {
				match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
					Err(_) => break,
					Ok(stat) if stat.rsplit_once(')').unwrap().1.trim_start().starts_with('Z') => break,
					_ => tokio::time::sleep(Duration::from_millis(10)).await,
				}
			}
		})
		.await
		.expect("a descendant survived normal parent exit");
	}

	#[cfg(windows)]
	#[tokio::test]
	async fn hidden_children_keep_stdout_and_stderr_without_a_console() {
		let mut command = Command::new("powershell");
		command
			.args([
				"-NoProfile",
				"-NonInteractive",
				"-Command",
				concat!(
					"Add-Type -Namespace Testd -Name Console -MemberDefinition ",
					"'[System.Runtime.InteropServices.DllImport(\"kernel32.dll\")] ",
					"public static extern System.IntPtr GetConsoleWindow();'; ",
					"[Console]::Out.WriteLine([Testd.Console]::GetConsoleWindow().ToInt64()); ",
					"[Console]::Out.WriteLine('stdout'); [Console]::Error.WriteLine('stderr'); exit 7"
				),
			])
			.stdin(Stdio::null())
			.stdout(Stdio::piped())
			.stderr(Stdio::piped());
		let (child, _tree) = ProcessTree::spawn(&mut command, Console::Hidden).unwrap();
		let output = tokio::time::timeout(Duration::from_secs(30), child.wait_with_output())
			.await
			.expect("hidden child did not exit")
			.unwrap();
		assert_eq!(output.status.code(), Some(7));
		assert_eq!(String::from_utf8(output.stdout).unwrap().lines().collect::<Vec<_>>(), ["0", "stdout"]);
		assert_eq!(String::from_utf8(output.stderr).unwrap().trim(), "stderr");
	}

	#[cfg(windows)]
	#[tokio::test]
	async fn the_job_retains_descendants_after_parent_exit() {
		let mut command = Command::new("powershell");
		command
			.args([
				"-NoProfile",
				"-NonInteractive",
				"-Command",
				concat!(
					"$p = Start-Process powershell -NoNewWindow -ArgumentList ",
					"'-NoProfile -NonInteractive -Command Start-Sleep -Seconds 30' -PassThru; Write-Output $p.Id"
				),
			])
			.stdout(Stdio::piped());
		let (mut child, mut tree) = ProcessTree::spawn(&mut command, Console::Inherit).unwrap();
		let mut line = String::new();
		let mut output = BufReader::new(child.stdout.take().unwrap());
		tokio::time::timeout(Duration::from_secs(15), output.read_line(&mut line))
			.await
			.unwrap()
			.unwrap();
		assert!(line.trim().parse::<u32>().is_ok(), "the fixture must spawn a descendant");
		tokio::time::timeout(Duration::from_secs(15), child.wait())
			.await
			.unwrap()
			.unwrap();
		assert!(active_processes(&tree.job).unwrap() > 0);
		tree.stop(&mut child).await.unwrap();
		assert_eq!(active_processes(&tree.job).unwrap(), 0);
	}
}
