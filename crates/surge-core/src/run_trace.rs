//! A run's event log as an OpenTelemetry trace.
//!
//! [`to_otlp_json`] folds the events of one run into an OTLP/JSON
//! `ExportTraceServiceRequest`, the payload Jaeger, Tempo, Honeycomb and any
//! OTLP collector accept on `/v1/traces`. Nothing here runs at execution time:
//! the trace is *derived* from the append-only log, so it is identical however
//! many times it is produced, works for a run that finished last month, and adds
//! no dependency on an OpenTelemetry SDK.
//!
//! Shape: one `surge.run` span; one child span per stage attempt
//! (`StageEntered` → `StageCompleted` / `StageFailed`); span events for reported
//! outcomes, hook rejections, verified tasks, tool calls and token usage.
//! Trace and span ids are derived from the run id and event sequence, so they are
//! stable across exports.

use serde_json::{Value, json};

use crate::content_hash::ContentHash;
use crate::id::RunId;
use crate::run_event::{EventPayload, RunEvent};

/// OTLP status codes (`Status.StatusCode`).
const STATUS_UNSET: u8 = 0;
const STATUS_OK: u8 = 1;
const STATUS_ERROR: u8 = 2;

/// Convert one run's events into an OTLP/JSON trace request.
///
/// `events` should be in sequence order, as read from the run's log. An empty
/// slice yields a valid request with no spans.
#[must_use]
pub fn to_otlp_json(run_id: RunId, events: &[RunEvent]) -> Value {
    let run = run_id.to_string();
    let trace_id = hex_id(&format!("trace:{run}"), 16);
    let root_id = hex_id(&format!("span:{run}:root"), 8);

    let mut spans = Vec::new();
    let mut open: Option<StageSpan> = None;

    for event in events {
        let at = nanos(event);
        match &event.payload {
            EventPayload::StageEntered { node, attempt } => {
                if let Some(previous) = open.take() {
                    spans.push(previous.finish(&trace_id, &root_id, at, None));
                }
                open = Some(StageSpan::new(&run, event.seq, node.as_str(), *attempt, at));
            },
            EventPayload::StageCompleted { node, outcome } => {
                if let Some(mut stage) = take_if_node(&mut open, node.as_str()) {
                    stage.attr("surge.outcome", outcome.as_str());
                    spans.push(stage.finish(&trace_id, &root_id, at, Some(true)));
                }
            },
            EventPayload::StageFailed {
                node,
                reason,
                retry_available,
            } => {
                if let Some(mut stage) = take_if_node(&mut open, node.as_str()) {
                    stage.attr("surge.retry_available", *retry_available);
                    stage.error = Some(reason.clone());
                    spans.push(stage.finish(&trace_id, &root_id, at, Some(false)));
                }
            },
            other => {
                if let Some(stage) = open.as_mut() {
                    stage.observe(other, at);
                }
            },
        }
    }

    let (start, end) = match (events.first(), events.last()) {
        (Some(first), Some(last)) => (nanos(first), nanos(last)),
        _ => (0, 0),
    };
    if let Some(stage) = open.take() {
        spans.push(stage.finish(&trace_id, &root_id, end, None));
    }

    let (status, message, mut root_attrs) = run_outcome(events);
    root_attrs.push(attr("surge.run_id", run.as_str()));
    let mut all = vec![span_json(
        &trace_id,
        &root_id,
        None,
        "surge.run",
        start,
        end,
        root_attrs,
        Vec::new(),
        status,
        message,
    )];
    all.extend(spans);

    json!({
        "resourceSpans": [{
            "resource": { "attributes": [
                attr("service.name", "surge"),
                attr("surge.run_id", run.as_str()),
            ]},
            "scopeSpans": [{
                "scope": { "name": "surge.run_trace", "version": env!("CARGO_PKG_VERSION") },
                "spans": all,
            }],
        }],
    })
}

/// Take the open span only when it belongs to `node`.
///
/// A completion naming a different node (an interleaved or replayed log) must
/// leave the open span alone, not silently discard it.
fn take_if_node(open: &mut Option<StageSpan>, node: &str) -> Option<StageSpan> {
    if open.as_ref().is_some_and(|stage| stage.node == node) {
        open.take()
    } else {
        None
    }
}

