# Closed privileged actions. Ownership records and their backups stay administrator-only.
[CmdletBinding()]
param(
	[Parameter(Mandatory = $true)]
	[ValidateSet('InstallRoot', 'RemoveRoot', 'InstallNrpt', 'RemoveNrpt', 'RemoveInstallation')]
	[string]$Action,
	[string]$CertPath,
	[string]$Thumbprint,
	[string]$Fingerprint,
	[string]$Tld,
	[string]$NameServer,
	[ValidatePattern('^([0-9a-f]{64})?$')]
	[string]$Owner = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$ConfirmPreference = 'None'

function Assert-Tld([string]$value) {
	if ($value -notin @('test', 'testdcheck')) { throw 'invalid TLD' }
}

function Assert-NameServer([string]$value) {
	if ($value -ne '127.0.0.1') { throw 'name server must be the loopback DNS server' }
}

function Assert-RootIdentity([string]$thumbprint, [string]$fingerprint) {
	if ($thumbprint -notmatch '^[0-9A-Fa-f]{40}$') { throw 'invalid certificate thumbprint' }
	if ($fingerprint -notmatch '^[0-9A-Fa-f]{64}$') { throw 'invalid certificate SHA-256 fingerprint' }
}

function Stop-OwnershipConflict([string]$message) {
	$failure = [IO.InvalidDataException]::new($message)
	$failure.Data['testdExitCode'] = 17
	throw $failure
}

function Assert-Owner([string]$owner) {
	if ($owner -notmatch '^[0-9a-f]{64}$') { Stop-OwnershipConflict 'missing private installation ownership ID' }
}

function Assert-Administrator {
	$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
	$principal = [Security.Principal.WindowsPrincipal]::new($identity)
	if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
		throw 'the closed testd helper requires administrator authorization'
	}
}

function Assert-SystemAccess([string]$path) {
	$item = Get-Item -LiteralPath $path -Force -ErrorAction Stop
	if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
		Stop-OwnershipConflict 'ownership path is redirected; left untouched'
	}
	$acl = Get-Acl -LiteralPath $path -ErrorAction Stop
	$trusted = @('S-1-5-18', 'S-1-5-32-544')
	if ($acl.GetOwner([Security.Principal.SecurityIdentifier]).Value -notin $trusted) {
		Stop-OwnershipConflict 'ownership path belongs to another owner; left untouched'
	}
	$raw = [Security.AccessControl.RawSecurityDescriptor]::new($acl.GetSecurityDescriptorBinaryForm(), 0)
	if ($null -eq $raw.DiscretionaryAcl) { Stop-OwnershipConflict 'ownership path has no verifiable access list' }
	foreach ($rule in $acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier])) {
		if ($rule.AccessControlType -eq 'Allow' -and $rule.IdentityReference.Value -notin $trusted) {
			Stop-OwnershipConflict 'ownership path grants access outside SYSTEM and Administrators; left untouched'
		}
	}
}

function New-SystemDirectory([string]$path) {
	if (-not (Test-Path -LiteralPath $path)) {
		$acl = [Security.AccessControl.DirectorySecurity]::new()
		$acl.SetSecurityDescriptorSddlForm('O:BAG:BAD:P(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)')
		if ($PSVersionTable.PSEdition -eq 'Desktop') {
			[IO.Directory]::CreateDirectory($path, $acl) | Out-Null
		} else {
			[IO.FileSystemAclExtensions]::CreateDirectory($acl, $path) | Out-Null
		}
	}
	if (-not (Test-Path -LiteralPath $path -PathType Container)) { throw 'ownership directory conflicts' }
	Assert-SystemAccess $path
}

function Get-OwnershipDirectory {
	$common = [Environment]::GetFolderPath([Environment+SpecialFolder]::CommonApplicationData)
	$item = Get-Item -LiteralPath $common -Force -ErrorAction Stop
	if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'common application data is redirected' }
	$path = Join-Path $common 'testd'
	New-SystemDirectory $path
	$path = Join-Path $path 'ownership'
	New-SystemDirectory $path
	$path
}

