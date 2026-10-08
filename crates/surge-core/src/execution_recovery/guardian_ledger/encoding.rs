//! Frozen version-one byte framing; no serde layout or native-sized field enters the hash.
use super::super::guardian::{
    GuardianAuthority, WindowsGuardianContainer, WindowsGuardianProcessIdentity,
};
use super::*;

pub(super) fn encode(record: &WindowsGuardianRecord) -> Vec<u8> {
    let mut output = b"surge.windows-guardian-ledger.v1\0".to_vec();
    output.extend(record.version().to_be_bytes());
    let context = record.context();
    output.extend(context.run().as_ulid().to_bytes());
    output.extend(context.invocation().as_ulid().to_bytes());
    output.extend(context.occurrence().as_ulid().to_bytes());
    authority(&mut output, context.authority());
    output.extend(record.operation().as_bytes());
    match record.predecessor() {
        None => output.push(0),
        Some(hash) => {
            output.push(1);
            output.extend(hash.as_bytes());
        },
    }
    match record.lease() {
        None => output.push(0),
        Some(lease) => {
            output.push(1);
            output.extend(lease.get().to_be_bytes());
        },
    }
    body(&mut output, record.body());
    output
}
fn authority(output: &mut Vec<u8>, value: &GuardianAuthority) {
    output.extend(value.installation_id().as_bytes());
    output.extend(value.executable_hash().as_bytes());
    output.extend(value.protocol_version().to_be_bytes());
}
fn process(output: &mut Vec<u8>, value: &WindowsGuardianProcessIdentity) {
    output.extend(value.pid().to_be_bytes());
    output.extend(value.creation_filetime().to_be_bytes());
}
fn binding(output: &mut Vec<u8>, value: &WindowsGuardianBinding) {
    authority(output, value.authority());
    output.extend(value.occurrence().as_ulid().to_bytes());
    process(output, value.guardian());
    // Constructor fixes this ASCII label to 106 bytes, independent of platform usize.
    output.extend(106u32.to_be_bytes());
    output.extend(value.endpoint().as_bytes());
}
fn container(output: &mut Vec<u8>, value: &WindowsGuardianContainer) {
    output.push(1); // windows_guardian
    output.extend(value.version().to_be_bytes());
    binding(output, &WindowsGuardianBinding::from_container(value));
    process(output, value.identity().child());
    output.push(1); // GroupOnly
}
fn body(output: &mut Vec<u8>, value: &GuardianRecordBody) {
    match value {
        GuardianRecordBody::LaunchIntent(value) => {
            output.push(1);
            process(output, value.host());
            output.extend(value.registry_generation().get().to_be_bytes());
        },
        GuardianRecordBody::CancelledBeforeSpawn(value) => {
            output.push(2);
            output.extend(value.launch_operation().as_bytes());
            process(output, value.host());
            output.extend(value.registry_generation().get().to_be_bytes());
            output.extend(value.revocation_generation().get().to_be_bytes());
        },
        GuardianRecordBody::GuardianBound(value) => {
            output.push(3);
            binding(output, value.binding());
            process(output, value.host());
            output.extend(value.registry_generation().get().to_be_bytes());
        },
        GuardianRecordBody::LeaseTakenOver(value) => takeover(output, value),
        GuardianRecordBody::ChildEstablished(value) => {
            output.push(5);
            container(output, value.container());
        },
        GuardianRecordBody::ResumeAuthorized(value) => {
            output.push(6);
            container(output, value.container());
            output.extend(value.barrier_generation().get().to_be_bytes());
        },
        GuardianRecordBody::ResumeObserved(value) => {
            output.push(7);
            container(output, value.container());
            output.extend(value.resume_operation().as_bytes());
            output.extend(value.barrier_generation().get().to_be_bytes());
            outcome(output, value.outcome());
        },
        GuardianRecordBody::SettlementRequested(value) => {
            output.push(8);
            binding(output, value.binding());
            output.extend(value.barrier_generation().get().to_be_bytes());
        },
        GuardianRecordBody::SettlementObserved(value) => {
            output.push(9);
            output.extend(value.settlement_operation().as_bytes());
            proof(output, value.proof());
        },
        GuardianRecordBody::SettlementConsumed(value) => {
            output.push(10);
            output.extend(value.settlement_operation().as_bytes());
            output.extend(value.settlement_hash().as_bytes());
            output.extend(value.settlement_sequence().get().to_be_bytes());
            output.extend(value.barrier_generation().get().to_be_bytes());
            output.extend(value.query_generation().get().to_be_bytes());
        },
    }
}
fn takeover(output: &mut Vec<u8>, value: &GuardianLeaseTakenOver) {
    output.push(4);
    binding(output, value.binding());
    output.extend(value.previous_lease().get().to_be_bytes());
    process(output, value.previous_host());
    process(output, value.host());
    output.extend(value.previous_registry_generation().get().to_be_bytes());
    output.extend(value.registry_generation().get().to_be_bytes());
    match value.authorization() {
        GuardianTakeoverAuthorization::PreviousHostTerminated(evidence) => {
            output.push(1);
            process(output, evidence.host());
            output.extend(evidence.lease().get().to_be_bytes());
            output.extend(evidence.observation_generation().get().to_be_bytes());
        },
        GuardianTakeoverAuthorization::PreviousHostRelinquished(evidence) => {
            output.push(2);
            process(output, evidence.host());
            output.extend(evidence.lease().get().to_be_bytes());
            output.extend(evidence.operation().as_bytes());
            output.extend(evidence.hash().as_bytes());
        },
    }
}
fn outcome(output: &mut Vec<u8>, value: &ResumeObservation) {
    match value {
        ResumeObservation::Started(value) => {
            output.push(1);
            output.extend(value.previous_suspend_count().to_be_bytes());
        },
        ResumeObservation::Failed(value) => {
            output.push(2);
            output.extend(value.win32_error().to_be_bytes());
        },
        ResumeObservation::Uncertain(_) => output.push(3),
    }
}
fn proof(output: &mut Vec<u8>, value: &GuardianSettlementProof) {
    match value {
        GuardianSettlementProof::BoundJobEmpty(value) => {
            output.push(1);
            binding(output, value.binding());
            output.extend(value.barrier_generation().get().to_be_bytes());
            output.extend(value.query_generation().get().to_be_bytes());
        },
        GuardianSettlementProof::ChildJobEmpty(value) => {
            output.push(2);
            container(output, value.container());
            output.extend(value.barrier_generation().get().to_be_bytes());
            output.extend(value.query_generation().get().to_be_bytes());
        },
    }
}
