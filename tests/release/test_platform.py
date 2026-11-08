import contextlib
import importlib.util
import io
import pathlib
import subprocess
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import Mock, patch

ROOT = pathlib.Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('portable_runner', ROOT / 'tests/run.py')
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


class EntryPointTests(unittest.TestCase):
	def invoke(self, arguments):
		run = Mock(results=[], metadata={'incomplete': False})
		checks = Mock()
		with patch.object(runner, 'Run', return_value=run) as create, \
			patch.object(runner, 'helper', return_value=checks), \
			patch.object(runner, 'all_suites') as execute, \
			patch.object(runner, 'match_redirected_encoding'), patch.object(runner.signal, 'signal'), \
			patch.object(runner, 'run_transcript', return_value=contextlib.nullcontext()), \
			contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
			try:
				status = runner.main(arguments)
			except SystemExit as error:
				status = error.code
		return status, create, execute, checks

	def test_no_arguments_runs_unprivileged_checks_on_every_supported_os(self):
		for system in ('Linux', 'Darwin', 'Windows'):
			with self.subTest(system=system), patch.object(runner, 'SYSTEM', system):
				status, create, execute, _ = self.invoke([])
				self.assertEqual(status, 0)
				args = create.call_args.args[0]
				self.assertEqual(args.suite, 'check')
				self.assertFalse(args.gui or args.system or args.interactive)
				execute.assert_called_once_with(create.return_value)

	def test_ci_includes_gui_but_not_privileged_checks(self):
		status, create, _, _ = self.invoke(['ci'])
		self.assertEqual(status, 0)
		args = create.call_args.args[0]
		self.assertTrue(args.gui)
		self.assertFalse(args.system or args.interactive)

	def test_plan_creates_no_report_and_runs_no_commands(self):
		for suite in ('check', 'ci', 'all', 'contracts', 'audit'):
			with self.subTest(suite=suite):
				status, create, execute, checks = self.invoke([suite, '--plan'])
				self.assertEqual(status, 0)
				create.assert_not_called()
				execute.assert_not_called()
				checks.print_plan.assert_called_once()

	def test_irrelevant_and_unsafe_options_fail_before_execution(self):
		cases = [
			['check', '--system'], ['ci', '--system'], ['ci', '--interactive'],
			['all', '--interactive'], ['all', '--app', 'candidate'],
			['all', '--artifacts', 'packages'], ['all', '--browser-engine', 'chromium'],
			['all', '--system', '--app', 'candidate'], ['check', '--browser-engine', 'chromium'],
			['audit', '--gui'], ['host', '--system', '--service'], ['artifacts'], ['desktop'],
			['check', '--bin', 'missing-candidate'], ['check', '--port', '8080'],
			['browser', '--port', '0'], ['browser', '--port', '65535'],
			['browser', '--edge-port', '65536'], ['browser', '--intake-port', '0'],
			['all', '--previous-bin', 'missing'], ['check', '--build'], ['check', '--strict'],
		]
		for arguments in cases:
			with self.subTest(arguments=arguments):
				status, create, execute, _ = self.invoke(arguments)
				self.assertEqual(status, 2)
				create.assert_not_called()
				execute.assert_not_called()

	def test_default_and_service_profiles_accept_an_identified_cli(self):
		with tempfile.TemporaryDirectory() as directory:
			candidate = pathlib.Path(directory) / 'candidate'
			candidate.write_bytes(b'fixture; never executed')
			for suite in ('check', 'service'):
				with self.subTest(suite=suite):
					parser = runner.argument_parser()
					args = parser.parse_args([suite, '--bin', str(candidate)])
					runner.validate_options(parser, args)
					self.assertEqual(args.bin, candidate.resolve())

	def test_supplied_candidate_must_exist_before_a_report_is_created(self):
		status, create, execute, _ = self.invoke(['all', '--bin', str(ROOT / 'missing-candidate')])
		self.assertEqual(status, 2)
		create.assert_not_called()
		execute.assert_not_called()