function New-SystemFile([string]$path) {
	$acl = [Security.AccessControl.FileSecurity]::new()
	$acl.SetSecurityDescriptorSddlForm('O:BAG:BAD:P(A;;FA;;;BA)(A;;FA;;;SY)')
	$rights = [Security.AccessControl.FileSystemRights]::FullControl
	if ($PSVersionTable.PSEdition -eq 'Desktop') {
		return [IO.FileStream]::new($path, [IO.FileMode]::CreateNew, $rights, [IO.FileShare]::None,
			4096, [IO.FileOptions]::WriteThrough, $acl)
	}
	[IO.FileSystemAclExtensions]::Create([IO.FileInfo]::new($path), [IO.FileMode]::CreateNew,
		$rights, [IO.FileShare]::None, 4096, [IO.FileOptions]::WriteThrough, $acl)
}

function Enter-OwnershipLock {
	$path = Join-Path (Get-OwnershipDirectory) 'operations.lock'
	if (Test-Path -LiteralPath $path) {
		Assert-SystemAccess $path
		return [IO.File]::Open($path, [IO.FileMode]::Open, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
	}
	New-SystemFile $path
}

function Read-OwnershipRecord([string]$key) {
	$path = Join-Path (Get-OwnershipDirectory) ($key + '.json')
	if (-not (Test-Path -LiteralPath $path)) { return $null }
	Assert-SystemAccess $path
	if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw 'ownership record is not a regular file' }
	$record = [IO.File]::ReadAllText($path, [Text.Encoding]::UTF8) | ConvertFrom-Json
	if ($null -eq $record -or $record.Version -ne 1) { Stop-OwnershipConflict 'invalid ownership record; left untouched' }
	$record
}

function Get-OwnershipKeys {
	Get-ChildItem -LiteralPath (Get-OwnershipDirectory) -Filter '*.json' -Force |
		ForEach-Object { [IO.Path]::GetFileNameWithoutExtension($_.Name) }
}

function Get-RecordBackupPath([string]$key) {
	$history = Join-Path (Get-OwnershipDirectory) 'history'
	New-SystemDirectory $history
	Join-Path $history ($key + '-' + [guid]::NewGuid().ToString('N') + '.json')
}

function Write-OwnershipRecord([string]$key, $record) {
	$path = Join-Path (Get-OwnershipDirectory) ($key + '.json')
	$temporary = Join-Path (Get-OwnershipDirectory) ([guid]::NewGuid().ToString('N') + '.next')
	$bytes = [Text.UTF8Encoding]::new($false).GetBytes((ConvertTo-Json -InputObject $record -Depth 12 -Compress))
	$stream = New-SystemFile $temporary
	try {
		$stream.Write($bytes, 0, $bytes.Length)
		$stream.Flush($true)
	} finally { $stream.Dispose() }
	if (Test-Path -LiteralPath $path) {
		Assert-SystemAccess $path
		[IO.File]::Replace($temporary, $path, (Get-RecordBackupPath $key))
	} else {
		[IO.File]::Move($temporary, $path)
	}
}

function Archive-OwnershipRecord([string]$key) {
	$path = Join-Path (Get-OwnershipDirectory) ($key + '.json')
	Assert-SystemAccess $path
	[IO.File]::Move($path, (Get-RecordBackupPath $key))
}

function Assert-OwnedRecord($record, [string]$owner, [string]$kind, [string]$target) {
	Assert-Owner $owner
	if ($record.Owner -cne $owner -or $record.Kind -cne $kind -or $record.Target -cne $target -or
		$record.State -notin @('prepared', 'active', 'removing')) {
		Stop-OwnershipConflict 'system integration is unowned by this installation or its record changed; left untouched'
	}
}

function Get-RuleSnapshot($rule) {
	$properties = [ordered]@{}
	foreach ($property in @($rule.PSObject.Properties | Sort-Object Name)) {
		if ($property.Name -notin @('CimClass', 'CimInstanceProperties', 'CimSystemProperties',
			'PSComputerName', 'RunspaceId', 'PSShowComputerName')) {
			$properties[$property.Name] = $property.Value
		}
	}
	ConvertTo-Json -InputObject $properties -Depth 12 -Compress
}