/// One stage attempt while its span is still open.
struct StageSpan {
    span_id: String,
    node: String,
    start: u64,
    attrs: Vec<Value>,
    events: Vec<Value>,
    error: Option<String>,
}

impl StageSpan {
    fn new(run: &str, seq: u64, node: &str, attempt: u32, start: u64) -> Self {
        Self {
            span_id: hex_id(&format!("span:{run}:{seq}"), 8),
            node: node.to_owned(),
            start,
            attrs: vec![
                attr("surge.node", node),
                attr("surge.attempt", i64::from(attempt)),
            ],
            events: Vec::new(),
            error: None,
        }
    }

    fn attr(&mut self, key: &str, value: impl Into<AttrValue>) {
        self.attrs.push(attr(key, value));
    }

    fn observe(&mut self, payload: &EventPayload, at: u64) {
        match payload {
            EventPayload::SessionOpened {
                agent, agent_id, ..
            } => {
                self.attr("surge.profile", agent.as_str());
                if let Some(id) = agent_id {
                    self.attr("surge.agent_id", id.as_str());
                }
            },
            EventPayload::OutcomeReported {
                outcome, summary, ..
            } => self.events.push(span_event(
                "surge.outcome_reported",
                at,
                vec![
                    attr("surge.outcome", outcome.as_str()),
                    attr("surge.summary", summary.as_str()),
                ],
            )),
            EventPayload::OutcomeRejectedByHook {
                outcome, hook_id, ..
            } => self.events.push(span_event(
                "surge.outcome_rejected",
                at,
                vec![
                    attr("surge.outcome", outcome.as_str()),
                    attr("surge.hook_id", hook_id.as_str()),
                ],
            )),
            EventPayload::TaskVerified { task_id, .. } => self.events.push(span_event(
                "surge.task_verified",
                at,
                vec![attr("surge.task_id", task_id.as_str())],
            )),
            EventPayload::ToolCalled {
                tool, mcp_server, ..
            } => {
                let mut attrs = vec![attr("tool.name", tool.as_str())];
                if let Some(server) = mcp_server {
                    attrs.push(attr("mcp.server", server.as_str()));
                }
                self.events.push(span_event("tool.call", at, attrs));
            },
            EventPayload::TokensConsumed {
                prompt_tokens,
                output_tokens,
                cache_hits,
                model,
                cost_usd,
                ..
            } => {
                let mut attrs = vec![
                    attr("gen_ai.request.model", model.as_str()),
                    attr("gen_ai.usage.input_tokens", i64::from(*prompt_tokens)),
                    attr("gen_ai.usage.output_tokens", i64::from(*output_tokens)),
                    attr("surge.cache_hits", i64::from(*cache_hits)),
                ];
                if let Some(cost) = cost_usd {
                    attrs.push(attr("surge.cost_usd", *cost));
                }
                self.events.push(span_event("gen_ai.usage", at, attrs));
            },
            _ => {},
        }
    }

    /// Close the span. `succeeded` is `None` when the stage never finished.
    fn finish(self, trace_id: &str, root_id: &str, end: u64, succeeded: Option<bool>) -> Value {
        let (status, message) = match succeeded {
            Some(true) => (STATUS_OK, None),
            Some(false) => (STATUS_ERROR, self.error),
            None => (STATUS_UNSET, None),
        };
        span_json(
            trace_id,
            &self.span_id,
            Some(root_id),
            &format!("stage {}", self.node),
            self.start,
            end.max(self.start),
            self.attrs,
            self.events,
            status,
            message,
        )
    }
}

/// Status and attributes for the run's root span from its terminal event.
fn run_outcome(events: &[RunEvent]) -> (u8, Option<String>, Vec<Value>) {
    for event in events.iter().rev() {
        match &event.payload {
            EventPayload::RunCompleted { terminal_node } => {
                return (
                    STATUS_OK,
                    None,
                    vec![attr("surge.terminal_node", terminal_node.as_str())],
                );
            },
            EventPayload::RunFailed { error } => {
                return (STATUS_ERROR, Some(error.clone()), Vec::new());
            },
            EventPayload::RunAborted { reason } => {
                return (STATUS_ERROR, Some(reason.clone()), Vec::new());
            },
            _ => {},
        }
    }
    (STATUS_UNSET, None, vec![attr("surge.in_progress", true)])
}

