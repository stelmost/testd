# Load only policy functions; every OS-facing operation is replaced before any case runs.
[CmdletBinding()]
param([Parameter(Mandatory = $true)][string]$Helper)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$ConfirmPreference = 'None'
$tokens = $null
$errors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile($Helper, [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw ($errors.Message -join '; ') }
$backends = @('Assert-Administrator', 'Assert-SystemAccess', 'New-SystemDirectory', 'Get-OwnershipDirectory',
	'New-SystemFile', 'Enter-OwnershipLock', 'Read-OwnershipRecord', 'Get-RecordBackupPath', 'Write-OwnershipRecord',
	'Archive-OwnershipRecord', 'Get-MachineRoots', 'Add-MachineRoot', 'Remove-MachineRoot', 'Get-RootSnapshot')
foreach ($definition in $ast.FindAll({ $args[0] -is [Management.Automation.Language.FunctionDefinitionAst] }, $true)) {
	if ($definition.Name -notin $backends) { . ([scriptblock]::Create($definition.Extent.Text)) }
}

$owner = 'a' * 64
$otherOwner = 'b' * 64
$script:cases = 0

function Assert([bool]$condition, [string]$message) {
	if (-not $condition) { throw $message }
}

function Assert-Fails([scriptblock]$action, [string]$pattern) {
	$failed = $false
	try { & $action } catch {
		if ($_.Exception.Message -notmatch $pattern) { throw }
		$failed = $true
	}
	Assert $failed ('expected failure: ' + $pattern)
}

function Reset-Fixture {
	$script:rules = @()
	$script:records = @{}
	$script:history = @()
	$script:roots = @()
	$script:added = 0
	$script:removed = 0
	$script:failActivation = $false
	$script:failRemoval = $false
	$script:removalRecordHook = $null
	$script:rootTag = 'original properties'
	$script:raceRoot = $false
	$script:cases++
}

function Copy-Record($record) {
	ConvertTo-Json -InputObject $record -Depth 12 -Compress | ConvertFrom-Json
}

function Read-OwnershipRecord([string]$key) { $script:records[$key] }
function Write-OwnershipRecord([string]$key, $record) {
	if ($script:failActivation -and $record.State -eq 'active') { throw 'fixture ownership write failed' }
	if ($script:records.ContainsKey($key)) { $script:history += Copy-Record $script:records[$key] }
	$script:records[$key] = Copy-Record $record
	if ($record.State -eq 'removing' -and $null -ne $script:removalRecordHook) {
		& $script:removalRecordHook
	}
}
function Archive-OwnershipRecord([string]$key) {
	$script:history += Copy-Record $script:records[$key]
	$script:records.Remove($key)
}

function Get-DnsClientNrptRule { [CmdletBinding()]param(); $script:rules }
function Add-DnsClientNrptRule {
	[CmdletBinding()]param([string[]]$Namespace, [string[]]$NameServers, [switch]$PassThru)
	Assert ($script:records['resolver-testdcheck'].State -eq 'prepared') 'missing durable NRPT intent'
	$script:added++
	$rule = [pscustomobject]@{
		Name = [guid]::NewGuid().ToString(); Namespace = $Namespace; NameServers = $NameServers -join ','
		DisplayName = ''; Comment = ''; DirectAccessEnabled = $false; DnsSecEnabled = $false
		DnsSecValidationRequired = $false; NameEncoding = 'Utf8WithoutMapping'
	}
	$script:rules += $rule
	if ($PassThru) { $rule }
}
function Remove-DnsClientNrptRule {
	[CmdletBinding(SupportsShouldProcess = $true)]param([string]$Name, [switch]$Force)
	Assert ($Force -and $PSBoundParameters.ContainsKey('Confirm') -and -not $PSBoundParameters['Confirm']) `
		'removal can prompt for confirmation'
	Assert ($script:records['resolver-testdcheck'].State -eq 'removing') 'missing durable NRPT removal intent'
	if ($script:failRemoval) { throw 'fixture removal failed' }
	$script:rules = @($script:rules | Where-Object { $_.Name -ne $Name })
	$script:removed++
}

function Get-MachineRoots([string]$thumbprint) { @($script:roots | Where-Object { $_.Thumbprint -eq $thumbprint }) }
function Get-RootSnapshot($cert) { [Convert]::ToBase64String($cert.RawData) + ':' + $script:rootTag }
function Add-MachineRoot($cert) {
	$key = 'root-' + (Get-CertificateFingerprint $cert)
	Assert ($script:records[$key].State -eq 'prepared') 'missing durable root intent'
	if ($script:raceRoot) {
		$script:roots += [Security.Cryptography.X509Certificates.X509Certificate2]::new($cert.RawData)
		throw 'fixture new-root collision'
	}
	Assert (-not $script:roots.Count) 'must not replace an existing root'
	$script:roots += [Security.Cryptography.X509Certificates.X509Certificate2]::new($cert.RawData)
	$script:added++
}
function Remove-MachineRoot($cert) {
	$key = 'root-' + (Get-CertificateFingerprint $cert)
	Assert ($script:records[$key].State -eq 'removing') 'missing durable root removal intent'
	if ($script:failRemoval) { throw 'fixture removal failed' }
	$script:roots = @($script:roots | Where-Object { $_.Thumbprint -ne $cert.Thumbprint })
	$script:removed++
}

foreach ($name in @('Get-DnsClientNrptRule', 'Add-DnsClientNrptRule', 'Remove-DnsClientNrptRule')) {
	Assert ((Get-Command $name).CommandType -eq 'Function') ('missing cmdlet mock: ' + $name)
}

Reset-Fixture
Install-TestdNrpt testdcheck 127.0.0.1 $owner
Assert ($script:added -eq 1 -and $script:records['resolver-testdcheck'].State -eq 'active') 'not recorded'
Install-TestdNrpt testdcheck 127.0.0.1 $owner
Assert ($script:added -eq 1) 'idempotent setup added a rule'
Remove-TestdNrpt testdcheck $owner
Remove-TestdNrpt testdcheck $owner
Assert ($script:removed -eq 1 -and -not $script:rules.Count) 'repeat removal changed another rule'

Reset-Fixture
$script:rules = @([pscustomobject]@{ Name = 'preexisting'; Namespace = '.testdcheck'; NameServers = '127.0.0.1' })
Install-TestdNrpt testdcheck 127.0.0.1 $owner
Assert ($script:added -eq 0 -and $script:records.Count -eq 0) 'compatible unowned rule was adopted'
Assert-Fails { Remove-TestdNrpt testdcheck $owner } 'unowned'
Assert ($script:removed -eq 0) 'unowned rule was deleted'

Reset-Fixture
$script:rules = @([pscustomobject]@{ Name = 'conflict'; Namespace = '.testdcheck'; NameServers = '192.0.2.1' })
Assert-Fails { Install-TestdNrpt testdcheck 127.0.0.1 $owner } 'conflict'
Assert ($script:added -eq 0 -and $script:records.Count -eq 0) 'conflict changed state'

Reset-Fixture
Install-TestdNrpt testdcheck 127.0.0.1 $owner
Assert-Fails { Remove-TestdNrpt testdcheck $otherOwner } 'unowned'
$script:rules[0].DnsSecValidationRequired = $true
Assert-Fails { Remove-TestdNrpt testdcheck $owner } 'changed'
Assert ($script:removed -eq 0) 'changed or foreign rule was removed'

foreach ($change in @(
	{ $script:rules[0].Comment = 'changed while recording removal' },
	{
		$replacement = Copy-Record $script:rules[0]
		$replacement.Name = 'replacement'
		$script:rules = @($replacement)
	}
)) {
	Reset-Fixture
	Install-TestdNrpt testdcheck 127.0.0.1 $owner
	$script:removalRecordHook = $change
	Assert-Fails { Remove-TestdNrpt testdcheck $owner } 'changed while recording removal'
	Assert ($script:removed -eq 0) 'a rule changed after removal intent was deleted'
	Assert ($script:records.Count -eq 1 -and
		$script:records['resolver-testdcheck'].State -eq 'removing') 'NRPT recovery record was lost'
	Assert ($script:history[-1].State -eq 'active') 'NRPT active-state backup was lost'
}

Reset-Fixture
Install-TestdNrpt testdcheck 127.0.0.1 $owner
$script:rules[0].Name = 'replacement'
Assert-Fails { Remove-TestdNrpt testdcheck $owner } 'replacement'
Assert ($script:removed -eq 0) 'replacement was deleted'

Reset-Fixture
Install-TestdNrpt testdcheck 127.0.0.1 $owner
$script:rules += [pscustomobject]@{ Name = 'unrelated'; Namespace = '.example'; NameServers = '192.0.2.1' }
Remove-TestdNrpt testdcheck $owner
Assert ($script:rules.Count -eq 1 -and $script:rules[0].Name -eq 'unrelated') 'unrelated rule was deleted'

Reset-Fixture
$script:failActivation = $true
Assert-Fails { Install-TestdNrpt testdcheck 127.0.0.1 $owner } 'fixture ownership write failed'
$script:failActivation = $false
Assert-Fails { Remove-TestdNrpt testdcheck $owner } 'replacement|incomplete'
Assert-Fails { Install-TestdNrpt testdcheck 127.0.0.1 $owner } 'interrupted'
Assert ($script:rules.Count -eq 1 -and $script:removed -eq 0) 'incomplete ownership was adopted'

Reset-Fixture
Install-TestdNrpt testdcheck 127.0.0.1 $owner
$script:failRemoval = $true
Assert-Fails { Remove-TestdNrpt testdcheck $owner } 'fixture removal failed'
$script:failRemoval = $false
Remove-TestdNrpt testdcheck $owner
Assert ($script:removed -eq 1 -and $script:records.Count -eq 0) 'failed removal could not be retried'

Reset-Fixture
Assert-Fails { Install-TestdNrpt com 127.0.0.1 $owner } 'invalid TLD'
Assert-Fails { Install-TestdNrpt testdcheck 192.0.2.1 $owner } 'loopback'
Assert-Fails { Install-TestdNrpt testdcheck 127.0.0.1 '' } 'ownership ID'
Assert ($script:records.Count -eq 0 -and $script:added -eq 0) 'invalid arguments changed state'

$directory = Join-Path ([IO.Path]::GetTempPath()) ('testd-helper-fixture-' + [guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($directory) | Out-Null
$key = [Security.Cryptography.ECDsa]::Create([Security.Cryptography.ECCurve+NamedCurves]::nistP256)
$request = [Security.Cryptography.X509Certificates.CertificateRequest]::new(
	'CN=testd root', $key, [Security.Cryptography.HashAlgorithmName]::SHA256)
$request.CertificateExtensions.Add([Security.Cryptography.X509Certificates.X509BasicConstraintsExtension]::new(
	$true, $false, 0, $true))
$certificate = $request.CreateSelfSigned([DateTimeOffset]::UtcNow.AddMinutes(-1), [DateTimeOffset]::UtcNow.AddDays(1))
try {
	$path = Join-Path $directory 'root.cer'
	[IO.File]::WriteAllBytes($path, $certificate.Export([Security.Cryptography.X509Certificates.X509ContentType]::Cert))
	$thumbprint = $certificate.Thumbprint
	$fingerprint = Get-CertificateFingerprint $certificate
	$recordKey = 'root-' + $fingerprint

	Reset-Fixture
	Install-TestdRoot $path $thumbprint $fingerprint $owner
	Install-TestdRoot $path $thumbprint $fingerprint $owner
	Assert ($script:added -eq 1) 'repeat root install replaced a certificate'
	Remove-TestdRoot $thumbprint $fingerprint $owner
	Remove-TestdRoot $thumbprint $fingerprint $owner
	Assert ($script:removed -eq 1 -and $script:roots.Count -eq 0) 'repeat root removal changed state'

	Reset-Fixture
	$script:roots = @([Security.Cryptography.X509Certificates.X509Certificate2]::new($certificate.RawData))
	Install-TestdRoot $path $thumbprint $fingerprint $owner
	Assert ($script:records.Count -eq 0 -and $script:added -eq 0) 'preexisting root was adopted'
	Assert-Fails { Remove-TestdRoot $thumbprint $fingerprint $owner } 'unowned'
	Assert ($script:removed -eq 0) 'unowned root was deleted'

	Reset-Fixture
	Install-TestdRoot $path $thumbprint $fingerprint $owner
	Assert-Fails { Remove-TestdRoot $thumbprint $fingerprint $otherOwner } 'unowned'
	$script:rootTag = 'later trust-property edit'
	Assert-Fails { Remove-TestdRoot $thumbprint $fingerprint $owner } 'changed'
	Assert ($script:removed -eq 0) 'foreign or changed root was deleted'

	foreach ($change in @(
		{ $script:rootTag = 'changed while recording removal' },
		{ $script:roots = @() }
	)) {
		Reset-Fixture
		Install-TestdRoot $path $thumbprint $fingerprint $owner
		$script:removalRecordHook = $change
		Assert-Fails { Remove-TestdRoot $thumbprint $fingerprint $owner } 'changed while recording removal'
		Assert ($script:removed -eq 0) 'root removal used a stale certificate snapshot'
		Assert ($script:records.Count -eq 1 -and
			$script:records[$recordKey].State -eq 'removing') 'root recovery record was lost'
		Assert ($script:history[-1].State -eq 'active') 'root active-state backup was lost'
	}

	Reset-Fixture
	$script:failActivation = $true
	Assert-Fails { Install-TestdRoot $path $thumbprint $fingerprint $owner } 'fixture ownership write failed'
	$script:failActivation = $false
	Assert-Fails { Remove-TestdRoot $thumbprint $fingerprint $owner } 'incomplete'
	Assert-Fails { Install-TestdRoot $path $thumbprint $fingerprint $owner } 'interrupted'
	Assert ($script:removed -eq 0) 'incomplete root ownership was adopted'

	Reset-Fixture
	Install-TestdRoot $path $thumbprint $fingerprint $owner
	$script:failRemoval = $true
	Assert-Fails { Remove-TestdRoot $thumbprint $fingerprint $owner } 'fixture removal failed'
	$script:failRemoval = $false
	Remove-TestdRoot $thumbprint $fingerprint $owner
	Assert ($script:removed -eq 1 -and $script:records.Count -eq 0) 'root removal retry failed'

	Reset-Fixture
	$script:raceRoot = $true
	Assert-Fails { Install-TestdRoot $path $thumbprint $fingerprint $owner } 'new-root collision'
	Assert-Fails { Remove-TestdRoot $thumbprint $fingerprint $owner } 'incomplete'
	Assert ($script:roots.Count -eq 1 -and $script:removed -eq 0) 'raced-in root was adopted or removed'

	Reset-Fixture
	Assert-Fails { Install-TestdRoot $path ('0' * 40) $fingerprint $owner } 'identity mismatch'
	Assert-Fails { Install-TestdRoot $path $thumbprint ('0' * 64) $owner } 'identity mismatch'
	Assert ($script:records.Count -eq 0 -and $script:added -eq 0) 'wrong certificate changed state'
} finally {
	$certificate.Dispose()
	$key.Dispose()
	Remove-Item -LiteralPath $directory -Recurse -Force
}
Write-Output ('PASS: ' + $script:cases + ' isolated ownership cases; no NRPT or machine trust changes')
