"""One coordinated, explicitly scoped run of the platform test suites."""

import copy
import ctypes
import json
import os
import pathlib
import platform
import re
import shutil
import subprocess
import sys
import tempfile
import time
import traceback

ROOT = pathlib.Path(__file__).resolve().parents[2]


class Blocked(RuntimeError):
	"""A check could not run, so it must not be reported as a pass."""


class NotApplicable(RuntimeError):
	"""The current platform does not ship the feature being tested."""


class Stage:
	def __init__(self, name, action, *, system=False, interactive=False, checkpoint='', precondition=None,
			stop_on_failure=True):
		self.name = name
		self.action = action
		self.system = system
		self.interactive = interactive
		self.checkpoint = checkpoint
		self.precondition = precondition
		self.stop_on_failure = stop_on_failure


def is_ephemeral(path):
	roots = [pathlib.Path(tempfile.gettempdir())]
	roots.extend(pathlib.Path(value) for name in ('TMPDIR', 'TMP', 'TEMP') if (value := os.environ.get(name)))
	if os.name != 'nt':
		roots.extend(pathlib.Path(value) for value in ('/tmp', '/run', '/dev/shm'))
	resolved = path.expanduser().resolve()
	return any(resolved.is_relative_to(root.resolve()) for root in roots)


def validate(parser, args):
	if platform.system() not in ('Linux', 'Darwin', 'Windows'):
		parser.error('test profiles support Linux, macOS, and Windows')
	if args.suite == 'ci' and (args.system or args.interactive or args.app or args.artifacts or
			args.previous_bin or args.no_previous_release or getattr(args, 'browser_engine', None) or
			getattr(args, 'service', False) or getattr(args, 'host', False)):
		parser.error('ci excludes system, service, artifact, desktop, and real-browser checks')
	for name, port, maximum in [('port', args.port, 65534), ('edge-port', args.edge_port, 65535),
			('intake-port', args.intake_port, 65535)]:
		if not 1 <= port <= maximum:
			parser.error(f'--{name} must be between 1 and {maximum}')
	if args.interactive and not args.system:
		parser.error('all --interactive requires --system and a disposable, backed-up host')
	if args.app and not args.interactive:
		parser.error('all --app requires --interactive')
	if getattr(args, 'browser_engine', None) and not args.interactive:
		parser.error('all --browser-engine requires --interactive')
	if args.previous_bin and not args.artifacts:
		parser.error('--previous-bin requires --artifacts in the all suite')
	if args.plan:
		return
	if args.system and (args.report is None or not sys.stdin.isatty()):
		parser.error('all --system requires an interactive terminal and a new persisted --report')
	base = args.report or pathlib.Path(os.environ.get('LOCALAPPDATA', pathlib.Path.home() / '.local/share'))
	if is_ephemeral(base):
		parser.error('test profiles need a persisted report outside temporary directories')
	if args.report:
		args.report = args.report.expanduser().absolute()
		if args.report.exists() or args.report.is_symlink():
			parser.error(f'report path already exists: {args.report}')
	for field, kind in [('bin', 'candidate CLI'), ('previous_bin', 'previous CLI')]:
		path = getattr(args, field)
		if path:
			path = path.expanduser().resolve()
			if not path.is_file():
				parser.error(f'{kind} is not a file: {path}')
			setattr(args, field, path)
	if args.app and not args.app.expanduser().exists():
		parser.error(f'candidate app does not exist: {args.app}')
	if args.artifacts and not args.artifacts.expanduser().is_dir():
		parser.error(f'artifact directory does not exist: {args.artifacts}')


def print_plan(args):
	checks = AllChecks(None, None, args)
	for index, stage in enumerate(checks.stages(), 1):
		kind = ' [disposable host]' if stage.system else ''
		kind += ' [interactive]' if stage.interactive else ''
		print(f'{index:02d}. {stage.name}{kind}')