#[allow(clippy::too_many_arguments)]
fn span_json(
    trace_id: &str,
    span_id: &str,
    parent: Option<&str>,
    name: &str,
    start: u64,
    end: u64,
    attributes: Vec<Value>,
    events: Vec<Value>,
    status: u8,
    message: Option<String>,
) -> Value {
    let mut span = json!({
        "traceId": trace_id,
        "spanId": span_id,
        "name": name,
        "kind": 1,
        "startTimeUnixNano": start.to_string(),
        "endTimeUnixNano": end.to_string(),
        "attributes": attributes,
        "events": events,
        "status": { "code": status },
    });
    if let Some(parent) = parent {
        span["parentSpanId"] = json!(parent);
    }
    if let Some(message) = message {
        span["status"]["message"] = json!(message);
    }
    span
}

fn span_event(name: &str, at: u64, attributes: Vec<Value>) -> Value {
    json!({ "name": name, "timeUnixNano": at.to_string(), "attributes": attributes })
}

/// A typed OTLP `AnyValue`.
enum AttrValue {
    Str(String),
    Int(i64),
    Bool(bool),
    Double(f64),
}

impl From<&str> for AttrValue {
    fn from(value: &str) -> Self {
        Self::Str(value.to_owned())
    }
}
impl From<i64> for AttrValue {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}
impl From<bool> for AttrValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}
impl From<f64> for AttrValue {
    fn from(value: f64) -> Self {
        Self::Double(value)
    }
}

fn attr(key: &str, value: impl Into<AttrValue>) -> Value {
    let value = match value.into() {
        AttrValue::Str(s) => json!({ "stringValue": s }),
        // OTLP/JSON carries 64-bit integers as strings.
        AttrValue::Int(i) => json!({ "intValue": i.to_string() }),
        AttrValue::Bool(b) => json!({ "boolValue": b }),
        AttrValue::Double(d) => json!({ "doubleValue": d }),
    };
    json!({ "key": key, "value": value })
}

/// Deterministic lowercase-hex id of `bytes` bytes derived from `seed`.
fn hex_id(seed: &str, bytes: usize) -> String {
    ContentHash::compute(seed.as_bytes()).to_hex()[..bytes * 2].to_owned()
}

