#!/usr/bin/env python3
"""Portable test runner. Defaults to unprivileged checks; system and interactive checks are opt-in."""

import argparse
import codecs
import contextlib
import ctypes
import datetime
import functools
import hashlib
import http.client
import importlib.util
import ipaddress
import json
import math
import os
import pathlib
import platform
import re
import shutil
import signal
import socket
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import tomllib
import traceback
import types
import uuid
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
SYSTEM = platform.system()
# Every tool the suites drive writes UTF-8, and the host scripts switch PowerShell
# to it. The locale encoding is the ANSI code page on non-English Windows, so
# decoding with it corrupts cargo trees and any non-ASCII path or message.
ENCODING = 'utf-8'
# Throwaway TLD for the host suite, so it never collides with a real .test setup.
HOST_TLD = os.environ.get('TESTD_HOST_TLD', 'testdcheck')
# The desktop setup wizard always configures the product TLD.
DESKTOP_TLD = 'test'
COORDINATED_SUITES = ('check', 'ci', 'all', 'contracts', 'audit', 'site')

def require(tool):
	path = shutil.which(tool)
	if not path:
		raise RuntimeError(f'please install {tool} before running this suite')
	return path


def console_code_page():
	"""The console output code page, or zero when no console is attached."""
	kernel32 = ctypes.WinDLL('kernel32', use_last_error=True)
	return kernel32.GetConsoleOutputCP()


def console_encoding():
	"""The encoding the shell capturing our redirected output will decode with.

	PowerShell 7 decodes a child's output with [Console]::OutputEncoding, which
	follows the console code page (cp936 on a Chinese system), but this runner
	writes redirected streams as UTF-8. A captured run would then show the
	report and every localized error as mojibake even though the bytes are
	right. Writing the console code page instead keeps the mirror readable; a
	UTF-8 console (65001) needs nothing, and the console itself is written
	through the Windows console API whatever we choose here.
	"""
	if SYSTEM != 'Windows':
		return None
	code_page = console_code_page()
	if not code_page or code_page == 65001:
		return None
	return f'cp{code_page}'


def match_redirected_encoding():
	"""Print redirected streams in the code page the console expects."""
	encoding = console_encoding()
	if not encoding:
		return
	for stream in (sys.stdout, sys.stderr):
		try:
			if stream is not None and not stream.isatty():
				stream.reconfigure(encoding=encoding, errors='replace')
		except (AttributeError, ValueError):
			continue


@functools.cache
def helper(name):
	"""Load a sibling module from tests/release, which is not an installed package."""
	spec = importlib.util.spec_from_file_location(name, ROOT / 'tests/release' / f'{name}.py')
	module = importlib.util.module_from_spec(spec)
	spec.loader.exec_module(module)
	return module


def file_digest(path):
	with path.open('rb') as source:
		return hashlib.file_digest(source, 'sha256').hexdigest()


def git_diff(root, scope):
	command = ['git', 'diff', '--binary', '--no-ext-diff', '--no-textconv', '--no-color']
	command.extend({'staged': ['--cached'], 'unstaged': []}[scope])
	return subprocess.run(command, cwd=root, capture_output=True, check=True, timeout=60).stdout


def capture_source_diff(run):
	"""Retain tracked changes without collecting untracked file contents."""
	before = {name: git_diff(ROOT, name) for name in ('staged', 'unstaged')}
	for name, data in before.items():
		path = run.output / f'git-diff-{name}.patch'
		with path.open('xb') as output:
			output.write(data)
			output.flush()
			os.fsync(output.fileno())
	after = {name: git_diff(ROOT, name) for name in ('staged', 'unstaged')}
	commit = subprocess.run(['git', 'rev-parse', 'HEAD'], cwd=ROOT, capture_output=True,
		encoding=ENCODING, errors='replace', check=True, timeout=60).stdout.strip()
	consistent = before == after and commit == run.metadata['commit']
	run.metadata['source_diff'] = {
		name: {'path': f'git-diff-{name}.patch', 'sha256': hashlib.sha256(data).hexdigest()}
		for name, data in before.items()
	}
	run.metadata['source_diff']['untracked_contents'] = 'not captured'
	run.metadata['source_diff']['consistent'] = consistent
	if not consistent:
		run.record('source-provenance', 'skip', 'blocked: tracked source changed while its diff was captured')


def safe_members(names):
	for name in names:
		path = pathlib.PurePosixPath(name.replace('\\', '/'))
		if path.is_absolute() or '..' in path.parts or ':' in name:
			raise ValueError(f'unsafe archive path: {name}')


class LoggedStream:
	"""Mirror terminal output to a shared, immediately flushed transcript."""

	def __init__(self, stream, transcript, lock):
		self.stream = stream
		self.transcript = transcript
		self.lock = lock

	def write(self, text):
		with self.lock:
			self.transcript.write(text)
			self.transcript.flush()
			return self.stream.write(text)

	def flush(self):
		with self.lock:
			self.transcript.flush()
			self.stream.flush()

	def __getattr__(self, name):
		return getattr(self.stream, name)


@contextlib.contextmanager
def run_transcript(run):
	"""Capture runner output without redirecting the operator's stdin."""
	path = run.output / 'run.log'
	with path.open('x', encoding=ENCODING, errors='backslashreplace', newline='') as transcript:
		original_stdout, original_stderr = sys.stdout, sys.stderr
		lock = threading.RLock()
		stdout = LoggedStream(original_stdout, transcript, lock)
		stderr = LoggedStream(original_stderr, transcript, lock)
		try:
			sys.stdout, sys.stderr = stdout, stderr
			print(f'Report: {run.output}', flush=True)
			run.metadata['transcript'] = str(path)
			run.save()
			yield
		finally:
			try:
				stdout.flush()
				stderr.flush()
			finally:
				sys.stdout, sys.stderr = original_stdout, original_stderr


class CommandOutput:
	"""Drain both child streams to the log; verbose mode also preserves their terminal destinations."""

	def __init__(self, process, log, *, verbose=False):
		self.process = process
		self.log = log
		self.verbose = verbose
		self.data = bytearray()
		self.errors = []
		self.lock = threading.Lock()
		self.threads = []
		for pipe, terminal in [(process.stdout, sys.stdout), (process.stderr, sys.stderr)]:
			thread = threading.Thread(target=self.copy, args=(pipe, terminal), daemon=True)
			thread.start()
			self.threads.append(thread)

	def copy(self, pipe, terminal):
		decoder = codecs.getincrementaldecoder(ENCODING)(errors='replace')
		try:
			while chunk := pipe.read1(64 * 1024):
				with self.lock:
					self.data.extend(chunk)
					self.log.write(chunk)
					self.log.flush()
				if self.verbose:
					terminal.write(decoder.decode(chunk))
					terminal.flush()
			if self.verbose:
				terminal.write(decoder.decode(b'', final=True))
				terminal.flush()
		except Exception as error:
			with self.lock:
				self.errors.append(error)
		finally:
			pipe.close()

	def wait(self, timeout):
		deadline = time.monotonic() + timeout
		self.process.wait(timeout=timeout)
		for thread in self.threads:
			thread.join(timeout=max(0, deadline - time.monotonic()))
			if thread.is_alive():
				raise subprocess.TimeoutExpired(self.process.args, timeout)
		if self.errors:
			raise RuntimeError('failed to mirror command output to the terminal or log') from self.errors[0]
		return self.text()

	def text(self):
		with self.lock:
			return self.data.decode(ENCODING, errors='replace')


def stop_command(process, signal_number):
	if SYSTEM == 'Windows':
		if process.poll() is None:
			subprocess.run(['taskkill', '/PID', str(process.pid), '/T', '/F'], check=True, capture_output=True)
		return
	try:
		# A child started in its own session leads its process group; one that kept
		# the operator's terminal does not, and must not signal the runner's group.
		if os.getpgid(process.pid) == process.pid:
			os.killpg(process.pid, signal_number)
		else:
			process.send_signal(signal_number)
	except ProcessLookupError:
		# The child can exit between the timeout and cleanup.
		return


