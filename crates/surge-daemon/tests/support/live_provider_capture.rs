//! Test-only ACP metadata oracle. It never retains raw messages.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SelectionMode {
    ExplicitSetAck,
    InitialProvider,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Capture {
    pub required_model: String,
    pub selection_mode: SelectionMode,
    pub selection_source: Option<SelectionMode>,
    pub adapter_name: Option<String>,
    pub adapter_version: Option<String>,
    pub provider_session: Option<String>,
    pub offered_models: Vec<String>,
    pub selected_model: Option<String>,
    pub prompts: usize,
    pub legacy_model_raw: Option<String>,
    pub legacy_model_base: Option<String>,
    pub legacy_model_effort: Option<String>,
}
#[derive(Clone)]
enum Pending {
    Initialize,
    New {
        opening_epoch: u64,
    },
    Selection {
        opening_epoch: u64,
        selection_epoch: u64,
        session: String,
        option: String,
        value: String,
    },
}
pub struct ProtocolCapture {
    pub snapshot: Capture,
    pending: std::collections::HashMap<String, Pending>,
    model_option: Option<String>,
    opening_epoch: u64,
    selection_epoch: u64,
}
fn identifier(value: &Value) -> Result<String, &'static str> {
    let value = value.as_str().ok_or("missing metadata identifier")?;
    if value.is_empty()
        || value.len() > 256
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@/_.:-".contains(&b))
    {
        return Err("invalid metadata identifier");
    }
    Ok(value.into())
}
fn request_id(frame: &Value) -> Result<String, &'static str> {
    let id = frame.get("id").ok_or("missing request identity")?;
    if !id.is_number() && !id.is_string() {
        return Err("invalid request identity");
    }
    let key = id.to_string();
    if key.len() > 256 {
        return Err("request identity limit");
    }
    Ok(key)
}
impl ProtocolCapture {
    pub fn new(model: String) -> Result<Self, &'static str> {
        identifier(&Value::String(model.clone()))?;
        Ok(Self {
            snapshot: Capture {
                required_model: model,
                selection_mode: SelectionMode::ExplicitSetAck,
                selection_source: None,
                adapter_name: None,
                adapter_version: None,
                provider_session: None,
                offered_models: vec![],
                selected_model: None,
                prompts: 0,
                legacy_model_raw: None,
                legacy_model_base: None,
                legacy_model_effort: None,
            },
            pending: Default::default(),
            model_option: None,
            opening_epoch: 0,
            selection_epoch: 0,
        })
    }
    pub fn with_mode(model: String, mode: SelectionMode) -> Result<Self, &'static str> {
        let mut capture = Self::new(model)?;
        capture.snapshot.selection_mode = mode;
        Ok(capture)
    }
    pub fn client(&mut self, frame: &Value) -> Result<(), &'static str> {
        let Some(method) = frame.get("method").and_then(Value::as_str) else {
            return Ok(());
        };
        let params = &frame["params"];
        let pending = match method {
            "initialize" => Pending::Initialize,
            "session/new" => {
                self.invalidate_selection()?;
                self.opening_epoch = self
                    .opening_epoch
                    .checked_add(1)
                    .ok_or("opening epoch exhausted")?;
                self.snapshot.provider_session = None;
                self.snapshot.legacy_model_raw = None;
                self.snapshot.legacy_model_base = None;
                self.snapshot.legacy_model_effort = None;
                self.clear_selection();
                self.snapshot.offered_models.clear();
                self.model_option = None;
                Pending::New {
                    opening_epoch: self.opening_epoch,
                }
            },
            "session/set_config_option" => {
                if self.model_option.as_deref() == params["configId"].as_str() {
                    self.invalidate_selection()?;
                }
                Pending::Selection {
                    opening_epoch: self.opening_epoch,
                    selection_epoch: self.selection_epoch,
                    session: identifier(&params["sessionId"])?,
                    option: identifier(&params["configId"])?,
                    value: identifier(&params["value"])?,
                }
            },
            "session/set_model" | "session/setModel" | "_session/set_model" => {
                self.invalidate_selection()?;
                return Ok(());
            },
            "session/prompt" => {
                if self.snapshot.adapter_name.is_none()
                    || self.snapshot.adapter_version.is_none()
                    || self.snapshot.provider_session.as_deref() != params["sessionId"].as_str()
                    || self.snapshot.selected_model.as_deref()
                        != Some(self.snapshot.required_model.as_str())
                    || !self
                        .snapshot
                        .offered_models
                        .contains(&self.snapshot.required_model)
                    || self.snapshot.prompts != 0
                {
                    return Err("exact-model prompt admission rejected");
                }
                self.snapshot.prompts += 1;
                return Ok(());
            },
            _ => return Ok(()),
        };
        let id = request_id(frame)?;
        if self.pending.len() >= 16 || self.pending.contains_key(&id) {
            return Err("request correlation limit");
        }
        self.pending.insert(id, pending);
        Ok(())
    }
    fn observe_initial(&mut self, result: &Value, option: &Value) -> Result<(), &'static str> {
        let configured = identifier(&option["currentValue"])?;
        if configured != self.snapshot.required_model
            || !self.snapshot.offered_models.contains(&configured)
        {
            return Err("initial categorical model mismatch");
        }
        if let Some(raw) = result
            .get("models")
            .and_then(|models| models.get("currentModelId"))
        {
            let raw = raw.as_str().ok_or("legacy model identifier missing")?;
            let (base, effort) = legacy_selection(raw, &self.snapshot, result)?;
            if base != configured {
                return Err("conflicting initial model metadata");
            }
            self.snapshot.legacy_model_raw = Some(raw.into());
            self.snapshot.legacy_model_base = Some(base);
            self.snapshot.legacy_model_effort = effort;
        }
        self.snapshot.selected_model = Some(configured);
        self.snapshot.selection_source = Some(SelectionMode::InitialProvider);
        Ok(())
    }
    fn invalidate_selection(&mut self) -> Result<(), &'static str> {
        self.clear_selection();
        self.selection_epoch = self
            .selection_epoch
            .checked_add(1)
            .ok_or("selection epoch exhausted")?;
        Ok(())
    }
    fn clear_selection(&mut self) {
        self.snapshot.selected_model = None;
        self.snapshot.selection_source = None;
    }
    pub fn provider(&mut self, frame: &Value) -> Result<(), &'static str> {
        // Provider-to-client requests have a separate identity namespace.
        if frame.get("method").is_some() {
            if frame["method"] == "session/update"
                && frame["params"]["sessionId"].as_str()
                    == self.snapshot.provider_session.as_deref()
                && frame["params"]["update"]["sessionUpdate"] == "config_option_update"
            {
                let update = &frame["params"]["update"];
                if model_option(update).is_none_or(|option| {
                    option["currentValue"].as_str() != Some(self.snapshot.required_model.as_str())
                }) {
                    self.invalidate_selection()?;
                }
            }
            return Ok(());
        }
        let Ok(id) = request_id(frame) else {
            return Ok(());
        };
        let Some(pending) = self.pending.remove(&id) else {
            return Ok(());
        };
        if frame.get("error").is_some() {
            return Ok(());
        }
        let result = frame.get("result").ok_or("missing correlated result")?;
        match pending {
            Pending::Initialize => {
                self.snapshot.adapter_name = Some(identifier(&result["agentInfo"]["name"])?);
                self.snapshot.adapter_version = Some(identifier(&result["agentInfo"]["version"])?);
            },
            Pending::New { opening_epoch } => {
                if opening_epoch != self.opening_epoch {
                    return Ok(());
                }
                self.invalidate_selection()?;
                self.snapshot.provider_session = Some(identifier(&result["sessionId"])?);
                let Some(option) = model_option(result) else {
                    return Ok(());
                };
                self.model_option = Some(identifier(&option["id"])?);
                self.snapshot.offered_models = select_values(option)?;
                if self.snapshot.selection_mode == SelectionMode::InitialProvider {
                    self.observe_initial(result, option)?;
                }
            },
            Pending::Selection {
                opening_epoch,
                selection_epoch,
                session,
                option,
                value,
            } => {
                if opening_epoch != self.opening_epoch
                    || selection_epoch != self.selection_epoch
                    || self.snapshot.provider_session.as_ref() != Some(&session)
                    || self.model_option.as_ref() != Some(&option)
                    || value != self.snapshot.required_model
                {
                    return Ok(());
                }
                self.clear_selection();
                if let Some(returned) = model_option(result)
                    && returned["id"].as_str() == Some(option.as_str())
                    && returned["currentValue"].as_str() == Some(value.as_str())
                {
                    self.snapshot.selected_model = Some(value);
                    self.snapshot.selection_source = Some(SelectionMode::ExplicitSetAck);
                }
            },
        }
        Ok(())
    }
}
fn legacy_selection(
    raw: &str,
    capture: &Capture,
    result: &Value,
) -> Result<(String, Option<String>), &'static str> {
    if raw.len() > 256 {
        return Err("legacy model limit");
    }
    if !raw.contains(['[', ']']) {
        return Ok((identifier(&Value::String(raw.into()))?, None));
    }
    if capture.adapter_name.as_deref() != Some("@agentclientprotocol/codex-acp")
        || capture.adapter_version.as_deref() != Some("2.2.2")
    {
        return Err("unsupported legacy model representation");
    }
    let (base, suffix) = raw.split_once('[').ok_or("legacy model format")?;
    let effort = suffix.strip_suffix(']').ok_or("legacy model suffix")?;
    let base = identifier(&Value::String(base.into()))?;
    let effort = identifier(&Value::String(effort.into()))?;
    let thoughts: Vec<_> = result["configOptions"]
        .as_array()
        .ok_or("legacy effort options missing")?
        .iter()
        .filter(|option| option["category"] == "thought_level" && option["type"] == "select")
        .collect();
    let [thought] = thoughts.as_slice() else {
        return Err("legacy effort option ambiguous");
    };
    if let Some(current) = thought.get("currentValue")
        && current.as_str() != Some(effort.as_str())
    {
        return Err("conflicting current reasoning effort");
    }
    let offered = select_values(thought)?.iter().any(|value| value == &effort);
    if !offered {
        return Err("legacy effort not offered");
    }
    if let Some(models) = result["models"].get("availableModels")
        && !models
            .as_array()
            .ok_or("legacy model list invalid")?
            .iter()
            .any(|model| model["modelId"].as_str() == Some(raw))
    {
        return Err("legacy selection not offered");
    }
    Ok((base, Some(effort)))
}

