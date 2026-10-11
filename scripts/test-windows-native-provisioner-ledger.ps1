# Test-only independent oracle. Does not change the provisioner or native runner.
[CmdletBinding()]
param([switch]$SelfTest, [string]$ReceiptPath, [string]$Case = 'assembly-emitted')
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
function Assert-FaultPrefix($Expected, $Receipt) {
    foreach ($name in @('mutations', 'resources', 'children')) {
        if (-not $Receipt.ContainsKey($name) -or $null -eq $Receipt[$name]) { throw "Missing $name" }
        $rows = @($Receipt[$name]); $wanted = @($Expected[$name])
        if ($rows.Count -ne $wanted.Count) { throw "Wrong $name count" }
        for ($i = 0; $i -lt $wanted.Count; $i++) {
            $row = $rows[$i]; $want = $wanted[$i]
            if ($row.ordinal -ne ($i + 1)) { throw "Noncontiguous $name ordinal" }
            foreach ($key in $want.Keys) {
                if (-not $row.ContainsKey($key) -or [string]$row[$key] -cne [string]$want[$key]) { throw "Wrong $name $key" }
            }
            if ($name -ceq 'mutations') {
                if ($row.terminal -cnotin @('Removed', 'Preserved', 'Refused', 'Unknown')) { throw 'Nonterminal mutation' }
                if ($row.terminal -ceq 'Removed' -and $row.absence_observed -ne $true) { throw 'Missing actual absence' }
                if ($row.terminal -cin @('Preserved', 'Unknown') -and -not $row.reason) { throw 'Missing preservation cause' }
            }
            if ($name -ceq 'resources' -and $row.disposition -cnotin @('Closed', 'Preserved', 'Unknown')) { throw 'Resource disposition missing' }
            if ($name -ceq 'children' -and $row.exit_observed -ne $true) { throw 'Unsettled child' }
        }
    }
    $target = @($Receipt.mutations)[$Expected.target_mutation_ordinal - 1]
    if ($target.outcome -cne $Expected.target_outcome -or $target.validated -ne $Expected.target_validated) { throw 'Wrong target acquisition stage' }
    if ($Receipt.cleanup_all_independent_attempted -ne $true) { throw 'Independent cleanup missing' }
}
$manifest = Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot 'windows-native-provisioner-failure-cases.json') | ConvertFrom-Json -AsHashtable
$selected = @($manifest.cases | Where-Object { $_.id -ceq $Case })
if ($selected.Count -ne 1 -or -not $selected[0].ContainsKey('expected_prefix')) { throw 'Literal prefix not frozen; case not executable' }
$expected = $selected[0].expected_prefix
if ($SelfTest) {
    # Literal independent example, deliberately not generated from live emitter.
    $literal = '{"mutations":[{"ordinal":1,"kind":"directory","role":"bootstrap","parent":"CommonApplicationData","component":"surge-provisioner-code-{run_nonce}","outcome":"Created","validated":false,"terminal":"Removed","absence_observed":true},{"ordinal":2,"kind":"file","role":"helper-source","parent":1,"component":"windows-native-provisioner.cs","outcome":"Created","validated":false,"terminal":"Removed","absence_observed":true},{"ordinal":3,"kind":"file","role":"dependency-classification","parent":1,"component":"windows-native-dependencies.json","outcome":"Created","validated":false,"terminal":"Removed","absence_observed":true},{"ordinal":4,"kind":"file","role":"helper-assembly","parent":1,"component":"Surge.NativeFixtures.dll","outcome":"Created","validated":false,"terminal":"Removed","absence_observed":true}],"resources":[{"ordinal":1,"kind":"stream","role":"helper-source-pin","disposition":"Closed"},{"ordinal":2,"kind":"stream","role":"dependency-classification-pin","disposition":"Closed"}],"children":[],"cleanup_all_independent_attempted":true}'
    if ($Case -cne 'assembly-emitted') { throw 'SelfTest uses literal assembly-emitted fixture' }
    Assert-FaultPrefix $expected ($literal | ConvertFrom-Json -AsHashtable)
    $negatives = @(
        { param($r) $r.Remove('mutations') },
        { param($r) $r.mutations = @() },
        { param($r) $r.mutations = @($r.mutations[0..1]) },
        { param($r) $r.mutations[1].ordinal = 1 },
        { param($r) $r.mutations[1].ordinal = 4 },
        { param($r) $r.mutations = @($r.mutations[1], $r.mutations[0], $r.mutations[2], $r.mutations[3]) },
        { param($r) $r.mutations[3].kind = 'directory' },
        { param($r) $r.mutations[3].parent = 2 },
        { param($r) $r.mutations[3].component = 'other.dll' },
        { param($r) $r.mutations[3].terminal = 'Created' },
        { param($r) $r.mutations[3].absence_observed = $false },
        { param($r) $r.Remove('resources') },
        { param($r) $r.resources = @() },
        { param($r) $r.resources = @($r.resources[0]) },
        { param($r) $r.resources[1].disposition = 'Acquired' },
        { param($r) $r.Remove('children') },
        { param($r) $r.cleanup_all_independent_attempted = $false }
    )
    foreach ($mutate in $negatives) {
        $receipt = $literal | ConvertFrom-Json -AsHashtable
        & $mutate $receipt | Out-Null
        $refused = $false
        try { Assert-FaultPrefix $expected $receipt } catch { $refused = $true }
        if (-not $refused) { throw 'Negative ledger accepted' }
    }
    # Independent nonempty child prefix exercises missing/empty/truncated and
    # false settlement; bootstrap hooks intentionally expect no child at all.
    $childExpected = $expected | ConvertTo-Json -Depth 12 | ConvertFrom-Json -AsHashtable
    $childExpected.children = @(@{ ordinal = 1; kind = 'process'; role = 'admin-default-owner' })
    $withChild = $literal | ConvertFrom-Json -AsHashtable
    $withChild.children = @(@{ ordinal = 1; kind = 'process'; role = 'admin-default-owner'; exit_observed = $true })
    Assert-FaultPrefix $childExpected $withChild
    foreach ($mode in @('missing', 'empty', 'unobserved')) {
        $bad = $withChild | ConvertTo-Json -Depth 12 | ConvertFrom-Json -AsHashtable
        if ($mode -ceq 'missing') { $bad.Remove('children') | Out-Null }
        elseif ($mode -ceq 'empty') { $bad.children = @() }
        else { $bad.children[0].exit_observed = $false }
        $refused = $false
        try { Assert-FaultPrefix $childExpected $bad } catch { $refused = $true }
        if (-not $refused) { throw 'Negative child ledger accepted' }
    }
    [Console]::Out.WriteLine('Pure ledger oracle self-tests passed; no provisioner/native acceptance')
} elseif ($ReceiptPath) {
    Assert-FaultPrefix $expected (Get-Content -Raw -LiteralPath $ReceiptPath | ConvertFrom-Json -AsHashtable)
} else {
    [Console]::Out.WriteLine('NOT_EXECUTED: specify SelfTest or explicit ReceiptPath; bootstrap subset only')
}