class Run:
	def __init__(self, args, *, parent=None):
		self.args = args
		self.parent = parent
		base = pathlib.Path(os.environ.get('LOCALAPPDATA', pathlib.Path.home() / '.local/share'))
		stamp = datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ')
		self.output = (args.report or base / 'testd/release-checks' / f'{stamp}-{uuid.uuid4().hex[:8]}').resolve()
		self.output.mkdir(parents=True, exist_ok=False, mode=0o700)
		self.results = []
		self.log_count = 0
		self.lock = parent.lock if parent is not None else threading.RLock()
		self.metadata = {'os': platform.platform(), 'python': sys.version, 'suite': args.suite}
		if parent is not None:
			self.metadata.update({name: parent.metadata[name] for name in ('commit', 'dirty')})
		else:
			git = subprocess.run(['git', 'rev-parse', 'HEAD'], cwd=ROOT, capture_output=True,
				encoding=ENCODING, errors='replace', check=True)
			self.metadata['commit'] = git.stdout.strip()
			status = subprocess.run(['git', 'status', '--porcelain'], cwd=ROOT, capture_output=True,
				encoding=ENCODING, errors='replace', check=True)
			self.metadata['dirty'] = bool(status.stdout.strip())
		if getattr(args, 'include_git_diff', False):
			capture_source_diff(self)
		self.save()

	def save(self):
		if self.parent is not None:
			self.parent.save()
			return
		with self.lock:
			# Keep the last complete report readable if a write is interrupted.
			pending = self.output / 'report.next.json'
			pending.write_text(json.dumps({**self.metadata, 'checks': self.results}, indent=2) + '\n', encoding=ENCODING)
			pending.replace(self.output / 'report.json')

	def record(self, name, status, detail='', **evidence):
		with self.lock:
			if status == 'skip' and not detail.startswith('not applicable:'):
				self.metadata['incomplete'] = True
			self.results.append({'name': name, 'status': status, 'detail': detail, **evidence})
			self.save()
			print(f'{status:8} {name}: {detail}', flush=True)

	def log_path(self, name):
		with self.lock:
			path = self.output / f'{self.log_count:02d}-{re.sub(r"[^a-zA-Z0-9-]", "-", name)}.log'
			self.log_count += 1
			return path

	def command(self, name, command, timeout=300, env=None, expected=0, interrupt_ok=False, terminal=False):
		"""Run a command, retaining full output and optionally mirroring it.

		`terminal` keeps the operator's terminal attached, stdin and session
		alike. sudo keys its cached credential to that terminal, so a detached
		check asks for a password even right after `sudo -v` succeeded in the
		same shell. The child still leads its own process group, so a timeout can
		stop the processes it started.
		"""
		if command[0] in ('cargo', 'rustc') and len(command) > 1 and command[1].startswith('+'):
			# rustup run does not auto-install toolchains, unlike the +toolchain shim.
			command = [require('rustup'), 'run', command[1][1:], command[0], *command[2:]]
		log = self.log_path(name)
		started = time.monotonic()
		print(f'Running: {" ".join(map(str, command))}', flush=True)
		# Nested Python tools would otherwise write the ANSI code page on Windows.
		env = {**(os.environ if env is None else env), 'PYTHONUTF8': '1'}
		with log.open('xb') as log_file:
			process = subprocess.Popen(command, cwd=ROOT, env=env, stdout=subprocess.PIPE,
				stderr=subprocess.PIPE, stdin=None if terminal else subprocess.DEVNULL,
				start_new_session=SYSTEM != 'Windows' and not terminal,
				**({'process_group': 0} if terminal and SYSTEM != 'Windows' else {}))
			output = CommandOutput(process, log_file, verbose=getattr(self.args, 'verbose', False))
			try:
				text = output.wait(timeout)
			except (subprocess.TimeoutExpired, KeyboardInterrupt) as error:
				stop_command(process, signal.SIGTERM)
				try:
					text = output.wait(10)
				except subprocess.TimeoutExpired:
					stop_command(process, signal.SIGKILL if SYSTEM != 'Windows' else signal.SIGTERM)
					text = output.wait(10)
				if interrupt_ok and isinstance(error, KeyboardInterrupt):
					self.record(name, 'pass', f'fixture stopped by operator; {log}')
					return text
				self.show_failure(text, log)
				self.record(name, 'fail', f'timed out or interrupted; {log}',
					command=list(map(str, command)), log=str(log.relative_to(self.output)),
					seconds=round(time.monotonic() - started, 2))
				raise
		status = 'pass' if process.returncode == expected else 'fail'
		self.record(name, status, f'exit {process.returncode}; {log}', command=list(map(str, command)),
			returncode=process.returncode, seconds=round(time.monotonic() - started, 2),
			log=str(log.relative_to(self.output)))
		if status == 'fail':
			self.show_failure(text, log)
			raise RuntimeError(f'{name} failed; inspect {log}')
		return text

	def show_failure(self, text, log):
		if not getattr(self.args, 'verbose', False) and text:
			print(f'Last command output (full log: {log}):', file=sys.stderr)
			print('\n'.join(text[-8000:].splitlines()[-20:]), file=sys.stderr, flush=True)

	@contextlib.contextmanager
	def serve(self, name, command, env=None, stop_signal=signal.SIGTERM):
		"""Run a long-lived fixture and stop it when the block exits."""
		log = self.log_path(name)
		print(f'Running: {" ".join(map(str, command))}', flush=True)
		env = {**(os.environ if env is None else env), 'PYTHONUTF8': '1'}
		with log.open('xb') as log_file:
			process = subprocess.Popen(command, cwd=ROOT, env=env, stdout=subprocess.PIPE,
				stderr=subprocess.PIPE, stdin=subprocess.DEVNULL, start_new_session=SYSTEM != 'Windows')
			output = CommandOutput(process, log_file, verbose=getattr(self.args, 'verbose', False))
			try:
				yield process, output
			finally:
				was_running = process.poll() is None
				if was_running:
					stop_command(process, stop_signal)
				try:
					output.wait(10)
				except subprocess.TimeoutExpired:
					stop_command(process, signal.SIGKILL if SYSTEM != 'Windows' else signal.SIGTERM)
					output.wait(10)
				status = 'pass' if was_running or process.returncode == 0 else 'fail'
				self.record(name, status, f'exit {process.returncode}; {log}')


def candidate_build_environment(run):
	env = dict(os.environ)
	if env.get('CARGO_BUILD_TARGET'):
		raise RuntimeError('automatic candidate builds do not support CARGO_BUILD_TARGET; use --bin instead')
	identity = f'{ROOT.resolve()}\0{run.metadata.get("commit", "")}'.encode(ENCODING)
	fingerprint = hashlib.sha256(identity).hexdigest()[:16]
	metadata = f'metadata=testd-check-{fingerprint}'
	if 'CARGO_ENCODED_RUSTFLAGS' in env:
		flags = env['CARGO_ENCODED_RUSTFLAGS']
		env['CARGO_ENCODED_RUSTFLAGS'] = '\x1f'.join(filter(None, [flags, '-C', metadata]))
	else:
		env['RUSTFLAGS'] = ' '.join(filter(None, [env.get('RUSTFLAGS', ''), '-C', metadata]))
	run.metadata['build_fingerprint'] = fingerprint
	return env


def native_build_target(run, env):
	output = run.command('rustc-host', ['rustc', '+stable', '-vV'], 60, env=env)
	for line in output.splitlines():
		if line.startswith('host: '):
			target = line.removeprefix('host: ').strip()
			if target not in ('.', '..') and re.fullmatch(r'[A-Za-z0-9_.-]+', target):
				return target
	raise RuntimeError('could not determine the native Rust target from rustc')


def backup_build_candidate(run, path):
	if path.is_symlink():
		raise RuntimeError(f'existing build candidate is a symlink: {path}')
	if not path.exists():
		return
	if not path.is_file():
		raise RuntimeError(f'existing build candidate is not a regular file: {path}')
	backup = run.output / f'pre-build-{path.name}'
	index = 2
	while backup.exists() or backup.is_symlink():
		backup = run.output / f'pre-build-{index}-{path.name}'
		index += 1
	before = file_digest(path)
	shutil.copy2(path, backup)
	if file_digest(backup) != before:
		raise RuntimeError(f'could not verify the pre-build candidate backup: {backup}')
	run.record('build-backup', 'pass', f'{backup}; sha256 {before}')


def binary(run):
	if run.args.bin:
		path = run.args.bin.resolve()
	else:
		base = pathlib.Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target'))
		if not base.is_absolute():
			base = ROOT / base
		base = base.resolve()
		env = candidate_build_environment(run)
		env['CARGO_TARGET_DIR'] = str(base)
		target = native_build_target(run, env)
		run.metadata['build_target'] = target
		path = base / target / 'release' / ('testd.exe' if SYSTEM == 'Windows' else 'testd')
		backup_build_candidate(run, path)
		run.command('build-cli', ['cargo', '+stable', 'build', '--release', '--bin', 'testd', '--locked',
			'--target', target], 1200, env=env)
		path = path.resolve()
	run.metadata.setdefault('binaries', {})[str(path)] = file_digest(path)
	run.save()
	return path


def system_snapshot(run, tld, interactive=False):
	"""Snapshot OS integration and refuse pre-existing state before system checks."""
	state = helper('system_state')
	if SYSTEM == 'Darwin':
		manager = run.command('macos-session', ['launchctl', 'managername'])
		if manager.strip() != 'Aqua':
			raise RuntimeError('macOS trust checks require a terminal in the logged-in desktop session; '
				'SSH cannot present the system trust removal authorization')
	if SYSTEM != 'Windows' and not interactive:
		require('sudo')
		run.command('sudo-ready', ['sudo', '-n', 'true'], terminal=True)
	backup = run.output / 'system-before'
	index = 2
	# Every snapshot stays on disk; a later run must not overwrite the state an
	# earlier, possibly failed, restore would need.
	while backup.exists():
		backup = run.output / f'system-before-{index}'
		index += 1
	backup.mkdir(mode=0o700)
	manifest = state.snapshot(state.Host(SYSTEM, tld, interactive), backup)
	occupied = state.occupied(backup)
	if occupied:
		detail = f'pre-existing integration: {", ".join(occupied)}; snapshot kept at {backup}'
		run.record('system-snapshot', 'fail', detail)
		raise RuntimeError(f'system checks require a clean disposable host; {detail}')
	run.record('system-snapshot', 'pass', str(manifest))
	state.prepare_directories(state.Host(SYSTEM, tld, interactive), backup)
	return backup


def system_restore(run, backup, tld, interactive=False):
	"""Remove proven fixtures and verify only the selected snapshot state."""
	state = helper('system_state')
	problems = state.restore(state.Host(SYSTEM, tld, interactive), backup)
	if problems:
		run.record('system-restore', 'fail', '; '.join(problems))
		raise RuntimeError('scoped system state was not verified; inspect conflicts or restore the full-system snapshot; '
			f'retry scoped recovery with --restore-report {run.output}')
	run.record('system-restore', 'pass', f'{backup}; scoped integration matches the snapshot')
	if SYSTEM in ('Linux', 'Darwin'):
		run.metadata['incomplete'] = True
		run.record('shared-trust-coverage', 'skip',
			'blocked: shared trust-store output and hooks need comparison with the full-system snapshot')


def require_owned_product_integration():
	if SYSTEM == 'Windows':
		raise RuntimeError(f'{SYSTEM} system integration checks are blocked until resolver and trust ownership is enforced')


