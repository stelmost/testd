"""Exercise AppImage startup with real host and bundled environments."""
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tomllib
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]


@unittest.skipIf(os.name == 'nt', 'not applicable: AppImage hook fixtures require Unix execution')
@unittest.skipUnless(shutil.which('bash'), 'Bash is required for AppImage hook checks')
class AppImageTests(unittest.TestCase):
	def hook_environment(self, original):
		metadata = tomllib.loads((ROOT / 'crates/testd-gui/Cargo.toml').read_text())
		appimage = metadata['package']['metadata']['packager']['appimage']
		self.assertNotIn('gtk', appimage.get('linuxdeploy-plugins', {}))
		self.assertEqual(len(appimage['files']), 1)
		source, target = next(iter(appimage['files'].items()))
		self.assertTrue(target.startswith('apprun-hooks/'))
		hook = str((ROOT / 'crates/testd-gui' / source).resolve())
		script = '''
source "$1"
export PYTHONHOME=/bundle/usr PYTHONPATH=/bundle/pyshared LD_LIBRARY_PATH=/bundle/lib
# Read the saved host values without using the dummy bundled Python runtime.
unset PYTHONHOME PYTHONPATH
exec "$2" -c 'import json, os; print(json.dumps(dict(os.environ)))'
'''
		environment = {'PATH': os.defpath, 'APPIMAGE': '/space and π/Testd.AppImage', **original}
		result = subprocess.run(['bash', '-eu', '-c', script, 'test-hook', hook, sys.executable],
			env=environment, capture_output=True, text=True, check=True, timeout=10)
		return json.loads(result.stdout)

	def test_wayland_selection_is_not_overridden_and_absent_host_variables_stay_distinct(self):
		environment = self.hook_environment({'WAYLAND_DISPLAY': 'wayland-0'})
		self.assertNotIn('GDK_BACKEND', environment)
		self.assertEqual(environment['TESTD_APPIMAGE_UNSET_GDK_BACKEND'], '1')
		self.assertEqual(environment['TESTD_APPIMAGE_UNSET_PYTHONHOME'], '1')
		self.assertEqual(environment['TESTD_APPIMAGE_UNSET_PYTHONPATH'], '1')
		self.assertEqual(environment['TESTD_APPIMAGE_ORIGINAL_PATH'], os.defpath)
		self.assertEqual(environment['LD_LIBRARY_PATH'], '/bundle/lib')

	def test_explicit_display_and_empty_runtime_values_are_preserved(self):
		for backend in ('wayland', 'x11', ''):
			with self.subTest(backend=backend):
				environment = self.hook_environment({'GDK_BACKEND': backend, 'PYTHONHOME': '',
					'PYTHONPATH': '/host/space and π', 'GTK_THEME': 'host-theme'})
				self.assertEqual(environment['GDK_BACKEND'], backend)
				self.assertEqual(environment['TESTD_APPIMAGE_ORIGINAL_PYTHONHOME'], '')
				self.assertNotIn('TESTD_APPIMAGE_UNSET_PYTHONHOME', environment)
				self.assertEqual(environment['TESTD_APPIMAGE_ORIGINAL_PYTHONPATH'], '/host/space and π')
				self.assertEqual(environment['TESTD_APPIMAGE_ORIGINAL_GTK_THEME'], 'host-theme')


if __name__ == '__main__':
	unittest.main()