function Get-NamespaceRules([string]$namespace) {
	@(Get-DnsClientNrptRule -ErrorAction Stop | Where-Object { $_.Namespace -contains $namespace })
}

function Test-CompatibleRule($rule, [string]$namespace, [string]$server) {
	$namespaces = @($rule.Namespace)
	$servers = @($rule.NameServers)
	if ($namespaces.Count -ne 1 -or $namespaces[0] -ne $namespace -or
		$servers.Count -ne 1 -or $servers[0] -ne $server) { return $false }
	foreach ($name in @('DirectAccessEnabled', 'DnsSecEnabled', 'DnsSecValidationRequired')) {
		$property = $rule.PSObject.Properties[$name]
		if ($null -ne $property -and $property.Value) { return $false }
	}
	$true
}

function Install-TestdNrpt([string]$tld, [string]$server, [string]$owner) {
	Assert-Tld $tld
	$tld = $tld.ToLowerInvariant()
	Assert-NameServer $server
	Assert-Owner $owner
	$namespace = '.' + $tld
	$key = 'resolver-' + $tld
	$rules = @(Get-NamespaceRules $namespace)
	$record = Read-OwnershipRecord $key
	foreach ($rule in $rules) {
		if (-not (Test-CompatibleRule $rule $namespace $server)) {
			Stop-OwnershipConflict 'existing NRPT rules conflict; unowned rules were left untouched'
		}
	}
	if ($rules.Count) {
		if ($null -ne $record -and $record.Owner -ceq $owner) {
			Assert-OwnedRecord $record $owner 'nrpt' $namespace
			$current = @($rules | Where-Object { $_.Name -eq $record.Name })
			if ($record.State -cne 'active' -or $current.Count -ne 1 -or
				(Get-RuleSnapshot $current[0]) -cne $record.Snapshot) {
				Stop-OwnershipConflict 'owned NRPT rule changed or setup was interrupted; left untouched'
			}
		}
		return
	}
	if ($null -ne $record) { Assert-OwnedRecord $record $owner 'nrpt' $namespace }
	$record = [pscustomobject]@{
		Version = 1; Owner = $owner; Kind = 'nrpt'; Target = $namespace
		State = 'prepared'; Name = ''; Snapshot = ''
	}
	Write-OwnershipRecord $key $record
	$rule = Add-DnsClientNrptRule -Namespace $namespace -NameServers $server -PassThru -ErrorAction Stop
	$record.Name = [string]$rule.Name
	$live = @(Get-NamespaceRules $namespace | Where-Object { $_.Name -eq $record.Name })
	if ($live.Count -ne 1 -or -not (Test-CompatibleRule $live[0] $namespace $server)) {
		Stop-OwnershipConflict 'NRPT creation could not be verified; ownership remains incomplete'
	}
	$record.Snapshot = Get-RuleSnapshot $live[0]
	$record.State = 'active'
	Write-OwnershipRecord $key $record
}

function Get-NrptRemoval([string]$tld, [string]$owner) {
	Assert-Tld $tld
	$tld = $tld.ToLowerInvariant()
	$namespace = '.' + $tld
	$key = 'resolver-' + $tld
	$all = @(Get-DnsClientNrptRule -ErrorAction Stop)
	$rules = @($all | Where-Object { $_.Namespace -contains $namespace })
	$record = Read-OwnershipRecord $key
	if ($null -eq $record) {
		if ($rules.Count) { Stop-OwnershipConflict 'NRPT rule is unowned; left untouched' }
		return
	}
	Assert-OwnedRecord $record $owner 'nrpt' $namespace
	$current = @($all | Where-Object { $_.Name -eq $record.Name })
	if (-not $current.Count -and $rules.Count) {
		Stop-OwnershipConflict 'a replacement or incompletely recorded NRPT rule is present; left untouched'
	}
	if ($current.Count -and ($record.State -eq 'prepared' -or $current.Count -ne 1 -or $rules.Count -ne 1 -or
		(Get-RuleSnapshot $current[0]) -cne $record.Snapshot)) {
		Stop-OwnershipConflict 'NRPT ownership is incomplete or the rule changed; left untouched'
	}
	[pscustomobject]@{ Key = $key; Namespace = $namespace; Record = $record; HasRule = ($current.Count -ne 0) }
}