def cleanup_owned_product_integration(run, cli, home, tld, interactive=False):
	"""Use the candidate's ownership checks, never name-based deletion."""
	config = home / '.config/testd'
	owner = config / 'system-ownership-id'
	if any(path.is_symlink() for path in (home, home / '.config', config, owner)):
		raise RuntimeError('fixture ownership path is a symlink; no system cleanup was attempted')
	if not owner.is_file():
		return
	env = isolated_environment(HOME=str(home), TESTD_HOST_TLD=tld, TESTD_TUNNEL=str(run.output))
	run.command('system-owned-cleanup', [str(cli), 'uninstall'], 300, env=env, terminal=interactive)


def host_fixture_home(directory):
	if directory.is_symlink() or not directory.is_dir():
		raise RuntimeError('the host fixture directory is not a regular directory; no system cleanup was attempted')
	candidates = [path for path in directory.iterdir() if path.name.startswith('testd-host-')]
	if not candidates:
		return None
	if len(candidates) != 1 or candidates[0].is_symlink() or not candidates[0].is_dir():
		raise RuntimeError('the host fixture directory is ambiguous; no system cleanup was attempted')
	return candidates[0]


def record_host_coverage(run, text, *, scope='host'):
	match = re.search(r'(?m)^passed (\d+), failed (\d+), skipped (\d+)\s*$', text)
	if not match:
		run.metadata['incomplete'] = True
		run.record('host-coverage', 'skip', 'blocked: the host summary is missing')
		return
	passed, failed, skipped = (int(value) for value in match.groups())
	if failed:
		run.record('host-summary', 'fail', f'{passed} passed, {failed} failed, {skipped} skipped')
		raise RuntimeError('the host summary reports failed checks')
	if not passed:
		run.metadata['incomplete'] = True
		run.record('host-coverage', 'skip', 'blocked: no host checks passed')
		return
	reasons = re.findall(r'(?m)^skip[ \t]+(.+?)\s*$', text)
	if len(reasons) != skipped:
		run.metadata['incomplete'] = True
		run.record('host-coverage', 'skip', 'blocked: host skip details do not match the summary')
		return
	expected = ('system changes disabled ', 'system checks blocked until ')
	separate = [reason for reason in reasons if not run.args.system and reason.startswith(expected)]
	uncovered = [reason for reason in reasons if reason not in separate]
	if separate:
		run.record('host-system-coverage', 'skip', 'not applicable: system checks are reported separately')
	if scope == 'ci' and not run.args.system:
		desktop = [reason for reason in uncovered if reason.startswith((
			'launch at login not configured yet (start the tray once): ',
			'launch-at-login entry not validated against the candidate ('))]
		uncovered = [reason for reason in uncovered if reason not in desktop]
		if desktop:
			run.record('host-desktop-coverage', 'skip', 'not applicable: automated checks exclude actual tray and login validation')
	if uncovered:
		run.metadata['incomplete'] = True
		run.record('host-coverage', 'skip', 'blocked: ' + '; '.join(uncovered))


def host(run, *, scope='host'):
	if scope == 'ci' and run.args.system:
		raise RuntimeError('ci host checks cannot change system integration')
	if run.args.system:
		require_owned_product_integration()
	cli = binary(run)
	cli_digest = file_digest(cli) if run.args.system else None
	tcp_ports = [7737, 7740, int(os.environ.get('TESTD_HOST_EDGE_PORT', '8443')),
		int(os.environ.get('TESTD_HOST_APP_PORT', '18080')), int(os.environ.get('TESTD_HOST_LAUNCH_PORT', '18081'))]
	udp_ports = [53 if SYSTEM == 'Windows' else 7753]
	# Refuse a busy fixture port before any system state is touched.
	check_ports(tcp_ports, udp_ports)
	state = run.output / 'host-state'
	state.mkdir(mode=0o700)
	port_options = {name: os.environ[name] for name in ('TESTD_HOST_EDGE_PORT', 'TESTD_HOST_APP_PORT',
		'TESTD_HOST_LAUNCH_PORT') if name in os.environ}
	env = isolated_environment(**port_options, TESTD_HOST_OUTPUT_DIR=str(state), TESTD_BIN=str(cli),
		TESTD_HOST_TEST_SYSTEM='1' if run.args.system else '0',
		TESTD_HOST_DISPOSABLE_CONFIRMED='1' if run.args.system else '0', TESTD_HOST_TLD=HOST_TLD)
	snapshot = system_snapshot(run, HOST_TLD) if run.args.system else None
	if snapshot:
		env['TESTD_HOST_SNAPSHOT'] = str(snapshot)
	if SYSTEM == 'Windows':
		env['TESTD_HOST_PYTHON'] = sys.executable
		command = ['powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', 'tests/host/windows.ps1']
	else:
		require('python3')
		command = ['bash', f'tests/host/{"macos" if SYSTEM == "Darwin" else "linux"}.sh']
	try:
		# With --system the script runs sudo itself, which needs the terminal the
		# credential was validated on. Plain runs keep fixtures off the operator's
		# stdin.
		text = run.command('host-checks', command, 600, env, terminal=run.args.system)
		record_host_coverage(run, text, scope=scope)
		if run.args.system:
			if 'ok    doctor --fix' not in text:
				raise RuntimeError('requested system checks did not run successfully')
			run.record('system-checks', 'pass', 'resolver and trust checks ran')
		if run.args.service:
			run.args.bin = cli
			service(run)
	finally:
		if snapshot:
			try:
				try:
					wait_for_fixture_ports(tcp_ports, udp_ports)
				except Exception as error:
					run.record('fixture-shutdown', 'fail', str(error))
					raise
				run.record('fixture-shutdown', 'pass', 'fixture ports are available')
				try:
					home = host_fixture_home(state)
					if home is not None:
						if file_digest(cli) != cli_digest:
							raise RuntimeError('the candidate CLI changed; no system cleanup was attempted')
						cleanup_owned_product_integration(run, cli, home, HOST_TLD, interactive=True)
				except Exception as error:
					if not any(item['name'] == 'system-owned-cleanup' and item['status'] == 'fail' for item in run.results):
						run.record('system-owned-cleanup', 'fail', str(error))
					raise
			finally:
				system_restore(run, snapshot, HOST_TLD, interactive=True)


def check_ports(tcp, udp=()):
	for kind, ports in [(socket.SOCK_STREAM, tcp), (socket.SOCK_DGRAM, udp)]:
		for port in ports:
			with socket.socket(socket.AF_INET, kind) as probe:
				if kind == socket.SOCK_STREAM and SYSTEM != 'Windows':
					# Closed connections in TIME_WAIT do not mean a live fixture remains.
					probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
				try:
					probe.bind(('127.0.0.1', port))
					if kind == socket.SOCK_STREAM:
						probe.listen(1)
				except OSError as error:
					raise RuntimeError(f'fixture port {port} is unavailable: {error}') from error


def wait_for_fixture_ports(tcp, udp, timeout=10):
	deadline = time.monotonic() + timeout
	while True:
		try:
			check_ports(tcp, udp)
			return
		except RuntimeError:
			if time.monotonic() >= deadline:
				raise
			time.sleep(0.25)


def extract_archive(path, destination):
	destination.mkdir(parents=True, mode=0o700)
	if path.suffix == '.zip':
		with zipfile.ZipFile(path) as archive:
			safe_members(archive.namelist())
			if sum(item.file_size for item in archive.infolist()) > 512 * 1024 * 1024:
				raise ValueError('archive is too large')
			archive.extractall(destination)
	else:
		with tarfile.open(path) as archive:
			members = archive.getmembers()
			safe_members(item.name for item in members)
			if any(not (item.isfile() or item.isdir()) for item in members):
				raise ValueError('CLI archives must contain only regular files and directories')
			if sum(item.size for item in members) > 512 * 1024 * 1024:
				raise ValueError('archive is too large')
			archive.extractall(destination, filter='data')


def verify_checksums(directory):
	listed = set()
	for line in (directory / 'SHA256SUMS').read_text(encoding=ENCODING).splitlines():
		expected, name = line.split('  ', 1)
		safe_members([name])
		path = directory / name
		if name in listed or not path.resolve().is_relative_to(directory.resolve()):
			raise ValueError(f'unsafe or duplicate checksum entry: {name}')
		listed.add(name)
		if not re.fullmatch('[a-f0-9]{64}', expected):
			raise ValueError(f'invalid digest: {name}')
		if path.exists() and file_digest(path) != expected:
			raise ValueError(f'checksum mismatch: {name}')
	actual = {file.relative_to(directory).as_posix() for file in directory.rglob('*')
		if file.is_file() and file.name != 'SHA256SUMS'}
	if not actual.issubset(listed):
		raise ValueError(f'unlisted downloads: {actual - listed}')


def artifacts(run):
	directory = run.args.artifacts.resolve()
	if (directory / 'SHA256SUMS').exists():
		verify_checksums(directory)
		run.record('checksums', 'pass', 'all downloaded files verified; other platform assets may be absent')
	elif run.args.require_checksums:
		raise RuntimeError('SHA256SUMS is required for downloaded releases')
	else:
		run.record('checksums', 'skip', 'local bundle smoke test; release checksums have not been generated yet')
	target = {'Windows': 'x86_64-pc-windows-msvc', 'Linux': 'x86_64-unknown-linux-gnu',
		'Darwin': 'aarch64-apple-darwin' if platform.machine() == 'arm64' else 'x86_64-apple-darwin'}[SYSTEM]
	archives = sorted(directory.glob(f'testd-*-{target}.*'))
	if not archives:
		raise RuntimeError(f'no CLI archive for {target} in {directory}')
	for index, archive in enumerate(archives):
		extracted = run.output / f'archive-{index}'
		extract_archive(archive, extracted)
		cli = next(extracted.rglob('testd.exe' if SYSTEM == 'Windows' else 'testd'))
		if not cli.is_file():
			raise RuntimeError('no regular CLI binary in archive')
		version = archive.name.removeprefix('testd-').split(f'-{target}', 1)[0]
		fresh_install(run, cli, index, version)
		run.args.bin = cli
		if run.args.host:
			host(run)
		elif run.args.service:
			service(run)
		if (run.args.system or run.args.service) and run.metadata.get('incomplete'):
			run.record('artifact-remaining', 'skip', 'blocked: earlier system or service cleanup is incomplete')
			return
	for image in sorted(directory.glob('*.AppImage')):
		run.command('appimage-runtime', [str(image), '--appimage-version'])
	for image in sorted(directory.glob('*.dmg')):
		if SYSTEM == 'Darwin':
			run.command('dmg-metadata', ['hdiutil', 'imageinfo', str(image)])
	for package in sorted(directory.glob('*.deb')):
		run.command('deb-metadata', [require('dpkg-deb'), '-I', str(package)])
		run.command('deb-contents', ['dpkg-deb', '-c', str(package)])
	run.record('uninstall', 'skip',
		'blocked: real uninstall and purge require separate disposable-host system validation')


