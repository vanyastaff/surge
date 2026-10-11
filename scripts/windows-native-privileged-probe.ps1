# Standalone Stage 1 fixture 1/2 provisioner. Existing standard-user probe is separate.
# This entry must run only after source/security review, on a dedicated Windows CI runner.
[CmdletBinding()]
param()
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (-not $IsWindows -or -not [Environment]::Is64BitProcess) { throw 'AMD64 Windows PowerShell 7 required' }
$runNonce = [Guid]::NewGuid().ToString('N')
$ledger = [ordered]@{
    schema_version = 1; run_nonce = $runNonce; status = 'RUNNING'
    candidate_revision = $null; candidate_manifest = $null; helper_sha256 = $null; helper_assembly_sha256 = $null; toolchain = @()
    dependency_classification_sha256 = $null; accounts = @(); profiles = @(); native_errors = @()
    children = @(); observations = @(); resources = @(); errors = @(); cleanup_errors = @()
    preservation = $false; external_interruption_means = 'settlement UNKNOWN; no surviving provisioner-handle claim'
}
$native = $null
$root = $null
$standardData = $null
$ordinary = $null
$trees = @()
$standardProcesses = @()
$standardSettled = [Collections.Generic.HashSet[Diagnostics.Process]]::new()
$standardObservationErrors = [Collections.Generic.HashSet[Diagnostics.Process]]::new()
$failedObservationGate = [object]::new()
$originalEnvironment = @{}
$environmentRestored = $true
$parentTokenBefore = $null
$checkpointWritten = $false
$standardPassword = $null
$foreignPassword = $null
$bootstrapPath = $null
$bootstrapCreated = $false
$bootstrapIdentity = $null
$assemblyPins = @()