fn nanos(event: &RunEvent) -> u64 {
    u64::try_from(event.timestamp.timestamp_nanos_opt().unwrap_or(0)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{NodeKey, OutcomeKey};
    use chrono::{TimeZone, Utc};

    fn ev(run: RunId, seq: u64, secs: i64, payload: EventPayload) -> RunEvent {
        RunEvent {
            run_id: run,
            seq,
            timestamp: Utc.timestamp_opt(1_790_000_000 + secs, 0).unwrap(),
            payload,
        }
    }

    fn node(name: &str) -> NodeKey {
        NodeKey::try_from(name).unwrap()
    }

    fn outcome(name: &str) -> OutcomeKey {
        OutcomeKey::try_from(name).unwrap()
    }

    fn sample(run: RunId) -> Vec<RunEvent> {
        vec![
            ev(
                run,
                1,
                0,
                EventPayload::StageEntered {
                    node: node("impl"),
                    attempt: 1,
                },
            ),
            ev(
                run,
                2,
                4,
                EventPayload::OutcomeRejectedByHook {
                    node: node("impl"),
                    outcome: outcome("done"),
                    hook_id: "validate".into(),
                },
            ),
            ev(
                run,
                3,
                9,
                EventPayload::StageCompleted {
                    node: node("impl"),
                    outcome: outcome("done"),
                },
            ),
            ev(
                run,
                4,
                10,
                EventPayload::StageEntered {
                    node: node("verify"),
                    attempt: 1,
                },
            ),
            ev(
                run,
                5,
                20,
                EventPayload::StageFailed {
                    node: node("verify"),
                    reason: "verifier crashed".into(),
                    retry_available: false,
                },
            ),
            ev(
                run,
                6,
                21,
                EventPayload::RunFailed {
                    error: "verify failed".into(),
                },
            ),
        ]
    }

    fn spans(value: &Value) -> &Vec<Value> {
        value["resourceSpans"][0]["scopeSpans"][0]["spans"]
            .as_array()
            .unwrap()
    }

    #[test]
    fn a_run_becomes_a_root_span_with_one_child_per_stage() {
        let run = RunId::new();
        let trace = to_otlp_json(run, &sample(run));
        let spans = spans(&trace);
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0]["name"], "surge.run");
        assert!(spans[0].get("parentSpanId").is_none());
        assert_eq!(spans[0]["status"]["code"], 2, "the run failed");
        for child in &spans[1..] {
            assert_eq!(child["parentSpanId"], spans[0]["spanId"]);
            assert_eq!(child["traceId"], spans[0]["traceId"]);
        }
        assert_eq!(spans[1]["name"], "stage impl");
        assert_eq!(spans[1]["status"]["code"], 1);
        assert_eq!(spans[2]["name"], "stage verify");
        assert_eq!(spans[2]["status"]["code"], 2);
        assert_eq!(spans[2]["status"]["message"], "verifier crashed");
    }

    #[test]
    fn spans_carry_timing_and_the_events_seen_inside_them() {
        let run = RunId::new();
        let trace = to_otlp_json(run, &sample(run));
        let stage = &spans(&trace)[1];
        let start: u64 = stage["startTimeUnixNano"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let end: u64 = stage["endTimeUnixNano"].as_str().unwrap().parse().unwrap();
        assert_eq!(end - start, 9_000_000_000);
        assert_eq!(stage["events"][0]["name"], "surge.outcome_rejected");
    }

    #[test]
    fn ids_are_wellformed_and_stable_across_exports() {
        let run = RunId::new();
        let a = to_otlp_json(run, &sample(run));
        let b = to_otlp_json(run, &sample(run));
        assert_eq!(a, b);
        let root = &spans(&a)[0];
        assert_eq!(root["traceId"].as_str().unwrap().len(), 32);
        assert_eq!(root["spanId"].as_str().unwrap().len(), 16);
        assert_ne!(
            to_otlp_json(RunId::new(), &sample(run))["resourceSpans"][0]["scopeSpans"][0]["spans"]
                [0]["traceId"],
            root["traceId"],
            "a different run id is a different trace"
        );
    }

    #[test]
    fn a_stage_still_running_is_left_open_and_the_run_is_marked_in_progress() {
        let run = RunId::new();
        let events = vec![ev(
            run,
            1,
            0,
            EventPayload::StageEntered {
                node: node("impl"),
                attempt: 2,
            },
        )];
        let trace = to_otlp_json(run, &events);
        let spans = spans(&trace);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0]["status"]["code"], 0);
        assert_eq!(spans[1]["status"]["code"], 0);
        assert_eq!(spans[1]["attributes"][1]["value"]["intValue"], "2");
    }

    #[test]
    fn a_completion_for_another_node_does_not_discard_the_open_span() {
        let run = RunId::new();
        let events = vec![
            ev(
                run,
                1,
                0,
                EventPayload::StageEntered {
                    node: node("impl"),
                    attempt: 1,
                },
            ),
            ev(
                run,
                2,
                5,
                EventPayload::StageCompleted {
                    node: node("other"),
                    outcome: outcome("done"),
                },
            ),
        ];
        let trace = to_otlp_json(run, &events);
        let spans = spans(&trace);
        assert_eq!(
            spans.len(),
            2,
            "the impl span must still be exported: {spans:?}"
        );
        assert_eq!(spans[1]["name"], "stage impl");
        assert_eq!(spans[1]["status"]["code"], 0, "never finished, so unset");
    }

    #[test]
    fn an_empty_log_is_a_valid_request_with_just_the_root_span() {
        let run = RunId::new();
        let trace = to_otlp_json(run, &[]);
        assert_eq!(spans(&trace).len(), 1);
    }
}