def fresh_install(run, cli, index, expected_version):
	home = run.output / f'fresh-home-{index}'
	home.mkdir(mode=0o700)
	env = isolated_environment(HOME=str(home), USERPROFILE=str(home), APPDATA=str(home / 'AppData/Roaming'))
	run.metadata.setdefault('binaries', {})[str(cli)] = file_digest(cli)
	version = run.command('archive-version', [str(cli), '--version'], env=env).strip()
	if version != f'testd {expected_version}':
		raise RuntimeError(f'invalid version output: {version}')
	self_config = home / ('AppData/Roaming/testd' if SYSTEM == 'Windows' else '.config/testd')
	initial = run.args.previous_bin.resolve() if run.args.previous_bin else cli
	run.command('upgrade-base-init' if run.args.previous_bin else 'fresh-init',
		[str(initial), 'init', '--caddyfile', ''], env=env)
	pki = self_config / 'pki'
	root_before = {name: file_digest(pki / name) for name in ('root.crt', 'root.key')}
	before = tomllib.loads((self_config / 'config.toml').read_text(encoding=ENCODING))
	backup_fresh_state(run, self_config, index, 'upgrade')
	run.command('re-init', [str(cli), 'init', '--caddyfile', ''], env=env)
	after = tomllib.loads((self_config / 'config.toml').read_text(encoding=ENCODING))
	assert_preserved_config(before, after)
	assert_root_unchanged(pki, root_before, 'upgrade')
	if run.args.previous_bin:
		backup_fresh_state(run, self_config, index, 'rollback')
		run.command('rollback-init', [str(initial), 'init', '--caddyfile', ''], env=env)
		rolled_back = tomllib.loads((self_config / 'config.toml').read_text(encoding=ENCODING))
		try:
			assert_preserved_config(after, rolled_back)
		except RuntimeError as error:
			raise RuntimeError(f'rollback changed configuration: {error}') from error
		assert_root_unchanged(pki, root_before, 'rollback')
		run.record('rollback', 'pass', 'configuration and root material preserved')
	run.record('archive-init-and-reinit', 'pass', version)


def backup_fresh_state(run, directory, index, step):
	path = run.output / f'fresh-state-before-{step}-{index}.tar.gz'
	with tarfile.open(path, 'x:gz') as backup:
		backup.add(directory, arcname='testd')


def assert_root_unchanged(pki, expected, phase):
	for name, digest in expected.items():
		path = pki / name
		if not path.is_file() or file_digest(path) != digest:
			raise RuntimeError(f'{phase} changed {name}')


def assert_preserved_config(before, after, section=''):
	for key, value in before.items():
		path = f'{section}.{key}' if section else key
		if isinstance(value, dict) and isinstance(after.get(key), dict):
			assert_preserved_config(value, after[key], path)
		elif key not in after or after[key] != value:
			raise RuntimeError(f'configuration value changed during upgrade: {path}')


def service(run):
	helper('service').run_service(run, binary(run), SYSTEM)


class HostUnavailable(RuntimeError):
	"""The intake endpoint did not answer like a running testd host."""


def local_host_status(port, credential=None):
	"""Read the owned host's status with its local operator credential."""
	connection = http.client.HTTPConnection('127.0.0.1', port, timeout=5)
	try:
		headers = {'Authorization': f'Bearer {credential}'} if credential else {}
		connection.request('GET', '/api/status', headers=headers)
		response = connection.getresponse()
		body = response.read()
	except OSError as error:
		raise HostUnavailable(f'no testd host is listening at 127.0.0.1:{port}') from error
	finally:
		connection.close()
	if response.status != 200:
		raise RuntimeError(f'the service at 127.0.0.1:{port} returned {response.status} for /api/status; '
			'is a testd host running there?')
	try:
		return json.loads(body)
	except (UnicodeDecodeError, ValueError) as error:
		raise RuntimeError(f'the host at 127.0.0.1:{port} returned an invalid /api/status document') from error


def wait_for_host(port, process, output, timeout=30, environment=None):
	"""Wait for a host this suite started to accept connections."""
	deadline = time.monotonic() + timeout
	while time.monotonic() < deadline:
		if process.poll() is not None:
			detail = output.text().strip()
			raise RuntimeError(f'the host exited with {process.returncode} before listening on 127.0.0.1:{port}; '
				f'is it initialized with `testd init`?\n{detail[-4000:]}')
		try:
			return local_host_status(port, browser_owner_credential(environment or os.environ))
		except HostUnavailable:
			time.sleep(0.25)
	raise RuntimeError(f'the host did not listen on 127.0.0.1:{port} within {timeout} seconds')


def require_host_resolution(run, cli, environment=None):
	"""Fail before a browser opens if the OS resolver does not reach the host TLD."""
	log = run.log_path('host-doctor')
	print(f'Running: {cli} doctor --json', flush=True)
	env = isolated_environment(PYTHONUTF8='1') if environment is None else {**environment, 'PYTHONUTF8': '1'}
	try:
		result = subprocess.run([str(cli), 'doctor', '--json'], cwd=ROOT,
			env=env, capture_output=True, timeout=60)
	except subprocess.TimeoutExpired as error:
		raise RuntimeError(f'`testd doctor --json` timed out; inspect {log}') from error
	log.write_bytes(result.stdout + result.stderr)
	try:
		checks = {item['name']: item for item in json.loads(result.stdout)['checks']}
	except (KeyError, TypeError, ValueError) as error:
		raise RuntimeError(f'could not read `testd doctor --json`; inspect {log}') from error
	for name in ('resolver', 'resolution'):
		check = checks.get(name)
		if not check or not check.get('ok'):
			detail = check.get('detail', 'missing check') if check else 'missing check'
			fix = (check.get('fix') if check else None) or 'run `testd doctor --fix` on the host'
			raise RuntimeError(f'{name}: {detail}; {fix}')
	run.record('host-resolution', 'pass', 'the OS resolver resolves the host TLD')
	return checks


def isolated_environment(**values):
	"""Do not pass operator tokens, host overrides, or provider settings to fixtures."""
	env = {key: value for key, value in os.environ.items() if not key.upper().startswith('TESTD_')}
	env.update(values)
	return env


def browser_owner_credential(environment):
	base = pathlib.Path(environment.get('APPDATA', pathlib.Path.home() / 'AppData/Roaming')) if SYSTEM == 'Windows' \
		else pathlib.Path(environment.get('HOME', str(pathlib.Path.home()))) / '.config'
	control = base / 'testd/control.json'
	if control.is_symlink():
		raise RuntimeError('the browser host control file is linked')
	if not control.is_file():
		raise HostUnavailable('the owned host has not written its control descriptor yet')
	if SYSTEM != 'Windows' and control.stat().st_mode & 0o077:
		raise RuntimeError('the browser host control file is not owner-private')
	credential = json.loads(control.read_text(encoding=ENCODING)).get('dashboard_token')
	if not isinstance(credential, str) or not re.fullmatch(r'[A-Za-z0-9_-]{16,}', credential):
		raise RuntimeError('the browser host dashboard credential is missing or invalid')
	return credential


class HostFixture:
	"""A host the browser suite owns. An already-running host is left untouched."""

	def __init__(self, run, cli, port, environment=None):
		self.run = run
		self.cli = cli
		self.port = port
		self.environment = environment
		self.started = False
		self.starts = 0
		self.status = None
		self.process = None
		self.output = None
		self.context = None

	def __enter__(self):
		# Do not send even a status request to an unrelated listener.
		check_ports([self.port])
		self.started = True
		self.start()
		return self

	def start(self):
		name = 'host-fixture' if self.starts == 0 else f'host-fixture-restart-{self.starts}'
		self.starts += 1
		# The daemon's startup sweep must never invoke a real tunnel provider.
		env = isolated_environment() if self.environment is None else dict(self.environment)
		env['TESTD_TUNNEL'] = str(self.run.output)
		self.context = self.run.serve(name, [str(self.cli), 'host'], env=env, stop_signal=signal.SIGINT)
		self.process, self.output = self.context.__enter__()
		try:
			self.status = wait_for_host(self.port, self.process, self.output, environment=env)
		except BaseException:
			self.stop()
			raise

	def stop(self):
		context = self.context
		if context is None:
			return
		self.context = None
		self.process = self.output = None
		context.__exit__(None, None, None)

	def restart(self):
		if not self.started:
			raise RuntimeError('the suite did not start this host; restart it manually')
		self.stop()
		self.start()

	def __exit__(self, *exc_info):
		self.stop()


def write_restart_result(run, request_id, result):
	path = run.output / f'restart-result-{request_id}.json'
	temporary = path.with_name(f'{path.name}.tmp')
	temporary.write_text(json.dumps(result) + '\n', encoding=ENCODING)
	temporary.replace(path)


def watch_restart_requests(run, host, stop, errors):
	"""Answer restart requests from the browser page while the fixture runs."""
	handled = set()
	try:
		while not stop.wait(0.25):
			for request in sorted(run.output.glob('restart-request-*.json')):
				if stop.is_set():
					return
				if request.name in handled:
					continue
				handled.add(request.name)
				request_id = request.name.removeprefix('restart-request-').removesuffix('.json')
				try:
					host.restart()
					result = {'status': 'ok'}
				except Exception as error:
					result = {'status': 'failed', 'detail': str(error)}
				write_restart_result(run, request_id, result)
				request.unlink(missing_ok=True)
	except Exception as error:
		errors.append(error)
		raise


