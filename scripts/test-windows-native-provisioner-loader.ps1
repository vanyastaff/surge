# Regression driver only. The nonprivileged entry seam is deliberately required
# before launching; current baseline lacks it. No account/token fallback allowed.
[CmdletBinding()]
param([switch]$Run)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (-not $Run) { [Console]::Out.WriteLine('NOT_EXECUTED: nonprivileged Windows loader regression'); return }
if (-not $IsWindows -or -not [Environment]::Is64BitProcess) { throw 'AMD64 Windows required' }
$entry = Join-Path $PSScriptRoot 'windows-native-privileged-probe.ps1'
$tokens = $null; $errors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile($entry, [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw 'Entry parser errors' }
$names = @($ast.ParamBlock.Parameters | ForEach-Object { $_.Name.VariablePath.UserPath })
if ('LoaderProbe' -cnotin $names) { throw 'RED: actual entry has no nonprivileged LoaderProbe seam' }
# New fresh process: loaded helper types from another test cannot satisfy proof.
$output = & (Join-Path $PSHOME 'pwsh.exe') -NoLogo -NoProfile -NonInteractive -File $entry -LoaderProbe
if ($LASTEXITCODE -ne 0) { throw 'Nonprivileged loader probe failed' }
$receipt = ($output -join "`n") | ConvertFrom-Json -AsHashtable
if ($receipt.protocol -cne 'surge-provisioner-loader-v1' -or $receipt.status -cne 'PASS') { throw 'Missing exact loader proof' }
foreach ($fact in @('verified_descriptor_bytes', 'empty_location', 'type_assembly_reference_equal', 'same_assembly_after_delete', 'same_type_after_delete', 'same_mvid_after_delete', 'first_invocation_succeeded', 'second_invocation_succeeded', 'bootstrap_absent', 'no_provisioner_constructed', 'no_accounts_created', 'no_privileges_adjusted')) {
    if (-not $receipt.ContainsKey($fact) -or $receipt[$fact] -ne $true) { throw "Missing loader fact $fact" }
}
if ($receipt.dll_sha256 -cnotmatch '^[0-9a-f]{64}$' -or -not $receipt.dll_identity) { throw 'Missing original verified DLL binding' }
$expected = @('Surge.NativeFixtures.dll', 'windows-native-provisioner.cs', 'windows-native-dependencies.json')
if (@($receipt.removed_inventory).Count -ne 3) { throw 'Wrong fixed inventory count' }
for ($i = 0; $i -lt 3; $i++) {
    $row = $receipt.removed_inventory[$i]
    if ($row.component -cne $expected[$i] -or $row.same_descriptor_identity -ne $true -or $row.absent -ne $true) { throw 'Exact DLL-first original-identity deletion missing' }
}
if ($receipt.dll_identity -cne $receipt.removed_inventory[0].identity) { throw 'Deleted DLL differs from byte-loaded DLL' }
[Console]::Out.WriteLine('Loader regression passed; privileged fixture/native Stage1 acceptance remains separate')
