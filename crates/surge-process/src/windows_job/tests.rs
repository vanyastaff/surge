use super::*;
use std::ffi::OsStr;
use std::process::Command;
use std::time::{Duration, Instant};
use windows::Win32::System::Threading::CREATE_BREAKAWAY_FROM_JOB;

const FIXTURE: &str = "windows_job::tests::native_fixture";

fn fixture_command(mode: &str, root: &std::path::Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", FIXTURE, "--nocapture"])
        .env("SURGE_JOB_FIXTURE_MODE", mode)
        .env("SURGE_JOB_FIXTURE_ROOT", root);
    command
}

fn spec(command: &Command) -> LaunchSpec {
    let mut environment: std::collections::BTreeMap<_, _> = std::env::vars_os()
        .filter(|(key, _)| !key.as_encoded_bytes().starts_with(b"="))
        .collect();
    for (key, value) in command.get_envs() {
        if let Some(value) = value {
            environment.insert(key.to_owned(), value.to_owned());
        } else {
            environment.remove(key);
        }
    }
    LaunchSpec {
        program: command.get_program().to_owned(),
        args: command.get_args().map(OsStr::to_owned).collect(),
        environment: environment.into_iter().collect(),
        current_dir: command.get_current_dir().map(std::path::Path::to_owned),
    }
}