function Remove-TestdNrpt([string]$tld, [string]$owner) {
	$target = Get-NrptRemoval $tld $owner
	if ($null -eq $target) { return }
	if (-not $target.HasRule) { Archive-OwnershipRecord $target.Key; return }
	$record = $target.Record
	$record.State = 'removing'
	Write-OwnershipRecord $target.Key $record
	# Keep recovery records if another administrator changed the rule while intent was being recorded.
	$current = @(Get-NamespaceRules $target.Namespace)
	if ($current.Count -ne 1 -or $current[0].Name -ne $record.Name -or
		(Get-RuleSnapshot $current[0]) -cne $record.Snapshot) {
		Stop-OwnershipConflict 'NRPT rule changed while recording removal; left untouched'
	}
	Remove-DnsClientNrptRule -Name $record.Name -Force -Confirm:$false -ErrorAction Stop
	if (@(Get-DnsClientNrptRule -ErrorAction Stop | Where-Object {
		$_.Name -eq $record.Name -or $_.Namespace -contains $target.Namespace
	}).Count) {
		throw 'owned or replacement NRPT rule remains; removal is incomplete'
	}
	Archive-OwnershipRecord $target.Key
}

function Get-CertificateFingerprint($cert) {
	$hash = [Security.Cryptography.SHA256]::Create()
	try { ([BitConverter]::ToString($hash.ComputeHash($cert.RawData))).Replace('-', '').ToLowerInvariant() }
	finally { $hash.Dispose() }
}

function Get-MachineRoots([string]$thumbprint) {
	$store = [Security.Cryptography.X509Certificates.X509Store]::new('Root', 'LocalMachine')
	try {
		$store.Open([Security.Cryptography.X509Certificates.OpenFlags]::OpenExistingOnly)
		@($store.Certificates | Where-Object { $_.Thumbprint -eq $thumbprint })
	} finally { $store.Close() }
}

function Get-RootSnapshot($cert) {
	[Convert]::ToBase64String($cert.Export([Security.Cryptography.X509Certificates.X509ContentType]::SerializedCert))
}

function ConvertTo-ComparableRootSnapshot([string]$snapshot) {
	$bytes = [Convert]::FromBase64String($snapshot)
	$type = [Security.Cryptography.X509Certificates.X509Certificate2]
	if ($type::GetCertContentType($bytes) -ne 'SerializedCert') {
		Stop-OwnershipConflict 'invalid serialized root snapshot; left untouched'
	}
	$copy = $type::new($bytes)
	try {
		$derOnly = $type::new($copy.RawData)
		try {
			# Work on detached copies and preserve all values not derived from this exact certificate.
			[TestdRootStore]::DropDerivedProperties($copy.Handle, $derOnly.Handle)
			Get-RootSnapshot $copy
		} finally { $derOnly.Dispose() }
	} finally { $copy.Dispose() }
}

function Test-RootSnapshot($cert, [string]$snapshot) {
	$live = Get-RootSnapshot $cert
	if ($live -ceq $snapshot) { return $true }
	if ([string]::IsNullOrEmpty($snapshot)) { return $false }
	Initialize-RootStore
	(ConvertTo-ComparableRootSnapshot $live) -ceq (ConvertTo-ComparableRootSnapshot $snapshot)
}