class PreparationTests(unittest.TestCase):
	def test_missing_tools_are_listed_together_without_installing_anything(self):
		output = io.StringIO()
		with patch.object(runner.shutil, 'which', return_value=None), \
			patch.object(runner.subprocess, 'run') as command, contextlib.redirect_stdout(output):
			self.assertEqual(runner.prepare(SimpleNamespace(gui=False)), 2)
		for tool in ('cargo', 'rustup', 'bun', 'cargo-deny', 'website dependencies'):
			self.assertIn(tool, output.getvalue())
		command.assert_not_called()

	def test_missing_toolchain_is_reported_as_incomplete(self):
		output = io.StringIO()
		with patch.object(runner.shutil, 'which', return_value='installed'), \
			patch.object(runner, 'require_toolchain', side_effect=[None, RuntimeError('install 1.90.0')]), \
			contextlib.redirect_stdout(output):
			self.assertEqual(runner.prepare(SimpleNamespace(gui=False)), 2)
		self.assertIn('Missing: install 1.90.0', output.getvalue())

	def test_installed_prerequisites_pass_without_a_build(self):
		with patch.object(runner.shutil, 'which', return_value='installed'), \
			patch.object(runner, 'require_toolchain') as toolchain, \
			patch.object(pathlib.Path, 'symlink_to'), contextlib.redirect_stdout(io.StringIO()):
			self.assertEqual(runner.prepare(SimpleNamespace(gui=False)), 0)
		self.assertEqual([call.args[0] for call in toolchain.call_args_list], ['stable', '1.90.0'])

	def test_windows_preparation_checks_file_and_directory_symlinks_without_elevation(self):
		with patch.object(runner, 'SYSTEM', 'Windows'), \
			patch.object(runner.shutil, 'which', return_value='installed'), \
			patch.object(runner, 'require_toolchain'), patch.object(pathlib.Path, 'symlink_to') as symlink, \
			patch.object(runner.subprocess, 'run') as command, contextlib.redirect_stdout(io.StringIO()):
			self.assertEqual(runner.prepare(SimpleNamespace(gui=False)), 0)
		self.assertEqual(symlink.call_count, 2)
		self.assertEqual(symlink.call_args_list[0].kwargs, {})
		self.assertEqual(symlink.call_args_list[1].kwargs, {'target_is_directory': True})
		command.assert_not_called()

	def test_windows_symlink_prerequisite_reports_privilege_and_filesystem_blockers(self):
		for code in (50, 1314):
			for successful_links in (0, 1):
				with self.subTest(code=code, successful_links=successful_links):
					error = OSError('fixture: symlink unavailable')
					error.winerror = code
					output = io.StringIO()
					with patch.object(runner, 'SYSTEM', 'Windows'), \
						patch.object(runner.shutil, 'which', return_value='installed'), \
						patch.object(runner, 'require_toolchain'), \
						patch.object(pathlib.Path, 'symlink_to', side_effect=[None] * successful_links + [error]), \
						contextlib.redirect_stdout(output):
						self.assertEqual(runner.prepare(SimpleNamespace(gui=False)), 2)
					self.assertIn('Missing: Windows symlink creation', output.getvalue())
					self.assertIn('Windows Developer Mode', output.getvalue())
					self.assertIn('SeCreateSymbolicLinkPrivilege', output.getvalue())

	def test_windows_symlink_probe_does_not_hide_unexpected_io_errors(self):
		error = OSError('fixture: unexpected access failure')
		error.winerror = 5
		with patch.object(runner, 'SYSTEM', 'Windows'), \
			patch.object(runner.shutil, 'which', return_value='installed'), \
			patch.object(pathlib.Path, 'symlink_to', side_effect=error), self.assertRaisesRegex(OSError, 'unexpected'):
			runner.prepare(SimpleNamespace(gui=False))

	def test_toolchain_check_never_installs_a_missing_toolchain(self):
		for code in (0, 1):
			with self.subTest(code=code), patch.object(runner.shutil, 'which', return_value='rustup'), \
				patch.object(runner.subprocess, 'run',
					return_value=SimpleNamespace(returncode=code, stderr='missing toolchain')) as command:
				if code == 0:
					runner.require_toolchain('stable')
				else:
					with self.assertRaisesRegex(RuntimeError, 'rustup toolchain install stable'):
						runner.require_toolchain('stable')
				self.assertEqual(command.call_args.args[0], ['rustup', 'run', 'stable', 'rustc', '--version'])
				self.assertNotIn('--install', command.call_args.args[0])

	def test_missing_gui_packages_are_reported_before_a_build(self):
		output = io.StringIO()
		with patch.object(runner, 'SYSTEM', 'Linux'), \
			patch.object(runner.shutil, 'which', side_effect=lambda name, **options:
				None if name == 'pkg-config' else 'installed'), \
			patch.object(runner, 'require_toolchain'), patch.object(runner.subprocess, 'run') as command, \
			contextlib.redirect_stdout(output):
			self.assertEqual(runner.prepare(SimpleNamespace(gui=True)), 2)
		self.assertIn('Missing: pkg-config', output.getvalue())
		command.assert_not_called()

	def test_failed_toolchain_query_is_not_a_pass(self):
		with patch.object(runner.shutil, 'which', return_value='installed'), \
			patch.object(runner, 'require_toolchain', side_effect=subprocess.CalledProcessError(1, 'rustup')), \
			contextlib.redirect_stdout(io.StringIO()):
			self.assertEqual(runner.prepare(SimpleNamespace(gui=False)), 2)


if __name__ == '__main__':
	unittest.main()
