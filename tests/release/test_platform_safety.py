import pathlib
import shutil
import subprocess
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
POWERSHELL = shutil.which('pwsh') or shutil.which('powershell')


class ResolverSafetyTests(unittest.TestCase):
	@unittest.skipUnless(POWERSHELL, 'PowerShell is required for NRPT behavior fixtures')
	def test_existing_resolver_rules_are_preserved(self):
		path = str(ROOT / 'scripts/testd-elevated.ps1').replace("'", "''")
		command = r'''
function Get-DnsClientNrptRule { $global:rule }
function Remove-DnsClientNrptRule { throw 'fixture: must not remove a rule' }
function Add-DnsClientNrptRule { throw 'fixture: must not replace a rule' }
foreach ($servers in @('192.0.2.1', '127.0.0.1')) {
	$global:rule = [pscustomobject]@{ Namespace = @('.test'); NameServers = @($servers) }
	$failed = $false
	try { & 'HELPER' -Action InstallNrpt -Tld test -NameServer 127.0.0.1 } catch {
		if ($_ -notmatch 'existing NRPT rules conflict') { throw }
		$failed = $true
	}
	if ($failed -ne ($servers -ne '127.0.0.1')) { throw 'incorrect conflict decision' }
	if ($global:rule.NameServers[0] -ne $servers) { throw 'existing rule changed' }
}
'''.replace('HELPER', path)
		result = subprocess.run([POWERSHELL, '-NoProfile', '-NonInteractive', '-Command', command],
			capture_output=True, text=True, timeout=30)
		self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