fn wait_file(path: &std::path::Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(value) = std::fs::read_to_string(path)
            && !value.is_empty()
        {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "fixture receipt missing: {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn settle(child: &mut NativeChild) -> ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(15);
    settle_before(child, deadline)
}

fn settle_before(child: &mut NativeChild, deadline: Instant) -> ExitStatus {
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        assert!(Instant::now() < deadline, "native job failed to settle");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_process_before(process: &OwnedHandle, deadline: Instant) {
    // Job accounting and the independently retained process object are separate
    // observations. Wait for actual termination without extending settlement's deadline.
    while !wait_state(raw(process)).unwrap() {
        assert!(
            Instant::now() < deadline,
            "descendant process failed to terminate"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn inherited_command_preserves_casefolded_overrides_removals_and_inherited_values() {
    let root = tempfile::tempdir().unwrap();
    let inherited = std::env::var_os("SystemRoot").unwrap();
    let mut command = fixture_command("environment", root.path());
    command
        .env("pAtH", "surge inherited environment fixture")
        .env_remove("tEmP");
    let launch = LaunchSpec::from_inherited_command(&command).unwrap();
    let mut child = NativeChild::spawn(&launch).unwrap();
    assert!(settle(&mut child).success());
    assert_eq!(
        std::fs::read_to_string(root.path().join("environment")).unwrap(),
        format!(
            "surge inherited environment fixture\n{}\nabsent",
            inherited.to_string_lossy()
        )
    );
}

#[test]
fn final_launch_admission_preserves_refusal_and_prevents_child_effect() {
    #[derive(Debug)]
    struct Refused;
    impl std::fmt::Display for Refused {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("native fixture refused")
        }
    }
    impl std::error::Error for Refused {}
    let root = tempfile::tempdir().unwrap();
    let launch = spec(&fixture_command("environment", root.path()));
    let mut checks = 0;
    let error = NativeChild::spawn_admitted(&launch, || {
        checks += 1;
        Err(io::Error::other(Refused))
    })
    .err()
    .unwrap();
    assert_eq!(checks, 1);
    assert!(error.get_ref().unwrap().is::<Refused>());
    assert!(!root.path().join("environment").exists());
}

#[test]
fn native_launch_contains_immediate_descendants_and_observes_empty_job() {
    let standard_user_probe = assert_requested_standard_user();
    let root = tempfile::tempdir().unwrap();
    let mut child = NativeChild::spawn(&spec(&fixture_command("leader", root.path()))).unwrap();
    assert!(child.job_name().starts_with("Global\\SurgeMcp-"));
    let descendant: u32 = wait_file(&root.path().join("descendant")).parse().unwrap();
    let job = Job::open(child.job_name()).unwrap().unwrap();
    assert!(job.active_processes().unwrap() >= 2);
    let process = process_handle(descendant);
    let deadline = Instant::now() + Duration::from_secs(15);
    child.kill().unwrap();
    settle_before(&mut child, deadline);
    wait_process_before(&process, deadline);
    assert_eq!(job.active_processes().unwrap(), 0);
    if standard_user_probe {
        println!("SURGE_NATIVE_STANDARD_USER_PASS");
    }
}

fn assert_requested_standard_user() -> bool {
    use windows::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let Ok(expected_sid) = std::env::var("SURGE_NATIVE_PROBE_SID") else {
        return false;
    };
    assert!(
        !expected_sid.is_empty(),
        "standard-user probe SID cannot be empty"
    );
    assert_eq!(
        security::current_sid().unwrap(),
        expected_sid,
        "probe did not execute as the requested Windows user"
    );
    let mut token = HANDLE::default();
    // SAFETY: current-process pseudo handle is valid; token output is uniquely owned below.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) }.unwrap();
    let token = owned(token).unwrap();
    let mut elevation = TOKEN_ELEVATION::default();
    let mut length = 0;
    // SAFETY: correctly aligned and exactly sized TOKEN_ELEVATION output buffer.
    unsafe {
        GetTokenInformation(
            raw(&token),
            TokenElevation,
            Some((&raw mut elevation).cast()),
            size_u32::<TOKEN_ELEVATION>().unwrap(),
            &raw mut length,
        )
    }
    .unwrap();
    assert_eq!(length, size_u32::<TOKEN_ELEVATION>().unwrap());
    assert_eq!(
        elevation.TokenIsElevated, 0,
        "probe requires a non-elevated Windows token"
    );
    true
}

#[test]
fn native_leader_exit_settles_its_remaining_descendant() {
    let root = tempfile::tempdir().unwrap();
    let mut child =
        NativeChild::spawn(&spec(&fixture_command("leader_exit", root.path()))).unwrap();
    let descendant: u32 = wait_file(&root.path().join("descendant")).parse().unwrap();
    let process = process_handle(descendant);
    let job = Job::open(child.job_name()).unwrap().unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    assert!(settle_before(&mut child, deadline).success());
    wait_process_before(&process, deadline);
    assert_eq!(job.active_processes().unwrap(), 0);
}

#[test]
fn native_owner_death_kills_job_without_inherited_job_handles() {
    let root = tempfile::tempdir().unwrap();
    let mut owner = fixture_command("owner", root.path()).spawn().unwrap();
    let job_name = wait_file(&root.path().join("job"));
    let descendant: u32 = wait_file(&root.path().join("descendant")).parse().unwrap();
    let process = process_handle(descendant);
    owner.kill().unwrap();
    owner.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !wait_state(raw(&process)).unwrap() {
        assert!(
            Instant::now() < deadline,
            "owner death left descendant alive"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    while Job::open(&job_name).unwrap().is_some() {
        assert!(
            Instant::now() < deadline,
            "terminated job namespace did not disappear"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn native_job_refuses_collision_malformed_nonce_and_wrong_creation_identity() {
    for name in [
        "",
        "Local\\Other-123",
        "Global\\SurgeMcp-0000000000000000000000000000000000000000000000000000000000000000",
    ] {
        assert!(Job::open(name).is_err());
    }
    let root = tempfile::tempdir().unwrap();
    let mut child = NativeChild::spawn(&spec(&fixture_command("leader", root.path()))).unwrap();
    wait_file(&root.path().join("descendant"));
    assert!(Job::create_fresh(child.job_name().to_owned()).is_err());
    let job = Job::open(child.job_name()).unwrap().unwrap();
    assert!(!job.contains_process(child.id(), 1).unwrap());
    assert!(!job.contains_process(u32::MAX, 1).unwrap());
    child.kill().unwrap();
    settle(&mut child);
}

#[test]
fn native_job_reopening_rejects_broad_acl_and_denied_access_without_claiming_absence() {
    for permissive in [true, false] {
        let fresh = Job::fresh().unwrap();
        let name = fresh.name.clone();
        drop(fresh);
        let descriptor = security::Descriptor::untrusted(permissive).unwrap();
        let attributes = descriptor.attributes(false);
        let wide = command::wide(OsStr::new(&name)).unwrap();
        // SAFETY: this creates an empty test-owned object with a deliberately untrusted ACL.
        let handle =
            owned(unsafe { CreateJobObjectW(Some(&attributes), PCWSTR(wide.as_ptr())) }.unwrap())
                .unwrap();
        assert!(
            Job::open(&name).is_err(),
            "untrusted job was accepted or counted absent"
        );
        drop(handle);
    }
}

#[test]
fn native_child_cannot_break_away_from_job() {
    let root = tempfile::tempdir().unwrap();
    let mut child = NativeChild::spawn(&spec(&fixture_command("breakaway", root.path()))).unwrap();
    assert_eq!(wait_file(&root.path().join("breakaway")), "refused");
    assert!(settle(&mut child).success());
}

#[test]
fn native_batch_arguments_match_std_and_do_not_execute_metacharacters() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("batch directory with spaces");
    std::fs::create_dir(&directory).unwrap();
    let batch = directory.join("argument echo.cmd");
    std::fs::write(
        &batch,
        format!(
            "@echo off\r\n\"{}\" --ignored --exact {FIXTURE} --nocapture --skip %1\r\n",
            std::env::current_exe().unwrap().display()
        ),
    )
    .unwrap();
    let marker = root.path().join("injected");
    let attack = format!("\" & echo injected>\"{}\" & rem \"", marker.display());
    for value in [
        "with spaces",
        "%PATH%",
        "bang!value",
        "caret^value",
        "amp&value",
        "pipe|value",
        "quote\"value",
        "trailing\\",
        attack.as_str(),
    ] {
        let output = root.path().join("argument");
        let mut command = Command::new(&batch);
        command
            .arg(value)
            .env("SURGE_JOB_FIXTURE_MODE", "arguments")
            .env("SURGE_JOB_FIXTURE_ROOT", root.path());
        let reference = command.output().unwrap();
        assert!(
            reference.status.success(),
            "std batch baseline failed for {value:?}"
        );
        let reference = std::fs::read(&output).unwrap();
        std::fs::remove_file(&output).unwrap();
        let mut child = NativeChild::spawn(&spec(&command)).unwrap();
        assert!(
            settle(&mut child).success(),
            "native batch failed for {value:?}"
        );
        let actual = std::fs::read(&output).unwrap();
        assert_eq!(
            actual, reference,
            "native/std batch argv mismatch: {value:?}"
        );
        let expected = value
            .encode_utf16()
            .map(|unit| format!("{unit:04x}"))
            .collect::<String>();
        assert_eq!(
            String::from_utf8(actual).unwrap(),
            expected,
            "batch changed literal argument"
        );
        assert!(
            !marker.exists(),
            "batch argument executed an injected command"
        );
        std::fs::remove_file(&output).unwrap();
    }
    for value in ["line\nbreak", "line\rbreak", "nul\0value"] {
        let mut command = Command::new(&batch);
        command.arg(value);
        assert!(NativeChild::spawn(&spec(&command)).is_err());
    }
}

#[test]
fn native_executable_arguments_and_explicit_path_preserve_utf16() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("executable directory with spaces");
    std::fs::create_dir(&directory).unwrap();
    let program = directory.join("surge_native_fixture.exe");
    std::fs::copy(std::env::current_exe().unwrap(), &program).unwrap();
    let values = [
        "with spaces",
        "quote\"value",
        "quote\\\"value",
        "trailing\\",
        "%PATH%",
        "amp&pipe|caret^bang!",
        "日本語",
    ];
    for value in values {
        let mut command = fixture_command("arguments", root.path());
        command.args(["--skip", value]);
        let mut launch = spec(&command);
        launch.program = "surge_native_fixture".into();
        launch.current_dir = Some(root.path().to_owned());
        launch
            .environment
            .retain(|(key, _)| !key.eq_ignore_ascii_case("PATH"));
        launch
            .environment
            .push(("Path".into(), directory.clone().into_os_string()));
        let mut child = NativeChild::spawn(&launch).unwrap();
        assert!(settle(&mut child).success());
        let encoded = std::fs::read_to_string(root.path().join("argument")).unwrap();
        assert_eq!(
            encoded,
            value
                .encode_utf16()
                .map(|unit| format!("{unit:04x}"))
                .collect::<String>()
        );
    }
}

#[test]
fn native_empty_argument_matches_std_argument_parser_for_exe_and_batch() {
    use std::io::Read;
    let root = tempfile::tempdir().unwrap();
    let batch = root.path().join("empty argument.cmd");
    std::fs::write(
        &batch,
        format!(
            "@\"{}\" --color %1\r\n",
            std::env::current_exe().unwrap().display()
        ),
    )
    .unwrap();
    for batch_mode in [false, true] {
        let mut command = if batch_mode {
            Command::new(&batch)
        } else {
            Command::new(std::env::current_exe().unwrap())
        };
        if !batch_mode {
            command.arg("--color");
        }
        command.arg("");
        let expected = command.output().unwrap();
        assert!(
            !expected.status.success(),
            "empty color must reach the parser as invalid input"
        );
        let mut child = NativeChild::spawn(&spec(&command)).unwrap();
        let mut stderr = child.take_stderr().unwrap();
        let status = settle(&mut child);
        let mut actual = Vec::new();
        stderr.read_to_end(&mut actual).unwrap();
        assert_eq!(status.code(), expected.status.code());
        assert_eq!(
            actual, expected.stderr,
            "empty argument was lost or changed"
        );
    }
}

#[test]
fn native_batch_refuses_verbatim_path_that_would_redirect_to_a_different_script() {
    let root = tempfile::tempdir().unwrap();
    let canonical = std::fs::canonicalize(root.path()).unwrap();
    let intended = canonical.join("redirected.");
    let redirected = root.path().join("redirected");
    std::fs::create_dir(&intended).unwrap();
    std::fs::create_dir(&redirected).unwrap();
    let marker = root.path().join("ran");
    for (directory, name) in [(&intended, "intended"), (&redirected, "redirected")] {
        std::fs::write(
            directory.join("fixture.cmd"),
            format!("@echo {name}>\"{}\"\r\n", marker.display()),
        )
        .unwrap();
    }
    let command = Command::new(intended.join("fixture.cmd"));
    assert!(NativeChild::spawn(&spec(&command)).is_err());
    assert!(
        !marker.exists(),
        "verbatim batch path executed a redirected script"
    );
}

fn process_handle(pid: u32) -> OwnedHandle {
    // SAFETY: query-only process opening, handle uniquely transferred to the test.
    owned(
        unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                false,
                pid,
            )
        }
        .unwrap(),
    )
    .unwrap()
}

#[test]
#[ignore = "subprocess-only Windows native ownership fixture"]
fn native_fixture() {
    let mode = std::env::var("SURGE_JOB_FIXTURE_MODE").expect("fixture marker required");
    let root = PathBuf::from(std::env::var_os("SURGE_JOB_FIXTURE_ROOT").unwrap());
    match mode.as_str() {
        "environment" => {
            assert!(std::env::var_os("TEMP").is_none());
            std::fs::write(
                root.join("environment"),
                format!(
                    "{}\n{}\nabsent",
                    std::env::var("PATH").unwrap(),
                    std::env::var("SystemRoot").unwrap()
                ),
            )
            .unwrap();
        },
        "arguments" => {
            use std::os::windows::ffi::OsStrExt;
            let argument = std::env::args_os().next_back().unwrap();
            let encoded = argument
                .encode_wide()
                .map(|unit| format!("{unit:04x}"))
                .collect::<String>();
            std::fs::write(root.join("argument"), encoded).unwrap();
        },
        "descendant" => {
            std::fs::write(root.join("descendant"), std::process::id().to_string()).unwrap();
            std::thread::sleep(Duration::from_secs(60));
        },
        "leader" | "leader_exit" => {
            let mut child = fixture_command("descendant", &root).spawn().unwrap();
            wait_file(&root.join("descendant"));
            if mode == "leader_exit" {
                std::process::exit(0);
            }
            std::thread::sleep(Duration::from_secs(60));
            let _ = child.kill();
            let _ = child.wait();
        },
        "owner" => {
            let child = NativeChild::spawn(&spec(&fixture_command("leader", &root))).unwrap();
            std::fs::write(root.join("job"), child.job_name()).unwrap();
            std::thread::sleep(Duration::from_secs(60));
            drop(child);
        },
        "breakaway" => {
            use std::os::windows::process::CommandExt;
            let result = fixture_command("descendant", &root)
                .creation_flags(CREATE_BREAKAWAY_FROM_JOB.0)
                .spawn();
            assert!(result.is_err(), "breakaway unexpectedly succeeded");
            std::fs::write(root.join("breakaway"), "refused").unwrap();
        },
        _ => panic!("unrecognized fixture mode"),
    }
}
