//! Fixture-table and property tests for the terminal verdict. Every consumer
//! policy test reuses [`fixtures`], so no consumer can drift on an edge case.
use super::*;
use crate::capacity::WakeBasis;
use crate::id::RunId;
use proptest::prelude::*;

fn node(name: &str) -> NodeKey {
    NodeKey::try_from(name).unwrap()
}

pub(crate) fn completed() -> EventPayload {
    EventPayload::RunCompleted {
        terminal_node: node("end"),
    }
}

pub(crate) fn failed() -> EventPayload {
    EventPayload::RunFailed {
        error: "boom".into(),
    }
}

pub(crate) fn aborted() -> EventPayload {
    EventPayload::RunAborted {
        reason: "stopped".into(),
    }
}

fn parked() -> EventPayload {
    EventPayload::RunParked {
        wake_at: chrono::DateTime::from_timestamp(0, 0).unwrap(),
        runtime: Some("claude-acp".into()),
        worktree: "/tmp/w".into(),
        basis: WakeBasis::PolicyBackoff,
        reason: "limit".into(),
    }
}

fn woke() -> EventPayload {
    EventPayload::RunWokeFromPark {}
}

fn fork() -> EventPayload {
    EventPayload::ForkCreated {
        new_run: RunId::new(),
        fork_at_seq: 1,
    }
}

fn stage_entered() -> EventPayload {
    EventPayload::StageEntered {
        node: node("impl"),
        attempt: 1,
    }
}

fn seq(payloads: Vec<EventPayload>) -> Vec<(u64, EventPayload)> {
    payloads
        .into_iter()
        .enumerate()
        .map(|(i, payload)| (i as u64 + 1, payload))
        .collect()
}

fn verdict(events: &[(u64, EventPayload)]) -> TerminalVerdict {
    TerminalVerdict::of(events.iter().map(|(seq, payload)| (*seq, payload)))
}

fn kinds(verdict: &TerminalVerdict) -> Vec<TerminalKind> {
    match verdict {
        TerminalVerdict::Open => Vec::new(),
        TerminalVerdict::Terminal(claim) => vec![claim.outcome.kind()],
        TerminalVerdict::Conflict { claims } => {
            claims.iter().map(|claim| claim.outcome.kind()).collect()
        },
    }
}

/// The shared fixture table: name, history, expected verdict kinds
/// (empty = Open, one = Terminal, two or more = Conflict).
pub(crate) fn fixtures() -> Vec<(&'static str, Vec<EventPayload>, Vec<TerminalKind>)> {
    use TerminalKind::{Aborted, Completed, Failed};
    let mut table = vec![
        ("empty", vec![], vec![]),
        ("parked only", vec![stage_entered(), parked()], vec![]),
        (
            "completed",
            vec![stage_entered(), completed()],
            vec![Completed],
        ),
        ("failed", vec![failed()], vec![Failed]),
        ("aborted", vec![aborted()], vec![Aborted]),
        (
            "identical duplicate",
            vec![completed(), completed()],
            vec![Completed, Completed],
        ),
        (
            "triple claims",
            vec![completed(), failed(), aborted()],
            vec![Completed, Failed, Aborted],
        ),
        (
            "completed then fork",
            vec![completed(), fork()],
            vec![Completed],
        ),
        (
            "completed then parked",
            vec![completed(), parked(), woke()],
            vec![Completed],
        ),
        (
            "completed then stage activity",
            vec![completed(), stage_entered()],
            vec![Completed],
        ),
    ];
    let all = [
        (Completed, completed as fn() -> EventPayload),
        (Failed, failed),
        (Aborted, aborted),
    ];
    for (first_kind, first) in all {
        for (second_kind, second) in all {
            if first_kind != second_kind {
                table.push((
                    "ordered pair",
                    vec![first(), second()],
                    vec![first_kind, second_kind],
                ));
            }
        }
    }
    table
}

#[test]
fn every_fixture_yields_its_expected_verdict() {
    for (name, history, expected) in fixtures() {
        let got = verdict(&seq(history));
        assert_eq!(kinds(&got), expected, "{name}: {got:?}");
        match expected.len() {
            0 => assert_eq!(got, TerminalVerdict::Open, "{name}"),
            1 => assert!(matches!(got, TerminalVerdict::Terminal(_)), "{name}"),
            _ => assert!(matches!(got, TerminalVerdict::Conflict { .. }), "{name}"),
        }
    }
}

#[test]
fn claims_carry_their_sequence_numbers() {
    let got = verdict(&seq(vec![stage_entered(), completed(), parked(), failed()]));
    assert_eq!(got.conflicting_seqs(), vec![2, 4]);
    let one = verdict(&seq(vec![stage_entered(), aborted()]));
    assert_eq!(
        one.terminal(),
        Some(&TerminalClaim {
            seq: 2,
            outcome: TerminalOutcome::Aborted {
                reason: "stopped".into()
            }
        })
    );
}

#[test]
fn post_terminal_activity_is_reported_without_changing_the_verdict() {
    let events = seq(vec![
        stage_entered(),
        completed(),
        fork(),
        parked(),
        stage_entered(),
    ]);
    let history = TerminalHistory::of(events.iter().map(|(seq, payload)| (*seq, payload)));
    assert_eq!(history.post_terminal_activity(), &[5]);
    assert!(matches!(history.verdict(), TerminalVerdict::Terminal(_)));
    let quiet = seq(vec![stage_entered(), completed(), fork(), parked(), woke()]);
    let quiet = TerminalHistory::of(quiet.iter().map(|(seq, payload)| (*seq, payload)));
    assert!(quiet.post_terminal_activity().is_empty());
}

fn any_event() -> impl Strategy<Value = EventPayload> {
    prop_oneof![
        Just(completed()),
        Just(failed()),
        Just(aborted()),
        Just(parked()),
        Just(woke()),
        Just(fork()),
        Just(stage_entered()),
    ]
}

fn rank(verdict: &TerminalVerdict) -> u8 {
    match verdict {
        TerminalVerdict::Open => 0,
        TerminalVerdict::Terminal(_) => 1,
        TerminalVerdict::Conflict { .. } => 2,
    }
}

fn is_terminal(payload: &EventPayload) -> bool {
    matches!(
        payload,
        EventPayload::RunCompleted { .. }
            | EventPayload::RunFailed { .. }
            | EventPayload::RunAborted { .. }
    )
}

proptest! {
    #[test]
    fn the_verdict_is_monotone_over_prefixes(events in prop::collection::vec(any_event(), 0..12)) {
        let events = seq(events);
        let mut previous = 0;
        for end in 0..=events.len() {
            let now = rank(&verdict(&events[..end]));
            prop_assert!(now >= previous, "verdict went back at prefix {end}");
            previous = now;
        }
    }

    #[test]
    fn non_terminal_events_never_change_the_verdict(events in prop::collection::vec(any_event(), 0..12)) {
        let events = seq(events);
        let terminal_only: Vec<_> = events
            .iter()
            .filter(|(_, payload)| is_terminal(payload))
            .cloned()
            .collect();
        prop_assert_eq!(verdict(&events), verdict(&terminal_only));
    }

    #[test]
    fn conflict_claims_are_ordered_and_at_least_two(events in prop::collection::vec(any_event(), 0..12)) {
        if let TerminalVerdict::Conflict { claims } = verdict(&seq(events)) {
            prop_assert!(claims.len() >= 2);
            prop_assert!(claims.windows(2).all(|pair| pair[0].seq < pair[1].seq));
        }
    }
}