function Add-Failure([string]$Phase, $ErrorRecord, [bool]$Cleanup = $false) {
    $entry = [ordered]@{ phase = $Phase; message = $ErrorRecord.Exception.Message }
    if ($Cleanup) { $ledger.cleanup_errors += $entry } else { $ledger.errors += $entry }
    $ledger.status = 'FAIL'
}
function Try-Cleanup([string]$Phase, [scriptblock]$Action) {
    try { & $Action } catch { Add-Failure $Phase $_ $true }
}
function Restore-Environment {
    if (-not $script:environmentRestored) {
        $failures = [Collections.Generic.List[string]]::new()
        foreach ($name in @([Environment]::GetEnvironmentVariables('Process').Keys)) {
            try { [Environment]::SetEnvironmentVariable([string]$name, $null, 'Process') }
            catch { $failures.Add('clear ' + $name + ': ' + $_.Exception.Message) }
        }
        foreach ($entry in $script:originalEnvironment.GetEnumerator()) {
            try { [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value, 'Process') }
            catch { $failures.Add('restore ' + $entry.Key + ': ' + $_.Exception.Message) }
        }
        if ($failures.Count) { throw ('Environment restoration incomplete: ' + ($failures -join '; ')) }
        $script:environmentRestored = $true
    }
}
function Get-UnsettledStandard {
    foreach ($process in $standardProcesses) {
        if ($standardSettled.Contains($process)) { continue }
        try {
            if ($process.HasExited) { [void]$standardSettled.Add($process); continue }
        } catch {
            if ($standardObservationErrors.Add($process)) { Add-Failure 'standard-exit-observation' $_ $true }
        }
        Write-Output $process
    }
}
function Hold-StandardUntilExit($Processes) {
    foreach ($process in $Processes) {
        while (-not $standardSettled.Contains($process)) {
            try {
                if ($process.WaitForExit(1000)) { [void]$standardSettled.Add($process) }
            } catch {
                if ($standardObservationErrors.Add($process)) { Add-Failure 'retained-standard-exit-observation' $_ $true }
                # A failed observation never authorizes closing the original owner.
                [Threading.Monitor]::Enter($failedObservationGate)
                try { [void][Threading.Monitor]::Wait($failedObservationGate, 1000) }
                finally { [Threading.Monitor]::Exit($failedObservationGate) }
            }
        }
    }
}
function Set-ActorEnvironment($Values) {
    $script:originalEnvironment = @{}
    foreach ($entry in [Environment]::GetEnvironmentVariables('Process').GetEnumerator()) { $script:originalEnvironment[$entry.Key] = $entry.Value }
    $script:environmentRestored = $false
    foreach ($name in @([Environment]::GetEnvironmentVariables('Process').Keys)) { [Environment]::SetEnvironmentVariable([string]$name, $null, 'Process') }
    foreach ($entry in $Values.GetEnumerator()) { [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value, 'Process') }
}
function Update-NativeReceipt {
    if ($null -ne $native) {
        $ledger.resources = @($native.Resources)
        $ledger.children = @($native.Children | ForEach-Object {
            [ordered]@{ pid = $_.Pid; filter = $_.Filter; actor_sid = $_.Actor; nonce = $_.Nonce; creation_time = $_.CreationTime; exit_observed = $_.ExitObserved; exit_code = $_.ExitCode; timed_out = $_.TimedOut }
        })
        $ledger.native_errors = @($native.Errors)
        if ($native.Errors.Count -gt 0) { $ledger.status = 'FAIL' }
        if ($native.PreserveResources) { $ledger.preservation = $true }
    }
}
function Write-Interim {
    Update-NativeReceipt
    $ledger.status = 'FAIL'
    $ledger.preservation = $true
    if (-not $script:checkpointWritten) {
        # Output is a receipt, never an ownership transfer to the CI log collector.
        [Console]::Out.WriteLine(($ledger | ConvertTo-Json -Depth 30 -Compress))
        $script:checkpointWritten = $true
    }
}
function Observe-Profiles {
    foreach ($account in $ledger.accounts) {
        if (-not $account.created) { continue }
        try {
            $matches = @(Get-CimInstance Win32_UserProfile -Filter "SID='$($account.sid)'")
            foreach ($profile in $matches) {
                if (-not $account.profile_baseline_known -or $account.preexisting_profile) { throw 'Profile creation ownership cannot be established' }
                if (-not @($ledger.profiles | Where-Object { $_.sid -eq $account.sid }).Count) {
                    $ledger.profiles += [ordered]@{ sid = $account.sid; local_path = $profile.LocalPath; created = $true; removed = $false }
                }
            }
        } catch {
            $account.profile_unknown = $true
            Add-Failure ('profile-observation-' + $account.sid) $_ $true
        }
    }
}
function Initialize-TrustedHelper([string]$Source, [string]$DependencySource) {
    # Bootstrap executes before either ephemeral account exists. The reviewed script
    # and build checkout are the initial provisioner trust root, never actor input.
    $actor = [Security.Principal.WindowsIdentity]::GetCurrent().User
    $common = [Environment]::GetFolderPath('CommonApplicationData')
    $script:bootstrapPath = Join-Path $common ('surge-provisioner-code-' + $runNonce)
    if ([IO.Directory]::Exists($script:bootstrapPath)) { throw 'Bootstrap tree collision' }
    $security = [Security.AccessControl.DirectorySecurity]::new()
    $security.SetOwner($actor)
    $security.SetAccessRuleProtection($true, $false)
    foreach ($sid in @($actor.Value, 'S-1-5-18', 'S-1-5-32-544')) {
        $rule = [Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]::new($sid), [Security.AccessControl.FileSystemRights]::FullControl, [Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit', [Security.AccessControl.PropagationFlags]::None, [Security.AccessControl.AccessControlType]::Allow)
        $security.AddAccessRule($rule)
    }
    [IO.FileSystemAclExtensions]::Create([IO.DirectoryInfo]::new($script:bootstrapPath), $security)
    $script:bootstrapCreated = $true
    $copy = Join-Path $script:bootstrapPath 'windows-native-provisioner.cs'
    $dependencies = Join-Path $script:bootstrapPath 'windows-native-dependencies.json'
    Copy-Item -LiteralPath $Source -Destination $copy
    Copy-Item -LiteralPath $DependencySource -Destination $dependencies
    if ((Get-FileHash -LiteralPath $copy -Algorithm SHA256).Hash.ToLowerInvariant() -ne $ledger.helper_sha256) { throw 'Protected helper source copy mismatch' }
    if ((Get-FileHash -LiteralPath $dependencies -Algorithm SHA256).Hash.ToLowerInvariant() -ne $ledger.dependency_classification_sha256) { throw 'Protected dependency classification copy mismatch' }
    # Read pins omit write/delete sharing; compiler source and classification cannot
    # be substituted by either later test account. Keep them through all child exits.
    $script:assemblyPins += [IO.File]::Open($copy, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    $script:assemblyPins += [IO.File]::Open($dependencies, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    $tempBefore = $env:TEMP
    $tmpBefore = $env:TMP
    try {
        $env:TEMP = $script:bootstrapPath
        $env:TMP = $script:bootstrapPath
        $assembly = Join-Path $script:bootstrapPath 'Surge.NativeFixtures.dll'
        Add-Type -Path $copy -OutputAssembly $assembly -OutputType Library
        $ledger.helper_assembly_sha256 = (Get-FileHash -LiteralPath $assembly -Algorithm SHA256).Hash.ToLowerInvariant()
        $script:assemblyPins += [IO.File]::Open($assembly, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
        Add-Type -Path $assembly
    } finally {
        $env:TEMP = $tempBefore
        $env:TMP = $tmpBefore
    }
    return $dependencies
}
function Run-Elevated([string]$Filter, $Data, [string]$Label) {
    $invocation = $native.CreateTree($root, $Label, $false)
    $nonce = [Guid]::NewGuid().ToString('N')
    $child = $native.LaunchElevated($Filter, $nonce, $Data, $standardData, $invocation)
    $native.ObserveReady($child)
    $native.WaitSuccessful($child)
    $ledger.observations += ($native.ValidateReceipt($child, $false) | ConvertFrom-Json -AsHashtable)
}
function Run-Standard([string]$Filter, [string]$Label) {
    $nonce = [Guid]::NewGuid().ToString('N')
    $receipt = Join-Path $ordinary ($Label + '-complete.json')
    $stdout = Join-Path $ordinary ($Label + '-stdout.log')
    $stderr = Join-Path $ordinary ($Label + '-stderr.log')
    $envValues = $native.ActorEnvironment($Filter, $nonce, $standardData.Path, $standardData.Path, $receipt, '')
    # This is the separate ordinary standard actor, never an elevated launch input.
    $envValues['TEMP'] = $ordinary
    $envValues['TMP'] = $ordinary
    $envValues['SURGE_NATIVE_PROBE_ROOT'] = $ordinary
    try {
        Set-ActorEnvironment $envValues
        $arguments = @('--ignored', '--exact', $Filter, '--nocapture', '--test-threads=1')
        $process = Start-Process -FilePath $native.Executable -ArgumentList $arguments -Credential $script:standardCredential -LoadUserProfile -WorkingDirectory $ordinary -RedirectStandardOutput $stdout -RedirectStandardError $stderr -PassThru
        # Keep the original Process owner even if native duplicate registration fails.
        $script:standardProcesses += $process
        $child = $native.RegisterStandard($process.Handle, [uint32]$process.Id, $Filter, $nonce, $receipt)
        $native.WaitSuccessful($child)
        $ledger.observations += ($native.ValidateReceipt($child, $false) | ConvertFrom-Json -AsHashtable)
    } finally {
        Try-Cleanup 'restore-standard-environment' { Restore-Environment }
        Observe-Profiles
    }
}
try {
    # Build BEFORE test users can execute; exact cargo JSON artifact and actual HEAD are recorded.
    $revision = (& git rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0 -or $revision -notmatch '^[0-9a-f]{40}$') { throw 'Cannot resolve exact source candidate' }
    $ledger.candidate_revision = $revision
    $dirty = @(& git status --porcelain)
    if ($LASTEXITCODE -ne 0 -or $dirty.Count -ne 0) { throw 'Exact native candidate requires a clean reviewed checkout' }
    $messages = & cargo test --locked -p surge-persistence --lib --no-run --message-format=json
    if ($LASTEXITCODE -ne 0) { throw 'Persistence native candidate build failed' }
    $executables = @($messages | ForEach-Object {
        $message = $_ | ConvertFrom-Json
        if ($message.reason -eq 'compiler-artifact' -and $message.profile.test -and $message.target.name -eq 'surge_persistence' -and $message.executable) { $message.executable }
    })
    if ($executables.Count -ne 1) { throw 'Exactly one persistence test executable is required' }
    if ((& git rev-parse HEAD).Trim() -ne $revision) { throw 'Candidate revision changed during build' }
    if (@(& git status --porcelain).Count -ne 0) { throw 'Candidate source changed during build' }
    $helper = Join-Path $PSScriptRoot 'windows-native-provisioner.cs'
    $classification = Join-Path $PSScriptRoot 'windows-native-dependencies.json'
    $ledger.helper_sha256 = (Get-FileHash -LiteralPath $helper -Algorithm SHA256).Hash.ToLowerInvariant()
    $ledger.dependency_classification_sha256 = (Get-FileHash -LiteralPath $classification -Algorithm SHA256).Hash.ToLowerInvariant()
    $lockHash = (Get-FileHash -LiteralPath (Join-Path $PSScriptRoot '../Cargo.lock') -Algorithm SHA256).Hash.ToLowerInvariant()
    # Compile only the protected helper copy before either test account exists.
    $protectedClassification = Initialize-TrustedHelper $helper $classification
    foreach ($kind in @('standard', 'foreign')) {
        $name = 'sg' + $kind.Substring(0, 1) + [Guid]::NewGuid().ToString('N').Substring(0, 12)
        $password = ConvertTo-SecureString ([Guid]::NewGuid().ToString('N') + [Guid]::NewGuid().ToString('N') + 'aA1!') -AsPlainText -Force
        $account = [ordered]@{ kind = $kind; name = $name; sid = $null; created = $false; attempted = $true; removed = $false; preexisting_profile = $false; profile_baseline_known = $false; profile_unknown = $false }
        $ledger.accounts += $account
        $user = New-LocalUser -Name $name -Password $password -AccountNeverExpires -Description ('Surge fixture run ' + $runNonce)
        $account.sid = $user.SID.Value
        $account.created = $true
        # SID is new, nevertheless record actual preexisting-profile absence before any launch.
        $account.preexisting_profile = @(Get-CimInstance Win32_UserProfile -Filter "SID='$($account.sid)'").Count -ne 0
        $account.profile_baseline_known = $true
        if ($account.preexisting_profile) { throw 'Unexpected profile already exists for newly created SID' }
        Add-LocalGroupMember -SID 'S-1-5-32-545' -Member $user
        if (@(Get-LocalGroupMember -SID 'S-1-5-32-544' | Where-Object { $_.SID.Value -eq $account.sid }).Count) { throw 'Test account must not be an administrator' }
        if ($kind -eq 'standard') { $standardPassword = $password; $standardAccount = $account } else { $foreignPassword = $password; $foreignAccount = $account }
    }
    $script:standardCredential = [PSCredential]::new("$env:COMPUTERNAME\$($standardAccount.name)", $standardPassword)
    $native = [Surge.NativeFixtures.Provisioner]::new($standardAccount.sid, $foreignAccount.sid)
    $parentTokenBefore = $native.SnapshotParentToken()
    $bootstrapTree = $native.Retain($bootstrapPath)
    $bootstrapIdentity = $native.TreeIdentity($bootstrapTree)
    # Enumerate actual already-loaded installed compiler/runtime assemblies, not
    # names supplied by the standard actor. Dynamic helper identity is its pinned DLL hash.
    $loadedPaths = @([AppDomain]::CurrentDomain.GetAssemblies() | Where-Object { -not $_.IsDynamic -and $_.Location } | ForEach-Object { $_.Location } | Sort-Object -Unique)
    foreach ($toolPath in $loadedPaths) { $ledger.toolchain += $native.PinToolArtifact($toolPath) }
    $trees += [ordered]@{ path = $bootstrapPath; identity = $bootstrapIdentity; role = 'bootstrap'; created = $true }
    $trustedParent = $native.Retain([Environment]::GetFolderPath('CommonApplicationData'))
    $root = $native.CreateTree($trustedParent, ('surge-privileged-' + $runNonce), $true)
    $trees += [ordered]@{ path = $root.Path; identity = $native.TreeIdentity($root); role = 'launch'; created = $true }
    $adminData = $native.CreateTree($root, 'admin-data', $false)
    $standardData = $native.CreateTree($root, 'standard-data', $true)
    $ordinary = $native.CreateOrdinaryTree($trustedParent, ('surge-ordinary-' + $runNonce))
    $trees += [ordered]@{ path = $ordinary; identity = $native.ObjectIdentity($ordinary); role = 'ordinary'; created = $true }
    $ledger.candidate_manifest = $native.PrepareCandidate($root, $executables[0], $revision, $lockHash, (Get-Content -LiteralPath $protectedClassification -Raw)) | ConvertFrom-Json -AsHashtable
    # Literal reviewed order, no actor-supplied filter or source/receipt command interpolation.
    Run-Elevated 'state_home::privileged_fixture_tests::admin_default_owner_creation' $adminData 'admin-owner-invocation'
    Run-Standard 'state_home::privileged_fixture_tests::standard_user_below_admin_ancestor' 'standard-admin'
    Run-Elevated 'state_home::privileged_fixture_tests::provision_foreign_owner_fixture' $standardData 'foreign-setup-invocation'
    Run-Standard 'state_home::privileged_fixture_tests::foreign_owner_refuses_before_initialization' 'standard-foreign'
    if ($native.SnapshotParentToken() -ne $parentTokenBefore) { throw 'Provisioner effective token changed' }
    if ($ledger.errors.Count -eq 0 -and $ledger.cleanup_errors.Count -eq 0) { $ledger.status = 'OBSERVATIONS_PASSED_CLEANUP_PENDING' }
} catch {
    Add-Failure 'setup-or-test' $_
} finally {
    Try-Cleanup 'restore-environment' { Restore-Environment }
    if ($null -ne $native) {
        Try-Cleanup 'settle-native-children' { $native.SettleBounded() }
    }
    # Independently settle original standard Process owners, including failed registration.
    foreach ($process in $standardProcesses) {
        Try-Cleanup 'settle-standard-original-owner' {
            if (-not $process.HasExited) {
                $process.Kill()
                if (-not $process.WaitForExit(10000)) { throw 'Standard actor exit unobserved' }
            }
            [void]$standardSettled.Add($process)
        }
    }
    $externalUnsettled = @(Get-UnsettledStandard)
    if ($externalUnsettled.Count -gt 0 -or ($null -ne $native -and $native.HasUnsettled)) {
        $ledger.preservation = $true
        Try-Cleanup 'interim-receipt' { Write-Interim }
        if ($null -ne $native) { $native.HoldUntilExit([Action]{ Write-Interim }) }
        # No deadline reset: acceptance has already FAILED. Keep original owners alive.
        Hold-StandardUntilExit $externalUnsettled
    }
    Try-Cleanup 'observe-owned-profiles' { Observe-Profiles }
    if (@($ledger.accounts | Where-Object { $_.attempted -and -not $_.created }).Count) {
        $ledger.cleanup_errors += [ordered]@{ phase = 'account-creation'; message = 'Creation outcome lacks an owned SID; preserving diagnostic resources, no name-based deletion' }
    }
    if ($null -ne $native) {
        Try-Cleanup 'verify-parent-token-unchanged' {
            if ($parentTokenBefore -and $native.SnapshotParentToken() -ne $parentTokenBefore) { throw 'Provisioner token changed' }
        }
        Try-Cleanup 'aggregate-native-receipt' { Update-NativeReceipt }
        Try-Cleanup 'close-settled-native-handles' { $native.CloseSettledHandles() }
    }
    foreach ($process in $standardProcesses) { Try-Cleanup 'dispose-settled-standard-owner' { $process.Dispose() } }
    foreach ($pin in $assemblyPins) { Try-Cleanup 'close-protected-helper-pin' { $pin.Dispose() } }
    if (-not $ledger.preservation) {
        foreach ($tree in @($trees | Sort-Object { $_.path.Length } -Descending)) {
            try { $native.RemoveOwnedTree($tree.path, $tree.identity, $tree.role) }
            catch { Add-Failure ('tree-' + $tree.path) $_ $true; $ledger.preservation = $true }
        }
        # A failed actor may have left a standard-user-only runtime object. Never
        # assume Administrators can traverse/repair it or delete its owning account.
        if ($bootstrapCreated -and -not $bootstrapIdentity) {
            $ledger.bootstrap_preserved = $true
            $ledger.cleanup_errors += [ordered]@{ phase = 'bootstrap-ownership'; message = 'Preserving bootstrap tree: retained native identity was never established' }
        }
    }
    if (-not $ledger.preservation) {
        foreach ($profile in $ledger.profiles) {
            Try-Cleanup ('profile-' + $profile.sid) {
                $actual = @(Get-CimInstance Win32_UserProfile -Filter "SID='$($profile.sid)'")
                foreach ($entry in $actual) {
                    if ($entry.LocalPath -ne $profile.local_path) { throw 'Profile identity/path changed' }
                    $entry | Remove-CimInstance
                }
                if (@(Get-CimInstance Win32_UserProfile -Filter "SID='$($profile.sid)'").Count) { throw 'Profile deletion not observed' }
                $profile.removed = $true
            }
        }
        foreach ($account in @($ledger.accounts | Where-Object { $_.created })) {
            Try-Cleanup ('account-' + $account.sid) {
                if (-not $account.profile_baseline_known -or $account.profile_unknown -or $account.preexisting_profile) { throw 'Preserving account with unresolved/preexisting profile ownership' }
                if (@($ledger.profiles | Where-Object { $_.sid -eq $account.sid -and -not $_.removed }).Count) { throw 'Preserving account while profile cleanup remains unresolved' }
                $actual = Get-LocalUser -Name $account.name
                if ($actual.SID.Value -ne $account.sid) { throw 'Account SID changed' }
                Remove-LocalUser -SID $actual.SID
                $account.removed = $true
            }
        }
    }
    if ($null -ne $standardPassword) { Try-Cleanup 'standard-secret-dispose' { $standardPassword.Dispose() } }
    if ($null -ne $foreignPassword) { Try-Cleanup 'foreign-secret-dispose' { $foreignPassword.Dispose() } }
}
Update-NativeReceipt
if ($ledger.errors.Count -eq 0 -and $ledger.cleanup_errors.Count -eq 0 -and -not $ledger.preservation -and $ledger.observations.Count -eq 4 -and ($null -ne $native) -and $native.Errors.Count -eq 0) { $ledger.status = 'PASS' } else { $ledger.status = 'FAIL' }
[Console]::Out.WriteLine(($ledger | ConvertTo-Json -Depth 30 -Compress))
if ($ledger.status -ne 'PASS') { exit 1 }