fn select_values(option: &Value) -> Result<Vec<String>, &'static str> {
    let mut choices = Vec::new();
    for choice in option["options"]
        .as_array()
        .ok_or("select choices missing")?
    {
        let values = choice
            .get("options")
            .and_then(Value::as_array)
            .map_or_else(|| std::slice::from_ref(choice), Vec::as_slice);
        for value in values {
            choices.push(identifier(&value["value"])?);
        }
        if choices.len() > 128 {
            return Err("select choices limit");
        }
    }
    Ok(choices)
}

fn model_option(result: &Value) -> Option<&Value> {
    let mut matching = result
        .get("configOptions")?
        .as_array()?
        .iter()
        .filter(|option| option["category"] == "model" && option["type"] == "select");
    let option = matching.next()?;
    if matching.next().is_some() {
        return None;
    }
    Some(option)
}

/// Child ownership reused by the fixture and proxy; it is never runtime authority.
pub struct OwnedProcess {
    waited: bool,
    #[cfg(not(windows))]
    child: tokio::process::Child,
    #[cfg(windows)]
    child: surge_process::windows_job::NativeChild,
    #[cfg(unix)]
    group: Option<u32>,
}
pub type ProcessPipes = (
    tokio::process::ChildStdin,
    tokio::process::ChildStdout,
    tokio::process::ChildStderr,
);
impl OwnedProcess {
    pub fn spawn(command: std::process::Command, own_group: bool) -> std::io::Result<Self> {
        #[cfg(unix)]
        let mut command = command;
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            if own_group {
                command.process_group(0);
            }
        }
        #[cfg(not(windows))]
        {
            let child = tokio::process::Command::from(command)
                .kill_on_drop(true)
                .spawn()?;
            #[cfg(unix)]
            let group = if own_group { child.id() } else { None };
            Ok(Self {
                child,
                waited: false,
                #[cfg(unix)]
                group,
            })
        }
        #[cfg(windows)]
        {
            let _ = own_group;
            let spec = surge_process::windows_job::LaunchSpec::from_inherited_command(&command)?;
            Ok(Self {
                child: surge_process::windows_job::NativeChild::spawn(&spec)?,
                waited: false,
            })
        }
    }
    #[cfg(windows)]
    pub fn container(
        &self,
    ) -> Result<
        surge_core::execution_recovery::process::WriterContainer,
        surge_acp::process_evidence::ObservationError,
    > {
        surge_acp::process_evidence::observe_named_container(
            self.child.id(),
            self.child.job_name(),
            surge_core::execution_recovery::process::WriterCoverage::CoveredDomain,
        )
    }
    pub fn id(&self) -> u32 {
        #[cfg(not(windows))]
        {
            self.child.id().unwrap_or(0)
        }
        #[cfg(windows)]
        {
            self.child.id()
        }
    }
    pub fn pipes(&mut self) -> std::io::Result<ProcessPipes> {
        let missing = || std::io::Error::other("owned process pipes unavailable");
        #[cfg(not(windows))]
        {
            Ok((
                self.child.stdin.take().ok_or_else(missing)?,
                self.child.stdout.take().ok_or_else(missing)?,
                self.child.stderr.take().ok_or_else(missing)?,
            ))
        }
        #[cfg(windows)]
        {
            Ok((
                tokio::process::ChildStdin::from_std(self.child.take_stdin().ok_or_else(missing)?)?,
                tokio::process::ChildStdout::from_std(
                    self.child.take_stdout().ok_or_else(missing)?,
                )?,
                tokio::process::ChildStderr::from_std(
                    self.child.take_stderr().ok_or_else(missing)?,
                )?,
            ))
        }
    }
    pub fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.child.try_wait()
    }
    pub fn terminate(&mut self) -> std::io::Result<()> {
        #[cfg(unix)]
        if let Some(group) = self.group {
            use nix::{
                sys::signal::{Signal, killpg},
                unistd::Pid,
            };
            match killpg(
                Pid::from_raw(i32::try_from(group).map_err(std::io::Error::other)?),
                Signal::SIGKILL,
            ) {
                Ok(()) | Err(nix::errno::Errno::ESRCH) => {},
                Err(error) => return Err(std::io::Error::from_raw_os_error(error as i32)),
            }
        }
        #[cfg(not(windows))]
        {
            self.child.start_kill()
        }
        #[cfg(windows)]
        {
            self.child.kill()
        }
    }
    pub async fn wait(
        &mut self,
        duration: std::time::Duration,
    ) -> std::io::Result<std::process::ExitStatus> {
        tokio::time::timeout(duration, async {
            loop {
                if let Some(status) = self.try_wait()? {
                    self.waited = true;
                    return Ok(status);
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .map_err(std::io::Error::other)?
    }
}
impl Drop for OwnedProcess {
    fn drop(&mut self) {
        if !self.waited {
            let _ = self.terminate();
        }
    }
}

#[cfg(any(not(windows), test))]
pub fn write_capture(file: &mut std::fs::File, capture: &Capture) -> Result<(), &'static str> {
    use std::io::{Seek, Write};
    file.rewind().map_err(|_| "capture seek")?;
    file.set_len(0).map_err(|_| "capture truncate")?;
    serde_json::to_writer(&mut *file, capture).map_err(|_| "capture encode")?;
    file.flush().map_err(|_| "capture flush")
}

pub struct EvidenceFile {
    #[cfg(not(windows))]
    file: std::fs::File,
    #[cfg(windows)]
    file: surge_persistence::RuntimeControlFile,
}
impl EvidenceFile {
    pub fn create(path: &std::path::Path) -> Result<Self, &'static str> {
        let directory = path.parent().ok_or("capture parent missing")?;
        if !directory.is_absolute() {
            return Err("capture parent must be absolute");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
            let metadata = std::fs::symlink_metadata(directory)
                .map_err(|_| "capture directory unavailable")?;
            if !metadata.is_dir()
                || metadata.file_type().is_symlink()
                || metadata.uid() != nix::unistd::geteuid().as_raw()
                || metadata.permissions().mode() & 0o077 != 0
            {
                return Err("capture directory is not private");
            }
            let file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .custom_flags(nix::libc::O_NOFOLLOW)
                .open(path)
                .map_err(|_| "capture create")?;
            Ok(Self { file })
        }
        #[cfg(windows)]
        {
            use surge_persistence::{RuntimeDirectory, RuntimeHomeOwner};
            if directory.file_name() != Some(std::ffi::OsStr::new("daemon")) {
                return Err("Windows capture requires a retained runtime daemon directory");
            }
            let home =
                RuntimeHomeOwner::prepare(directory.parent().ok_or("capture runtime missing")?)
                    .map_err(|_| "capture runtime security")?;
            let directory = home
                .directory(RuntimeDirectory::Daemon)
                .map_err(|_| "capture directory security")?;
            let mut file = directory
                .open_control(path.file_name().ok_or("capture name missing")?)
                .map_err(|_| "capture create")?;
            if !file.was_created() || !file.try_lock_exclusive().map_err(|_| "capture lock")? {
                return Err("capture already exists");
            }
            Ok(Self { file })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = directory;
            Err("capture platform unsupported")
        }
    }
    pub fn write(&mut self, capture: &Capture) -> Result<(), &'static str> {
        #[cfg(not(windows))]
        {
            write_capture(&mut self.file, capture)
        }
        #[cfg(windows)]
        {
            let bytes = serde_json::to_vec(capture).map_err(|_| "capture encode")?;
            if bytes.len() > 65536 {
                return Err("capture size limit");
            }
            self.file.replace_bytes(&bytes).map_err(|_| "capture write")
        }
    }
}