def execute(run, api):
	checks = AllChecks(run, api, run.args)
	checks.execute()


class AllChecks:
	def __init__(self, run, api, args):
		self.run = run
		self.api = api
		self.args = args
		self.cli = None
		self.site_ready = False
		self.system_confirmed = False
		self.system_unsafe = False
		self.interrupted = False
		self.outcomes = []

	def stages(self):
		stages = self.automated_stages()
		focused = {'contracts': 'rust-tests', 'audit': 'dependency-audit', 'site': 'site'}
		if self.args.suite in focused:
			return [stage for stage in stages if stage.name == focused[self.args.suite]]
		if self.args.suite == 'ci':
			# The site workflow runs the same website checks once, not per OS.
			return [stage for stage in stages if stage.name != 'site']
		return stages + (self.system_stages() if self.args.suite == 'all' else [])

	def automated_stages(self):
		cargo = ('cargo', 'rustup')
		packages = ['--workspace', '--all-features', '--locked']
		if not (getattr(self.args, 'gui', False) or self.args.suite == 'ci'):
			packages += ['--exclude', 'testd-gui']
		tests_toolchain = '+1.90.0' if self.args.suite == 'contracts' else '+stable'
		ignored = r'(?m)^test result: .*?; (\d+) ignored;'
		return [
			Stage('format', self.command('format', ['cargo', '+stable', 'fmt', '--all', '--', '--check'], cargo)),
			Stage('clippy', self.command('clippy', ['cargo', '+stable', 'clippy', *packages,
				'--all-targets', '--', '-D', 'warnings'], cargo)),
			Stage('msrv', self.command('msrv', ['cargo', '+1.90.0', 'check', *packages, '--all-targets'], cargo)),
			Stage('rust-tests', self.command('rust-tests', ['cargo', tests_toolchain, 'test', *packages,
				'--all-targets', '--no-fail-fast', '--', '--include-ignored', '--show-output'], cargo,
				skip_pattern=ignored)),
			Stage('doctests', self.command('doctests', ['cargo', '+stable', 'test', *packages,
				'--doc', '--', '--include-ignored', '--show-output'], cargo, skip_pattern=ignored)),
			Stage('release-script-fixtures', self.release_script_fixtures),
			Stage('browser-server-fixtures', self.command('browser-server-fixtures', ['bun', '--no-install', 'test',
				str(ROOT / 'tests/release/browser.test.ts'), str(ROOT / 'tests/release/dashboard.test.ts')],
				('bun',), 300, skip_pattern=r'(?m)^\s*(\d+) skip\b')),
			Stage('dependency-audit', self.command('dependency-audit', ['cargo', '+stable', 'deny', '--workspace',
				'--locked', 'check'], (*cargo, 'cargo-deny'))),
			Stage('release-version', self.command('release-version', [sys.executable, 'scripts/release.py',
				'verify-version'], (), 300)),
			Stage('site', self.site),
			Stage('cli-candidate', self.candidate),
			Stage('host-unprivileged', self.host),
		]

	def system_stages(self):
		stages = []
		if self.args.artifacts:
			stages.append(Stage('artifacts', self.artifacts, system=True, precondition=self.artifacts_available,
				checkpoint='This executes candidate binaries and extracted artifacts on the confirmed disposable host.'))
		if self.args.system:
			stages += [
				Stage('system-restore-self-test', self.system_self_test, system=True,
					precondition=self.self_test_available, checkpoint='This writes real resolver and trust fixtures.'),
				Stage('host-system', self.host_system, system=True, precondition=self.host_system_available),
				Stage('service-supervision', self.service, system=True, precondition=self.service_available),
			]
		if self.args.interactive:
			stages += [
				Stage('desktop', self.desktop, system=True, interactive=True, precondition=self.desktop_available),
				Stage('browser', self.browser, system=True, interactive=True, precondition=self.require_candidate),
			]
			for name, path in [('manual-browser', 'browser.md'), ('manual-system', 'system-integration.md'),
				('manual-installation', 'installation.md'), ('manual-network-sharing', 'network-and-sharing.md')]:
				stages.append(Stage(name, self.manual(f'tests/manual/{path}'), system=True, interactive=True,
					stop_on_failure=False, checkpoint='Use the separately restored hosts required by this checklist.'))
		return stages

	def command(self, name, argv, tools, timeout=1200, environment=None, skip_pattern=None):
		def action(child):
			self.require(*tools)
			if self.api is not None and len(argv) > 1 and argv[1].startswith('+'):
				try:
					self.api.require_toolchain(argv[1][1:])
				except RuntimeError as error:
					raise Blocked(str(error)) from error
			env = environment() if environment else None
			output = child.command(name, argv, timeout, env=env)
			if skip_pattern and any(int(value) for value in re.findall(skip_pattern, output)):
				child.record('coverage', 'skip', 'blocked: checks were skipped; inspect the command log')
		return action

	def release_script_fixtures(self, child):
		output = child.command('release-script-fixtures', [sys.executable, '-m', 'unittest', 'discover',
			'-s', 'tests/release', '-p', 'test_*.py', '-v'], 300)
		output = output.replace('\r\n', '\n')
		ran = re.search(r'(?m)^Ran (\d+) tests? in ', output)
		summaries = re.findall(r'(?m)^OK(?: \(([^\n]*)\))?[ \t]*$', output)
		if not ran or int(ran[1]) == 0 or not summaries:
			child.record('coverage', 'skip', 'blocked: the unittest result summary is missing or empty')
			return
		if re.search(r'expected failures=[1-9]\d*', summaries[-1]):
			child.record('coverage', 'skip', 'blocked: fixture tests include expected failures')
			return
		match = re.search(r'\bskipped=(\d+)', summaries[-1])
		count = int(match[1]) if match else 0
		reasons = [match[1] for match in re.findall(r'(?m)^.*?\.\.\. skipped ([\'"])(.*?)\1[ \t]*$', output)]
		if not count and not reasons:
			return
		if len(reasons) == count and all(reason.startswith('not applicable:') for reason in reasons):
			child.record('coverage', 'skip', f'not applicable: {count} platform-specific fixture skips')
		elif len(reasons) == count:
			blocked = [reason for reason in reasons if not reason.startswith('not applicable:')]
			child.record('coverage', 'skip', f'blocked: fixture check skipped: {blocked[0]}')
		else:
			child.record('coverage', 'skip', 'blocked: fixture skips or their reasons are incomplete; inspect the log')

	def require(self, *tools):
		missing = [tool for tool in tools if not shutil.which(tool)]
		if missing:
			raise Blocked(f'please install: {", ".join(missing)}')

	def site_environment(self):
		return {**os.environ, 'TESTD_SITE_OUT_DIR': str(self.run.output / 'site-dist'),
			'TESTD_DOCS_CACHE_DIR': str(self.run.output / 'site-cache')}

	def require_site_dependencies(self):
		self.require('bun')
		bin_dir = str(ROOT / 'node_modules/.bin')
		if any(not shutil.which(tool, path=bin_dir) for tool in ('tsc', 'vite', 'vitepress')):
			raise Blocked('website dependencies are missing; run bun install --frozen-lockfile first')

	def site(self, child):
		self.site_check(child)
		self.site_build(child)
		self.site_links(child)

	def site_check(self, child):
		self.require_site_dependencies()
		self.command('site-check', ['bun', '--no-install', 'run', 'check'], (), 600, self.site_environment)(child)

	def site_build(self, child):
		self.require_site_dependencies()
		self.command('site-build', ['bun', '--no-install', 'run', 'build'], (), 600, self.site_environment)(child)
		self.site_ready = True

	def site_links(self, child):
		if not self.site_ready:
			raise Blocked('the isolated site build did not complete')
		self.command('site-links', ['bun', '--no-install', 'run', 'check:links'], ('bun',), 600,
			self.site_environment)(child)

	def candidate(self, child):
		if self.args.bin is None:
			self.require('cargo', 'rustup')
			try:
				self.api.require_toolchain('stable')
			except RuntimeError as error:
				raise Blocked(str(error)) from error
		self.cli = self.api.binary(child)
		self.run.metadata['candidate'] = {'path': str(self.cli), 'sha256': self.api.file_digest(self.cli)}
		self.run.save()

	def require_candidate(self):
		if self.cli is None:
			raise Blocked('the candidate CLI was not available')
		return self.cli

	def host(self, child):
		child.args.bin = self.require_candidate()
		child.args.system = False
		child.args.service = False
		self.api.host(child, scope='ci')

	def artifacts_available(self):
		if not self.args.artifacts:
			raise Blocked('provide --artifacts with the candidate assets and SHA256SUMS')

	def artifacts(self, child):
		child.args.artifacts = self.args.artifacts.expanduser().resolve()
		child.args.previous_bin = self.args.previous_bin.expanduser().resolve() if self.args.previous_bin else None
		child.args.require_checksums = True
		child.args.host = True
		child.args.system = False
		child.args.service = False
		self.api.artifacts(child)
		if not child.args.previous_bin:
			if self.args.no_previous_release:
				child.record('upgrade-rollback', 'skip', 'not applicable: no previous release')
			else:
				child.record('upgrade-rollback', 'skip', 'blocked: provide --previous-bin or --no-previous-release')

	def authorize(self, child):
		if self.api.SYSTEM == 'Windows':
			if not ctypes.windll.shell32.IsUserAnAdmin():
				raise Blocked('run the system checks from an elevated PowerShell')
		else:
			self.require('sudo')
			print('Validating sudo in the current terminal.', flush=True)
			result = subprocess.run(['sudo', '-v'], cwd=ROOT, timeout=300)
			if result.returncode:
				raise Blocked('sudo authorization was not granted')
		child.record('authorization', 'pass', 'system test authorization is available')

	def self_test_available(self):
		if self.api.SYSTEM == 'Windows':
			raise Blocked('the Windows fixture cannot yet prove ownership of its NRPT rule')

	def system_self_test(self, child):
		self.authorize(child)
		output = child.command('system-restore-self-test', [sys.executable, 'tests/release/system_state.py', '--self-test',
			'--workdir', str(child.output / 'self-test-state')], 600, expected=2, terminal=True)
		if 'ok       scoped fixture state was restored and verified' not in output:
			child.record('system-restore-self-test-result', 'fail', 'the scoped self-test did not complete')
			raise RuntimeError('the scoped system-state self-test did not complete')
		child.record('shared-trust-coverage', 'skip',
			'blocked: shared trust-store output and hooks need comparison with the full-system snapshot')

	def require_owned_product_integration(self):
		if self.api.SYSTEM == 'Windows':
			raise Blocked(f'{self.api.SYSTEM} resolver and trust ownership protections are incomplete')

	def host_system_available(self):
		self.require_owned_product_integration()
		self.require_candidate()

	def host_system(self, child):
		child.args.bin = self.require_candidate()
		if self.api.HOST_TLD != 'testdcheck':
			raise Blocked('unset TESTD_HOST_TLD; all system host checks use the throwaway .testdcheck zone')
		self.authorize(child)
		child.args.system = True
		child.args.service = False
		self.api.host(child)

	def service_available(self):
		if self.api.SYSTEM == 'Windows':
			raise NotApplicable('no Windows CLI service is shipped; test launch at login in the desktop checklist')
		self.require_candidate()

	def service(self, child):
		child.args.bin = self.require_candidate()
		self.api.service(child)

	def desktop_available(self):
		self.require_owned_product_integration()
		self.require_candidate()
		if not self.args.app:
			raise Blocked('provide --app with the actual candidate')

	def desktop(self, child):
		self.authorize(child)
		child.args.bin = self.require_candidate()
		child.args.app = self.args.app.expanduser().resolve()
		self.api.desktop(child)

	def browser(self, child):
		child.args.bin = self.require_candidate()
		self.api.check_ports([child.args.intake_port])
		self.api.browser(child)

	def manual(self, path):
		def action(child):
			items = self.api.checklist_items((ROOT / path).read_text(encoding='utf-8'))
			if not items:
				raise RuntimeError(f'no checklist items in {path}')
			print(f'Record evidence for {path}; do not include credentials or private keys.', flush=True)
			for item in items:
				print(f'\n{item}', flush=True)
				answer = input('[p]ass / [f]ail / [b]locked / [n]ot applicable / [q]uit: ').strip().lower()
				while answer not in ('p', 'f', 'b', 'n', 'q'):
					answer = input('Enter p, f, b, n, or q: ').strip().lower()
				if answer == 'q':
					child.metadata['incomplete'] = True
					child.record('manual-checklist', 'skip', 'blocked: unanswered items')
					return
				evidence = input('Evidence or reason (required): ').strip()
				while not evidence:
					evidence = input('Evidence or reason is required: ').strip()
				status = {'p': 'pass', 'f': 'fail', 'b': 'skip', 'n': 'skip'}[answer]
				prefix = {'b': 'blocked: ', 'n': 'not applicable: '}.get(answer, '')
				child.record(item, status, prefix + evidence)
		return action

	def confirm(self, message):
		print(message, flush=True)
		try:
			return input('Type yes to continue; anything else marks this stage blocked: ').strip().lower() == 'yes'
		except EOFError:
			return False

	def check_stage(self, stage):
		if self.interrupted:
			raise Blocked('run interrupted by the operator')
		if stage.precondition:
			stage.precondition()
		if stage.system:
			if not self.args.system or not self.system_confirmed:
				raise Blocked('requires --system and confirmation of a disposable host and persisted full-system snapshot')
			if self.system_unsafe:
				raise Blocked('an earlier system stage failed or was incomplete; inspect recovery before continuing')
		if stage.interactive and not self.args.interactive:
			raise Blocked('requires --interactive and actual operator observations')
		if stage.checkpoint:
			print(stage.checkpoint, flush=True)

	def child_run(self, index, stage):
		args = copy.copy(self.args)
		args.suite = stage.name
		args.report = self.run.output / f'{index:02d}-{stage.name}'
		args.include_git_diff = False
		return self.api.Run(args, parent=self.run)

	def classify(self, child):
		if any(item['status'] == 'fail' for item in child.results):
			return 'fail', 'one or more checks failed'
		if child.metadata.get('incomplete') or any(item['status'] == 'skip' and
			not item.get('detail', '').startswith('not applicable:') for item in child.results):
			return 'blocked', 'one or more checks were skipped or incomplete'
		return 'pass', ''

	def boundary(self, text):
		print(text, flush=True)

	def cleanup_verified(self, stage, child):
		passed = {item['name'] for item in child.results if item['status'] == 'pass'}
		if any(item['name'] == 'shared-trust-coverage' and item['status'] != 'pass' for item in child.results):
			return False
		if stage.name == 'host-system':
			cleanup = {'fixture-shutdown', 'system-owned-cleanup', 'system-restore'}
			return {'host-checks', 'fixture-shutdown', 'system-restore'}.issubset(passed) and not any(
				item['name'] in cleanup and item['status'] != 'pass' for item in child.results)
		if stage.name == 'desktop':
			cleanup = {'fixture-shutdown', 'system-owned-cleanup', 'system-restore'}
			return {'fixture-shutdown', 'system-restore'}.issubset(passed) and not any(
				item['name'] in cleanup and item['status'] != 'pass' for item in child.results)
		if stage.name == 'browser':
			fixtures = [item for item in child.results if item['name'].startswith('host-fixture')]
			return bool(fixtures) and all(item['status'] == 'pass' for item in fixtures) and 'browser-fixture' in passed
		return False

	def stage(self, index, total, stage):
		self.boundary(f'\n========== BEGIN [{index:02d}/{total:02d}] {stage.name} ==========')
		started = time.monotonic()
		child = None
		status, detail = 'pass', ''
		try:
			self.check_stage(stage)
			child = self.child_run(index, stage)
			self.run.metadata['stages'] = [*self.outcomes,
				{'name': stage.name, 'status': 'running', 'checks': child.results, 'metadata': child.metadata}]
			self.run.save()
			stage.action(child)
			status, detail = self.classify(child)
		except NotApplicable as error:
			status, detail = 'not-applicable', f'not applicable: {error}'
		except Blocked as error:
			status, detail = 'blocked', str(error)
		except (KeyboardInterrupt, EOFError):
			self.interrupted = True
			status, detail = 'blocked', 'interrupted by the operator or input closed'
		except Exception as error:
			if child is None or not any(item['status'] == 'fail' for item in child.results):
				traceback.print_exc()
			status, detail = 'fail', str(error)
		if child is not None:
			try:
				child.save()
			except Exception as error:
				status, detail = 'fail', f'could not save stage report: {error}'
			detail = '; '.join(filter(None, [detail, f'logs: {child.output}']))
		if stage.system and stage.stop_on_failure and child is not None and status in ('fail', 'blocked'):
			if not self.cleanup_verified(stage, child):
				self.system_unsafe = True
		elapsed = time.monotonic() - started
		self.outcomes.append({'name': stage.name, 'status': status, 'detail': detail, 'seconds': round(elapsed, 2),
			'checks': child.results if child is not None else [],
			'metadata': child.metadata if child is not None else {}})
		self.run.metadata['stages'] = self.outcomes
		if status == 'blocked':
			self.run.metadata['incomplete'] = True
		result = 'fail' if status == 'fail' else 'skip' if status in ('blocked', 'not-applicable') else 'pass'
		try:
			self.run.record(stage.name, result, detail)
		finally:
			self.boundary(f'========== END [{index:02d}/{total:02d}] {stage.name}: {status.upper()} ==========')

	def summary(self):
		label = self.args.suite.upper()
		self.boundary(f'\n========== {label} TESTS: SUMMARY ==========')
		for outcome in self.outcomes:
			self.boundary(f'{outcome["status"].upper():14} {outcome["name"]}: {outcome["detail"]}')
		self.boundary(f'Report: {self.run.output / "report.json"}')
		if self.args.suite == 'ci':
			self.write_ci_summary()

	def write_ci_summary(self):
		# Full reports can contain fixture credentials and keys; publish only allowlisted status metadata.
		summary = {name: self.run.metadata.get(name) for name in ('os', 'python', 'commit', 'dirty', 'scope')}
		summary['suite'] = 'ci'
		summary['stages'] = [{name: outcome[name] for name in ('name', 'status', 'seconds')}
			for outcome in self.outcomes]
		path = self.run.output / 'ci-summary.json'
		path.write_text(json.dumps(summary, indent=2) + '\n', encoding='utf-8')

	def execute(self):
		stages = self.stages()
		self.run.metadata['scope'] = {
			'gui': getattr(self.args, 'gui', False) or self.args.suite == 'ci',
			'system': self.args.system, 'interactive': self.args.interactive,
			'artifacts': bool(self.args.artifacts), 'site': any(stage.name == 'site' for stage in stages),
		}
		if self.args.system:
			try:
				self.system_confirmed = self.confirm('Confirm a disposable host with a persisted full-system snapshot. '
					'Scoped runner snapshots do not replace the full backup.')
			except KeyboardInterrupt:
				self.interrupted = True
		for index, stage in enumerate(stages, 1):
			self.stage(index, len(stages), stage)
		self.summary()
