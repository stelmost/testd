#!/usr/bin/env python3
"""Build free-release archives, Homebrew definitions, and checksums without signing."""

import argparse
import hashlib
import json
import pathlib
import re
import shutil
import tarfile
import tomllib
import urllib.request
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
REPOSITORY = 'https://github.com/stelmost/testd'
TARGETS = {
	'aarch64-apple-darwin': ('macos', 'aarch64'),
	'x86_64-apple-darwin': ('macos', 'x64'),
	'x86_64-unknown-linux-gnu': ('linux', 'x64'),
	'x86_64-unknown-linux-musl': ('linux', 'x64'),
	'x86_64-pc-windows-msvc': ('windows', 'x64'),
}


def digest(path):
	with path.open('rb') as source:
		return hashlib.file_digest(source, 'sha256').hexdigest()


def check_version(version):
	if not re.fullmatch(r'\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?', version):
		raise ValueError(f'invalid release version: {version}')
	manifests = [ROOT / 'Cargo.toml', *sorted(ROOT.glob('crates/*/Cargo.toml'))]
	for path in manifests:
		actual = tomllib.loads(path.read_text())['package']['version']
		if actual != version:
			raise ValueError(f'{path}: expected {version}, found {actual}')
		package = tomllib.loads(path.read_text())['package']
		if 'version' in package.get('metadata', {}).get('packager', {}):
			raise ValueError(f'{path}: packager must inherit the Cargo package version')
	if f'## [{version}]' not in (ROOT / 'CHANGELOG.md').read_text():
		raise ValueError(f'CHANGELOG.md has no released entry for {version}')


def package(args):
	check_version(args.version)
	if args.target.endswith('-musl') and not args.cli_only:
		raise ValueError('musl releases provide only the CLI; use --cli-only')
	platform, arch = TARGETS[args.target]
	binary = args.build_dir / ('testd.exe' if platform == 'windows' else 'testd')
	if not binary.is_file():
		raise FileNotFoundError(binary)
	args.output.mkdir(parents=True, exist_ok=True)
	base = f'testd-{args.version}-{args.target}'
	archive = args.output / (base + ('.zip' if platform == 'windows' else '.tar.gz'))
	if archive.exists():
		raise FileExistsError(archive)
	documents = ['LICENSE', 'NOTICE', 'README.md', 'DEVELOPING.md', 'SECURITY.md', 'docs/quickstart.md']
	files = [(binary, binary.name), *[(ROOT / name, name) for name in documents]]
	files += [(path, str(path.relative_to(ROOT))) for path in ROOT.glob('packaging/systemd/*.service')]
	files += [(ROOT / 'packaging/launchd/dev.testd.host.plist', 'packaging/launchd/dev.testd.host.plist')]
	if platform == 'windows':
		helper = 'scripts/testd-elevated.ps1'
		files.append((ROOT / helper, helper))
		with zipfile.ZipFile(archive, 'w', compression=zipfile.ZIP_DEFLATED) as target:
			for source, name in files:
				target.write(source, f'{base}/{name}')
	else:
		with tarfile.open(archive, 'w:gz') as target:
			for source, name in files:
				target.add(source, arcname=f'{base}/{name}')
	if not args.cli_only:
		copy_bundles(args, platform, arch)
	print(archive)


def copy_bundles(args, platform, arch):
	extensions = {'macos': ['.dmg'], 'linux': ['.AppImage', '.deb'], 'windows': ['.exe']}[platform]
	for extension in extensions:
		candidates = sorted((args.build_dir / 'bundle').glob('*' + extension))
		if len(candidates) != 1:
			raise ValueError(f'expected one {extension} bundle, found {candidates}')
		suffix = '-setup' if platform == 'windows' else ''
		name = f'Testd_{args.version}_{arch}{suffix}{extension}'
		destination = args.output / name
		if destination.exists():
			raise FileExistsError(destination)
		shutil.copy2(candidates[0], destination)


def homebrew(args):
	check_version(args.version)
	arm_sha256 = digest(args.artifacts / f'Testd_{args.version}_aarch64.dmg')
	intel = args.artifacts / f'Testd_{args.version}_x64.dmg'
	# Path.exists() can hide broken links and access errors.
	try:
		intel.lstat()
	except FileNotFoundError:
		intel_support = '  depends_on arch: :arm64'
	else:
		intel_support = f"  on_intel do\n    sha256 '{digest(intel)}'\n  end"
	args.output.mkdir(parents=True, exist_ok=True)
	source = args.artifacts / f'testd-{args.version}-source.tar.gz'
	if not source.exists():
		url = f'{REPOSITORY}/archive/refs/tags/v{args.version}.tar.gz'
		partial = source.with_suffix(source.suffix + '.partial')
		if partial.exists():
			raise FileExistsError(f'{partial}: retain the failed download and use a fresh staging directory')
		with urllib.request.urlopen(url, timeout=120) as response, partial.open('xb') as target:
			shutil.copyfileobj(response, target)
		partial.rename(source)
	values = {
		'VERSION': args.version,
		'SOURCE_SHA256': digest(source),
		'ARM_SHA256': arm_sha256,
		'INTEL_SUPPORT': intel_support,
	}
	for name in ['testd', 'testd-gui']:
		text = (ROOT / f'packaging/homebrew/{name}.rb.in').read_text()
		for key, value in values.items():
			text = text.replace(f'@{key}@', value)
		if re.search(r'@[A-Z0-9_]+@', text):
			raise ValueError(f'unexpanded Homebrew template: {name}')
		path = args.output / f'{name}.rb'
		if path.exists():
			raise FileExistsError(path)
		path.write_text(text)
		print(path)


