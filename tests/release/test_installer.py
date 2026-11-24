import hashlib
import io
import os
import pathlib
import shutil
import subprocess
import tarfile
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
VERSION = '0.1.0-alpha.1'
TARGET = 'aarch64-apple-darwin'
BINARY = b'#!/bin/sh\nprintf "testd fixture\\n"\n'


@unittest.skipIf(os.name == 'nt', 'not applicable: the website installer is POSIX-only')
class InstallerTests(unittest.TestCase):
	def setUp(self):
		temporary = tempfile.TemporaryDirectory()
		self.addCleanup(temporary.cleanup)
		self.root = pathlib.Path(temporary.name)
		tools = self.root / 'tools'
		tools.mkdir()
		uname = tools / 'uname'
		uname.write_text('#!/bin/sh\ncase "$1" in -s) echo Darwin;; -m) echo arm64;; *) exit 1;; esac\n')
		uname.chmod(0o700)
		curl = tools / 'curl'
		shutil.copyfile(ROOT / 'tests/release/fixtures/installer-curl.sh', curl)
		curl.chmod(0o700)
		self.archive = self.root / f'testd-{VERSION}-{TARGET}.tar.gz'
		with tarfile.open(self.archive, 'w:gz') as archive:
			entry = tarfile.TarInfo(f'testd-{VERSION}-{TARGET}/testd')
			entry.mode = 0o755
			entry.size = len(BINARY)
			archive.addfile(entry, io.BytesIO(BINARY))
		digest = hashlib.sha256(self.archive.read_bytes()).hexdigest()
		(self.root / 'SHA256SUMS').write_text(f'{digest}  {self.archive.name}\n')
		self.installed = self.root / 'installed/testd'
		self.requests = self.root / 'requests'
		self.env = {**os.environ, 'PATH': str(tools) + os.pathsep + os.environ.get('PATH', ''),
			'GITHUB_TOKEN': '', 'TESTD_REPO': 'fixture/testd', 'TESTD_VERSION': '',
			'TESTD_INSTALL_DIR': str(self.installed.parent), 'TMPDIR': temporary.name,
			'FIXTURE_REQUESTS': str(self.requests), 'FIXTURE_ASSETS': str(self.root),
			'FIXTURE_VERSION': VERSION, 'FIXTURE_LOOKUP': 'success',
			'FIXTURE_REDIRECT': f'https://github.com/fixture/testd/releases/tag/v{VERSION}'}

	def run_installer(self, **env):
		return subprocess.run(['sh', str(ROOT / 'install.sh')], env={**self.env, **env},
			capture_output=True, text=True, timeout=15)

	def test_anonymous_install_avoids_the_rate_limited_api(self):
		result = self.run_installer()
		self.assertEqual(result.returncode, 0, result.stderr)
		self.assertEqual(self.installed.read_bytes(), BINARY)
		self.assertIn(f'testd {VERSION} ({TARGET})', result.stdout)
		requests = self.requests.read_text()
		self.assertIn('https://github.com/fixture/testd/releases/latest', requests)
		self.assertNotIn('api.github.com', requests)
		self.assertIn(self.archive.name, requests)
		self.assertIn('SHA256SUMS', requests)

	def test_authenticated_lookup_preserves_token_support(self):
		result = self.run_installer(GITHUB_TOKEN='fixture-token', FIXTURE_LOOKUP='api-success')
		self.assertEqual(result.returncode, 0, result.stderr)
		self.assertEqual(self.installed.read_bytes(), BINARY)
		self.assertIn('Authorization: Bearer fixture-token', self.requests.read_text())
		self.assertNotIn('fixture-token', result.stdout + result.stderr)

	def test_pinned_version_skips_discovery(self):
		result = self.run_installer(TESTD_VERSION=VERSION, FIXTURE_LOOKUP='forbidden')
		self.assertEqual(result.returncode, 0, result.stderr)
		self.assertEqual(self.installed.read_bytes(), BINARY)
		self.assertNotIn('/latest', self.requests.read_text())

	def test_lookup_failures_report_access_and_version_override(self):
		for lookup, token in (('forbidden', ''), ('missing', ''), ('network', ''), ('forbidden', 'fixture-token')):
			with self.subTest(lookup=lookup, authenticated=bool(token)):
				result = self.run_installer(FIXTURE_LOOKUP=lookup, GITHUB_TOKEN=token)
				self.assertNotEqual(result.returncode, 0)
				self.assertIn('check GitHub access', result.stderr)
				self.assertIn('TESTD_VERSION', result.stderr)
				self.assertNotIn('could not find a stable release', result.stderr)
				self.assertFalse(self.installed.exists())
				self.assertNotIn('/releases/download/', self.requests.read_text())

	def test_unexpected_redirects_do_not_select_a_release(self):
		for redirect in ('https://github.com/fixture/testd/releases',
				'https://github.com/fixture/testd/releases/tag/',
				'https://github.com/fixture/testd/releases/tag/v',
				'https://github.com/other/testd/releases/tag/v1.0.0'):
			with self.subTest(redirect=redirect):
				result = self.run_installer(FIXTURE_REDIRECT=redirect)
				self.assertNotEqual(result.returncode, 0)
				self.assertIn('could not read the release tag', result.stderr)
				self.assertFalse(self.installed.exists())
				self.assertNotIn('/releases/download/', self.requests.read_text())

	def test_checksum_failure_preserves_an_existing_install(self):
		self.installed.parent.mkdir()
		self.installed.write_bytes(b'previous installation')
		with self.archive.open('ab') as archive:
			archive.write(b'corrupted download')
		result = self.run_installer()
		self.assertNotEqual(result.returncode, 0)
		self.assertIn('checksum mismatch', result.stderr)
		self.assertEqual(self.installed.read_bytes(), b'previous installation')