PLAYWRIGHT_CHECKS = frozenset({
	'trusted TLS and secure context',
	'automatic browser checks before restart',
	'dashboard and negative routing controls',
	'restart and renewed trusted TLS',
	'automatic browser checks after restart',
})


BROWSER_CHECKS = frozenset({
	'HTTPS and secure context',
	'WebSocket, including 60 seconds idle',
	'Streaming SSE and 100 MiB upload',
	'Cookie sharing and project isolation',
})


def browser_report_error(result, commit, edge_port):
	if not isinstance(result, dict) or result.get('manualConfirmed') is not True:
		return 'manual browser observations are incomplete'
	for field in ('browser', 'notes'):
		if not isinstance(result.get(field), str) or not result[field].strip():
			return f'missing browser evidence: {field}'
	fingerprint = result.get('fingerprint')
	if not isinstance(fingerprint, str) or not re.fullmatch(r'[0-9a-f]{64}', fingerprint.lower()):
		return 'missing or invalid leaf fingerprint'
	origin = 'https://app.releasecheck.test' + (f':{edge_port}' if edge_port != 443 else '')
	if result.get('commit') != commit or result.get('origin') != origin:
		return 'browser report does not match the candidate commit and origin'
	checks = result.get('checks')
	if not isinstance(checks, list) or len(checks) != len(BROWSER_CHECKS):
		return 'browser report is missing checks'
	if any(not isinstance(check, dict) or not isinstance(check.get('name'), str) for check in checks):
		return 'browser report has malformed checks'
	if {check['name'] for check in checks} != BROWSER_CHECKS:
		return 'browser report has duplicate, missing, or unknown checks'
	if any(check.get('status') != 'pass' for check in checks):
		return 'a browser check did not pass'
	return None


def browser_driver_environment(**values):
	env = isolated_environment(**values)
	blocked = {'HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'NO_PROXY', 'SSL_CERT_FILE', 'SSL_CERT_DIR',
		'CURL_CA_BUNDLE', 'REQUESTS_CA_BUNDLE', 'NODE_EXTRA_CA_CERTS', 'NODE_TLS_REJECT_UNAUTHORIZED',
		'NODE_OPTIONS', 'SSLKEYLOGFILE', 'RES_OPTIONS', 'HOSTALIASES', 'LOCALDOMAIN', 'DBUS_SESSION_BUS_ADDRESS',
		'WAYLAND_DISPLAY', 'DISPLAY', 'XAUTHORITY', 'SSH_AUTH_SOCK', 'GPG_AGENT_INFO', 'OPENSSL_CONF',
		'OPENSSL_MODULES', 'OPENSSL_ENGINES', 'LD_PRELOAD', 'LD_LIBRARY_PATH', 'DYLD_INSERT_LIBRARIES',
		'DYLD_LIBRARY_PATH', 'PYTHONPATH', 'PYTHONHOME', 'PYTHONUSERBASE'}
	return {name: value for name, value in env.items() if name.upper() not in blocked}


def playwright_available(run, engine):
	bun = require('bun')
	if SYSTEM == 'Linux' and engine in ('chromium', 'firefox') and not shutil.which('certutil'):
		run.record('playwright-prerequisite', 'skip', 'blocked: install NSS certutil for the isolated browser trust store')
		return False
	log = run.log_path('playwright-prerequisite')
	try:
		result = subprocess.run([bun, str(ROOT / 'tests/release/browser-playwright.ts'), engine, '--probe'],
			cwd=ROOT, env=browser_driver_environment(), capture_output=True, timeout=30)
	except subprocess.TimeoutExpired as error:
		with log.open('xb') as output:
			output.write((error.stdout or b'') + (error.stderr or b''))
		run.record('playwright-prerequisite', 'fail', f'timed out; {log}')
		raise RuntimeError('the Playwright prerequisite check timed out') from error
	with log.open('xb') as output:
		output.write(result.stdout + result.stderr)
	if result.returncode == 2:
		run.record('playwright-prerequisite', 'skip', f'blocked: install Playwright and {engine}; {log}')
		return False
	if result.returncode != 0:
		run.record('playwright-prerequisite', 'fail', f'exit {result.returncode}; {log}')
		raise RuntimeError('the Playwright prerequisite check failed')
	run.record('playwright-prerequisite', 'pass', f'{engine}; {log}')
	return True


def playwright_report_error(result, engine, edge_port, commit, candidate, root_digest):
	if not isinstance(result, dict) or result.get('status') != 'pass' or result.get('engine') != engine or \
			result.get('cleanupComplete') is not True:
		return 'the Playwright browser run or its cleanup did not pass'
	if result.get('origin') != f'https://app.releasecheck.test:{edge_port}' or \
			result.get('commit') != commit or result.get('candidate') != candidate or \
			result.get('rootSha256') != root_digest:
		return 'the Playwright browser report does not match the candidate, root, and origin'
	checks = result.get('checks')
	if not isinstance(checks, list) or len(checks) != len(PLAYWRIGHT_CHECKS) or \
			any(not isinstance(item, str) for item in checks) or set(checks) != PLAYWRIGHT_CHECKS:
		return 'the Playwright browser report has missing or duplicate checks'
	fingerprints = [result.get('firstFingerprint'), result.get('secondFingerprint')]
	if any(not isinstance(value, str) or not re.fullmatch(r'[0-9a-f]{64}', value) for value in fingerprints):
		return 'the Playwright browser report is missing leaf fingerprints'
	if fingerprints[0] == fingerprints[1]:
		return 'the Playwright browser report did not verify a renewed leaf'
	for field in ('firstCertificate', 'secondCertificate'):
		details = result.get(field)
		if not isinstance(details, dict) or details.get('subjectName') != 'app.releasecheck.test' or \
				details.get('issuer') != 'testd intermediate':
			return 'the Playwright browser report is missing the expected certificate chain details'
		valid_from, valid_to = details.get('validFrom'), details.get('validTo')
		if any(isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value)
				for value in (valid_from, valid_to)) or not valid_from <= time.time() < valid_to <= time.time() + 90000:
			return 'the Playwright browser report has an invalid leaf lifetime'
	if not isinstance(result.get('userAgent'), str) or not result['userAgent'].strip():
		return 'the Playwright browser report is missing its user agent'
	return None


def browser_system_tls_error(run, result):
	for stage, fingerprint in (('before', result.get('firstFingerprint')), ('after', result.get('secondFingerprint'))):
		report = run.output / f'browser-system-tls-{stage}.json'
		if report.is_symlink() or not report.is_file():
			return f'the OS TLS {stage} report is missing'
		try:
			evidence = json.loads(report.read_text(encoding=ENCODING))
			addresses = evidence['addresses']
			if not isinstance(addresses, list) or not addresses or any(
					not ipaddress.ip_address(address.split('%', 1)[0]).is_loopback for address in addresses):
				return f'the OS TLS {stage} report contains a non-loopback address'
		except (OSError, UnicodeError, json.JSONDecodeError, KeyError, TypeError, ValueError, AttributeError):
			return f'the OS TLS {stage} report is malformed'
		if evidence.get('host') != 'app.releasecheck.test' or evidence.get('port') != run.args.edge_port or \
				evidence.get('leafSha256') != fingerprint or not evidence.get('openssl'):
			return f'the OS TLS {stage} report does not match the browser leaf'
	return None


def browser_ports(run):
	if not 1 <= run.args.port <= 65534 or not 1 <= run.args.edge_port <= 65535 or \
			not 1 <= run.args.intake_port <= 65535:
		raise RuntimeError('browser fixture ports are outside the permitted range')
	tcp = [run.args.port, run.args.port + 1, run.args.intake_port, run.args.edge_port, 7740]
	if len(set(tcp)) != len(tcp):
		raise RuntimeError('browser fixture ports must be distinct')
	return tcp, [53 if SYSTEM == 'Windows' else 7753]


def initialize_browser_home(run, cli, home):
	env = isolated_environment(HOME=str(home), TESTD_HOST_TLD='test', TESTD_TUNNEL=str(run.output))
	run.command('browser-init', [str(cli), 'init', '--caddyfile', ''], 120, env=env)
	config = home / '.config/testd/config.toml'
	if config.is_symlink() or not config.is_file():
		raise RuntimeError('the browser fixture config is missing or linked')
	before = config.lstat()
	content = config.read_text(encoding=ENCODING)
	digest = hashlib.sha256(content.encode(ENCODING)).hexdigest()
	host = tomllib.loads(content).get('host', {})
	if not isinstance(host, dict) or any(key in host for key in ('listen_addr', 'edge_addr', 'http_addr')):
		raise RuntimeError('the browser fixture config already sets listener addresses')
	marker = '[host]\n'
	if content.count(marker) != 1:
		raise RuntimeError('the browser fixture config has an unexpected host section')
	backup = run.output / 'browser-config-before.toml'
	with backup.open('xb') as output:
		output.write(content.encode(ENCODING))
		output.flush()
		os.fsync(output.fileno())
	updated = content.replace(marker, marker + f'listen_addr = "127.0.0.1:{run.args.intake_port}"\n'
		f'edge_addr = "127.0.0.1:{run.args.edge_port}"\nhttp_addr = "127.0.0.1:7740"\n')
	temporary = config.with_name('config.toml.browser-fixture')
	flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW
	descriptor = os.open(temporary, flags, 0o600)
	with os.fdopen(descriptor, 'wb') as output:
		output.write(updated.encode(ENCODING))
		output.flush()
		os.fsync(output.fileno())
	after = config.lstat()
	if config.is_symlink() or (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns) != \
			(after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns) or file_digest(config) != digest:
		raise RuntimeError('the browser fixture config changed; leaving it untouched')
	os.replace(temporary, config)
	return env


