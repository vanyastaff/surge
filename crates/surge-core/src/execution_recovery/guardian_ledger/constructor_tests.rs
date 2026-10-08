use super::*;
use crate::execution_recovery::guardian::WindowsGuardianProcessIdentity;
use crate::id::{RunId, StageInvocationId};
use serde_json::{Value, json};

fn wire(name: &str) -> Value {
    let vectors: Vec<Value> = serde_json::from_str(include_str!("literal_vectors.json")).unwrap();
    vectors.into_iter().find(|v| v["name"] == name).unwrap()["record"].clone()
}

fn decoded(name: &str) -> WindowsGuardianRecord {
    serde_json::from_value(wire(name)).expect("independent literal is valid before mutation")
}

fn record_with(
    template: &WindowsGuardianRecord,
    context: GuardianRecordContext,
    predecessor: Option<ContentHash>,
    lease: Option<GuardianLease>,
) -> Result<WindowsGuardianRecord, GuardianRecordError> {
    WindowsGuardianRecord::new(
        context,
        *template.operation(),
        predecessor,
        lease,
        template.body().clone(),
    )
}

#[test]
fn direct_context_and_childless_binding_reject_nil_identifiers() {
    let template = decoded("bound");
    let context = template.context();
    assert_eq!(
        GuardianRecordContext::new(
            context.run(),
            context.invocation(),
            context.occurrence(),
            context.authority().clone()
        )
        .unwrap(),
        *context
    );
    for (run, invocation, occurrence) in [
        (RunId::nil(), context.invocation(), context.occurrence()),
        (
            context.run(),
            StageInvocationId::nil(),
            context.occurrence(),
        ),
        (
            context.run(),
            context.invocation(),
            ExecutionWriterId::nil(),
        ),
    ] {
        assert_eq!(
            GuardianRecordContext::new(run, invocation, occurrence, context.authority().clone()),
            Err(GuardianRecordError::InvalidIdentifier)
        );
    }
    let GuardianRecordBody::GuardianBound(body) = template.body() else {
        panic!("bound")
    };
    let binding = body.binding();
    assert_eq!(
        WindowsGuardianBinding::new(
            binding.authority().clone(),
            binding.occurrence(),
            binding.guardian().clone()
        )
        .unwrap(),
        *binding
    );
    assert_eq!(
        WindowsGuardianBinding::new(
            binding.authority().clone(),
            ExecutionWriterId::nil(),
            binding.guardian().clone()
        ),
        Err(GuardianRecordError::InvalidIdentifier)
    );
    assert!(
        GuardianBound::new(
            binding.clone(),
            body.host().clone(),
            *body.registry_generation()
        )
        .is_ok()
    );
    let reused_guardian_pid =
        WindowsGuardianProcessIdentity::new(binding.guardian().pid(), 999).unwrap();
    assert_eq!(
        GuardianBound::new(
            binding.clone(),
            reused_guardian_pid,
            *body.registry_generation()
        ),
        Err(GuardianRecordError::InvalidBinding)
    );
}

struct TakeoverInputs {
    binding: WindowsGuardianBinding,
    previous_lease: GuardianLease,
    previous_host: WindowsGuardianProcessIdentity,
    host: WindowsGuardianProcessIdentity,
    previous_registry: GuardianRegistryGeneration,
    registry: GuardianRegistryGeneration,
    authorization: GuardianTakeoverAuthorization,
}

impl TakeoverInputs {
    fn from_literal(name: &str) -> Self {
        let record = decoded(name);
        let GuardianRecordBody::LeaseTakenOver(body) = record.body() else {
            panic!("takeover")
        };
        Self {
            binding: body.binding().clone(),
            previous_lease: *body.previous_lease(),
            previous_host: body.previous_host().clone(),
            host: body.host().clone(),
            previous_registry: *body.previous_registry_generation(),
            registry: *body.registry_generation(),
            authorization: body.authorization().clone(),
        }
    }

    fn build(self) -> Result<GuardianLeaseTakenOver, GuardianRecordError> {
        GuardianLeaseTakenOver::new(
            self.binding,
            self.previous_lease,
            self.previous_host,
            self.host,
            self.previous_registry,
            self.registry,
            self.authorization,
        )
    }

    fn invalidate(&mut self, case: &str) {
        match case {
            "same_host" => self.host = self.previous_host.clone(),
            "old_guardian_pid" => {
                self.previous_host =
                    WindowsGuardianProcessIdentity::new(self.binding.guardian().pid(), 777)
                        .unwrap();
                self.match_terminated_authorization();
            },
            "new_guardian_pid" => {
                self.host =
                    WindowsGuardianProcessIdentity::new(self.binding.guardian().pid(), 777).unwrap()
            },
            "equal_registry" => self.registry = self.previous_registry,
            "older_registry" => self.registry = GuardianRegistryGeneration::new(1).unwrap(),
            "wrong_authorization_host" => {
                self.previous_host = WindowsGuardianProcessIdentity::new(44, 45).unwrap()
            },
            "wrong_authorization_lease" => self.previous_lease = GuardianLease::new(3).unwrap(),
            "lease_overflow" => {
                self.previous_lease = GuardianLease::new(u64::MAX).unwrap();
                self.match_terminated_authorization();
            },
            _ => unreachable!(),
        }
    }