/// Forward one complete frame through the actual transport seam.
pub async fn forward<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    bytes: &[u8],
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    writer.write_all(bytes).await?;
    writer.flush().await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmokePhase {
    Preparing,
    Running,
    Completion,
    Completed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmokeFailure {
    ProviderAuthenticationUnavailable,
    ProviderTimedOut,
    ProviderModelUnavailable,
    ProviderFailed,
    HostFailed,
    VerificationFailed,
    CleanupUnconfirmed,
    WatchdogTimedOut,
    ChildFailed,
    DiagnosticUnavailable,
    DiagnosticInvalid,
    DiagnosticOversized,
    CaptureUnavailable,
    CaptureInvalid,
    CaptureOversized,
    OutputUnavailable,
    FixtureCleanupFailed,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmokeCounts {
    pub session_opened: u64,
    pub human_input_requested: u64,
    pub human_input_resolved: u64,
    pub stage_tool_receipt: u64,
    pub outcome_reported: u64,
    pub run_completed: u64,
    pub run_failed: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmokeDiagnostic {
    pub phase: SmokePhase,
    pub failure: Option<SmokeFailure>,
    pub settled: bool,
    pub binding_verified: bool,
    pub counts: Option<SmokeCounts>,
}
impl Default for SmokeDiagnostic {
    fn default() -> Self {
        Self {
            phase: SmokePhase::Preparing,
            failure: None,
            settled: false,
            binding_verified: false,
            counts: None,
        }
    }
}
impl EvidenceFile {
    pub fn write_diagnostic(&mut self, diagnostic: &SmokeDiagnostic) -> Result<(), &'static str> {
        let bytes = serde_json::to_vec(diagnostic).map_err(|_| "diagnostic encode")?;
        self.replace_diagnostic_bytes(&bytes)
    }
    fn replace_diagnostic_bytes(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        #[cfg(not(windows))]
        {
            use std::io::{Seek, Write};
            self.file.rewind().map_err(|_| "diagnostic seek")?;
            self.file.set_len(0).map_err(|_| "diagnostic truncate")?;
            self.file.write_all(bytes).map_err(|_| "diagnostic write")?;
            self.file.flush().map_err(|_| "diagnostic flush")
        }
        #[cfg(windows)]
        {
            self.file
                .replace_bytes(bytes)
                .map_err(|_| "diagnostic write")
        }
    }
    #[cfg(test)]
    pub fn write_diagnostic_fixture(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        self.replace_diagnostic_bytes(bytes)
    }
}
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadStatus {
    Valid,
    Missing,
    Invalid,
    Oversized,
    Unconfirmed,
}
#[cfg(test)]
pub fn read_bounded<T: serde::de::DeserializeOwned>(
    path: &std::path::Path,
) -> Result<T, ReadStatus> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|_| ReadStatus::Missing)?;
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| ReadStatus::Invalid)?;
    if bytes.len() > 65536 {
        return Err(ReadStatus::Oversized);
    }
    serde_json::from_slice(&bytes).map_err(|_| ReadStatus::Invalid)
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    const MODEL: &str = "gpt-6.1-sol";
    #[test]
    fn private_diagnostic_reader_is_bounded_and_rejects_unknown_fields() {
        #[cfg(windows)]
        let root =
            tempfile::tempdir_in(surge_persistence::RuntimeHomeOwner::user_profile_path().unwrap())
                .unwrap();
        #[cfg(not(windows))]
        let root = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        let owner =
            surge_persistence::RuntimeHomeOwner::prepare(&root.path().join("state")).unwrap();
        #[cfg(windows)]
        let path = owner.path().join("daemon/report.json");
        #[cfg(unix)]
        let path = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
            root.path().join("report.json")
        };
        assert_eq!(
            read_bounded::<SmokeDiagnostic>(&path).unwrap_err(),
            ReadStatus::Missing
        );
        let mut file = EvidenceFile::create(&path).unwrap();
        file.write_diagnostic(&SmokeDiagnostic::default()).unwrap();
        assert!(!read_bounded::<SmokeDiagnostic>(&path).unwrap().settled);
        file.write_diagnostic_fixture(br#"{"unknown":"SECRET"}"#)
            .unwrap();
        assert_eq!(
            read_bounded::<SmokeDiagnostic>(&path).unwrap_err(),
            ReadStatus::Invalid
        );
        file.write_diagnostic_fixture(&vec![b'x'; 65537]).unwrap();
        assert_eq!(
            read_bounded::<SmokeDiagnostic>(&path).unwrap_err(),
            ReadStatus::Oversized
        );
        assert_ne!(ReadStatus::Valid, ReadStatus::Unconfirmed);
        drop(file);
        #[cfg(windows)]
        drop(owner);
        root.close().unwrap();
    }

    fn session(c: &mut ProtocolCapture) {
        c.client(&json!({"id":1,"method":"initialize","params":{}}))
            .unwrap();
        c.provider(&json!({"id":1,"result":{"agentInfo":{"name":"@agentclientprotocol/codex-acp","version":"2.2.2"}}}))
            .unwrap();
        c.client(&json!({"id":2,"method":"session/new","params":{}}))
            .unwrap();
        c.provider(&json!({"id":2,"result":{"sessionId":"session-a","configOptions":[{
            "id":"model","category":"model","type":"select","currentValue":"other","options":[{"value":MODEL,"name":"Sol"}]}]}})).unwrap();
    }
    fn select(c: &mut ProtocolCapture, current: &str) {
        c.client(&json!({"id":3,"method":"session/set_config_option","params":{"sessionId":"session-a","configId":"model","value":MODEL}})).unwrap();
        c.provider(&json!({"id":3,"result":{"configOptions":[{"id":"model","category":"model","type":"select","currentValue":current,"options":[{"value":MODEL,"name":"Sol"}]}]}})).unwrap();
    }
    fn prompt(session: &str) -> Value {
        json!({"id":4,"method":"session/prompt","params":{"sessionId":session,"prompt":[{"text":"SECRET_PROMPT"}]}})
    }
    #[test]
    fn wrong_acknowledgement_cannot_forward_prompt() {
        let mut c = ProtocolCapture::new(MODEL.into()).unwrap();
        session(&mut c);
        select(&mut c, "other");
        assert!(c.client(&prompt("session-a")).is_err());
        assert_eq!(c.snapshot.prompts, 0);
    }
    #[test]
    fn recommendation_without_explicit_acknowledgement_is_insufficient() {
        let mut c = ProtocolCapture::new(MODEL.into()).unwrap();
        session(&mut c);
        assert!(c.client(&prompt("session-a")).is_err());
    }
    #[test]
    fn exact_selection_permits_one_prompt_on_same_session_only() {
        let mut c = ProtocolCapture::new(MODEL.into()).unwrap();
        session(&mut c);
        select(&mut c, MODEL);
        c.client(&prompt("session-a")).unwrap();
        assert_eq!(c.snapshot.selected_model.as_deref(), Some(MODEL));
        assert_eq!(c.snapshot.prompts, 1);
        assert!(c.client(&prompt("session-a")).is_err());
    }
    #[test]
    fn absent_model_and_cross_session_or_uncorrelated_acknowledgement_are_rejected() {
        for variant in ["absent", "cross-session", "uncorrelated"] {
            let mut c = ProtocolCapture::new(MODEL.into()).unwrap();
            session(&mut c);
            if variant == "absent" {
                c.snapshot.offered_models.clear();
            }
            c.client(&json!({"id":3,"method":"session/set_config_option","params":{"sessionId":if variant == "cross-session" { "session-b" } else { "session-a" },"configId":"model","value":MODEL}})).unwrap();
            c.provider(&json!({"id":if variant == "uncorrelated" { 9 } else { 3 },"result":{"configOptions":[{"id":"model","category":"model","type":"select","currentValue":MODEL,"options":[]}]}})).unwrap();
            assert!(
                c.client(&prompt("session-a")).is_err(),
                "accepted {variant}"
            );
            assert_eq!(c.snapshot.prompts, 0);
        }
    }
    #[test]
    fn new_session_invalidates_selection_and_provider_requests_do_not_correlate_as_replies() {
        let mut c = ProtocolCapture::new(MODEL.into()).unwrap();
        session(&mut c);
        select(&mut c, MODEL);
        assert!(c.client(&prompt("session-b")).is_err());
        c.client(&json!({"id":8,"method":"session/new","params":{}}))
            .unwrap();
        assert!(c.client(&prompt("session-a")).is_err());
        c.provider(&json!({"id":8,"method":"session/request_permission","params":{"auth":"AUTH_SENTINEL"}})).unwrap();
        assert!(c.snapshot.provider_session.is_none());
    }
    #[test]
    fn capture_excludes_auth_prompts_tools_errors_and_nonce() {
        let mut c = ProtocolCapture::new(MODEL.into()).unwrap();
        session(&mut c);
        select(&mut c, MODEL);
        c.client(&prompt("session-a")).unwrap();
        for value in [
            json!({"id":900,"method":"authenticate","params":{"token":"AUTH_SENTINEL"}}),
            json!({"id":901,"result":{"tool":"TOOL_SENTINEL","nonce":"NONCE_SENTINEL"}}),
            json!({"id":902,"error":{"message":"ERROR_SENTINEL"}}),
        ] {
            c.client(&value).unwrap();
            c.provider(&value).unwrap();
        }
        let text = serde_json::to_string(&c.snapshot).unwrap();
        for secret in [
            "AUTH_SENTINEL",
            "SECRET_PROMPT",
            "TOOL_SENTINEL",
            "NONCE_SENTINEL",
            "ERROR_SENTINEL",
        ] {
            assert!(!text.contains(secret));
        }
        assert_eq!(c.snapshot.adapter_version.as_deref(), Some("2.2.2"));
    }
    #[test]
    fn later_failed_model_selection_invalidates_previous_acknowledgement() {
        let mut c = ProtocolCapture::new(MODEL.into()).unwrap();
        session(&mut c);
        select(&mut c, MODEL);
        c.client(&json!({"id":10,"method":"session/set_config_option","params":{"sessionId":"session-a","configId":"model","value":"other"}})).unwrap();
        c.provider(&json!({"id":10,"error":{"message":"SECRET_ERROR"}}))
            .unwrap();
        assert!(c.client(&prompt("session-a")).is_err());
        assert_eq!(c.snapshot.prompts, 0);
    }
    fn initial(
        c: &mut ProtocolCapture,
        config: Option<&str>,
        legacy: Option<&str>,
        offered: bool,
        session_id: Option<&str>,
    ) {
        c.client(&json!({"id":1,"method":"initialize","params":{}}))
            .unwrap();
        c.provider(&json!({"id":1,"result":{"agentInfo":{"name":"@agentclientprotocol/codex-acp","version":"2.2.2"}}}))
            .unwrap();
        c.client(&json!({"id":2,"method":"session/new","params":{}}))
            .unwrap();
        let mut result = json!({"sessionId":session_id,"configOptions":[{"id":"model","category":"model","type":"select","options":if offered { json!([{"value":MODEL}]) } else { json!([]) }}]});
        if let Some(value) = config {
            result["configOptions"][0]["currentValue"] = json!(value);
        }
        if let Some(value) = legacy {
            result["models"] = json!({"currentModelId":value});
        }
        let _ = c.provider(&json!({"id":2,"result":result}));
    }
    #[test]
    fn initial_mode_requires_exact_initial_selection_and_marks_its_source() {
        for (config, legacy) in [(Some(MODEL), None), (Some(MODEL), Some(MODEL))] {
            let mut c =
                ProtocolCapture::with_mode(MODEL.into(), SelectionMode::InitialProvider).unwrap();
            initial(&mut c, config, legacy, true, Some("session-a"));
            c.client(&prompt("session-a")).unwrap();
            assert_eq!(
                c.snapshot.selection_source,
                Some(SelectionMode::InitialProvider)
            );
        }
    }
    #[test]
    fn initial_mode_rejects_wrong_missing_offered_missing_session_and_conflicting_models() {
        for (current, legacy, offered, session_id) in [
            (Some("other"), None, true, Some("session-a")),
            (Some(MODEL), None, false, Some("session-a")),
            (Some(MODEL), None, true, None),
            (Some(MODEL), Some("other"), true, Some("session-a")),
            (None, None, true, Some("session-a")),
            (None, Some(MODEL), true, Some("session-a")),
        ] {
            let mut c =
                ProtocolCapture::with_mode(MODEL.into(), SelectionMode::InitialProvider).unwrap();
            initial(&mut c, current, legacy, offered, session_id);
            assert!(c.client(&prompt("session-a")).is_err());
            assert_eq!(c.snapshot.prompts, 0);
        }
    }
    #[test]
    fn strict_mode_never_downgrades_to_initial_provider_selection() {
        let mut c = ProtocolCapture::new(MODEL.into()).unwrap();
        initial(&mut c, Some(MODEL), Some(MODEL), true, Some("session-a"));
        assert!(c.client(&prompt("session-a")).is_err());
        assert_eq!(c.snapshot.selection_mode, SelectionMode::ExplicitSetAck);
    }
    #[test]
    fn capture_publication_replaces_only_the_metadata_snapshot() {
        let mut file = tempfile::tempfile().unwrap();
        let mut c = ProtocolCapture::new(MODEL.into()).unwrap();
        session(&mut c);
        select(&mut c, MODEL);
        write_capture(&mut file, &c.snapshot).unwrap();
        c.snapshot.offered_models.clear();
        write_capture(&mut file, &c.snapshot).unwrap();
        use std::io::{Read, Seek};
        file.rewind().unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(
            serde_json::from_slice::<Capture>(&bytes).unwrap(),
            c.snapshot
        );
    }
    fn composite(
        c: &mut ProtocolCapture,
        legacy: &str,
        version: &str,
        offered_effort: &str,
        listed: &str,
    ) {
        c.client(&json!({"id":1,"method":"initialize","params":{}}))
            .unwrap();
        c.provider(&json!({"id":1,"result":{"agentInfo":{"name":"@agentclientprotocol/codex-acp","version":version}}})).unwrap();
        c.client(&json!({"id":2,"method":"session/new","params":{}}))
            .unwrap();
        let _ = c.provider(&json!({"id":2,"result":{"sessionId":"session-a",
            "configOptions":[{"id":"model","category":"model","type":"select","currentValue":MODEL,"options":[{"value":MODEL}]},
                {"id":"effort","category":"thought_level","type":"select","currentValue":offered_effort,"options":[{"value":offered_effort}]}],
            "models":{"currentModelId":legacy,"availableModels":[{"modelId":listed}]}}}));
    }
    #[test]
    fn known_adapter_composite_crosscheck_preserves_exact_base_and_effort() {
        let mut c =
            ProtocolCapture::with_mode(MODEL.into(), SelectionMode::InitialProvider).unwrap();
        composite(
            &mut c,
            "gpt-6.1-sol[medium]",
            "2.2.2",
            "medium",
            "gpt-6.1-sol[medium]",
        );
        c.client(&prompt("session-a")).unwrap();
        assert_eq!(c.snapshot.selected_model.as_deref(), Some(MODEL));
        let text = serde_json::to_string(&c.snapshot).unwrap();
        assert!(text.contains("gpt-6.1-sol[medium]"));
    }
    #[test]
    fn composite_crosscheck_rejects_bad_format_base_version_effort_and_unlisted_selection() {
        for (legacy, version, effort, listed) in [
            ("other[medium]", "2.2.2", "medium", "other[medium]"),
            (
                "gpt-6.1-sol-extra[medium]",
                "2.2.2",
                "medium",
                "gpt-6.1-sol-extra[medium]",
            ),
            (
                "gpt-6.1-sol[medium]trailing",
                "2.2.2",
                "medium",
                "gpt-6.1-sol[medium]trailing",
            ),
            (
                "gpt-6.1-sol[[medium]]",
                "2.2.2",
                "medium",
                "gpt-6.1-sol[[medium]]",
            ),
            ("gpt-6.1-sol[]", "2.2.2", "medium", "gpt-6.1-sol[]"),
            (
                "gpt-6.1-sol[medium]",
                "9.0.0",
                "medium",
                "gpt-6.1-sol[medium]",
            ),
            ("gpt-6.1-sol[high]", "2.2.2", "medium", "gpt-6.1-sol[high]"),
            (
                "gpt-6.1-sol[medium]",
                "2.2.2",
                "medium",
                "gpt-6.1-sol[high]",
            ),
        ] {
            let mut c =
                ProtocolCapture::with_mode(MODEL.into(), SelectionMode::InitialProvider).unwrap();
            composite(&mut c, legacy, version, effort, listed);
            assert!(
                c.client(&prompt("session-a")).is_err(),
                "accepted bad legacy metadata"
            );
            assert_eq!(c.snapshot.prompts, 0);
        }
    }
    #[cfg(unix)]
    #[test]
    fn capture_publication_refuses_a_directory_accessible_to_other_users() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = root.path().join("capture.json");
        assert!(EvidenceFile::create(&path).is_err());
        assert!(!path.exists());
    }
    #[test]
    fn exclusive_capture_publication_refuses_reuse_and_roundtrips() {
        #[cfg(windows)]
        let root = {
            let profile = surge_persistence::RuntimeHomeOwner::user_profile_path().unwrap();
            tempfile::tempdir_in(profile).unwrap()
        };
        #[cfg(not(windows))]
        let root = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        let owner =
            surge_persistence::RuntimeHomeOwner::prepare(&root.path().join("state")).unwrap();
        #[cfg(windows)]
        let path = owner.path().join("daemon/capture.json");
        #[cfg(not(windows))]
        let path = {
            use std::os::unix::fs::DirBuilderExt;
            let directory = root.path().join("private");
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&directory)
                .unwrap();
            directory.join("capture.json")
        };
        let mut file = EvidenceFile::create(&path).unwrap();
        let c = ProtocolCapture::new(MODEL.into()).unwrap();
        file.write(&c.snapshot).unwrap();
        assert!(EvidenceFile::create(&path).is_err());
        drop(file);
        assert_eq!(
            serde_json::from_slice::<Capture>(&std::fs::read(path).unwrap()).unwrap(),
            c.snapshot
        );
    }
    #[test]
    fn legacy_effort_must_agree_with_reported_current_thought_level() {
        let mut c =
            ProtocolCapture::with_mode(MODEL.into(), SelectionMode::InitialProvider).unwrap();
        c.client(&json!({"id":1,"method":"initialize","params":{}}))
            .unwrap();
        c.provider(&json!({"id":1,"result":{"agentInfo":{"name":"@agentclientprotocol/codex-acp","version":"2.2.2"}}})).unwrap();
        c.client(&json!({"id":2,"method":"session/new","params":{}}))
            .unwrap();
        let _ = c.provider(&json!({"id":2,"result":{"sessionId":"session-a","configOptions":[
            {"id":"model","category":"model","type":"select","currentValue":MODEL,"options":[{"value":MODEL}]},
            {"id":"effort","category":"thought_level","type":"select","currentValue":"medium","options":[{"value":"medium"},{"value":"high"}]}],
            "models":{"currentModelId":"gpt-6.1-sol[high]","availableModels":[{"modelId":"gpt-6.1-sol[high]"}]}}}));
        assert!(c.client(&prompt("session-a")).is_err());
        assert_eq!(c.snapshot.prompts, 0);
    }
    fn ack(id: u64) -> Value {
        json!({"id":id,"result":{"configOptions":[{"id":"model","category":"model","type":"select","currentValue":MODEL}]}})
    }
    #[test]
    fn delayed_selection_ack_cannot_restore_readiness_after_newer_change_or_update() {
        for update in [false, true] {
            let mut c = ProtocolCapture::new(MODEL.into()).unwrap();
            session(&mut c);
            c.client(&json!({"id":3,"method":"session/set_config_option","params":{"sessionId":"session-a","configId":"model","value":MODEL}})).unwrap();
            if update {
                c.provider(&json!({"method":"session/update","params":{"sessionId":"session-a","update":{"sessionUpdate":"config_option_update","configOptions":[{"id":"model","category":"model","type":"select","currentValue":"other"}]}}})).unwrap();
            } else {
                c.client(&json!({"id":5,"method":"session/set_config_option","params":{"sessionId":"session-a","configId":"model","value":"other"}})).unwrap();
                c.provider(&json!({"id":5,"error":{"code":-1}})).unwrap();
            }
            c.provider(&ack(3)).unwrap();
            assert!(
                c.client(&prompt("session-a")).is_err(),
                "stale selection restored admission"
            );
            assert_eq!(c.snapshot.prompts, 0);
        }
    }
    #[test]
    fn delayed_opening_reply_cannot_take_over_newer_acknowledged_session() {
        let mut c = ProtocolCapture::new(MODEL.into()).unwrap();
        c.client(&json!({"id":1,"method":"initialize","params":{}}))
            .unwrap();
        c.provider(&json!({"id":1,"result":{"agentInfo":{"name":"adapter","version":"1"}}}))
            .unwrap();
        c.client(&json!({"id":2,"method":"session/new","params":{}}))
            .unwrap();
        c.client(&json!({"id":20,"method":"session/new","params":{}}))
            .unwrap();
        let response = |id, session_id| json!({"id":id,"result":{"sessionId":session_id,"configOptions":[{"id":"model","category":"model","type":"select","currentValue":"other","options":[{"value":MODEL}]}]}});
        c.provider(&response(20, "session-b")).unwrap();
        c.client(&json!({"id":21,"method":"session/set_config_option","params":{"sessionId":"session-b","configId":"model","value":MODEL}})).unwrap();
        c.provider(&ack(21)).unwrap();
        c.provider(&response(2, "session-a")).unwrap();
        assert!(
            c.client(&prompt("session-a")).is_err(),
            "old opening inherited a newer session acknowledgement"
        );
        assert_eq!(c.snapshot.provider_session.as_deref(), Some("session-b"));
        c.client(&prompt("session-b")).unwrap();
    }
    #[tokio::test]
    async fn forward_flushes_a_buffered_writer_before_returning() {
        use tokio::io::{AsyncReadExt, BufWriter};
        let (write, mut read) = tokio::io::duplex(256);
        let mut write = BufWriter::with_capacity(4096, write);
        let frame = b"{\"id\":1}\n";
        forward(&mut write, frame).await.unwrap();
        let mut observed = vec![0; frame.len()];
        tokio::time::timeout(
            std::time::Duration::from_millis(200),
            read.read_exact(&mut observed),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(observed, frame);
    }
}
