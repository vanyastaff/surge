# Source-only native failure runner. Entry protocol is intentionally checked before
# any fixture/account creation. Review and explicit Run are required to execute.
[CmdletBinding()]
param(
    [switch]$Run,
    [string[]]$Case = @(),
    [string]$EvidenceDirectory = (Join-Path $PSScriptRoot '../target/ci-repair-evidence/windows-privileged-provisioner/native-failures')
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$entry = Join-Path $PSScriptRoot 'windows-native-privileged-probe.ps1'
$manifest = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'windows-native-provisioner-failure-cases.json') -Raw | ConvertFrom-Json -AsHashtable
if ($manifest.schema_version -ne 1) { throw 'Failure matrix schema differs' }
$selected = @($manifest.cases | Where-Object { $Case.Count -eq 0 -or $_.id -cin $Case })
foreach ($id in $Case) { if (-not @($selected | Where-Object { $_.id -ceq $id }).Count) { throw "Unknown literal failure case: $id" } }
if (-not $Run) {
    [Console]::Out.WriteLine((@{ status = 'NOT_EXECUTED'; cases = @($selected.id); required_protocol = 1; separate_observer_case = $manifest.separate_observer_case } | ConvertTo-Json -Depth 8))
    return
}
if (-not $IsWindows -or -not [Environment]::Is64BitProcess) { throw 'Native AMD64 Windows required' }
# Parameter metadata is parsed without invoking the entry. Reject incomplete
# integration before a provisioner could create accounts or compile anything.
$tokens = $null
$parseErrors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile($entry, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count) { throw 'Provisioner entry parser errors' }
$parameters = @($ast.ParamBlock.Parameters | ForEach-Object { $_.Name.VariablePath.UserPath })
foreach ($required in @('DescribeFaultProtocol', 'FaultCase')) {
    if ($required -cnotin $parameters) { throw "Native failure runner is not wired: missing entry parameter $required" }
}
$protocolText = & (Join-Path $PSHOME 'pwsh.exe') -NoLogo -NoProfile -NonInteractive -File $entry -DescribeFaultProtocol
if ($LASTEXITCODE -ne 0) { throw 'Failure protocol negotiation failed' }
$protocol = ($protocolText -join "`n") | ConvertFrom-Json -AsHashtable
if ($protocol.schema_version -ne 1 -or $protocol.protocol -cne 'surge-provisioner-fault-v1') { throw 'Failure protocol differs' }
foreach ($test in $selected) {
    if ($test.id -cnotin $protocol.cases) { throw "Entry has no implemented native case: $($test.id)" }
}
[IO.Directory]::CreateDirectory($EvidenceDirectory) | Out-Null
$results = @()
foreach ($test in $selected) {
    $start = [Diagnostics.ProcessStartInfo]::new((Join-Path $PSHOME 'pwsh.exe'))
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($argument in @('-NoLogo', '-NoProfile', '-NonInteractive', '-File', $entry, '-FaultCase', $test.id)) { $start.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $start
    $started = $false
    $observedExit = $false
    $overdue = $false
    try {
        $started = $process.Start()
        if (-not $started) { throw 'Native fault owner failed to start' }
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(900000)) {
            $overdue = $true
            [Console]::Error.WriteLine("FAILED deadline for $($test.id); retaining original provisioner owner until actual exit. External kill means UNKNOWN.")
            # Never kill an owner that may retain unsettled child handles. This
            # failed hold cannot become acceptance success if exit is later seen.
            while (-not $process.WaitForExit(1000)) { }
        }
        $observedExit = $true
        $out = $stdout.GetAwaiter().GetResult()
        $err = $stderr.GetAwaiter().GetResult()
        [IO.File]::WriteAllText((Join-Path $EvidenceDirectory ($test.id + '.stdout.log')), $out)
        [IO.File]::WriteAllText((Join-Path $EvidenceDirectory ($test.id + '.stderr.log')), $err)
        if ($overdue) { throw 'Failure-test owner exceeded deadline; later exit does not clear failure' }
        if ($process.ExitCode -ne 1) { throw "Expected injected terminal exit 1, observed $($process.ExitCode)" }
        $receipts = @($out -split '\r?\n' | Where-Object { $_.StartsWith('{') } | ForEach-Object {
            $value = $_ | ConvertFrom-Json -AsHashtable
            if ($value.ContainsKey('fault_protocol')) { $value }
        })
        if ($receipts.Count -ne 1) { throw 'Exactly one terminal fault receipt required' }
        $receipt = $receipts[0]
        if ($receipt.fault_protocol -cne 'surge-provisioner-fault-v1' -or $receipt.status -cne 'FAIL' -or $receipt.fault_case -cne $test.id -or -not $receipt.fault_reached) { throw 'Missing exact reached fault and terminal failure' }
        if (-not $receipt.parent_token_before -or $receipt.parent_token_before -cne $receipt.parent_token_after) { throw 'Parent token unchanged proof missing' }
        foreach ($mutation in $receipt.mutations) {
            if ($mutation.terminal -cnotin @('Removed', 'Preserved', 'Refused', 'Unknown')) { throw 'Unaccounted acquired mutation' }
            if ($mutation.terminal -cin @('Preserved', 'Unknown') -and -not $mutation.reason) { throw 'Preservation reason missing' }
        }
        foreach ($child in $receipt.children) { if (-not $child.exit_observed) { throw 'Fault owner exited with unobserved child' } }
        if (-not $receipt.cleanup_all_independent_attempted) { throw 'Finalizer aggregation proof missing' }
        $results += @{ case = $test.id; status = 'EXPECTED_FAILURE_OBSERVED'; native_fixture_acceptance = $false }
    } finally {
        # Start failure owns no process; every successful launch must observe exit
        # before releasing the original process wrapper.
        if ($started -and -not $observedExit) {
            $reported = $false
            $holdGate = [object]::new()
            while (-not $observedExit) {
                try { $observedExit = $process.WaitForExit(1000) }
                catch {
                    if (-not $reported) { [Console]::Error.WriteLine('FAILED owner observation; keeping original wrapper alive, settlement UNKNOWN'); $reported = $true }
                    [Threading.Monitor]::Enter($holdGate)
                    try { [void][Threading.Monitor]::Wait($holdGate, 1000) }
                    finally { [Threading.Monitor]::Exit($holdGate) }
                }
            }
        }
        $process.Dispose()
    }
}
[IO.File]::WriteAllText((Join-Path $EvidenceDirectory 'matrix.json'), (@{ schema_version = 1; cases = $results; coverage = 'selected cases only'; separate_observer_case = 'NOT_EXECUTED' } | ConvertTo-Json -Depth 10))