def cleanup_browser_system(run, cli, home, snapshot, tcp_ports, udp_ports, cli_digest):
	try:
		if run.metadata.get('browser_watcher_active'):
			raise RuntimeError('the browser restart watcher is still running')
		if run.metadata.get('browser_cleanup_unverified'):
			raise RuntimeError('browser process cleanup is unverified; restore the full-system snapshot')
		wait_for_fixture_ports(tcp_ports, udp_ports)
	except Exception as error:
		run.record('fixture-shutdown', 'fail', str(error))
		run.record('system-restore', 'skip',
			'blocked: stop fixtures and restore the full-system snapshot; scoped recovery cannot remove candidate state')
		raise
	run.record('fixture-shutdown', 'pass', 'browser fixture ports are available')
	try:
		try:
			if file_digest(cli) != cli_digest:
				raise RuntimeError('the candidate CLI changed; no system cleanup was attempted')
			cleanup_owned_product_integration(run, cli, home, DESKTOP_TLD, interactive=True)
		except Exception as error:
			if not any(item['name'] == 'system-owned-cleanup' and item['status'] == 'fail' for item in run.results):
				run.record('system-owned-cleanup', 'fail', str(error))
			raise
	finally:
		system_restore(run, snapshot, DESKTOP_TLD, interactive=True)


def browser(run):
	require('bun')
	tcp_ports, udp_ports = browser_ports(run)
	engine = getattr(run.args, 'browser_engine', None)
	changes_system = getattr(run.args, 'system', False)
	if changes_system:
		require_owned_product_integration()
	if engine and not playwright_available(run, engine):
		return
	if engine or changes_system:
		if not sys.stdin.isatty() or ask('Confirm this disposable host blocks external network egress [y/N]: ') != 'y':
			run.record('browser-network', 'skip', 'blocked: isolated network egress was not confirmed')
			return
	check_ports(tcp_ports, udp_ports)
	cli = binary(run)
	if not changes_system:
		return browser_session(run, cli, engine)
	home = run.output / 'browser-home'
	home.mkdir(mode=0o700)
	cli_digest = file_digest(cli)
	snapshot = system_snapshot(run, DESKTOP_TLD, interactive=True)
	try:
		environment = initialize_browser_home(run, cli, home)
		browser_session(run, cli, engine, environment, home / '.config/testd')
	finally:
		cleanup_browser_system(run, cli, home, snapshot, tcp_ports, udp_ports, cli_digest)


def browser_session(run, cli, engine, environment=None, config=None):
	root = (desktop_config_dir() if config is None else config) / 'pki/root.crt' if engine else None
	if engine and (root.is_symlink() or not root.is_file()):
		raise RuntimeError(f'the candidate public root certificate is missing or linked: {root}')
	root_digest = file_digest(root) if engine else None
	with HostFixture(run, cli, run.args.intake_port, environment) as host:
		if host.status.get('tld') != 'test':
			raise RuntimeError(f'the host at 127.0.0.1:{run.args.intake_port} serves .{host.status.get("tld")}, '
				'but testd serves .test; run `testd init` and restart the host')
		if environment is not None:
			run.command('browser-system-fix', [str(cli), 'doctor', '--fix', '--json'], 300,
				env=environment, terminal=True)
		checks = require_host_resolution(run, cli, environment)
		if engine and not checks.get('certificate', {}).get('ok'):
			raise RuntimeError('the operating system does not report the candidate root as trusted')
		run.metadata['browser_host_managed'] = host.started
		run.save()
		browser_fixture(run, cli, host, engine, root, environment)
	if engine:
		report = run.output / f'playwright-{engine}.json'
		if not report.is_file() or report.is_symlink():
			raise RuntimeError('the Playwright browser report is missing')
		result = json.loads(report.read_text(encoding=ENCODING))
		if file_digest(root) != root_digest:
			raise RuntimeError('the candidate root certificate changed during the browser run')
		error = playwright_report_error(result, engine, run.args.edge_port, run.metadata['commit'],
			run.metadata['binaries'][str(cli)], root_digest)
		if not error:
			error = browser_system_tls_error(run, result)
		run.record('browser-results', 'fail' if error else 'pass', f'{report}; {error}' if error else str(report))
		return
	reports = list(run.output.glob('browser-*.json'))
	if not reports:
		run.record('browser-results', 'skip', 'blocked: no browser report was saved')
	for report in reports:
		result = json.loads(report.read_text(encoding=ENCODING))
		error = browser_report_error(result, run.metadata['commit'], run.args.edge_port)
		status = 'fail' if error else 'pass'
		run.record('browser-results', status, f'{report}; {error}' if error else str(report))


def browser_cleanup_confirmed(run, engine):
	report = run.output / f'playwright-{engine}.json'
	if report.is_symlink() or not report.is_file():
		return False
	try:
		result = json.loads(report.read_text(encoding=ENCODING))
	except (OSError, UnicodeError, json.JSONDecodeError):
		return False
	return isinstance(result, dict) and result.get('cleanupComplete') is True


def browser_system_tls(run, stage, environment):
	run.command(f'browser-system-tls-{stage}', [sys.executable, 'tests/release/browser_system_tls.py',
		'--port', str(run.args.edge_port), '--output', str(run.output / f'browser-system-tls-{stage}.json')],
		75, environment)


def browser_fixture(run, cli, host, engine, root, environment):
	url = f'https://app.releasecheck.test:{run.args.edge_port}/'
	if not engine:
		print(f'Open {url} in each browser. Do not bypass certificate warnings.\n'
			'Run the checks and save the JSON report using the page. Ctrl+C stops only the fixtures.', flush=True)
	options = {'TESTD_BROWSER_REPORT': str(run.output), 'TESTD_BROWSER_PORT': str(run.args.port),
		'TESTD_BROWSER_TLD': 'test', 'TESTD_BROWSER_EDGE_PORT': str(run.args.edge_port)}
	fixture_env = isolated_environment(**options) if environment is None else {**environment, **options}
	driver_env = browser_driver_environment(**options)
	if engine:
		driver_env['TESTD_BROWSER_ROOT'] = str(root)
		driver_env['TESTD_BROWSER_COMMIT'] = run.metadata['commit']
		driver_env['TESTD_BROWSER_CANDIDATE'] = run.metadata['binaries'][str(cli)]
	command = [str(cli), 'run', '--host', f'127.0.0.1:{run.args.intake_port}',
		f'app.releasecheck:{run.args.port}', f'api.releasecheck:{run.args.port}',
		f'app.othercheck:{run.args.port}', f'dead.releasecheck:{run.args.port + 1}',
		'--', 'bun', 'tests/release/browser-server.ts']
	stop = threading.Event()
	errors = []
	watcher = threading.Thread(target=watch_restart_requests, args=(run, host, stop, errors), daemon=True)
	run.metadata['browser_watcher_active'] = True
	if not engine and environment is not None:
		run.metadata['browser_cleanup_unverified'] = True
	watcher.start()
	driver_started = False
	try:
		if engine:
			with run.serve('browser-fixture', command, fixture_env, stop_signal=signal.SIGINT):
				browser_system_tls(run, 'before', driver_env)
				driver_started = True
				run.metadata['browser_cleanup_unverified'] = True
				run.command('playwright-browser', ['bun', 'tests/release/browser-playwright.ts', engine], 600, driver_env)
				browser_system_tls(run, 'after', driver_env)
		else:
			run.command('browser-fixture', command, 3600, fixture_env, interrupt_ok=True)
	except KeyboardInterrupt:
		if engine:
			raise
	finally:
		stop.set()
		watcher.join(60)
		if driver_started and browser_cleanup_confirmed(run, engine):
			run.metadata.pop('browser_cleanup_unverified')
		if not engine and environment is not None and \
				ask('Confirm all manual fixture browsers are stopped [y/N]: ') == 'y':
			run.metadata.pop('browser_cleanup_unverified')
		if watcher.is_alive():
			raise RuntimeError('the host restart watcher did not stop')
		run.metadata.pop('browser_watcher_active')
		if errors:
			raise RuntimeError('the host restart watcher failed') from errors[0]


def desktop_config_dir():
	if SYSTEM == 'Windows':
		appdata = os.environ.get('APPDATA')
		if appdata:
			return pathlib.Path(appdata) / 'testd'
		profile = os.environ.get('USERPROFILE')
		if not profile:
			raise RuntimeError('USERPROFILE or APPDATA is required for the desktop suite')
		return pathlib.Path(profile) / 'AppData/Roaming/testd'
	home = os.environ.get('HOME')
	if not home:
		raise RuntimeError('HOME is required for the desktop suite')
	return pathlib.Path(home) / '.config/testd'


def require_fresh_desktop_user():
	config = desktop_config_dir()
	try:
		config.lstat()
	except FileNotFoundError:
		return
	raise RuntimeError(f'existing testd user data at {config}; use a fresh disposable user and retain a backup')


def desktop_command(app):
	if SYSTEM != 'Darwin' or app.suffix != '.app':
		return [str(app)]
	import plistlib
	info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
	name = info['CFBundleExecutable']
	if not isinstance(name, str) or name in ('', '.', '..') or pathlib.Path(name).name != name:
		raise RuntimeError('the app bundle has an invalid executable name')
	executable = app / 'Contents/MacOS' / name
	if not executable.is_file() or not os.access(executable, os.X_OK):
		raise RuntimeError(f'the app bundle executable is unavailable: {executable}')
	# Direct launch preserves the isolated environment; Launch Services can reuse a different instance.
	return [str(executable)]


