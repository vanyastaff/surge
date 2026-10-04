use super::*;

fn requirements() -> WorkItemRequirements {
    WorkItemRequirements::new(
        " Accepted\nbytes ".into(),
        vec!["Works".into(), "Preserves order".into()],
    )
    .unwrap()
}

fn graph() -> Graph {
    toml::from_str(include_str!("../../../../examples/flow_terminal_only.toml")).unwrap()
}

fn binding(hash: ContentHash) -> WorkItemBinding {
    WorkItemBinding {
        item: "00000000000000000000000001".parse().unwrap(),
        revision: 1,
        requirements_hash: hash,
        generation: 1,
    }
}

#[test]
fn legacy_requirements_bytes_and_hash_remain_golden() {
    let accepted = requirements();
    assert_eq!(
        serde_json::to_vec(&accepted).unwrap(),
        br#"{"text":" Accepted\nbytes ","criteria":["Works","Preserves order"]}"#
    );
    assert_eq!(
        accepted.hash().unwrap().to_string(),
        "sha256:0103ba0039635c3f6671d2452f9bcca6504c69d994b03da7fff190b30272a744"
    );
}

#[test]
fn legacy_context_bytes_remain_golden() {
    let accepted = requirements();
    let context = WorkItemContext::new(binding(accepted.hash().unwrap()), accepted).unwrap();
    let bytes = br#"{"binding":{"item":"00000000000000000000000001","revision":1,"requirements_hash":"sha256:0103ba0039635c3f6671d2452f9bcca6504c69d994b03da7fff190b30272a744","generation":1},"requirements":{"text":" Accepted\nbytes ","criteria":["Works","Preserves order"]}}"#;
    assert_eq!(serde_json::to_vec(&context).unwrap(), bytes);
    assert_eq!(
        serde_json::from_slice::<WorkItemContext>(bytes).unwrap(),
        context
    );
    assert_eq!(
        context.prompt(),
        "Accepted task requirements (revision 1, hash sha256:0103ba0039635c3f6671d2452f9bcca6504c69d994b03da7fff190b30272a744)\n Accepted\nbytes \nAcceptance criteria:\n1. Works\n2. Preserves order"
    );
}

#[test]
fn flow_context_preserves_empty_whitespace_and_raw_prompt() {
    for prompt in ["", " \n\t", "  Exact\nraw Ω\r\n "] {
        let contract = AcceptedFlowContract::new(Box::new(graph()), prompt.into()).unwrap();
        let context =
            WorkItemContext::new_flow(binding(contract.hash().unwrap()), contract).unwrap();
        assert_eq!(context.prompt(), prompt);
        assert!(context.requirements().is_none());
        let decoded: WorkItemContext =
            serde_json::from_slice(&serde_json::to_vec(&context).unwrap()).unwrap();
        assert_eq!(decoded.prompt(), prompt);
        assert_eq!(decoded, context);
    }
}

#[test]
fn flow_deserialization_rejects_forged_graph_hash_and_oversized_prompt() {
    let contract = AcceptedFlowContract::new(Box::new(graph()), "raw".into()).unwrap();
    let mut value = serde_json::to_value(&contract).unwrap();
    value["graph_hash"] = serde_json::to_value(ContentHash::compute(b"foreign graph")).unwrap();
    assert!(serde_json::from_value::<AcceptedFlowContract>(value).is_err());
    let mut value = serde_json::to_value(&contract).unwrap();
    value["raw_prompt"] = serde_json::Value::String("x".repeat(131_073));
    assert!(serde_json::from_value::<AcceptedFlowContract>(value).is_err());
}

#[test]
fn flow_contract_hash_authenticates_domain_graph_and_exact_prompt() {
    let contract = AcceptedFlowContract::new(Box::new(graph()), "raw".into()).unwrap();
    assert_eq!(
        contract.hash().unwrap(),
        ContentHash::compute(&contract.canonical_bytes().unwrap())
    );
    assert_ne!(
        contract.hash().unwrap(),
        ContentHash::compute(&serde_json::to_vec(&contract).unwrap())
    );
    let changed = AcceptedFlowContract::new(Box::new(graph()), "raw ".into()).unwrap();
    assert_ne!(contract.hash().unwrap(), changed.hash().unwrap());
    let mut changed_graph = graph();
    changed_graph.metadata.name = "different".into();
    assert!(contract.validate_graph(&changed_graph).is_err());
}

#[test]
fn legacy_revision_bytes_remain_golden_and_hash_is_validated() {
    let bytes = br#"{"revision":1,"requirements":{"text":" Accepted\nbytes ","criteria":["Works","Preserves order"]},"hash":"sha256:0103ba0039635c3f6671d2452f9bcca6504c69d994b03da7fff190b30272a744","actor":"human","accepted_proposal":null,"accepted_at_ms":42}"#;
    let decoded: WorkItemRevision = serde_json::from_slice(bytes).unwrap();
    assert_eq!(decoded.origin.requirements(), Some(&requirements()));
    assert_eq!(serde_json::to_vec(&decoded).unwrap(), bytes);
    let mut forged = serde_json::to_value(&decoded).unwrap();
    forged["hash"] = serde_json::to_value(ContentHash::compute(b"foreign")).unwrap();
    assert!(serde_json::from_value::<WorkItemRevision>(forged).is_err());
}

#[test]
fn context_rejects_mixed_origin_forged_binding_and_invalid_graph() {
    let contract = AcceptedFlowContract::new(Box::new(graph()), "raw".into()).unwrap();
    let context = WorkItemContext::new_flow(binding(contract.hash().unwrap()), contract).unwrap();
    let mut mixed = serde_json::to_value(&context).unwrap();
    mixed["requirements"] = serde_json::to_value(requirements()).unwrap();
    assert!(serde_json::from_value::<WorkItemContext>(mixed).is_err());
    let mut forged = serde_json::to_value(&context).unwrap();
    forged["binding"]["requirements_hash"] =
        serde_json::to_value(ContentHash::compute(b"foreign")).unwrap();
    assert!(serde_json::from_value::<WorkItemContext>(forged).is_err());
    let mut invalid = graph();
    invalid.nodes.clear();
    let value = serde_json::json!({
        "graph_hash": ContentHash::compute(&serde_json::to_vec(&invalid).unwrap()),
        "graph": invalid,
        "raw_prompt": "",
    });
    assert!(serde_json::from_value::<AcceptedFlowContract>(value).is_err());
}

#[test]
fn flow_prompt_bound_measures_exact_utf8_bytes() {
    assert!(AcceptedFlowContract::new(Box::new(graph()), "x".repeat(131_072)).is_ok());
    assert!(AcceptedFlowContract::new(Box::new(graph()), "x".repeat(131_073)).is_err());
    assert!(AcceptedFlowContract::new(Box::new(graph()), "Ω".repeat(65_536)).is_ok());
    assert!(AcceptedFlowContract::new(Box::new(graph()), "Ω".repeat(65_537)).is_err());
}

#[test]
fn flow_contract_rejects_nonfinite_graph_payload_before_acceptance() {
    let mut invalid = graph();
    invalid.nodes.values_mut().next().unwrap().position.x = f32::NAN;
    assert!(matches!(
        AcceptedFlowContract::new(Box::new(invalid), "".into()),
        Err(AcceptedFlowError::InvalidGraph)
    ));
}
