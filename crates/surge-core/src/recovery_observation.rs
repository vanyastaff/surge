//! Read-only recovery history and supplied-prefix coverage, never execution authority.

use serde::{Deserialize, Serialize};

use crate::{EventPayload, RunEvent};

/// A recorded problem, retained after later progress or a terminal event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedRecoveryRequired {
    /// Exact durable event sequence.
    pub seq: u64,
    /// Recorded generation, not a grant of control authority.
    pub control_generation: u64,
    /// Original diagnostic; bound and neutralize only when presenting it.
    pub diagnostic: String,
    /// Presentation-only disposition, never a lifecycle verdict or control grant.
    pub disposition: RecoveryDisposition,
}

/// Why a recorded recovery request is active or historical in the supplied prefix.
/// A terminal observation never replaces RunReport completion or conflict evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RecoveryDisposition {
    /// No qualifying progress or terminal event was observed.
    Blocked,
    /// Accepted Continue or a later stage in a known-unbound complete prefix.
    ProgressObserved {
        /// Exact progress event, later than the recovery request.
        seq: u64,
    },
    /// A genuine terminal event was recorded; this diagnostic is historical.
    TerminalObserved {
        /// Exact terminal event, possibly before a later diagnostic record.
        seq: u64,
    },
}

/// Coverage of the supplied journal prefix, not EOF, liveness or settlement proof.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "observation", rename_all = "snake_case")]
pub enum RecoveryObservation {
    /// Legacy serialized projections did not load this observation.
    #[default]
    NotLoaded,
    /// Supplied envelopes start with RunStarted at seq1 and remain contiguous.
    CompletePrefix {
        /// Highest supplied sequence.
        through: u64,
        /// Latest problem by sequence, including historical problems.
        latest: Option<RecordedRecoveryRequired>,
    },
    /// Missing start, a gap, invalid ordering, or unavailable observation.
    PartialPrefix {
        /// Highest supplied sequence, if any.
        through: Option<u64>,
        /// Retained problem history; current blocked state is unconfirmed.
        latest: Option<RecordedRecoveryRequired>,
    },
}
impl RecoveryObservation {
    /// Latest recorded diagnostic, even when current coverage is unconfirmed.
    #[must_use]
    pub const fn latest(&self) -> Option<&RecordedRecoveryRequired> {
        match self {
            Self::NotLoaded => None,
            Self::CompletePrefix { latest, .. } | Self::PartialPrefix { latest, .. } => {
                latest.as_ref()
            },
        }
    }
    /// Whether only the supplied prefix is complete; says nothing about freshness.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        matches!(self, Self::CompletePrefix { .. })
    }
    /// A recorded blocked interval in a complete prefix, not proof of live ownership.
    #[must_use]
    pub fn active(&self) -> Option<&RecordedRecoveryRequired> {
        self.latest().filter(|record| {
            self.is_complete() && matches!(record.disposition, RecoveryDisposition::Blocked)
        })
    }
    /// Make coverage permanently partial while retaining diagnostic history.
    pub fn mark_partial(&mut self) {
        *self = match std::mem::take(self) {
            Self::NotLoaded => Self::PartialPrefix {
                through: None,
                latest: None,
            },
            Self::CompletePrefix { through, latest } => Self::PartialPrefix {
                through: Some(through),
                latest,
            },
            partial @ Self::PartialPrefix { .. } => partial,
        };
    }
}

/// Small shared journal observer used by state, reports and desktop feeds.
/// Private fields and no Deserialize impl prevent report data constructing an observer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecoveryObserver {
    observation: RecoveryObservation,
    bound: bool,
    control_generation: u64,
    terminal_seq: Option<u64>,
}
impl RecoveryObserver {
    /// Read-only serialized presentation snapshot.
    #[must_use]
    pub const fn observation(&self) -> &RecoveryObservation {
        &self.observation
    }

    /// Observation loss never discards a previously recorded diagnostic.
    pub fn invalidate(&mut self) {
        self.observation.mark_partial();
    }

