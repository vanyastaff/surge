# Pure managed source gate. No account creation, native token adjustment or actor launch.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$source = Join-Path $PSScriptRoot 'windows-native-provisioner.cs'
$tests = Join-Path $PSScriptRoot 'windows-native-provisioner-tests.cs'
$entry = Join-Path $PSScriptRoot 'windows-native-privileged-probe.ps1'
$tokens = $null
$errors = $null
[void][Management.Automation.Language.Parser]::ParseFile($entry, [ref]$tokens, [ref]$errors)
if ($errors.Count -ne 0) {
    foreach ($error in $errors) { [Console]::Error.WriteLine($error.ToString()) }
    throw 'Privileged provisioner PowerShell syntax failed'
}
Add-Type -Path @($source, $tests)
if ([ProvisionerContractTests]::Main() -ne 0) { throw 'Managed contract tests failed' }
[Console]::Out.WriteLine('PowerShell parse and managed contracts passed; Windows native/failure-injection acceptance remains separate')