def desktop(run):
	require_owned_product_integration()
	if not run.args.app:
		raise RuntimeError('provide --app with the candidate .app, .AppImage, or NSIS installer')
	if SYSTEM in ('Linux', 'Darwin') and not getattr(run.args, 'bin', None):
		raise RuntimeError('provide --bin with the candidate CLI for ownership-checked system cleanup')
	if SYSTEM in ('Linux', 'Darwin'):
		cli = pathlib.Path(run.args.bin).expanduser().resolve()
		if not cli.is_file() or not os.access(cli, os.X_OK):
			raise RuntimeError(f'the candidate CLI is missing or not executable: {cli}')
		run.args.bin = cli
		cli_digest = file_digest(cli)
		run.metadata.setdefault('binaries', {})[str(cli)] = cli_digest
	if not sys.stdin.isatty():
		raise RuntimeError('the desktop suite needs an interactive terminal; run it directly, '
			'not through a pipe or with stdin redirected')
	app = run.args.app.resolve()
	if not app.exists():
		raise RuntimeError(f'candidate app does not exist: {app}')
	require_fresh_desktop_user()
	items = checklist_items((ROOT / 'tests/host/gui-checklist.md').read_text(encoding=ENCODING))
	if not items:
		raise RuntimeError('no desktop checklist items were found')
	# macOS restricts loopback port 443; the actual setup must report its bind outcome.
	tcp_ports = [7737, 7740] if SYSTEM == 'Darwin' else [7737, 7740, 443]
	if SYSTEM in ('Linux', 'Darwin'):
		check_ports(tcp_ports, [7753])
	if ask('Confirm this is a disposable host with a persisted full-system backup [y/N]: ') != 'y':
		raise RuntimeError('desktop validation requires a disposable host and persisted full-system backup')
	snapshot = system_snapshot(run, DESKTOP_TLD, interactive=True)
	print('This suite requires a real desktop session and actual interaction. '
		'Cleanup removes only proven owned integration after candidate shutdown.', flush=True)
	command = desktop_command(app)
	app_log = run.log_path('desktop-app')
	app_process = None
	try:
		# The app must not inherit the checklist's stdin. Its daemon logs belong
		# in the report, not on top of the next prompt.
		with app_log.open('xb') as sink:
			env = isolated_environment(TESTD_TUNNEL=str(run.output))
			app_process = subprocess.Popen(
				command, stdin=subprocess.DEVNULL, stdout=sink, stderr=subprocess.STDOUT, env=env)
		print(f'Candidate app output: {app_log}', flush=True)
		if SYSTEM in ('Linux', 'Darwin'):
			time.sleep(1.5)
			if app_process.poll() is not None:
				raise RuntimeError(
					f'the candidate app exited immediately; see {app_log}. '
					'Check display/port privileges before rerunning the checklist')
			watch = threading.Thread(target=note_app_exit, args=(app_process, app_log), daemon=True)
			watch.start()
		for item in items:
			print(f'\n{item}')
			answer = ask('[p]ass / [f]ail / [n]ot applicable / [q]uit: ')
			while answer not in ['p', 'f', 'n', 'q']:
				answer = ask('Enter p, f, n, or q: ')
			if answer == 'q':
				run.metadata['incomplete'] = True
				run.record('desktop-checklist', 'skip', 'interrupted; unanswered checks are not passed')
				return
			detail = ask('Evidence or reason (required for fail/not applicable): ')
			if answer in ['f', 'n'] and not detail:
				raise RuntimeError('a reason is required')
			if answer == 'n':
				detail = 'not applicable: ' + detail
			run.record(item, {'p': 'pass', 'f': 'fail', 'n': 'skip'}[answer], detail)
	finally:
		if app_process is not None and SYSTEM in ('Linux', 'Darwin') and app_process.poll() is None:
			app_process.terminate()
			try:
				app_process.wait(timeout=5)
			except subprocess.TimeoutExpired:
				app_process.kill()
				app_process.wait(timeout=5)
		try:
			stopped = ask('Confirm all candidate apps, daemons, and providers are stopped before restore [y/N]: ') == 'y'
		except (KeyboardInterrupt, RuntimeError):
			run.metadata['incomplete'] = True
			run.record('system-restore', 'skip', f'shutdown not confirmed; stop processes and use --restore-report {run.output}')
			raise
		if stopped:
			try:
				try:
					if SYSTEM in ('Linux', 'Darwin'):
						wait_for_fixture_ports(tcp_ports, [7753])
				except Exception as error:
					run.record('fixture-shutdown', 'fail', str(error))
					raise
				run.record('fixture-shutdown', 'pass', 'operator confirmed shutdown and fixture ports are available')
				try:
					if SYSTEM in ('Linux', 'Darwin'):
						if file_digest(cli) != cli_digest:
							raise RuntimeError('the candidate CLI changed; no system cleanup was attempted')
						cleanup_owned_product_integration(run, cli, pathlib.Path.home(), DESKTOP_TLD, interactive=True)
				except Exception as error:
					if not any(item['name'] == 'system-owned-cleanup' and item['status'] == 'fail' for item in run.results):
						run.record('system-owned-cleanup', 'fail', str(error))
					raise
			finally:
				system_restore(run, snapshot, DESKTOP_TLD, interactive=True)
		else:
			run.metadata['incomplete'] = True
			run.record('system-restore', 'skip', f'shutdown not confirmed; stop processes and use --restore-report {run.output}')


def ask(prompt):
	print(prompt, flush=True)
	answer = sys.stdin.readline()
	if answer == '':
		raise RuntimeError('desktop checklist input closed; run in an interactive terminal')
	return answer.strip().lower()


def note_app_exit(process, log):
	try:
		code = process.wait()
	except Exception:
		return
	print(f'\n[desktop] candidate app exited with {code}; see {log}\n', flush=True)


def checklist_items(text):
	items = []
	for line in text.splitlines():
		if line.startswith('- [ ] '):
			items.append(line[6:])
		elif items and line.startswith('      '):
			items[-1] += ' ' + line.strip()
	return items


def restore_report(directory):
	"""Recover a host after a run that could not restore its own integration."""
	directory = directory.resolve()
	state = helper('system_state')
	snapshots = []
	for path in directory.glob('system-before*'):
		if (path / 'manifest.json').is_file():
			manifest = state.load(path)
			snapshots.append((manifest['created'], path, manifest))
	if not snapshots:
		raise SystemExit(f'no system snapshot in {directory}; system checks are only run by '
			'tests/run.py host --system')
	status = 0
	# Newest first; conflicting or unowned state is never replaced.
	for created, path, manifest in sorted(snapshots, key=lambda item: item[0], reverse=True):
		if manifest['system'] != SYSTEM:
			raise SystemExit(f'the snapshot at {path} belongs to {manifest["system"]}; run this on that host')
		print(f'Restoring integration recorded at {created} (tld {manifest["tld"]})', flush=True)
		try:
			problems = state.restore(state.Host(SYSTEM, manifest['tld']), path)
		except state.HostError as error:
			problems = [str(error)]
		for problem in problems:
			print(f'fail     {problem}', flush=True)
		if problems:
			status = 1
	if status:
		print('\nThe scoped state is not restored. Inspect conflicts or restore the full-system snapshot.', flush=True)
		return status
	print(f'Scoped state verified against every snapshot in {directory}', flush=True)
	if SYSTEM in ('Linux', 'Darwin'):
		print('Shared trust-store output and hooks still need comparison with the full-system snapshot.', flush=True)
		return 2
	return 0


def interrupt(signum, frame):
	raise KeyboardInterrupt(f'signal {signum}')


def require_disposable_host(parser, args):
	if args.suite in COORDINATED_SUITES:
		return
	changes_system = args.suite in ('artifacts', 'browser', 'desktop') or \
		args.suite == 'host' and args.system
	owns_service = args.suite == 'service' or args.suite == 'host' and args.service
	if not changes_system and not owns_service:
		return
	if args.report is None:
		parser.error('artifact, system, browser, desktop, and service checks require a new persisted --report')
	if helper('all_checks').is_ephemeral(args.report):
		parser.error('artifact, system, browser, desktop, and service checks require a report outside temporary directories')
	if args.report.exists() or args.report.is_symlink():
		parser.error(f'report path already exists: {args.report}')
	if not changes_system:
		return
	if not sys.stdin.isatty():
		parser.error('artifact, system, browser, and desktop checks require an interactive terminal')
	try:
		answer = input('Confirm a disposable host with a persisted full-system snapshot; type yes: ')
	except (EOFError, KeyboardInterrupt):
		parser.error('system checks were not confirmed')
	if answer.strip().lower() != 'yes':
		parser.error('system checks were not confirmed')


def all_suites(run):
	api = types.SimpleNamespace(Run=Run, SYSTEM=SYSTEM, HOST_TLD=HOST_TLD, DESKTOP_TLD=DESKTOP_TLD,
		binary=binary, file_digest=file_digest, require_toolchain=require_toolchain,
		host=host, service=service, artifacts=artifacts,
		desktop=desktop, browser=browser, system_snapshot=system_snapshot, system_restore=system_restore,
		check_ports=check_ports, checklist_items=checklist_items)
	helper('all_checks').execute(run, api)


def require_toolchain(toolchain):
	result = subprocess.run([require('rustup'), 'run', toolchain, 'rustc', '--version'], capture_output=True,
		encoding=ENCODING, errors='replace', timeout=30)
	if result.returncode:
		raise RuntimeError(f'{result.stderr.strip()}; prepare with: '
			f'rustup toolchain install {toolchain} --profile minimal')


