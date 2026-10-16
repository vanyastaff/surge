//! Test-only ACP metadata oracle. It never retains raw messages.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SelectionMode {
    ExplicitSetAck,
    InitialProvider,
}
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PromptPolicy {
    #[default]
    StrictOne,
    NativeSix,
}
fn strict_limit() -> usize {
    1
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Rejection {
    PromptLimitExhausted,
    OverlappingPrompt,
    RepeatedRequest,
    RequestLimit,
    WrongSession,
    ModelProofUnavailable,
    StaleAcknowledgement,
    MalformedResponse,
    RepeatedOpening,
    InitializationOrder,
    UnsupportedClientMethod,
    MalformedFrame,
    IncompletePrompt,
    ModelProofInvalidated,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Client,
    Provider,
    Eof,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Rejected {
    pub code: Rejection,
    pub direction: Direction,
    pub turn: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(try_from = "RawCapture")]
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
    #[serde(default)]
    pub prompt_policy: PromptPolicy,
    #[serde(default = "strict_limit")]
    pub prompt_limit: usize,
    #[serde(default)]
    pub successful_responses: usize,
    #[serde(default)]
    pub error_responses: usize,
    #[serde(default)]
    pub prompt_pending: bool,
    #[serde(default)]
    pub rejection: Option<Rejected>,
    pub legacy_model_raw: Option<String>,
    pub legacy_model_base: Option<String>,
    pub legacy_model_effort: Option<String>,
}
#[derive(Deserialize)]
struct RawCapture {
    pub required_model: String,
    pub selection_mode: SelectionMode,
    pub selection_source: Option<SelectionMode>,
    pub adapter_name: Option<String>,
    pub adapter_version: Option<String>,
    pub provider_session: Option<String>,
    pub offered_models: Vec<String>,
    pub selected_model: Option<String>,
    pub prompts: usize,
    #[serde(default)]
    pub prompt_policy: PromptPolicy,
    #[serde(default = "strict_limit")]
    pub prompt_limit: usize,
    #[serde(default)]
    pub successful_responses: usize,
    #[serde(default)]
    pub error_responses: usize,
    #[serde(default)]
    pub prompt_pending: bool,
    #[serde(default)]
    pub rejection: Option<Rejected>,
    pub legacy_model_raw: Option<String>,
    pub legacy_model_base: Option<String>,
    pub legacy_model_effort: Option<String>,
}
impl TryFrom<RawCapture> for Capture {
    type Error = &'static str;
    fn try_from(raw: RawCapture) -> Result<Self, Self::Error> {
        let expected = match raw.prompt_policy {
            PromptPolicy::StrictOne => 1,
            PromptPolicy::NativeSix => 6,
        };
        if raw.prompt_limit != expected
            || raw.prompts > expected
            || raw.successful_responses.saturating_add(raw.error_responses) > raw.prompts
        {
            return Err("inconsistent capture policy");
        }
        Ok(Self {
            required_model: raw.required_model,
            selection_mode: raw.selection_mode,
            selection_source: raw.selection_source,
            adapter_name: raw.adapter_name,
            adapter_version: raw.adapter_version,
            provider_session: raw.provider_session,
            offered_models: raw.offered_models,
            selected_model: raw.selected_model,
            prompts: raw.prompts,
            prompt_policy: raw.prompt_policy,
            prompt_limit: raw.prompt_limit,
            successful_responses: raw.successful_responses,
            error_responses: raw.error_responses,
            prompt_pending: raw.prompt_pending,
            rejection: raw.rejection,
            legacy_model_raw: raw.legacy_model_raw,
            legacy_model_base: raw.legacy_model_base,
            legacy_model_effort: raw.legacy_model_effort,
        })
    }
}
impl Capture {
    pub fn strict_policy(&self) -> bool {
        self.prompt_policy == PromptPolicy::StrictOne && self.prompt_limit == 1
    }
}
#[derive(Clone)]
enum Pending {
    Prompt {
        opening_epoch: u64,
        session: String,
        turn: usize,
    },
    Mode,
    UnrelatedConfig,
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
#[derive(Clone, Copy, PartialEq, Eq)]
enum RequestKind {
    Initialize,
    New,
    Mode,
    Config,
    Prompt,
}
impl Pending {
    fn kind(&self) -> RequestKind {
        match self {
            Self::Initialize => RequestKind::Initialize,
            Self::New { .. } => RequestKind::New,
            Self::Mode => RequestKind::Mode,
            Self::Selection { .. } | Self::UnrelatedConfig => RequestKind::Config,
            Self::Prompt { .. } => RequestKind::Prompt,
        }
    }
}
struct Reservation {
    kind: RequestKind,
    completed: bool,
}
pub struct ProtocolCapture {
    pub snapshot: Capture,
    pending: std::collections::HashMap<String, Pending>,
    model_option: Option<String>,
    opening_epoch: u64,
    selection_epoch: u64,
    reservations: std::collections::HashMap<String, Reservation>,
    initialized: bool,
    initialize_sent: bool,
    opening_sent: bool,
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
                prompt_policy: PromptPolicy::StrictOne,
                prompt_limit: 1,
                successful_responses: 0,
                error_responses: 0,
                prompt_pending: false,
                rejection: None,
                legacy_model_raw: None,
                legacy_model_base: None,
                legacy_model_effort: None,
            },
            pending: Default::default(),
            model_option: None,
            opening_epoch: 0,
            selection_epoch: 0,
            reservations: Default::default(),
            initialized: false,
            initialize_sent: false,
            opening_sent: false,
        })
    }
    pub fn with_mode(model: String, mode: SelectionMode) -> Result<Self, &'static str> {
        let mut capture = Self::new(model)?;
        capture.snapshot.selection_mode = mode;
        Ok(capture)
    }
    pub fn native(model: String, mode: SelectionMode) -> Result<Self, &'static str> {
        let mut capture = Self::with_mode(model, mode)?;
        capture.snapshot.prompt_policy = PromptPolicy::NativeSix;
        capture.snapshot.prompt_limit = 6;
        Ok(capture)
    }
    fn is_native(&self) -> bool {
        self.snapshot.prompt_policy == PromptPolicy::NativeSix
    }
    fn reject(&mut self, code: Rejection, direction: Direction) {
        if self.snapshot.rejection.is_none() {
            self.snapshot.rejection = Some(Rejected {
                code,
                direction,
                turn: match direction {
                    Direction::Client => self.snapshot.prompts + 1,
                    Direction::Provider | Direction::Eof => self.snapshot.prompts.max(1),
                },
            });
        }
    }
    pub fn framing_failure(&mut self, direction: Direction) {
        self.reject(Rejection::MalformedFrame, direction);
    }
    pub fn eof(&mut self) -> Result<(), &'static str> {
        if self.is_native() && self.snapshot.prompt_pending {
            self.reject(Rejection::IncompletePrompt, Direction::Eof);
            return Err("incomplete prompt");
        }
        Ok(())
    }
    pub fn client(&mut self, frame: &Value) -> Result<(), &'static str> {
        if self.is_native() {
            if self.snapshot.rejection.is_some() {
                return Err("observation rejected");
            }
            if let Err(code) = self.native_client(frame) {
                self.reject(code, Direction::Client);
                return Err("native client rejected");
            }
        }
        let result = self.client_frame(frame);
        if result.is_err() {
            self.reject(Rejection::ModelProofUnavailable, Direction::Client);
        }
        result
    }
    fn native_client(&mut self, frame: &Value) -> Result<(), Rejection> {
        if !frame.is_object() {
            return Err(Rejection::MalformedFrame);
        }
        let Some(method) = frame.get("method") else {
            // Replies to provider requests use their independent namespace.
            if request_id(frame).is_err()
                || (frame.get("result").is_some() == frame.get("error").is_some())
            {
                return Err(Rejection::MalformedFrame);
            }
            return Ok(());
        };
        if frame.get("result").is_some()
            || frame.get("error").is_some()
            || !frame["params"].is_object()
        {
            return Err(Rejection::MalformedFrame);
        }
        let method = method.as_str().ok_or(Rejection::MalformedFrame)?;
        if !matches!(
            method,
            "initialize"
                | "session/new"
                | "session/set_mode"
                | "session/set_config_option"
                | "session/prompt"
                | "session/cancel"
        ) {
            return Err(Rejection::UnsupportedClientMethod);
        }
        if method == "session/cancel" {
            if frame.get("id").is_some() {
                return Err(Rejection::MalformedFrame);
            }
            return self.same_session(&frame["params"]);
        }
        let id = request_id(frame).map_err(|_| Rejection::MalformedFrame)?;
        if self.reservations.contains_key(&id) {
            return Err(Rejection::RepeatedRequest);
        }
        if self.reservations.len() >= 64 || self.pending.len() >= 16 {
            return Err(Rejection::RequestLimit);
        }
        let kind = match method {
            "initialize" => RequestKind::Initialize,
            "session/new" => RequestKind::New,
            "session/set_mode" => RequestKind::Mode,
            "session/set_config_option" => RequestKind::Config,
            "session/prompt" => RequestKind::Prompt,
            _ => return Err(Rejection::UnsupportedClientMethod),
        };
        self.reservations.insert(
            id,
            Reservation {
                kind,
                completed: false,
            },
        );
        match method {
            "initialize" => {
                if self.initialize_sent {
                    return Err(Rejection::InitializationOrder);
                }
                self.initialize_sent = true;
            },
            "session/new" => {
                if !self.initialized {
                    return Err(Rejection::InitializationOrder);
                }
                if self.opening_sent {
                    return Err(Rejection::RepeatedOpening);
                }
                self.opening_sent = true;
            },
            _ => {
                self.same_session(&frame["params"])?;
                if method == "session/prompt" {
                    if self.snapshot.prompt_pending {
                        return Err(Rejection::OverlappingPrompt);
                    }
                    if self.snapshot.prompts >= 6 {
                        return Err(Rejection::PromptLimitExhausted);
                    }
                } else if method != "session/cancel" && self.snapshot.prompt_pending {
                    return Err(Rejection::OverlappingPrompt);
                }
            },
        }
        Ok(())
    }
    fn same_session(&self, params: &Value) -> Result<(), Rejection> {
        if self.snapshot.provider_session.is_none()
            || params["sessionId"].as_str() != self.snapshot.provider_session.as_deref()
        {
            return Err(Rejection::WrongSession);
        }
        Ok(())
    }
    fn client_frame(&mut self, frame: &Value) -> Result<(), &'static str> {
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
                if self.is_native() && self.model_option.as_deref() != params["configId"].as_str() {
                    identifier(&params["configId"])?;
                    identifier(&params["value"])?;
                    Pending::UnrelatedConfig
                } else {
                    Pending::Selection {
                        opening_epoch: self.opening_epoch,
                        selection_epoch: self.selection_epoch,
                        session: identifier(&params["sessionId"])?,
                        option: identifier(&params["configId"])?,
                        value: identifier(&params["value"])?,
                    }
                }
            },
            "session/set_mode" if self.is_native() => {
                self.invalidate_selection()?;
                Pending::Mode
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
                    || (!self.is_native() && self.snapshot.prompts != 0)
                {
                    return Err("exact-model prompt admission rejected");
                }
                self.snapshot.prompts += 1;
                if self.is_native() {
                    self.snapshot.prompt_pending = true;
                    Pending::Prompt {
                        opening_epoch: self.opening_epoch,
                        session: identifier(&params["sessionId"])?,
                        turn: self.snapshot.prompts,
                    }
                } else {
                    return Ok(());
                }
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
        if self.is_native() {
            if self.snapshot.rejection.is_some() {
                return Err("observation rejected");
            }
            if let Err(code) = self.native_provider(frame) {
                self.reject(code, Direction::Provider);
                return Err("native provider rejected");
            }
        }
        let result = self.provider_frame(frame);
        if let Err(error) = result {
            let code = if error == "stale selection acknowledgement" {
                Rejection::StaleAcknowledgement
            } else {
                Rejection::ModelProofUnavailable
            };
            self.reject(code, Direction::Provider);
        }
        result
    }
    fn native_provider(&mut self, frame: &Value) -> Result<(), Rejection> {
        if !frame.is_object() {
            return Err(Rejection::MalformedFrame);
        }
        if frame.get("method").is_some() {
            if frame.get("result").is_some()
                || frame.get("error").is_some()
                || !frame["method"].is_string()
            {
                return Err(Rejection::MalformedFrame);
            }
            return Ok(());
        }
        let id = request_id(frame).map_err(|_| Rejection::MalformedResponse)?;
        if self
            .reservations
            .get(&id)
            .is_some_and(|reservation| reservation.completed)
        {
            return Err(Rejection::RepeatedRequest);
        }
        if let Some(pending) = self.pending.get(&id) {
            if self
                .reservations
                .get(&id)
                .is_none_or(|reservation| reservation.kind != pending.kind())
            {
                return Err(Rejection::MalformedResponse);
            }
            if frame.get("result").is_some() == frame.get("error").is_some() {
                return Err(Rejection::MalformedResponse);
            }
            self.reservations
                .get_mut(&id)
                .ok_or(Rejection::MalformedResponse)?
                .completed = true;
        }
        Ok(())
    }
    fn native_model_option<'a>(&self, result: &'a Value) -> Result<&'a Value, &'static str> {
        let options = result["configOptions"]
            .as_array()
            .ok_or("model options missing")?;
        let related: Vec<_> = options
            .iter()
            .filter(|option| {
                option["category"] == "model"
                    || (self.model_option.is_some()
                        && option["id"].as_str() == self.model_option.as_deref())
            })
            .collect();
        let [option] = related.as_slice() else {
            return Err("model option ambiguous");
        };
        if option["category"] != "model" || option["type"] != "select" {
            return Err("model option malformed");
        }
        Ok(option)
    }
    fn refresh_model(
        &mut self,
        result: &Value,
        allow_initial_identity: bool,
    ) -> Result<String, &'static str> {
        self.snapshot.offered_models.clear();
        let option = self.native_model_option(result)?;
        let id = identifier(&option["id"])?;
        if !allow_initial_identity && self.model_option.as_ref() != Some(&id) {
            return Err("model option identity changed");
        }
        let offers = select_values(option)?;
        let current = identifier(&option["currentValue"])?;
        self.snapshot.offered_models = offers;
        if current != self.snapshot.required_model
            || !self.snapshot.offered_models.contains(&current)
        {
            return Err("current exact model unavailable");
        }
        if let Some(raw) = result
            .get("models")
            .and_then(|models| models.get("currentModelId"))
        {
            let raw = raw.as_str().ok_or("legacy model invalid")?;
            let (base, effort) = legacy_selection(raw, &self.snapshot, result)?;
            if base != current {
                return Err("conflicting model metadata");
            }
            self.snapshot.legacy_model_raw = Some(raw.into());
            self.snapshot.legacy_model_base = Some(base);
            self.snapshot.legacy_model_effort = effort;
        }
        Ok(current)
    }
    fn native_update(&mut self, frame: &Value) -> Result<(), &'static str> {
        if frame["params"]["sessionId"].as_str() != self.snapshot.provider_session.as_deref() {
            return Err("update session mismatch");
        }
        let update = &frame["params"]["update"];
        let options = update["configOptions"]
            .as_array()
            .ok_or("update options missing")?;
        let model_related = options.iter().any(|option| {
            option["category"] == "model" || option["id"].as_str() == self.model_option.as_deref()
        });
        if model_related {
            if self.refresh_model(update, false).is_err() {
                self.invalidate_selection()?;
                self.reject(Rejection::ModelProofInvalidated, Direction::Provider);
                return Err("model proof invalidated");
            }
        } else if options.iter().any(|option| {
            option["category"] != "thought_level"
                || option["type"] != "select"
                || identifier(&option["id"]).is_err()
                || identifier(&option["currentValue"]).is_err()
                || select_values(option).is_err()
        }) {
            self.invalidate_selection()?;
            return Err("ambiguous option update");
        }
        Ok(())
    }
    fn provider_frame(&mut self, frame: &Value) -> Result<(), &'static str> {
        // Provider-to-client requests have a separate identity namespace.
        if frame.get("method").is_some() {
            if self.is_native()
                && frame["method"] == "session/update"
                && frame["params"]["update"]["sessionUpdate"] == "current_mode_update"
            {
                // A mode update does not prove a model change. Conservatively
                // require renewed exact-model proof for this unproven transition.
                self.same_session(&frame["params"])
                    .map_err(|_| "mode update session mismatch")?;
                identifier(&frame["params"]["update"]["currentModeId"])?;
                self.invalidate_selection()?;
                if self.snapshot.prompt_pending {
                    self.reject(Rejection::ModelProofInvalidated, Direction::Provider);
                    return Err("model proof invalidated");
                }
                return Ok(());
            }
            if self.is_native()
                && frame["method"] == "session/update"
                && frame["params"]["update"]["sessionUpdate"] == "config_option_update"
            {
                return self.native_update(frame);
            }
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
        if let Pending::Prompt {
            opening_epoch,
            session,
            turn,
        } = &pending
        {
            if *opening_epoch != self.opening_epoch
                || self.snapshot.provider_session.as_ref() != Some(session)
                || *turn != self.snapshot.prompts
            {
                return Err("prompt opening mismatch");
            }
            self.snapshot.prompt_pending = false;
            if frame.get("error").is_some() {
                self.snapshot.error_responses += 1;
            } else {
                self.snapshot.successful_responses += 1;
            }
            return Ok(());
        }
        if frame.get("error").is_some() {
            if self.is_native() && matches!(pending, Pending::Initialize | Pending::New { .. }) {
                return Err("handshake failed");
            }
            return Ok(());
        }
        let result = frame.get("result").ok_or("missing correlated result")?;
        match pending {
            Pending::Prompt { .. } | Pending::Mode => {},
            Pending::UnrelatedConfig => {
                if result["configOptions"].as_array().is_some_and(|options| {
                    options.iter().any(|option| {
                        option["category"] == "model"
                            || option["id"].as_str() == self.model_option.as_deref()
                    })
                }) && self.refresh_model(result, false).is_err()
                {
                    self.invalidate_selection()?;
                    return Err("model proof invalidated");
                }
            },
            Pending::Initialize => {
                self.snapshot.adapter_name = Some(identifier(&result["agentInfo"]["name"])?);
                self.snapshot.adapter_version = Some(identifier(&result["agentInfo"]["version"])?);
                self.initialized = true;
            },
            Pending::New { opening_epoch } => {
                if opening_epoch != self.opening_epoch {
                    return Ok(());
                }
                self.invalidate_selection()?;
                self.snapshot.provider_session = Some(identifier(&result["sessionId"])?);
                let option = if self.is_native() {
                    self.native_model_option(result)?
                } else {
                    let Some(option) = model_option(result) else {
                        return Ok(());
                    };
                    option
                };
                self.model_option = Some(identifier(&option["id"])?);
                self.snapshot.offered_models = select_values(option)?;
                if self.is_native() {
                    // Even explicit selection needs one structurally valid offered-model identity.
                    identifier(&option["currentValue"])?;
                    if !self
                        .snapshot
                        .offered_models
                        .contains(&self.snapshot.required_model)
                    {
                        return Err("required model not offered");
                    }
                }
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
                    if self.is_native() {
                        return Err("stale selection acknowledgement");
                    }
                    return Ok(());
                }
                self.clear_selection();
                if self.is_native() {
                    let current = self.refresh_model(result, false)?;
                    self.snapshot.selected_model = Some(current);
                    self.snapshot.selection_source = Some(SelectionMode::ExplicitSetAck);
                    return Ok(());
                }
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
    let bytes = serde_json::to_vec(capture).map_err(|_| "capture encode")?;
    if bytes.len() > 65536 {
        return Err("capture size limit");
    }
    file.rewind().map_err(|_| "capture seek")?;
    file.set_len(0).map_err(|_| "capture truncate")?;
    file.write_all(&bytes).map_err(|_| "capture write")?;
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
        if !capture.strict_policy()
            && !(capture.prompt_policy == PromptPolicy::NativeSix && capture.prompt_limit == 6)
        {
            return Err("inconsistent capture policy");
        }
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
    fn native_serial_second_prompt_is_observable_after_correlated_response() {
        let mut c = ProtocolCapture::native(MODEL.into(), SelectionMode::ExplicitSetAck).unwrap();
        session(&mut c);
        select(&mut c, MODEL);
        c.client(&prompt("session-a")).unwrap();
        c.provider(&json!({"id":4,"result":{"stopReason":"end_turn"}}))
            .unwrap();
        let mut second = prompt("session-a");
        second["id"] = json!(5);
        c.client(&second)
            .expect("second serial exact-model prompt must be observable");
        assert_eq!(c.snapshot.prompts, 2);
    }
    fn native_ready() -> ProtocolCapture {
        let mut c = ProtocolCapture::native(MODEL.into(), SelectionMode::ExplicitSetAck).unwrap();
        session(&mut c);
        select(&mut c, MODEL);
        c
    }
    fn numbered_prompt(id: u64) -> Value {
        let mut frame = prompt("session-a");
        frame["id"] = json!(id);
        frame
    }
    fn prompt_ack(id: u64) -> Value {
        json!({"id":id,"result":{"stopReason":"end_turn"}})
    }
    fn model_update(options: Value) -> Value {
        json!({"method":"session/update","params":{"sessionId":"session-a","update":{"sessionUpdate":"config_option_update","configOptions":options}}})
    }
    fn model_options() -> Value {
        json!([{"id":"model","category":"model","type":"select","currentValue":MODEL,"options":[{"value":MODEL}]}])
    }
    #[test]
    fn native_six_serial_turns_are_bounded_in_both_selection_modes() {
        for mode in [
            SelectionMode::ExplicitSetAck,
            SelectionMode::InitialProvider,
        ] {
            let mut c = ProtocolCapture::native(MODEL.into(), mode).unwrap();
            if mode == SelectionMode::InitialProvider {
                initial(&mut c, Some(MODEL), None, true, Some("session-a"));
            } else {
                session(&mut c);
                select(&mut c, MODEL);
            }
            for id in 10..16 {
                c.client(&numbered_prompt(id)).unwrap();
                assert!(c.snapshot.prompt_pending);
                if id == 11 {
                    c.provider(&json!({"id":id,"error":{"message":"ERROR_SENTINEL"}}))
                        .unwrap();
                } else {
                    c.provider(&prompt_ack(id)).unwrap();
                }
                assert!(!c.snapshot.prompt_pending);
            }
            assert_eq!(
                (
                    c.snapshot.prompts,
                    c.snapshot.successful_responses,
                    c.snapshot.error_responses
                ),
                (6, 5, 1)
            );
            assert!(c.client(&numbered_prompt(16)).is_err());
            assert_eq!(
                c.snapshot.rejection.unwrap().code,
                Rejection::PromptLimitExhausted
            );
        }
    }
    #[test]
    fn native_overlap_replays_cross_namespace_and_response_shapes() {
        for case in [
            "overlap",
            "replay-init",
            "replay-new",
            "replay-config",
            "replay-prompt",
            "duplicate-init-response",
            "duplicate-prompt-response",
            "both",
            "neither",
            "wrong-session",
            "mode-during",
            "config-during",
        ] {
            let mut c = native_ready();
            c.client(&numbered_prompt(10)).unwrap();
            c.provider(&json!({"id":10,"method":"session/request_permission","params":{"tool":"TOOL_SENTINEL"}})).unwrap();
            assert!(
                c.snapshot.prompt_pending,
                "provider request completed client prompt"
            );
            c.client(&json!({"id":10,"result":{"outcome":"cancelled"}}))
                .unwrap();
            assert!(c.snapshot.prompt_pending);
            let rejected = match case {
                "overlap" => c.client(&numbered_prompt(11)),
                "replay-init" => { c.provider(&prompt_ack(10)).unwrap(); c.client(&numbered_prompt(1)) },
                "replay-new" => { c.provider(&prompt_ack(10)).unwrap(); c.client(&numbered_prompt(2)) },
                "replay-config" => { c.provider(&prompt_ack(10)).unwrap(); c.client(&numbered_prompt(3)) },
                "replay-prompt" => { c.provider(&prompt_ack(10)).unwrap(); c.client(&json!({"id":10,"method":"session/set_config_option","params":{"sessionId":"session-a","configId":"model","value":MODEL}})) },
                "duplicate-init-response" => c.provider(&json!({"id":1,"result":{"agentInfo":{"name":"replacement","version":"9"}}})),
                "duplicate-prompt-response" => { c.provider(&prompt_ack(10)).unwrap(); c.provider(&prompt_ack(10)) },
                "both" => c.provider(&json!({"id":10,"result":{},"error":{}})),
                "neither" => c.provider(&json!({"id":10})),
                "wrong-session" => { c.provider(&prompt_ack(10)).unwrap(); c.client(&prompt("session-b")) },
                "mode-during" => c.client(&json!({"id":20,"method":"session/set_mode","params":{"sessionId":"session-a","modeId":"code"}})),
                "config-during" => c.client(&json!({"id":20,"method":"session/set_config_option","params":{"sessionId":"session-a","configId":"model","value":MODEL}})),
                _ => unreachable!(),
            };
            assert!(rejected.is_err(), "accepted {case}");
            assert_eq!(c.snapshot.prompts, 1);
        }
    }
    #[test]
    fn native_fresh_handshake_and_closed_method_inventory() {
        for phase in 0..4 {
            for method in [
                "session/load",
                "session/resume",
                "session/fork",
                "session/set_model",
                "session/setModel",
                "_session/set_model",
                "_extension",
                "session/unknown",
            ] {
                let mut c =
                    ProtocolCapture::native(MODEL.into(), SelectionMode::ExplicitSetAck).unwrap();
                if phase > 0 {
                    c.client(&json!({"id":1,"method":"initialize","params":{}}))
                        .unwrap();
                }
                if phase > 1 {
                    c.provider(
                        &json!({"id":1,"result":{"agentInfo":{"name":"adapter","version":"1"}}}),
                    )
                    .unwrap();
                }
                if phase > 2 {
                    c.client(&json!({"id":2,"method":"session/new","params":{}}))
                        .unwrap();
                }
                assert!(
                    c.client(&json!({"id":30,"method":method,"params":{}}))
                        .is_err(),
                    "accepted {method} in phase {phase}"
                );
            }
        }
        for phase in 0..4 {
            let mut c =
                ProtocolCapture::native(MODEL.into(), SelectionMode::ExplicitSetAck).unwrap();
            if phase == 0 {
                assert!(
                    c.client(&json!({"id":2,"method":"session/new","params":{}}))
                        .is_err()
                );
            } else {
                c.client(&json!({"id":1,"method":"initialize","params":{}}))
                    .unwrap();
                if phase > 1 {
                    c.provider(
                        &json!({"id":1,"result":{"agentInfo":{"name":"adapter","version":"1"}}}),
                    )
                    .unwrap();
                }
                if phase > 2 {
                    c.client(&json!({"id":2,"method":"session/new","params":{}}))
                        .unwrap();
                }
                assert!(
                    c.client(&json!({"id":30,"method":"initialize","params":{}}))
                        .is_err()
                );
            }
        }
        let mut c = native_ready();
        assert!(
            c.client(&json!({"id":30,"method":"session/new","params":{}}))
                .is_err()
        );
    }
    #[test]
    fn native_request_budget_retains_completed_ids_and_pending_bound() {
        let mut c = native_ready();
        for id in 4..65 {
            c.client(&json!({"id":id,"method":"session/set_config_option","params":{"sessionId":"session-a","configId":"thought","value":"high"}})).unwrap();
            c.provider(&json!({"id":id,"result":{}})).unwrap();
        }
        assert_eq!(c.reservations.len(), 64);
        assert!(c.client(&numbered_prompt(65)).is_err());
        assert_eq!(c.snapshot.rejection.unwrap().code, Rejection::RequestLimit);
        let mut c = native_ready();
        for id in 4..20 {
            c.client(&json!({"id":id,"method":"session/set_config_option","params":{"sessionId":"session-a","configId":"thought","value":"high"}})).unwrap();
        }
        assert!(c.client(&numbered_prompt(20)).is_err());
        assert_eq!(c.pending.len(), 16);
    }
    #[test]
    fn native_model_metadata_is_current_for_updates_and_selection_acknowledgements() {
        for during in [false, true] {
            for kind in [
                "removed",
                "missing",
                "malformed",
                "ambiguous",
                "identity",
                "category",
                "type",
                "wrong-current",
                "conflicting-legacy",
            ] {
                let mut c = native_ready();
                if during {
                    c.client(&numbered_prompt(10)).unwrap();
                }
                let mut options = model_options();
                match kind {
                    "removed" => options[0]["options"] = json!([{"value":"other"}]),
                    "missing" => {
                        options[0].as_object_mut().unwrap().remove("options");
                    },
                    "malformed" => options[0]["options"] = json!("invalid"),
                    "ambiguous" => {
                        let duplicate = options[0].clone();
                        options.as_array_mut().unwrap().push(duplicate);
                    },
                    "identity" => options[0]["id"] = json!("other-model"),
                    "category" => options[0]["category"] = json!("thought_level"),
                    "type" => options[0]["type"] = json!("text"),
                    "wrong-current" => options[0]["currentValue"] = json!("other"),
                    _ => {},
                }
                let mut update = model_update(options.clone());
                if kind == "conflicting-legacy" {
                    update["params"]["update"]["models"] = json!({"currentModelId":"other"});
                }
                assert!(
                    c.provider(&update).is_err(),
                    "accepted update {kind} pending={during}"
                );
                assert!(c.snapshot.selected_model.is_none());
                assert!(
                    c.provider(&model_update(model_options())).is_err(),
                    "resurrected failed observation"
                );
                let mut c = native_ready();
                c.client(&json!({"id":20,"method":"session/set_config_option","params":{"sessionId":"session-a","configId":"model","value":MODEL}})).unwrap();
                let mut result = json!({"configOptions":options});
                if kind == "conflicting-legacy" {
                    result["models"] = json!({"currentModelId":"other"});
                }
                assert!(
                    c.provider(&json!({"id":20,"result":result})).is_err(),
                    "accepted ACK {kind}"
                );
                assert!(c.snapshot.selected_model.is_none());
            }
        }
    }
    #[test]
    fn native_mode_change_requires_new_ack_and_updates_never_resurrect_proof() {
        let mut c = native_ready();
        c.client(&json!({"id":20,"method":"session/set_mode","params":{"sessionId":"session-a","modeId":"code"}})).unwrap();
        c.provider(&json!({"id":20,"result":{}})).unwrap();
        c.provider(&model_update(model_options())).unwrap();
        assert!(c.snapshot.selected_model.is_none());
        c.client(&json!({"id":21,"method":"session/set_config_option","params":{"sessionId":"session-a","configId":"model","value":MODEL}})).unwrap();
        c.provider(&json!({"id":21,"result":{"configOptions":model_options()}}))
            .unwrap();
        c.client(&numbered_prompt(22)).unwrap();
        c.client(&json!({"method":"session/cancel","params":{"sessionId":"session-a"}}))
            .unwrap();
        assert!(c.snapshot.prompt_pending);
        c.provider(&prompt_ack(22)).unwrap();
        assert!(c.eof().is_ok());
    }
    #[test]
    fn native_error_response_ids_remain_reserved_and_config_collisions_cannot_prompt() {
        let mut c = native_ready();
        c.client(&numbered_prompt(10)).unwrap();
        c.provider(&json!({"id":10,"error":{"message":"ERROR_SENTINEL"}}))
            .unwrap();
        assert!(c.client(&numbered_prompt(10)).is_err());
        assert_eq!(c.snapshot.error_responses, 1);
        let mut c = native_ready();
        c.client(&json!({"id":10,"method":"session/set_config_option","params":{"sessionId":"session-a","configId":"thought","value":"high"}})).unwrap();
        assert!(c.client(&numbered_prompt(10)).is_err());
        assert_eq!(c.snapshot.prompts, 0);
    }
    #[test]
    fn native_unrelated_well_formed_thought_updates_preserve_only_existing_proof() {
        let mut c = native_ready();
        c.provider(&model_update(json!([{"id":"thought","category":"thought_level","type":"select","currentValue":"high","options":[{"value":"high"}]}]))).unwrap();
        assert_eq!(c.snapshot.selected_model.as_deref(), Some(MODEL));
        c.client(&numbered_prompt(10)).unwrap();
    }
    #[test]
    fn native_full_same_model_option_update_during_prompt_preserves_current_proof() {
        for mode in [
            SelectionMode::ExplicitSetAck,
            SelectionMode::InitialProvider,
        ] {
            let mut c = ProtocolCapture::native(MODEL.into(), mode).unwrap();
            if mode == SelectionMode::InitialProvider {
                initial(&mut c, Some(MODEL), None, true, Some("session-a"));
            } else {
                session(&mut c);
                select(&mut c, MODEL);
            }
            c.client(&numbered_prompt(10)).unwrap();
            let mut options = model_options();
            options.as_array_mut().unwrap().extend([
                json!({"id":"mode","category":"mode","type":"select","currentValue":"auto","options":[{"value":"auto"}]}),
                json!({"id":"thought","category":"thought_level","type":"select","currentValue":"high","options":[{"value":"high"}]}),
                json!({"id":"collaboration","type":"select","currentValue":"default","options":[{"value":"default"}]}),
            ]);
            c.provider(&model_update(options)).unwrap();
            assert_eq!(c.snapshot.selected_model.as_deref(), Some(MODEL));
            assert_eq!(c.snapshot.selection_source, Some(mode));
            assert!(c.snapshot.prompt_pending);
            c.provider(&prompt_ack(10)).unwrap();
            c.client(&numbered_prompt(11)).unwrap();
        }
    }
    #[test]
    fn native_provider_mode_update_invalidates_selection_and_fails_an_outstanding_turn() {
        for during in [false, true] {
            let mut c = native_ready();
            if during {
                c.client(&numbered_prompt(10)).unwrap();
            }
            let result=c.provider(&json!({"method":"session/update","params":{"sessionId":"session-a","update":{"sessionUpdate":"current_mode_update","currentModeId":"code"}}}));
            assert_eq!(result.is_err(), during);
            assert!(c.snapshot.selected_model.is_none());
            if !during {
                c.client(&json!({"id":20,"method":"session/set_config_option","params":{"sessionId":"session-a","configId":"model","value":MODEL}})).unwrap();
                c.provider(&json!({"id":20,"result":{"configOptions":model_options()}}))
                    .unwrap();
                c.client(&numbered_prompt(21)).unwrap();
            }
        }
    }
    #[test]
    fn native_unknown_response_does_not_close_prompt_and_eof_is_incomplete() {
        let mut c = native_ready();
        c.client(&numbered_prompt(10)).unwrap();
        c.provider(&prompt_ack(999)).unwrap();
        assert!(c.snapshot.prompt_pending);
        assert!(c.eof().is_err());
        assert_eq!(
            c.snapshot.rejection.unwrap().code,
            Rejection::IncompletePrompt
        );
        assert_eq!(c.snapshot.successful_responses, 0);
    }
    #[test]
    fn framing_failure_has_only_safe_metadata_and_cannot_be_resurrected() {
        let mut c = native_ready();
        c.framing_failure(Direction::Provider);
        assert_eq!(
            c.snapshot.rejection.as_ref().unwrap().code,
            Rejection::MalformedFrame
        );
        assert!(c.client(&numbered_prompt(10)).is_err());
        assert_eq!(c.snapshot.prompts, 0);
    }
    #[test]
    fn native_privacy_and_legacy_capture_defaults_and_policy_consistency() {
        let mut c = native_ready();
        let mut frame = numbered_prompt(10);
        frame["id"] = json!("REQUEST_ID_SENTINEL");
        frame["params"]["credentials"] = json!("AUTH_SENTINEL");
        c.client(&frame).unwrap();
        c.provider(&json!({"id":"REQUEST_ID_SENTINEL","error":{"message":"RAW_ERROR_SENTINEL","data":{"output":"OUTPUT_SENTINEL"}}})).unwrap();
        let bytes = serde_json::to_string(&c.snapshot).unwrap();
        for secret in [
            "SECRET_PROMPT",
            "REQUEST_ID_SENTINEL",
            "AUTH_SENTINEL",
            "RAW_ERROR_SENTINEL",
            "OUTPUT_SENTINEL",
        ] {
            assert!(!bytes.contains(secret));
        }
        let mut raw =
            serde_json::to_value(ProtocolCapture::new(MODEL.into()).unwrap().snapshot).unwrap();
        for field in [
            "prompt_policy",
            "prompt_limit",
            "successful_responses",
            "error_responses",
            "prompt_pending",
            "rejection",
        ] {
            raw.as_object_mut().unwrap().remove(field);
        }
        let legacy: Capture = serde_json::from_value(raw.clone()).unwrap();
        assert!(legacy.strict_policy());
        raw["prompt_limit"] = json!(6);
        assert!(serde_json::from_value::<Capture>(raw.clone()).is_err());
        raw["prompt_policy"] = json!("native_six");
        assert!(
            !serde_json::from_value::<Capture>(raw.clone())
                .unwrap()
                .strict_policy()
        );
        raw["prompt_limit"] = json!(1);
        assert!(serde_json::from_value::<Capture>(raw).is_err());
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
        let retained = std::fs::read(&path).unwrap();
        let mut oversized = c.snapshot.clone();
        oversized.required_model = "x".repeat(65537);
        assert!(file.write(&oversized).is_err());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            retained,
            "oversized snapshot replaced retained evidence"
        );
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