function Initialize-RootStore {
	if (-not ('TestdRootStore' -as [type])) {
		Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
public static class TestdRootStore {
	private const uint SignatureHash = 15;
	private const uint KeyIdentifier = 20;
	private const uint SubjectPublicKeyMd5Hash = 25;
	private const uint InhibitPersist = 0x40000000;

	[DllImport("crypt32.dll", SetLastError = true, EntryPoint = "CertAddCertificateContextToStore")]
	[return: MarshalAs(UnmanagedType.Bool)]
	public static extern bool AddNew(IntPtr store, IntPtr certificate, uint disposition, IntPtr added);

	[DllImport("crypt32.dll", SetLastError = true, EntryPoint = "CertGetCertificateContextProperty")]
	[return: MarshalAs(UnmanagedType.Bool)]
	private static extern bool GetProperty(IntPtr certificate, uint property, byte[] data, ref uint size);

	[DllImport("crypt32.dll", SetLastError = true, EntryPoint = "CertSetCertificateContextProperty")]
	[return: MarshalAs(UnmanagedType.Bool)]
	private static extern bool SetProperty(IntPtr certificate, uint property, uint flags, IntPtr data);

	private static string ReadProperty(IntPtr certificate, uint property) {
		uint size = 0;
		if (!GetProperty(certificate, property, null, ref size)) throw new Win32Exception(Marshal.GetLastWin32Error());
		byte[] value = new byte[size];
		if (!GetProperty(certificate, property, value, ref size)) throw new Win32Exception(Marshal.GetLastWin32Error());
		return Convert.ToBase64String(value);
	}

	public static void DropDerivedProperties(IntPtr certificate, IntPtr derOnly) {
		// Windows can persist these derived values after the certificate is installed.
		foreach (uint property in new uint[] { SignatureHash, KeyIdentifier, SubjectPublicKeyMd5Hash }) {
			if (ReadProperty(certificate, property) == ReadProperty(derOnly, property) &&
				!SetProperty(certificate, property, InhibitPersist, IntPtr.Zero))
				throw new Win32Exception(Marshal.GetLastWin32Error());
		}
	}
}
'@
	}
}

function Add-MachineRoot($cert) {
	# X509Store.Add replaces existing properties. CERT_STORE_ADD_NEW refuses a raced-in certificate instead.
	Initialize-RootStore
	$store = [Security.Cryptography.X509Certificates.X509Store]::new('Root', 'LocalMachine')
	try {
		$store.Open([Security.Cryptography.X509Certificates.OpenFlags]::ReadWrite)
		if (-not [TestdRootStore]::AddNew($store.StoreHandle, $cert.Handle, 1, [IntPtr]::Zero)) {
			throw [ComponentModel.Win32Exception]::new([Runtime.InteropServices.Marshal]::GetLastWin32Error())
		}
	} finally { $store.Close() }
}

function Remove-MachineRoot($cert) {
	$store = [Security.Cryptography.X509Certificates.X509Store]::new('Root', 'LocalMachine')
	try {
		$store.Open([Security.Cryptography.X509Certificates.OpenFlags]::ReadWrite)
		$store.Remove($cert)
	} finally { $store.Close() }
}

function Read-TestdRoot([string]$path, [string]$thumbprint, [string]$fingerprint) {
	Assert-RootIdentity $thumbprint $fingerprint
	$bytes = [IO.File]::ReadAllBytes($path)
	if ([Security.Cryptography.X509Certificates.X509Certificate2]::GetCertContentType($bytes) -ne 'Cert') {
		throw 'expected one public DER certificate, not a key container'
	}
	$cert = [Security.Cryptography.X509Certificates.X509Certificate2]::new($bytes)
	if ($cert.Thumbprint -ne $thumbprint -or (Get-CertificateFingerprint $cert) -ne $fingerprint) {
		throw 'certificate identity mismatch'
	}
	if ($cert.Subject -ne $cert.Issuer -or $cert.HasPrivateKey -or
		$cert.Subject -notmatch '(^|,\s*)CN=testd root(\s|,|$)') { throw 'certificate is not a public testd root' }
	$basic = @($cert.Extensions | Where-Object { $_.Oid.Value -eq '2.5.29.19' })
	if ($basic.Count -ne 1 -or
		-not ([Security.Cryptography.X509Certificates.X509BasicConstraintsExtension]$basic[0]).CertificateAuthority) {
		throw 'certificate is not a CA'
	}
	$cert
}

