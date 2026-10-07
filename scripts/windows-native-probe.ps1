# Isolated native durability design probe. Never grants access to the checkout.
$ErrorActionPreference = 'Stop'
$account = 'surgeprobe' + [Guid]::NewGuid().ToString('N').Substring(0, 8)
$root = Join-Path $env:PUBLIC ('surge-native-' + [Guid]::NewGuid().ToString('N'))
$created = $false
$userSid = $null
$process = $null
$stdout = $null
$stderr = $null
$printed = $false
$previousRoot = $env:SURGE_NATIVE_PROBE_ROOT
$previousSid = $env:SURGE_NATIVE_PROBE_SID
try {
    # Build the actual persistence unit-test binary; JSON selects it unambiguously.
    $messages = & cargo test --locked -p surge-persistence --lib --no-run --message-format=json
    if ($LASTEXITCODE -ne 0) { throw 'Native probe test binary build failed' }
    $executables = @($messages | ForEach-Object {
        $message = $_ | ConvertFrom-Json
        if ($message.reason -eq 'compiler-artifact' -and $message.profile.test -and $message.target.name -eq 'surge_persistence' -and $message.executable) {
            $message.executable
        }
    })
    if ($executables.Count -ne 1) { throw 'Expected exactly one persistence unit-test executable' }
    $password = ConvertTo-SecureString ([Guid]::NewGuid().ToString('N') + [Guid]::NewGuid().ToString('N') + 'aA1!') -AsPlainText -Force
    $user = New-LocalUser -Name $account -Password $password -AccountNeverExpires -Description 'Ephemeral Surge native CI probe'
    $created = $true
    $userSid = $user.SID.Value
    # A standard Users account, never Administrators. The child verifies elevation.
    Add-LocalGroupMember -SID 'S-1-5-32-545' -Member $user
    New-Item -ItemType Directory -Path $root | Out-Null
    $current = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    & icacls $root /inheritance:r /grant:r "*$($user.SID.Value):(OI)(CI)F" "*${current}:(OI)(CI)F" '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Isolated probe directory ACL setup failed' }
    $binary = Join-Path $root 'surge-persistence-probe.exe'
    Copy-Item -LiteralPath $executables[0] -Destination $binary
    $stdout = Join-Path $root 'stdout.log'
    $stderr = Join-Path $root 'stderr.log'
    $credential = [PSCredential]::new("$env:COMPUTERNAME\$account", $password)
    $env:SURGE_NATIVE_PROBE_ROOT = $root
    $env:SURGE_NATIVE_PROBE_SID = $userSid
    $cases = @(
        @{ Filter = 'state_home::windows::native::tests::non_admin_ntfs_complete_flush_probe'; Receipts = @('non-elevated=true filesystem=NTFS file=PASS directory=PASS readonly-error=PASS') },
        @{ Filter = 'runs::storage::windows_ownership_tests::'; Receipts = @('stage1 new-home protected-user-only=PASS', 'stage1 derived-pool ownership-and-settlement=PASS', 'stage1 unsafe-home unchanged-refusal=PASS') }
    )
    foreach ($case in $cases) {
        $printed = $false
        $process = Start-Process -FilePath $binary -ArgumentList @('--ignored', $case.Filter, '--nocapture', '--test-threads=1') -Credential $credential -LoadUserProfile -WorkingDirectory $root -RedirectStandardOutput $stdout -RedirectStandardError $stderr -PassThru
        if (-not $process.WaitForExit(120000)) {
            $process.Kill()
            if (-not $process.WaitForExit(10000)) { throw 'Owned native probe did not terminate after kill' }
            throw 'Native probe exceeded 120 seconds'
        }
        $process.Refresh()
        Get-Content -LiteralPath $stdout
        Get-Content -LiteralPath $stderr
        $printed = $true
        if ($process.ExitCode -ne 0) { throw "Native probe failed with exit code $($process.ExitCode)" }
        foreach ($receipt in $case.Receipts) {
            if (-not (Select-String -LiteralPath $stdout -SimpleMatch $receipt -Quiet)) {
                throw 'Native probe success receipt absent (test filter may not have run)'
            }
        }
        $process.Dispose()
        $process = $null
    }
} finally {
    $env:SURGE_NATIVE_PROBE_ROOT = $previousRoot
    $env:SURGE_NATIVE_PROBE_SID = $previousSid
    if ($process -and -not $process.HasExited) {
        $process.Kill()
        if (-not $process.WaitForExit(10000)) { throw 'Owned native probe still live; preserving its isolated directory/account' }
    }
    if (-not $printed) {
        if ($stdout -and (Test-Path -LiteralPath $stdout)) { Get-Content -LiteralPath $stdout }
        if ($stderr -and (Test-Path -LiteralPath $stderr)) { Get-Content -LiteralPath $stderr }
    }
    if ($process) { $process.Dispose() }
    if ($created) {
        $profile = Get-CimInstance Win32_UserProfile | Where-Object { $_.SID -eq $userSid }
        if ($profile) { $profile | Remove-CimInstance }
        Remove-LocalUser -Name $account
    }
    if (Test-Path -LiteralPath $root) { Remove-Item -LiteralPath $root -Recurse -Force }
}