    fn match_terminated_authorization(&mut self) {
        self.authorization = GuardianTakeoverAuthorization::PreviousHostTerminated(
            GuardianPreviousHostTerminated::new(
                self.previous_host.clone(),
                self.previous_lease,
                GuardianHostObservationGeneration::new(13).unwrap(),
            ),
        );
    }
}

#[test]
fn direct_takeover_refuses_identity_generation_authorization_and_overflow_conflicts() {
    for name in ["takeover", "takeover_relinquished"] {
        assert!(TakeoverInputs::from_literal(name).build().is_ok());
        for case in [
            "same_host",
            "old_guardian_pid",
            "new_guardian_pid",
            "equal_registry",
            "older_registry",
            "wrong_authorization_host",
            "wrong_authorization_lease",
            "lease_overflow",
        ] {
            let mut input = TakeoverInputs::from_literal(name);
            input.invalidate(case);
            assert_eq!(
                input.build(),
                Err(GuardianRecordError::InvalidTakeover),
                "{name} {case}"
            );
        }
    }
    let mut reused_pid = TakeoverInputs::from_literal("takeover");
    reused_pid.host =
        WindowsGuardianProcessIdentity::new(reused_pid.previous_host.pid(), 999).unwrap();
    assert!(
        reused_pid.build().is_ok(),
        "different FILETIME may represent PID reuse"
    );
}

#[test]
fn direct_record_phase_matrix_requires_exact_predecessor_and_lease_shape() {
    for name in [
        "genesis",
        "cancelled",
        "bound",
        "takeover",
        "child",
        "resume_authorized",
        "resume_started",
        "resume_failed",
        "resume_uncertain",
        "settlement_requested",
        "bound_job_empty",
        "child_job_empty",
        "consumed",
        "takeover_relinquished",
    ] {
        let template = decoded(name);
        assert_eq!(
            record_with(
                &template,
                template.context().clone(),
                template.predecessor().cloned(),
                template.lease().copied()
            )
            .unwrap(),
            template
        );
        for (has_predecessor, lease) in [
            (false, None),
            (true, None),
            (false, Some(1)),
            (true, Some(1)),
            (true, Some(2)),
            (true, Some(u64::MAX)),
        ] {
            let allowed = match name {
                "genesis" => !has_predecessor && lease.is_none(),
                "cancelled" => has_predecessor && lease.is_none(),
                "bound" => has_predecessor && lease == Some(1),
                "takeover" | "takeover_relinquished" => has_predecessor && lease == Some(2),
                _ => has_predecessor && lease.is_some(),
            };
            let result = record_with(
                &template,
                template.context().clone(),
                has_predecessor.then(|| ContentHash::from_bytes([0; 32])),
                lease.map(|value| GuardianLease::new(value).unwrap()),
            );
            assert_eq!(
                result.is_ok(),
                allowed,
                "{name}, predecessor={has_predecessor}, lease={lease:?}"
            );
        }
    }
}

#[test]
fn direct_record_rejects_context_mismatch_in_each_embedded_binding_branch() {
    for name in [
        "bound",
        "takeover",
        "child",
        "resume_authorized",
        "resume_started",
        "settlement_requested",
        "bound_job_empty",
        "child_job_empty",
    ] {
        let template = decoded(name);
        let context = template.context();
        assert!(
            record_with(
                &template,
                context.clone(),
                template.predecessor().cloned(),
                template.lease().copied()
            )
            .is_ok()
        );
        let changed_authority = GuardianAuthority::new(
            context.authority().installation_id().clone(),
            ContentHash::from_bytes([9; 32]),
            1,
        )
        .unwrap();
        let other_occurrence: ExecutionWriterId = "00000000000000000000000004".parse().unwrap();
        for changed in [
            GuardianRecordContext::new(
                context.run(),
                context.invocation(),
                other_occurrence,
                context.authority().clone(),
            )
            .unwrap(),
            GuardianRecordContext::new(
                context.run(),
                context.invocation(),
                context.occurrence(),
                changed_authority,
            )
            .unwrap(),
        ] {
            assert_eq!(
                record_with(
                    &template,
                    changed,
                    template.predecessor().cloned(),
                    template.lease().copied()
                ),
                Err(GuardianRecordError::InvalidBinding),
                "{name}"
            );
        }
    }
}