def scan_sbom():
	manifests = [ROOT / 'Cargo.toml', *sorted(ROOT.glob('crates/*/Cargo.toml'))]
	expected = {tomllib.loads(path.read_text())['package']['name'] for path in manifests}
	documents = {}
	for manifest in manifests:
		for path in manifest.parent.glob('*.json'):
			value = json.loads(path.read_text())
			if not isinstance(value, dict) or value.get('bomFormat') != 'CycloneDX':
				continue
			name = value.get('metadata', {}).get('component', {}).get('name')
			if name not in expected or name in documents:
				raise ValueError(f'unexpected or duplicate crate SBOM: {path}: {name}')
			documents[name] = path
	return expected, documents


def collect_sbom(output):
	expected, documents = scan_sbom()
	if documents.keys() != expected:
		raise ValueError(f'missing workspace SBOMs: {expected - documents.keys()}')
	output.mkdir(parents=True, exist_ok=True)
	for name, source in sorted(documents.items()):
		destination = output / f'{name}.cdx.json'
		if destination.exists():
			raise FileExistsError(destination)
		shutil.copy2(source, destination)
		print(destination)


def checksums(directory):
	path = directory / 'SHA256SUMS'
	if path.exists():
		raise FileExistsError(path)
	files = sorted(file for file in directory.rglob('*') if file.is_file())
	path.write_text(''.join(f'{digest(file)}  {file.relative_to(directory).as_posix()}\n' for file in files))
	print(path)


def main():
	parser = argparse.ArgumentParser(description=__doc__)
	commands = parser.add_subparsers(dest='command', required=True)
	verify = commands.add_parser('verify-version')
	version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['package']['version']
	verify.add_argument('version', nargs='?', default=version)
	notes = commands.add_parser('release-notes')
	notes.add_argument('version')
	build = commands.add_parser('package')
	build.add_argument('--version', required=True)
	build.add_argument('--target', choices=TARGETS, required=True)
	build.add_argument('--build-dir', type=pathlib.Path, required=True)
	build.add_argument('--output', type=pathlib.Path, required=True)
	build.add_argument('--cli-only', action='store_true', help='stage only the CLI archive without tray bundles')
	brew = commands.add_parser('homebrew')
	brew.add_argument('--version', required=True)
	brew.add_argument('--artifacts', type=pathlib.Path, required=True)
	brew.add_argument('--output', type=pathlib.Path, required=True)
	commands.add_parser('require-clean-sbom')
	sbom = commands.add_parser('collect-sbom')
	sbom.add_argument('--output', type=pathlib.Path, required=True)
	sums = commands.add_parser('checksums')
	sums.add_argument('directory', type=pathlib.Path)
	args = parser.parse_args()
	if args.command == 'verify-version':
		check_version(args.version)
	elif args.command == 'release-notes':
		check_version(args.version)
		text = (ROOT / 'CHANGELOG.md').read_text()
		section = text.split(f'## [{args.version}]', 1)[1].split('\n## [', 1)[0]
		print(f'## [{args.version}]' + section)
		print('\nUnsigned distribution: macOS is not Developer ID signed or notarized; Windows artifacts are unsigned.')
		print('An optional macOS ad-hoc resource seal is bundle-integrity evidence only, '
			'not trusted signing or Gatekeeper approval.')
		print('Verify `SHA256SUMS` before installing. Workspace SBOMs accompany the release.')
		print('Linux x86_64 CLI archives are available for GNU libc (`x86_64-unknown-linux-gnu`) '
			'and as a static musl build (`x86_64-unknown-linux-musl`). Desktop packages use GNU libc.')
		print('\nTo install this exact CLI release on Linux or macOS:')
		print('```sh')
		print('curl -fsSLo testd-install.sh https://testd.dev/install.sh')
		print(f"TESTD_VERSION='{args.version}' sh ./testd-install.sh")
		print('```\n')
		print('The installer detects GNU libc on Linux and otherwise selects musl. '
			'Set `TESTD_LIBC=gnu` or `TESTD_LIBC=musl` to override its Linux selection.')
		print('On Linux, macOS, and Windows, stop testd and run `testd uninstall` before removing the app.')
	elif args.command == 'package':
		package(args)
	elif args.command == 'homebrew':
		homebrew(args)
	elif args.command == 'require-clean-sbom':
		_, documents = scan_sbom()
		if documents:
			raise FileExistsError('retain existing SBOMs before regenerating; use a clean checkout/worktree')
	elif args.command == 'collect-sbom':
		collect_sbom(args.output)
	else:
		checksums(args.directory)


if __name__ == '__main__':
	main()
