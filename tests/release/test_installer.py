import hashlib
import io
import os
import pathlib
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
OVERRIDE_VERSION = '9.8.7-fixture.1'
TARGET = 'aarch64-apple-darwin'
BINARY = b'#!/bin/sh\nprintf "testd fixture\\n"\n'


@unittest.skipIf(os.name == 'nt', 'not applicable: the website installer is POSIX-only')
class InstallerTests(unittest.TestCase):
	def setUp(self):
		temporary = tempfile.TemporaryDirectory()
		self.addCleanup(temporary.cleanup)
		self.root = pathlib.Path(temporary.name)
		self.version = '1.2.3'
		tools = self.root / 'tools'
		tools.mkdir()
		uname = tools / 'uname'
		uname.write_text('#!/bin/sh\ncase "$1" in -s) echo Darwin;; -m) echo arm64;; *) exit 1;; esac\n')
		uname.chmod(0o700)
		curl = tools / 'curl'
		shutil.copyfile(ROOT / 'tests/release/fixtures/installer-curl.sh', curl)
		curl.chmod(0o700)
		checksums = []
		for version in (self.version, OVERRIDE_VERSION):
			path = self.root / f'testd-{version}-{TARGET}.tar.gz'
			with tarfile.open(path, 'w:gz') as archive:
				entry = tarfile.TarInfo(f'testd-{version}-{TARGET}/testd')
				entry.mode = 0o755
				entry.size = len(BINARY)
				archive.addfile(entry, io.BytesIO(BINARY))
			digest = hashlib.sha256(path.read_bytes()).hexdigest()
			checksums.append(f'{digest}  {path.name}\n')
		self.archive = self.root / f'testd-{self.version}-{TARGET}.tar.gz'
		(self.root / 'SHA256SUMS').write_text(''.join(checksums))
		self.installed = self.root / 'installed/testd'
		self.requests = self.root / 'requests'
		self.env = {**os.environ, 'PATH': str(tools) + os.pathsep + os.environ.get('PATH', ''),
			'TESTD_REPO': 'fixture/testd',
			'TESTD_INSTALL_DIR': str(self.installed.parent), 'TMPDIR': temporary.name,
			'FIXTURE_REQUESTS': str(self.requests), 'FIXTURE_ASSETS': str(self.root),
			'FIXTURE_VERSION': self.version, 'FIXTURE_DOWNLOAD': 'success'}
		self.env.pop('TESTD_VERSION', None)
		self.env.pop('GITHUB_TOKEN', None)

	def run_installer(self, installer=ROOT / 'install.sh', **env):
		return subprocess.run(['sh', str(installer)], env={**self.env, **env},
			capture_output=True, text=True, timeout=15)

	def test_default_version_resolves_the_latest_release(self):
		result = self.run_installer()
		self.assertEqual(result.returncode, 0, result.stderr)
		self.assertEqual(self.installed.read_bytes(), BINARY)
		self.assertIn(f'testd {self.version} ({TARGET})', result.stdout)
		requests = self.requests.read_text().splitlines()
		self.assertEqual(len(requests), 3)
		self.assertTrue(requests[0].endswith('https://github.com/fixture/testd/releases/latest'))
		base = f'https://github.com/fixture/testd/releases/download/v{self.version}'
		self.assertTrue(requests[1].endswith(f'{base}/{self.archive.name}'))
		self.assertTrue(requests[2].endswith(f'{base}/SHA256SUMS'))

	def test_empty_version_uses_latest(self):
		result = self.run_installer(TESTD_VERSION='')
		self.assertEqual(result.returncode, 0, result.stderr)
		self.assertEqual(self.installed.read_bytes(), BINARY)
		self.assertIn(f'testd {self.version} ({TARGET})', result.stdout)

	def test_explicit_version_overrides_the_default(self):
		result = self.run_installer(TESTD_VERSION=OVERRIDE_VERSION, FIXTURE_VERSION=OVERRIDE_VERSION)
		self.assertEqual(result.returncode, 0, result.stderr)
		self.assertEqual(self.installed.read_bytes(), BINARY)
		self.assertIn(f'testd {OVERRIDE_VERSION} ({TARGET})', result.stdout)
		self.assertIn(f'/releases/download/v{OVERRIDE_VERSION}/', self.requests.read_text())
		self.assertEqual(len(self.requests.read_text().splitlines()), 2)

	def test_generated_installer_uses_its_release_default(self):
		output = self.root / 'release'
		generation = subprocess.run([sys.executable, str(ROOT / 'scripts/release.py'), 'installer',
			'--version', OVERRIDE_VERSION, '--output', str(output)],
			capture_output=True, text=True, timeout=15)
		self.assertEqual(generation.returncode, 0, generation.stderr)
		result = self.run_installer(installer=output / 'install.sh', FIXTURE_VERSION=OVERRIDE_VERSION)
		self.assertEqual(result.returncode, 0, result.stderr)
		self.assertEqual(self.installed.read_bytes(), BINARY)
		self.assertIn(f'testd {OVERRIDE_VERSION} ({TARGET})', result.stdout)
		self.assertEqual(len(self.requests.read_text().splitlines()), 2)

	def test_github_token_does_not_send_credentials(self):
		result = self.run_installer(GITHUB_TOKEN='fixture-token')
		self.assertEqual(result.returncode, 0, result.stderr)
		self.assertEqual(self.installed.read_bytes(), BINARY)
		self.assertNotIn('fixture-token', self.requests.read_text() + result.stdout + result.stderr)

	def test_failed_or_invalid_discovery_leaves_no_installation(self):
		for env in ({'FIXTURE_DOWNLOAD': 'missing-release'},
			{'FIXTURE_RELEASE_URL': 'https://github.com/fixture/testd/releases/latest'}):
			with self.subTest(env=env):
				result = self.run_installer(**env)
				self.assertNotEqual(result.returncode, 0)
				self.assertIn('could not resolve latest release', result.stderr)
				self.assertFalse(self.installed.exists())

	def test_failed_downloads_leave_no_installation(self):
		for failure, message in (('forbidden', 'download failed:'), ('missing-checksums', 'checksum file unavailable')):
			with self.subTest(failure=failure):
				result = self.run_installer(TESTD_VERSION=self.version, FIXTURE_DOWNLOAD=failure)
				self.assertNotEqual(result.returncode, 0)
				self.assertIn(message, result.stderr)
				self.assertFalse(self.installed.exists())

	def test_checksum_failure_preserves_an_existing_install(self):
		self.installed.parent.mkdir()
		self.installed.write_bytes(b'previous installation')
		with self.archive.open('ab') as archive:
			archive.write(b'corrupted download')
		result = self.run_installer()
		self.assertNotEqual(result.returncode, 0)
		self.assertIn('checksum mismatch', result.stderr)
		self.assertEqual(self.installed.read_bytes(), b'previous installation')