function Install-TestdRoot([string]$path, [string]$thumbprint, [string]$fingerprint, [string]$owner) {
	Assert-Owner $owner
	$fingerprint = $fingerprint.ToLowerInvariant()
	$cert = Read-TestdRoot $path $thumbprint $fingerprint
	try {
		$key = 'root-' + $fingerprint.ToLowerInvariant()
		$roots = @(Get-MachineRoots $thumbprint)
		$record = Read-OwnershipRecord $key
		if ($roots.Count) {
			if ($roots.Count -ne 1 -or (Get-CertificateFingerprint $roots[0]) -ne $fingerprint) {
				Stop-OwnershipConflict 'the existing root conflicts; left untouched'
			}
			if ($null -ne $record -and $record.Owner -ceq $owner) {
				Assert-OwnedRecord $record $owner 'root' $fingerprint
				if ($record.State -cne 'active' -or -not (Test-RootSnapshot $roots[0] $record.Snapshot)) {
					Stop-OwnershipConflict 'root trust changed or setup was interrupted; left untouched'
				}
			}
			return
		}
		if ($null -ne $record) { Assert-OwnedRecord $record $owner 'root' $fingerprint }
		$record = [pscustomobject]@{
			Version = 1; Owner = $owner; Kind = 'root'; Target = $fingerprint
			State = 'prepared'; Snapshot = ''; Certificate = [Convert]::ToBase64String($cert.RawData)
		}
		Write-OwnershipRecord $key $record
		# Install the validated in-memory certificate, never reopen a mutable caller path.
		Add-MachineRoot $cert
		$roots = @(Get-MachineRoots $thumbprint)
		if ($roots.Count -ne 1 -or (Get-CertificateFingerprint $roots[0]) -ne $fingerprint) {
			Stop-OwnershipConflict 'root trust creation could not be verified; ownership remains incomplete'
		}
		$record.Snapshot = Get-RootSnapshot $roots[0]
		$record.State = 'active'
		Write-OwnershipRecord $key $record
	} finally { $cert.Dispose() }
}

function Get-RootRemoval([string]$thumbprint, [string]$fingerprint, [string]$owner) {
	Assert-RootIdentity $thumbprint $fingerprint
	$fingerprint = $fingerprint.ToLowerInvariant()
	$key = 'root-' + $fingerprint.ToLowerInvariant()
	$roots = @(Get-MachineRoots $thumbprint)
	$record = Read-OwnershipRecord $key
	if ($null -eq $record) {
		if ($roots.Count) { Stop-OwnershipConflict 'root trust is unowned; left untouched' }
		return
	}
	Assert-OwnedRecord $record $owner 'root' $fingerprint
	if ($roots.Count -and ($record.State -eq 'prepared' -or $roots.Count -ne 1 -or
		(Get-CertificateFingerprint $roots[0]) -ne $fingerprint -or
		-not (Test-RootSnapshot $roots[0] $record.Snapshot))) {
		Stop-OwnershipConflict 'root ownership is incomplete or trust changed; left untouched'
	}
	[pscustomobject]@{ Key = $key; Record = $record; HasRoot = ($roots.Count -ne 0) }
}

function Remove-TestdRoot([string]$thumbprint, [string]$fingerprint, [string]$owner) {
	$target = Get-RootRemoval $thumbprint $fingerprint $owner
	if ($null -eq $target) { return }
	if (-not $target.HasRoot) { Archive-OwnershipRecord $target.Key; return }
	$record = $target.Record
	$record.State = 'removing'
	Write-OwnershipRecord $target.Key $record
	# Keep recovery records if another administrator changed trust while intent was being recorded.
	$roots = @(Get-MachineRoots $thumbprint)
	if ($roots.Count -ne 1 -or (Get-CertificateFingerprint $roots[0]) -ne $fingerprint -or
		-not (Test-RootSnapshot $roots[0] $record.Snapshot)) {
		Stop-OwnershipConflict 'root trust changed while recording removal; left untouched'
	}
	Remove-MachineRoot $roots[0]
	if (@(Get-MachineRoots $thumbprint).Count) { throw 'owned root remains; removal is incomplete' }
	Archive-OwnershipRecord $target.Key
}

