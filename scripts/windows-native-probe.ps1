# Isolated native durability and full Windows support probes. Never grants access to the checkout.
$ErrorActionPreference = 'Stop'
$account = 'surgeprobe' + [Guid]::NewGuid().ToString('N').Substring(0, 8)
$root = Join-Path $env:PUBLIC ('surge-native-' + [Guid]::NewGuid().ToString('N'))
$created = $false
$userSid = $null
$process = $null
$stdout = $null
$stderr = $null
$printed = $false
$evidenceBase = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [System.IO.Path]::GetTempPath() }
$evidence = Join-Path $evidenceBase ('surge-standard-user-evidence-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $evidence | Out-Null
$revision = (& git rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0) { throw 'Cannot identify native probe source revision' }
$results = [System.Collections.Generic.List[object]]::new()
$binaryHashes = @{}
$complete = $false
$cases = @()
$previousRoot = $env:SURGE_NATIVE_PROBE_ROOT
$previousSid = $env:SURGE_NATIVE_PROBE_SID
$previousTemp = $env:TEMP
$previousTmp = $env:TMP
try {
    # Use actual libtest executables, not rebuilt substitutes in the other account.
    $executables = @{}
    foreach ($crate in @('surge-persistence', 'surge-process')) {
        $messages = & cargo test --locked -p $crate --lib --all-features --no-run --message-format=json
        if ($LASTEXITCODE -ne 0) { throw "Native probe $crate test binary build failed" }
        $target = $crate.Replace('-', '_')
        $artifacts = @($messages | ForEach-Object {
            $message = $_ | ConvertFrom-Json
            if ($message.reason -eq 'compiler-artifact' -and $message.profile.test -and $message.target.name -eq $target -and $message.executable) {
                $message.executable
            }
        })
        if ($artifacts.Count -ne 1) { throw "Expected exactly one $crate unit-test executable" }
        $executables[$crate] = $artifacts[0]
    }
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
    $binaries = @{}
    foreach ($crate in $executables.Keys) {
        $binary = Join-Path $root ($crate + '-probe.exe')
        Copy-Item -LiteralPath $executables[$crate] -Destination $binary
        $binaries[$crate] = $binary
        $binaryHashes[$crate] = (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    $stdout = Join-Path $root 'stdout.log'
    $stderr = Join-Path $root 'stderr.log'
    $credential = [PSCredential]::new("$env:COMPUTERNAME\$account", $password)
    # Start-Process may inherit the administrator runner's TEMP. All new process
    # fixtures need a writable temp directory inside their isolated account root.
    $temp = Join-Path $root 'temp'
    New-Item -ItemType Directory -Path $temp | Out-Null
    $env:TEMP = $temp
    $env:TMP = $temp
    $env:SURGE_NATIVE_PROBE_ROOT = $root
    $env:SURGE_NATIVE_PROBE_SID = $userSid
    # Exact parent tests only. The busy-close parent owns its fatal subprocess.
    $cases = @(
        @{ Filter = 'state_home::windows::native::tests::non_admin_ntfs_complete_flush_probe'; Receipts = @('non-elevated=true filesystem=NTFS file=PASS directory=PASS readonly-error=PASS') },
        @{ Filter = 'runs::storage::windows_ownership_tests::new_state_home_has_protected_current_user_acl_after_open'; Receipts = @('stage1 new-home protected-user-only=PASS') },
        @{ Filter = 'runs::storage::windows_ownership_tests::derived_pool_retains_original_database_until_every_owner_drops'; Receipts = @('stage1 derived-pool ownership-and-settlement=PASS') },
        @{ Filter = 'runs::storage::windows_ownership_tests::unsafe_existing_home_refuses_without_acl_or_byte_repair'; Receipts = @('stage1 unsafe-home unchanged-refusal=PASS') },
        @{ Filter = 'state_home::tests::control_creation_fact_survives_empty_reopen_and_owns_removal'; Receipts = @('stage1 control creation-fact/exclusive-read/removal=PASS') },
        @{ Filter = 'state_home::tests::run_reservation_is_exclusive_retains_both_directories_and_creates_no_sql'; Receipts = @('stage1 exclusive directory-only reservation and retained artifacts=PASS') },
        @{ Filter = 'runs::connection::windows_tests::connection_owners_outlive_manager_and_close_wal_normally'; Receipts = @('stage1 checked SQLite manager/connection lifetime and WAL close=PASS') },
        @{ Filter = 'runs::connection::windows_tests::failed_real_initializer_closes_before_releasing_namespace'; Receipts = @('stage1 checked SQLite initializer failure settlement=PASS') },
        @{ Filter = 'runs::connection::windows_tests::busy_close_returns_owner_and_drop_is_fatal'; Receipts = @('stage1 actual SQLITE_BUSY returns ownership and fatal Drop settles process=PASS') },
        @{ Filter = 'state_home::security_tests::unsafe_home_dacls_are_refused_without_mutating_existing_objects'; Receipts = @('stage1 actual null/unprotected/widened/inherited ACL unchanged refusal=PASS') },
        @{ Filter = 'state_home::security_tests::unsafe_sidefile_and_hardlinked_database_are_unchanged_after_refusal'; Receipts = @('stage1 unsafe WAL and hardlinked database unchanged refusal=PASS') },
        @{ Filter = 'state_home::security_tests::pathname_aliases_refuse_without_touching_the_existing_target'; Receipts = @('stage1 native pathname alias and ADS unchanged refusal=PASS') },
        @{ Filter = 'state_home::creation_security_tests::first_observable_creation_has_private_security'; Receipts = @('stage1 first-observable home/database private ACL zero-byte DB identity and settlement=PASS') },
        @{ Filter = 'state_home::control_lock_tests::standard_user_control_lock_and_payload_compatibility'; Receipts = @('stage1 non-elevated control readable payload exclusive writer and legacy lock compatibility=PASS') }
    )
    # These nonignored parents own their native children; helpers are never selected.
    $processCases = @(
        'native_launch_contains_immediate_descendants_and_observes_empty_job',
        'native_leader_exit_settles_its_remaining_descendant',
        'native_owner_death_kills_job_without_inherited_job_handles',
        'native_job_refuses_collision_malformed_nonce_and_wrong_creation_identity',
        'native_job_reopening_rejects_broad_acl_and_denied_access_without_claiming_absence',
        'native_child_cannot_break_away_from_job'
    )
    foreach ($name in $processCases) {
        $receipts = if ($name -eq $processCases[0]) { @('SURGE_NATIVE_STANDARD_USER_PASS') } else { @() }
        $cases += @{ Binary = 'surge-process'; Filter = "windows_job::tests::$name"; Native = $true; Receipts = $receipts }
    }
    $privateCases = @(
        'work_items::tests::windows_public_start_freezes_once_replays_and_preserves_original_identity',
        'work_items::start_preparation::secure_lock::windows_tests::private_task_ownership_preserves_stable_identity_and_last_arc_exclusion',
        'work_items::start_preparation::secure_lock::windows_tests::existing_private_lock_read_grants_are_refused_without_acl_or_byte_repair',
        'state_home::windows::namespace::private_objects::tests::publication_is_complete_immutable_and_concurrent_losers_read_the_winner',
        'state_home::windows::namespace::private_objects::tests::private_inputs_refuse_aliases_hardlinks_and_unsafe_existing_acl_without_repair',
        'state_home::windows::namespace::private_objects::tests::after_rename_failure_preserves_final_and_prepublication_failure_retains_cleanup_error',
        'state_home::windows::namespace::private_objects::tests::moved_staging_cleanup_disposes_only_original_before_barrier',
        'state_home::windows::namespace::private_objects::tests::final_and_private_ancestor_reparse_points_are_refused_without_following_target',
        'state_home::windows::namespace::private_objects::tests::independent_final_reader_completes_own_barriers_before_publisher_parent_flush',
        'state_home::windows::namespace::private_objects::tests::subprocess_death_at_publication_boundaries_never_accepts_partial_final',
        'state_home::windows::namespace::private_objects::tests::killed_native_preparation_owner_releases_os_lock_without_changing_original_identity',
        'work_items::owned_flow::tests::atomic_acceptance_retains_launch_arc_and_receipt_replay_conveys_no_authority',
        'work_items::owned_flow::tests::accepted_public_flow_replay_precedes_unavailable_private_storage',
        'work_items::owned_flow::private_inputs::tests::private_inputs_keep_actual_policy_and_transport_but_public_projection_is_opaque',
        'work_items::owned_flow::private_inputs::tests::established_corrupt_host_key_never_regenerates_or_authenticates',
        'work_items::owned_flow::private_inputs::tests::subprocess_death_around_key_marker_preserves_key_or_fails_closed',
        'work_items::owned_flow::private_inputs::tests::partial_list_and_object_transplants_fail_whole_envelope_authentication'
    )
    foreach ($filter in $privateCases) {
        $cases += @{ Binary = 'surge-persistence'; Filter = $filter; Native = $true; Receipts = @() }
    }
    $caseIndex = 0
    foreach ($case in $cases) {
        $printed = $false
        $crate = if ($case.Binary) { $case.Binary } else { 'surge-persistence' }
        $arguments = if ($case.Native) {
            @('--exact', $case.Filter, '--show-output', '--test-threads=1')
        } else {
            @('--ignored', '--exact', $case.Filter, '--nocapture', '--test-threads=1')
        }
        $process = Start-Process -FilePath $binaries[$crate] -ArgumentList $arguments -Credential $credential -LoadUserProfile -WorkingDirectory $root -RedirectStandardOutput $stdout -RedirectStandardError $stderr -PassThru
        if (-not $process.WaitForExit(120000)) {
            $process.Kill()
            if (-not $process.WaitForExit(10000)) { throw 'Owned native probe did not terminate after kill' }
            throw 'Native probe exceeded 120 seconds'
        }
        $process.Refresh()
        Get-Content -LiteralPath $stdout
        Get-Content -LiteralPath $stderr
        $printed = $true
        $caseIndex++
        $stem = $caseIndex.ToString('D2')
        Copy-Item -LiteralPath $stdout -Destination (Join-Path $evidence "$stem.stdout.log")
        Copy-Item -LiteralPath $stderr -Destination (Join-Path $evidence "$stem.stderr.log")
        $output = Get-Content -LiteralPath $stdout -Raw
        $result = [ordered]@{
            filter = $case.Filter; binary = $crate; binary_sha256 = $binaryHashes[$crate]
            exit_code = $process.ExitCode; status = 'incomplete'
            stdout = "$stem.stdout.log"; stderr = "$stem.stderr.log"
        }
        $results.Add($result)
        # An exact filter exiting zero may mean it selected zero tests. Require one.
        if ($output -notmatch '(?m)^running 1 test\s*$' -or $output -notmatch 'test result: ok\. 1 passed; 0 failed; 0 ignored;') {
            throw "Native probe did not report exactly one passed test: $($case.Filter)"
        }
        # Child OS stdout bypasses libtest capture and may interrupt the '... ok'
        # line. --show-output also lists every passed test by its exact full name.
        if ($case.Native) {
            $escapedFilter = [regex]::Escape($case.Filter)
            if ($output -notmatch ('(?m)^test ' + $escapedFilter + ' \.\.\.') -or
                $output -notmatch ('(?m)^    ' + $escapedFilter + '\s*$')) {
                throw "Native probe exact named success absent: $($case.Filter)"
            }
        }
        if ($process.ExitCode -ne 0) { throw "Native probe failed with exit code $($process.ExitCode)" }
        foreach ($receipt in $case.Receipts) {
            if (-not (Select-String -LiteralPath $stdout -SimpleMatch $receipt -Quiet)) {
                throw 'Native probe success receipt absent (test filter may not have run)'
            }
        }
        $result.status = 'passed'
        Write-Output "standard-user case=$($case.Filter) binary=$crate sha256=$($binaryHashes[$crate]) sid=$userSid result=PASS"
        $process.Dispose()
        $process = $null
    }
    $complete = $true
} finally {
    $env:SURGE_NATIVE_PROBE_ROOT = $previousRoot
    $env:SURGE_NATIVE_PROBE_SID = $previousSid
    $env:TEMP = $previousTemp
    $env:TMP = $previousTmp
    if ($process -and -not $process.HasExited) {
        $process.Kill()
        if (-not $process.WaitForExit(10000)) { throw 'Owned native probe still live; preserving its isolated directory/account' }
    }
    try {
        # Settled process logs can now be copied without racing open redirect handles.
        # This directory is outside the isolated tree so cleanup preserves evidence.
        if ($stdout -and (Test-Path -LiteralPath $stdout)) {
            Copy-Item -LiteralPath $stdout -Destination (Join-Path $evidence 'last.stdout.log')
        }
        if ($stderr -and (Test-Path -LiteralPath $stderr)) {
            Copy-Item -LiteralPath $stderr -Destination (Join-Path $evidence 'last.stderr.log')
        }
        [ordered]@{
            revision = $revision; status = $(if ($complete) { 'complete' } else { 'incomplete' })
            sid = $userSid; binary_sha256 = $binaryHashes
            inventory = @($cases | ForEach-Object { $_.Filter }); results = @($results.ToArray())
        } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $evidence 'manifest.json') -Encoding utf8
        Write-Output "Standard-user native evidence: $evidence"
    } finally {
        try {
            if (-not $printed) {
                if ($stdout -and (Test-Path -LiteralPath $stdout)) { Get-Content -LiteralPath $stdout }
                if ($stderr -and (Test-Path -LiteralPath $stderr)) { Get-Content -LiteralPath $stderr }
            }
        } finally {
            if ($process) { $process.Dispose() }
            if ($created) {
                $profile = Get-CimInstance Win32_UserProfile | Where-Object { $_.SID -eq $userSid }
                if ($profile) { $profile | Remove-CimInstance }
                Remove-LocalUser -Name $account
            }
            if (Test-Path -LiteralPath $root) { Remove-Item -LiteralPath $root -Recurse -Force }
        }
    }
}