#[test]
fn direct_resume_outcomes_reject_impossible_native_results() {
    assert_eq!(
        GuardianResumeStarted::new(1)
            .unwrap()
            .previous_suspend_count(),
        1
    );
    assert_eq!(
        GuardianResumeFailed::new(u32::MAX).unwrap().win32_error(),
        u32::MAX
    );
    for count in [0, 2, u32::MAX] {
        assert_eq!(
            GuardianResumeStarted::new(count),
            Err(GuardianRecordError::InvalidOutcome)
        );
    }
    assert_eq!(
        GuardianResumeFailed::new(0),
        Err(GuardianRecordError::InvalidOutcome)
    );
}

fn reject_literal_replacement(name: &str, original: &str, replacement: &str) {
    let literal = wire(name).to_string();
    assert!(serde_json::from_str::<WindowsGuardianRecord>(&literal).is_ok());
    let changed = literal.replacen(original, replacement, 1);
    assert_ne!(changed, literal, "mutation must reach {name}: {original}");
    assert!(
        serde_json::from_str::<WindowsGuardianRecord>(&changed).is_err(),
        "{name}: {replacement}"
    );
}

#[test]
fn literal_nested_duplicate_fields_and_tags_are_rejected_without_value_normalization() {
    for (name, field) in [
        ("genesis", "\"run\":\"00000000000000000000000001\""),
        ("genesis", "\"protocol_version\":1"),
        ("genesis", "\"pid\":4"),
        ("genesis", "\"registry_generation\":6"),
        ("bound", "\"kind\":\"guardian_bound\""),
        ("bound", "\"occurrence\":\"00000000000000000000000003\""),
        ("takeover", "\"observation_generation\":13"),
        ("takeover", "\"kind\":\"previous_host_terminated\""),
        (
            "takeover_relinquished",
            "\"kind\":\"previous_host_relinquished\"",
        ),
        ("child", "\"kind\":\"windows_guardian\""),
        ("resume_started", "\"previous_suspend_count\":1"),
        ("resume_failed", "\"win32_error\":5"),
        ("resume_uncertain", "\"kind\":\"uncertain\""),
        ("bound_job_empty", "\"kind\":\"bound_job_empty\""),
        ("child_job_empty", "\"query_generation\":16"),
    ] {
        reject_literal_replacement(name, field, &format!("{field},{field}"));
    }
}

#[test]
fn literal_nested_numeric_tokens_reject_wrong_types_and_overflow() {
    for (name, field, value) in [
        ("genesis", "pid", "4"),
        ("takeover", "observation_generation", "13"),
        ("cancelled", "revocation_generation", "17"),
        ("resume_authorized", "barrier_generation", "18"),
        ("bound_job_empty", "query_generation", "16"),
        ("consumed", "settlement_sequence", "23"),
        ("resume_started", "previous_suspend_count", "1"),
        ("resume_failed", "win32_error", "5"),
    ] {
        for bad in [
            "null",
            "true",
            "{}",
            "[]",
            "-1",
            "1.0",
            "\"1\"",
            "18446744073709551616",
        ] {
            reject_literal_replacement(
                name,
                &format!("\"{field}\":{value}"),
                &format!("\"{field}\":{bad}"),
            );
        }
    }
}

#[test]
fn nullable_presence_and_type_checks_preserve_valid_explicit_null_controls() {
    let genesis = wire("genesis");
    assert_eq!(genesis["predecessor"], Value::Null);
    assert_eq!(genesis["lease"], Value::Null);
    assert!(serde_json::from_value::<WindowsGuardianRecord>(genesis.clone()).is_ok());
    let mut absent = genesis.clone();
    absent.as_object_mut().unwrap().remove("predecessor");
    absent.as_object_mut().unwrap().remove("lease");
    assert!(serde_json::from_value::<WindowsGuardianRecord>(absent).is_err());
    for key in ["predecessor", "lease"] {
        for bad in [
            json!(true),
            json!([]),
            json!({}),
            json!(1.0),
            json!(-1),
            json!("1"),
        ] {
            let mut changed = genesis.clone();
            changed[key] = bad;
            assert!(
                serde_json::from_value::<WindowsGuardianRecord>(changed).is_err(),
                "{key}"
            );
        }
    }
    let bound = wire("bound");
    assert!(serde_json::from_value::<WindowsGuardianRecord>(bound.clone()).is_ok());
    for key in ["predecessor", "lease"] {
        let mut changed = bound.clone();
        changed[key] = Value::Null;
        assert!(
            serde_json::from_value::<WindowsGuardianRecord>(changed).is_err(),
            "bound {key}"
        );
    }
    reject_literal_replacement("genesis", "\"predecessor\":null", "\"predecessor\":7");
    reject_literal_replacement(
        "genesis",
        "\"lease\":null",
        "\"lease\":\"sha256:0000000000000000000000000000000000000000000000000000000000000000\"",
    );
}