function Get-RecordedThumbprint($record, [string]$fingerprint, [string]$owner) {
	Assert-OwnedRecord $record $owner 'root' $fingerprint
	$bytes = [Convert]::FromBase64String($record.Certificate)
	if ([Security.Cryptography.X509Certificates.X509Certificate2]::GetCertContentType($bytes) -ne 'Cert') {
		Stop-OwnershipConflict 'ownership record does not contain a public certificate; left untouched'
	}
	$cert = [Security.Cryptography.X509Certificates.X509Certificate2]::new($bytes)
	try {
		if ((Get-CertificateFingerprint $cert) -cne $fingerprint) {
			Stop-OwnershipConflict 'ownership record certificate identity changed; left untouched'
		}
		$cert.Thumbprint
	} finally { $cert.Dispose() }
}

function Remove-TestdInstallation([string]$tld, [string]$thumbprint, [string]$fingerprint, [string]$owner) {
	Assert-Tld $tld
	Assert-RootIdentity $thumbprint $fingerprint
	Assert-Owner $owner
	$tlds = @{ $tld.ToLowerInvariant() = $true }
	$roots = @{ $fingerprint.ToLowerInvariant() = $thumbprint }
	foreach ($key in @(Get-OwnershipKeys)) {
		if ($key -cnotmatch '^(resolver-(test|testdcheck)|root-[a-f0-9]{64})$') {
			Stop-OwnershipConflict 'unrecognized system ownership record; left untouched'
		}
		$record = Read-OwnershipRecord $key
		Assert-Owner $record.Owner
		if ($record.Owner -cne $owner) { continue }
		if ($key.StartsWith('resolver-')) {
			$tlds[$key.Substring(9)] = $true
		} else {
			$recordFingerprint = $key.Substring(5)
			$recordThumbprint = Get-RecordedThumbprint $record $recordFingerprint $owner
			if ($roots.ContainsKey($recordFingerprint) -and $roots[$recordFingerprint] -ne $recordThumbprint) {
				Stop-OwnershipConflict 'local and recorded certificate identities disagree; left untouched'
			}
			$roots[$recordFingerprint] = $recordThumbprint
		}
	}
	foreach ($target in $tlds.Keys) { $null = Get-NrptRemoval $target $owner }
	foreach ($target in $roots.Keys) { $null = Get-RootRemoval $roots[$target] $target $owner }
	foreach ($target in $tlds.Keys) { Remove-TestdNrpt $target $owner }
	foreach ($target in $roots.Keys) { Remove-TestdRoot $roots[$target] $target $owner }
}

if ($Action -like '*Nrpt' -or $Action -eq 'RemoveInstallation') {
	Assert-Tld $Tld
	if ($Action -eq 'InstallNrpt') { Assert-NameServer $NameServer }
}
if ($Action -like '*Root' -or $Action -eq 'RemoveInstallation') {
	Assert-RootIdentity $Thumbprint $Fingerprint
	if ($Action -eq 'InstallRoot' -and [string]::IsNullOrWhiteSpace($CertPath)) { throw 'missing certificate path' }
}
if ($Action -like 'Install*' -or $Action -eq 'RemoveInstallation') { Assert-Owner $Owner }
Assert-Administrator
$lock = $null
try {
	$lock = Enter-OwnershipLock
	switch ($Action) {
		'InstallNrpt' { Install-TestdNrpt $Tld $NameServer $Owner }
		'RemoveNrpt' { Remove-TestdNrpt $Tld $Owner }
		'InstallRoot' { Install-TestdRoot $CertPath $Thumbprint $Fingerprint $Owner }
		'RemoveRoot' { Remove-TestdRoot $Thumbprint $Fingerprint $Owner }
		'RemoveInstallation' { Remove-TestdInstallation $Tld $Thumbprint $Fingerprint $Owner }
	}
} catch {
	Write-Error -ErrorRecord $_ -ErrorAction Continue
	if ($_.Exception.Data.Contains('testdExitCode')) { exit ([int]$_.Exception.Data['testdExitCode']) }
	exit 1
} finally { if ($null -ne $lock) { $lock.Dispose() } }