def prepare(args):
	"""Report missing prerequisites without installing tools or touching product state."""
	missing = []
	tools = ['git', 'cargo', 'rustup', 'bun', 'cargo-deny', 'curl.exe' if SYSTEM == 'Windows' else 'curl']
	if SYSTEM != 'Windows':
		tools += ['bash', 'python3']
	for tool in tools:
		if not shutil.which(tool):
			missing.append(f'{tool} (check PATH; rustup and Bun normally use ~/.cargo/bin and ~/.bun/bin)')
	if not (shutil.which('pwsh') or shutil.which('powershell')):
		if SYSTEM == 'Windows':
			missing.append('PowerShell: required for Windows host and resolver-safety fixtures')
		else:
			print('Optional: PowerShell enables mocked Windows resolver-safety fixtures on Unix.')
	if SYSTEM == 'Windows':
		with tempfile.TemporaryDirectory(prefix='testd-symlink-check-') as directory:
			root = pathlib.Path(directory)
			try:
				(root / 'file-link').symlink_to(root / 'missing')
				(root / 'directory-link').symlink_to(root, target_is_directory=True)
			except OSError as error:
				if getattr(error, 'winerror', None) not in (50, 1314):
					raise
				missing.append(f'Windows symlink creation is unavailable: {error}. '
					'Use a symlink-capable filesystem and enable Windows Developer Mode '
					'or grant the test account SeCreateSymbolicLinkPrivilege.')
	if shutil.which('rustup'):
		for toolchain in ('stable', '1.90.0'):
			try:
				require_toolchain(toolchain)
			except (RuntimeError, subprocess.SubprocessError) as error:
				missing.append(str(error))
	for tool in ('tsc', 'vite', 'vitepress'):
		if not shutil.which(tool, path=str(ROOT / 'node_modules/.bin')):
			missing.append('website dependencies: bun install --frozen-lockfile')
			break
	if args.gui and SYSTEM == 'Linux':
		if not shutil.which('pkg-config'):
			missing.append('pkg-config and native GTK/AppIndicator packages (see .github/actions/linux-dependencies)')
		else:
			for library in ('gtk+-3.0', 'dbus-1'):
				if subprocess.run(['pkg-config', '--exists', library], timeout=30).returncode:
					missing.append(f'GUI development library: {library}')
			indicators = ('ayatana-appindicator3-0.1', 'appindicator3-0.1')
			if all(subprocess.run(['pkg-config', '--exists', library], timeout=30).returncode for library in indicators):
				missing.append('GUI development library: Ayatana AppIndicator or AppIndicator')
	for requirement in missing:
		print(f'Missing: {requirement}')
	print('Native GUI builds also require Xcode on macOS or Visual Studio C++ tools and the Windows SDK on Windows.'
		if args.gui else 'GUI checks are opt-in with --gui; ci always includes them.')
	print('This command only checks prerequisites; it does not install tools. '
		'See tests/README.md for setup commands.')
	return 2 if missing else 0


def validate_options(parser, args):
	scopes = {
		'gui': ('check', 'all', 'ci', 'contracts', 'prepare'),
		'bin': ('check', 'all', 'ci', 'host', 'browser', 'desktop', 'service'),
		'app': ('all', 'desktop'), 'artifacts': ('all', 'artifacts'),
		'previous_bin': ('all', 'artifacts'), 'no_previous_release': ('all', 'artifacts'),
		'interactive': ('all',), 'plan': COORDINATED_SUITES, 'system': ('all', 'host', 'browser', 'artifacts'),
		'host': ('artifacts',), 'service': ('host', 'artifacts'), 'require_checksums': ('all', 'artifacts'),
		'port': ('all', 'browser'), 'edge_port': ('all', 'browser'), 'intake_port': ('all', 'browser'),
		'browser_engine': ('all', 'browser'),
	}
	for name, suites in scopes.items():
		if getattr(args, name) and args.suite not in suites:
			parser.error(f'--{name.replace("_", "-")} is only available for {", ".join(suites)}')
	if args.suite == 'ci' and (args.app or args.artifacts or args.interactive or args.system):
		parser.error('ci excludes system changes, artifacts, desktop, and real-browser checks')
	if args.suite == 'all' and (args.artifacts or args.app or args.browser_engine) and not args.system:
		parser.error('all candidate execution and interactive checks require --system and a backed-up disposable host')
	if args.interactive and not args.system:
		parser.error('--interactive requires --system and a backed-up disposable host')
	if args.suite == 'all' and (args.app or args.browser_engine) and not args.interactive:
		parser.error('all --app and --browser-engine require --interactive')
	if args.suite == 'artifacts' and not args.artifacts:
		parser.error('artifacts requires --artifacts')
	if args.suite == 'desktop' and not args.app:
		parser.error('desktop requires --app')
	if args.suite in ('host', 'artifacts') and args.system and args.service:
		parser.error('--system and --service require separate runs and restored snapshots')
	if (args.previous_bin or args.no_previous_release or args.require_checksums) and not args.artifacts:
		parser.error('--previous-bin, --no-previous-release, and --require-checksums require --artifacts')
	if args.suite == 'all' and any(getattr(args, name) is not None for name in ('port', 'edge_port', 'intake_port')) \
			and not args.interactive:
		parser.error('all browser port options require --interactive')
	for field in ('bin', 'previous_bin'):
		path = getattr(args, field)
		if path:
			path = path.expanduser().resolve()
			if not args.plan and not path.is_file():
				parser.error(f'candidate is not a file: {path}')
			setattr(args, field, path)
	if not args.plan:
		if args.app and not args.app.expanduser().exists():
			parser.error(f'candidate app does not exist: {args.app}')
		if args.artifacts and not args.artifacts.expanduser().is_dir():
			parser.error(f'artifact directory does not exist: {args.artifacts}')
	if args.report:
		args.report = args.report.expanduser().absolute()
		if args.report.exists() or args.report.is_symlink():
			parser.error(f'report path already exists: {args.report}')
	for name, default, maximum in [('port', 19080, 65534), ('edge_port', 443, 65535), ('intake_port', 7737, 65535)]:
		value = getattr(args, name)
		if value is not None and not 1 <= value <= maximum:
			parser.error(f'--{name.replace("_", "-")} must be between 1 and {maximum}')
		setattr(args, name, default if value is None else value)
	if args.suite == 'ci':
		args.gui = True


def argument_parser():
	parser = argparse.ArgumentParser(description=__doc__)
	parser.add_argument('suite', nargs='?', default='check', choices=[*COORDINATED_SUITES, 'prepare',
		'host', 'artifacts', 'browser', 'desktop', 'service'])
	parser.add_argument('--report', type=pathlib.Path, help='new persisted report directory')
	parser.add_argument('--include-git-diff', action='store_true',
		help='save sensitive staged and unstaged tracked changes in the private report')
	parser.add_argument('--restore-report', type=pathlib.Path,
		help='restore the system integration recorded in an existing report directory, then exit')
	parser.add_argument('--bin', type=pathlib.Path, help='candidate CLI; otherwise build from source')
	parser.add_argument('--gui', action='store_true', help='include native GUI compilation and tests (always on in ci)')
	parser.add_argument('--verbose', action='store_true', help='mirror full command output; full logs are always retained')
	parser.add_argument('--system', action='store_true',
		help='test resolver/trust changes; integration is snapshotted and restored')
	parser.add_argument('--artifacts', type=pathlib.Path)
	parser.add_argument('--require-checksums', action='store_true')
	parser.add_argument('--host', action='store_true', help='run host suite against the extracted CLI')
	parser.add_argument('--service', action='store_true', help='also test launchd/systemd supervision')
	parser.add_argument('--app', type=pathlib.Path)
	previous = parser.add_mutually_exclusive_group()
	previous.add_argument('--previous-bin', type=pathlib.Path, help='previous CLI for artifact upgrade checks')
	previous.add_argument('--no-previous-release', action='store_true', help='record upgrade checks as not applicable')
	parser.add_argument('--interactive', action='store_true', help='include real browser, desktop, and manual checks')
	parser.add_argument('--plan', action='store_true', help='list the selected automated stages without running them')
	parser.add_argument('--port', type=int)
	parser.add_argument('--edge-port', type=int)
	parser.add_argument('--intake-port', type=int)
	parser.add_argument('--browser-engine', choices=['chromium', 'firefox', 'webkit'],
		help='run the real browser fixture with the selected Playwright engine')
	return parser


def main(argv=None):
	if sys.version_info < (3, 12):
		raise SystemExit('Please install Python 3.12 or newer for the test runner')
	# Start from UTF-8 so argparse and non-Windows captures never use the locale
	# encoding; match_redirected_encoding then narrows a Windows capture to the
	# code page its shell decodes with.
	for stream in (sys.stdout, sys.stderr):
		if hasattr(stream, 'reconfigure'):
			stream.reconfigure(encoding=ENCODING)
	parser = argument_parser()
	args = parser.parse_args(argv)
	match_redirected_encoding()
	if args.restore_report:
		if not sys.stdin.isatty():
			parser.error('restoring system state requires an interactive terminal')
		try:
			answer = input('Confirm the candidate is stopped and review the persisted full-system snapshot; type yes: ')
		except (EOFError, KeyboardInterrupt):
			parser.error('system recovery was not confirmed')
		if answer.strip().lower() != 'yes':
			parser.error('system recovery was not confirmed')
		return restore_report(args.restore_report)
	validate_options(parser, args)
	if args.suite == 'prepare':
		return prepare(args)
	if args.suite in COORDINATED_SUITES:
		all_checks = helper('all_checks')
		all_checks.validate(parser, args)
		if args.plan:
			all_checks.print_plan(args)
			return 0
	require_disposable_host(parser, args)
	# A supervisor restarting or an operator stopping the run must still restore.
	for number in ('SIGTERM', 'SIGHUP'):
		if hasattr(signal, number):
			signal.signal(getattr(signal, number), interrupt)
	run = Run(args)
	with run_transcript(run):
		try:
			if args.suite in COORDINATED_SUITES:
				all_suites(run)
			else:
				globals()[args.suite](run)
		except KeyboardInterrupt:
			run.metadata['incomplete'] = True
			run.record(args.suite, 'skip', 'interrupted; check system-restore and use --restore-report if needed')
			return 2
		except Exception as error:
			if args.verbose:
				traceback.print_exc()
			else:
				print(str(error), file=sys.stderr)
			run.record(args.suite, 'fail', str(error))
			return 1
		finally:
			run.save()
		if any(check['status'] == 'fail' for check in run.results):
			return 1
		return 2 if run.metadata.get('incomplete') else 0


if __name__ == '__main__':
	sys.exit(main())
