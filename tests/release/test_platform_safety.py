import pathlib
import platform
import shutil
import subprocess
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
POWERSHELLS = [engine for engine in (shutil.which('powershell'), shutil.which('pwsh')) if engine]
POWERSHELL_SKIP = ('PowerShell is required for Windows NRPT behavior fixtures' if platform.system() == 'Windows'
	else 'not applicable: Windows-only NRPT helper; PowerShell is unavailable')


class ResolverSafetyTests(unittest.TestCase):
	@unittest.skipUnless(POWERSHELLS, POWERSHELL_SKIP)
	def test_owned_resolver_and_root_actions_preserve_unowned_state(self):
		for engine in POWERSHELLS:
			with self.subTest(engine=pathlib.Path(engine).name):
				result = subprocess.run([engine, '-NoProfile', '-NonInteractive', '-File',
					str(ROOT / 'tests/release/windows-helper-fixtures.ps1'),
					'-Helper', str(ROOT / 'scripts/testd-elevated.ps1')],
					capture_output=True, text=True, timeout=30)
				self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
				self.assertIn('24 isolated ownership cases', result.stdout)
				if platform.system() == 'Windows':
					self.assertIn('PASS: native Windows certificate cache comparison', result.stdout)
