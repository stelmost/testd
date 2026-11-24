import hashlib
import json
import os
import pathlib
import shlex
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
FIXTURES = pathlib.Path(__file__).parent / 'fixtures/ssh'
VERSION = tomllib.loads((ROOT / 'Cargo.toml').read_text())['package']['version']
SCRIPT = (ROOT / 'crates/testd-client/src/ssh/bootstrap.sh').read_text()
APP_START = '{"type":"Start","app":{"command":{"program":"example"},"port":3000}}\n'
APPROVAL = '{"type":"Install"}\n'
TOOLS = ['awk', 'chmod', 'cp', 'gzip', 'mkdir', 'mktemp', 'mv', 'rm', 'sh', 'tar', 'sha256sum', 'shasum']


@unittest.skipUnless(os.name == 'posix', 'remote installation fixtures require a POSIX shell')
class SshInstallTests(unittest.TestCase):
	def setUp(self):
		self.temporary = tempfile.TemporaryDirectory()
		self.addCleanup(self.temporary.cleanup)
		self.root = pathlib.Path(self.temporary.name)
		self.home = self.root / "remote user's home"
		self.home.mkdir()
		self.commands = self.root / 'commands'
		self.commands.mkdir()
		for name in TOOLS:
			path = shutil.which(name)
			if path:
				(self.commands / name).symlink_to(path)
		self.install_fixture('curl', 'download.sh')
		self.install_fixture('uname', 'uname.sh')
		self.releases = self.root / 'releases'
		self.releases.mkdir()
		self.download_log = self.root / 'downloads.log'
		self.session_log = self.root / 'session.log'
		self.umask_log = self.root / 'umask.log'
		self.destination = self.home / '.local/bin/testd'
		self.env = {
			**os.environ,
			'HOME': str(self.home),
			'PATH': str(self.commands),
			'TESTD_RELEASE_FIXTURE': str(self.releases),
			'TESTD_DOWNLOAD_LOG': str(self.download_log),
			'TESTD_SESSION_LOG': str(self.session_log),
			'TESTD_UMASK_LOG': str(self.umask_log),
			'TESTD_FIXTURE_OS': 'Linux',
			'TESTD_FIXTURE_ARCH': 'x86_64',
		}

	def install_fixture(self, name, fixture):
		path = self.commands / name
		path.write_text((FIXTURES / fixture).read_text())
		path.chmod(0o755)

	def helper(self, path, version=VERSION):
		path.parent.mkdir(parents=True, exist_ok=True)
		path.write_text((FIXTURES / 'helper.sh').read_text().replace('@VERSION@', version))
		path.chmod(0o755)
		return path.read_bytes()

	def release(self, target='x86_64-unknown-linux-musl', payload_version=VERSION):
		base = f'testd-{VERSION}-{target}'
		binary = self.root / 'release-testd'
		self.helper(binary, payload_version)
		archive = self.releases / f'{base}.tar.gz'
		with tarfile.open(archive, 'w:gz') as output:
			output.add(binary, arcname=f'{base}/testd')
		digest = hashlib.sha256(archive.read_bytes()).hexdigest()
		(self.releases / 'SHA256SUMS').write_text(f'{digest}  {archive.name}\n')

	def launch(self, answer=APPROVAL + APP_START):
		return subprocess.run(
			[str(self.commands / 'sh'), '-c', f'umask 022\nversion={shlex.quote(VERSION)}\n{SCRIPT}'],
			input=answer, capture_output=True, text=True, env=self.env, timeout=10,
		)

	def frames(self, result):
		return [json.loads(line) for line in result.stdout.splitlines()]

	def backups(self):
		return list((self.home / '.local/share/testd/backups').glob('ssh-install.*/testd'))

	def assert_started(self, result):
		self.assertEqual(result.returncode, 0, result.stderr)
		self.assertEqual(self.frames(result)[-1], {'type': 'Hello', 'version': 1})
		self.assertEqual(self.session_log.read_text(), APP_START)
		self.assertEqual(int(self.umask_log.read_text().strip(), 8), 0o22)
		self.assertEqual(list(self.destination.parent.glob('.testd-install.*')), [])

	def test_matching_user_installation_skips_prompt_and_download(self):
		original = self.helper(self.destination)
		result = self.launch(APP_START)
		self.assert_started(result)
		self.assertEqual(len(self.frames(result)), 1)
		self.assertFalse(self.download_log.exists())
		self.assertEqual(self.destination.read_bytes(), original)
		self.assertEqual(self.backups(), [])

	def test_matching_path_installation_is_used_before_replacing_user_binary(self):
		original = self.helper(self.destination, '0.0.1')
		self.helper(self.commands / 'testd')
		result = self.launch(APP_START)
		self.assert_started(result)
		self.assertEqual(len(self.frames(result)), 1)
		self.assertEqual(self.destination.read_bytes(), original)
		self.assertFalse(self.download_log.exists())

	def test_missing_helper_is_installed_and_started_after_approval(self):
		self.release()
		result = self.launch()
		self.assert_started(result)
		self.assertEqual(self.frames(result)[0], {'type': 'Install', 'version': None})
		self.assertEqual(len(self.download_log.read_text().splitlines()), 2)
		self.assertIn(f'/v{VERSION}/testd-{VERSION}-x86_64-unknown-linux-musl.tar.gz',
			self.download_log.read_text())
		self.assertTrue(os.access(self.destination, os.X_OK))

	def test_mismatched_versions_require_approval_and_leave_a_persistent_backup(self):
		self.release()
		for version in ['0.0.1', '99.0.0']:
			with self.subTest(version=version):
				original = self.helper(self.destination, version)
				result = self.launch()
				self.assert_started(result)
				self.assertEqual(self.frames(result)[0], {'type': 'Install', 'version': version})
				self.assertIn(original, [path.read_bytes() for path in self.backups()])
				self.assertIn('Previous testd saved to', result.stderr)

	def test_cancelling_or_unrecognized_answers_make_no_remote_changes(self):
		for answer in ['', 'yes\n', '{"type":"Stop"}\n']:
			with self.subTest(answer=answer):
				result = self.launch(answer)
				self.assertNotEqual(result.returncode, 0)
				self.assertEqual(self.frames(result), [{'type': 'Install', 'version': None}])
				self.assertEqual(list(self.home.iterdir()), [])
				self.assertFalse(self.download_log.exists())

	def test_declining_an_update_preserves_the_installed_binary(self):
		original = self.helper(self.destination, '0.0.1')
		result = self.launch('')
		self.assertNotEqual(result.returncode, 0)
		self.assertEqual(self.destination.read_bytes(), original)
		self.assertEqual(self.backups(), [])
		self.assertFalse(self.download_log.exists())

	def test_download_failure_preserves_the_old_binary(self):
		original = self.helper(self.destination, '0.0.1')
		self.env['TESTD_FAIL_DOWNLOAD'] = '1'
		result = self.launch()
		self.assertNotEqual(result.returncode, 0)
		self.assertEqual(self.frames(result)[-1]['type'], 'Failed')
		self.assertEqual(self.destination.read_bytes(), original)
		self.assertEqual(self.backups(), [])

	def test_bad_checksum_never_executes_or_replaces_the_download(self):
		self.release()
		original = self.helper(self.destination, '0.0.1')
		archive = next(self.releases.glob('*.tar.gz'))
		(self.releases / 'SHA256SUMS').write_text(f'{"0" * 64}  {archive.name}\n')
		result = self.launch()
		self.assertNotEqual(result.returncode, 0)
		self.assertIn('checksum verification', self.frames(result)[-1]['message'])
		self.assertEqual(self.destination.read_bytes(), original)
		self.assertFalse(self.session_log.exists())
		self.assertEqual(self.backups(), [])

	def test_payload_version_is_checked_before_replacing_the_old_binary(self):
		self.release(payload_version='0.0.2')
		original = self.helper(self.destination, '0.0.1')
		result = self.launch()
		self.assertNotEqual(result.returncode, 0)
		self.assertIn('version does not match', self.frames(result)[-1]['message'])
		self.assertEqual(self.destination.read_bytes(), original)
		self.assertEqual(self.backups(), [])

	def test_replacing_a_symlink_backs_it_up_and_preserves_its_target(self):
		path_binary = self.commands / 'testd'
		original = self.helper(path_binary, '0.0.1')
		self.destination.parent.mkdir(parents=True)
		self.destination.symlink_to(path_binary)
		self.release()
		self.assert_started(self.launch())
		self.assertFalse(self.destination.is_symlink())
		self.assertEqual(path_binary.read_bytes(), original)
		backup, = self.backups()
		self.assertTrue(backup.is_symlink())
		self.assertEqual(backup.readlink(), path_binary)

	def test_unsupported_hosts_do_not_offer_an_unavailable_download(self):
		self.env['TESTD_FIXTURE_ARCH'] = 'aarch64'
		result = self.launch()
		self.assertNotEqual(result.returncode, 0)
		self.assertEqual(self.frames(result)[0]['type'], 'Failed')
		self.assertEqual(list(self.home.iterdir()), [])
		self.assertFalse(self.download_log.exists())

	def test_macos_uses_the_published_architecture(self):
		self.env['TESTD_FIXTURE_OS'] = 'Darwin'
		self.env['TESTD_FIXTURE_ARCH'] = 'arm64'
		self.release('aarch64-apple-darwin')
		self.assert_started(self.launch())
		self.assertIn('aarch64-apple-darwin.tar.gz', self.download_log.read_text())

	def test_wget_and_shasum_fallbacks_install_the_verified_binary(self):
		if not (self.commands / 'shasum').exists():
			self.skipTest('shasum is unavailable')
		(self.commands / 'curl').unlink()
		(self.commands / 'sha256sum').unlink(missing_ok=True)
		self.install_fixture('wget', 'download.sh')
		self.release()
		self.assert_started(self.launch())


if __name__ == '__main__':
	unittest.main()