    /// Observe exactly once per attributed envelope. Invalid ordering makes coverage
    /// partial and cannot replace a diagnostic or count as progress.
    pub fn observe(&mut self, event: &RunEvent) {
        let previous = match &self.observation {
            RecoveryObservation::NotLoaded => None,
            RecoveryObservation::CompletePrefix { through, .. } => Some(*through),
            RecoveryObservation::PartialPrefix { through, .. } => *through,
        };
        if event.seq == 0 || previous.is_some_and(|seq| event.seq <= seq) {
            self.invalidate();
            return;
        }
        let old = std::mem::take(&mut self.observation);
        let (complete, latest) = match old {
            RecoveryObservation::NotLoaded => (
                event.seq == 1 && matches!(event.payload, EventPayload::RunStarted { .. }),
                None,
            ),
            RecoveryObservation::CompletePrefix { through, latest } => {
                (through.checked_add(1) == Some(event.seq), latest)
            },
            RecoveryObservation::PartialPrefix { latest, .. } => (false, latest),
        };
        self.observation = if complete {
            RecoveryObservation::CompletePrefix {
                through: event.seq,
                latest,
            }
        } else {
            RecoveryObservation::PartialPrefix {
                through: Some(event.seq),
                latest,
            }
        };
        self.observe_payload(event, complete);
    }
    fn observe_payload(&mut self, event: &RunEvent, complete: bool) {
        match &event.payload {
            EventPayload::WorkItemAttemptBound { .. } => self.bound = true,
            EventPayload::RunRecoveryRequired {
                control_generation,
                diagnostic,
            } => {
                let record = RecordedRecoveryRequired {
                    seq: event.seq,
                    control_generation: *control_generation,
                    diagnostic: diagnostic.clone(),
                    disposition: self
                        .terminal_seq
                        .map_or(RecoveryDisposition::Blocked, |seq| {
                            RecoveryDisposition::TerminalObserved { seq }
                        }),
                };
                match &mut self.observation {
                    RecoveryObservation::CompletePrefix { latest, .. }
                    | RecoveryObservation::PartialPrefix { latest, .. } => *latest = Some(record),
                    RecoveryObservation::NotLoaded => {},
                }
                self.control_generation = self.control_generation.max(*control_generation);
            },
            EventPayload::RunSuspended { fence } => {
                self.control_generation = self.control_generation.max(fence.control_generation);
            },
            EventPayload::RunContinued { control_generation }
                if continuation_advances(self.control_generation, *control_generation) =>
            {
                self.control_generation = *control_generation;
                if complete {
                    self.progress(event.seq);
                }
            },
            EventPayload::StageEntered { .. } if complete && !self.bound => {
                self.progress(event.seq)
            },
            EventPayload::RunCompleted { .. }
            | EventPayload::RunFailed { .. }
            | EventPayload::RunAborted { .. } => {
                self.terminal_seq.get_or_insert(event.seq);
                self.set_disposition(RecoveryDisposition::TerminalObserved { seq: event.seq });
            },
            _ => {},
        }
    }
    fn progress(&mut self, seq: u64) {
        if self.terminal_seq.is_none() {
            self.set_disposition(RecoveryDisposition::ProgressObserved { seq });
        }
    }
    fn set_disposition(&mut self, disposition: RecoveryDisposition) {
        match &mut self.observation {
            RecoveryObservation::CompletePrefix {
                latest: Some(record),
                ..
            }
            | RecoveryObservation::PartialPrefix {
                latest: Some(record),
                ..
            } if matches!(record.disposition, RecoveryDisposition::Blocked)
                || matches!(disposition, RecoveryDisposition::TerminalObserved { .. }) =>
            {
                record.disposition = disposition;
            },
            _ => {},
        }
    }
}

/// Same strict predicate used by the execution memory's existing control fold.
pub(crate) const fn continuation_advances(current: u64, incoming: u64) -> bool {
    incoming > current
}

/// Maximum rendered diagnostic bytes, including a visible truncation marker.
pub const RECOVERY_DIAGNOSTIC_BYTES: usize = 4096;
const TRUNCATED: &str = "\n[diagnostic truncated]";

/// Render bounded literal text without changing the journal's original diagnostic.
#[must_use]
pub fn recovery_diagnostic(diagnostic: &str) -> String {
    let mut out = String::new();
    for character in diagnostic.chars() {
        let safe = if (character.is_control() && character != '\n')
            || matches!(character, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            '\u{fffd}'
        } else {
            character
        };
        if out.len() + safe.len_utf8() > RECOVERY_DIAGNOSTIC_BYTES {
            let mut end = RECOVERY_DIAGNOSTIC_BYTES - TRUNCATED.len();
            while !out.is_char_boundary(end) {
                end -= 1;
            }
            out.truncate(end);
            out.push_str(TRUNCATED);
            break;
        }
        out.push(safe);
    }
    out
}

#[cfg(test)]
mod tests;
