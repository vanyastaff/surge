//! App-owned read observations, independent of daemon connectivity.
use super::*;
use crate::run_observation::{Scope, Token};
use surge_persistence::runs::bootstrap_operations::{
    BootstrapOperationRecord, BootstrapStoredPayload,
};
use surge_persistence::runs::{ReadEvent, Storage};

type CheckedOperation = Result<
    Option<(
        BootstrapOperationRecord,
        Vec<(Token, Result<Option<Vec<ReadEvent>>, String>)>,
        bool,
    )>,
    String,
>;

type InspectionResult = (CheckedOperation, Vec<crate::app_state::ReadCompletion>);
struct OperationPublication {
    coverage: (
        Option<surge_core::bootstrap_operation::BootstrapOperationStatus>,
        bool,
    ),
    completions: Vec<crate::app_state::ReadCompletion>,
    retry: Option<(Vec<Token>, bool)>,
}

fn expects_related_history(record: &BootstrapOperationRecord, run: surge_core::RunId) -> bool {
    use surge_core::bootstrap_continuation::{BootstrapFailure as F, BootstrapTerminal as T};
    // This requests inspection only; an operation never supplies run lifecycle facts.
    record.status.continuations.iter().any(|attempt| {
        attempt.run == run
            && matches!(
                attempt.state,
                surge_core::bootstrap_attempt::BootstrapAttemptState::ForkReady
                    | surge_core::bootstrap_attempt::BootstrapAttemptState::Running
                    | surge_core::bootstrap_attempt::BootstrapAttemptState::Completed
                    | surge_core::bootstrap_attempt::BootstrapAttemptState::Failed
            )
    }) || (record.child.is_some() && run == record.status.planning_run)
        || matches!(&record.status.result,Some(T::Completed{run_id,..}|T::Failed{run_id,reason:F::RunFailed|F::RunAborted}) if *run_id==run)
}

pub(super) struct OperationDemand {
    request: u64,
    status: Option<surge_core::bootstrap_operation::BootstrapOperationStatus>,
    eligible: Option<Vec<Token>>,
}

pub(super) struct RetryEpisode {
    serial: u64,
    attempt: u32,
    status: Option<surge_core::bootstrap_operation::BootstrapOperationStatus>,
    pub(super) eligible: Vec<Token>,
    record: bool,
}

pub(super) async fn read_history(
    home: &std::path::Path,
    run: surge_core::RunId,
) -> Result<Option<Vec<ReadEvent>>, String> {
    let mut events = Vec::new();
    for _ in 0..16 {
        let page = Storage::inspect_existing_events_after(
            home.join("runs"),
            run,
            surge_persistence::runs::seq::EventSeq(
                events
                    .last()
                    .map_or(0, |event: &ReadEvent| event.seq.as_u64()),
            ),
            std::num::NonZeroU32::new(256).ok_or("invalid page limit")?,
        )
        .await
        .map_err(|error| error.to_string())?;
        let Some(page) = page else {
            return if events.is_empty() {
                Ok(None)
            } else {
                Err("journal disappeared during inspection".into())
            };
        };
        let complete = page.len() < 256;
        events.extend(page);
        if complete {
            return Ok(Some(events));
        }
    }
    Err("run history exceeds this bounded observation; inspect recorded history".into())
}
#[cfg(test)]
pub(super) fn bound_related(
    record: &BootstrapOperationRecord,
    run: surge_core::RunId,
    events: &[ReadEvent],
) -> Result<(), String> {
    crate::app_state::validate_bootstrap_history(record, run, events, None)
}
async fn bound_related_at_home(
    record: &BootstrapOperationRecord,
    run: surge_core::RunId,
    events: &[ReadEvent],
    home: &std::path::Path,
) -> Result<(), String> {
    crate::app_state::validate_bootstrap_history(record, run, events, Some(home))?;
    if record
        .status
        .continuations
        .iter()
        .any(|attempt| attempt.run == run)
    {
        let events = events
            .iter()
            .map(|event| event.to_run_event(run))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        crate::app_state::validate_bootstrap_successor(record, home, run, &events).await?;
    }
    Ok(())
}

async fn inspect_operation(
    scope: &Scope,
    operation: surge_core::RunId,
    state: &WeakEntity<AppState>,
    cx: &mut AsyncApp,
    eligible: Option<&[Token]>,
    before_recheck: impl std::future::Future<Output = ()>,
) -> InspectionResult {
    let mut retired = Vec::new();
    let result = async {
        let Some(observation) =
            Storage::inspect_existing_bootstrap_operation(scope.home.clone(), operation)
                .await
                .map_err(|error| error.to_string())?
        else {
            return Ok(None);
        };
        let record = observation.record().clone();
        let BootstrapStoredPayload::V1 { capture, .. } = &record.payload else {
            return Err("unsupported operation payload".into());
        };
        let owned = scope.repository.as_ref().map_or_else(
            || {
                capture
                    .fields()
                    .repository
                    .canonicalize()
                    .unwrap_or_else(|_| capture.fields().repository.clone())
                    == scope.project
            },
            |repository| *repository == capture.fields().git_common_dir,
        );
        if record.status.operation_id != operation || !owned {
            return Err("operation does not belong to the current project".into());
        }
        let mut histories = Vec::new();
        let mut blocked = false;
        for run in
            crate::app_state::bootstrap_related_runs(&record.status).map_err(str::to_owned)?
        {
            let (token, was_blocked) = cx.update(|cx| {
                let token = state
                    .update(cx, |state, _| {
                        if state.observation_scope().as_ref() != Some(scope) {
                            return None;
                        }
                        let owner = if let Some(eligible) = eligible {
                            let owner = eligible.iter().find(|token| token.run == run)?;
                            if !state.observations.retry_ready(owner) {
                                return None;
                            }
                            owner.clone()
                        } else {
                            state.observations.claim(scope, run, false)?
                        };
                        let token = state.begin_history_read(&owner)?;
                        #[cfg(test)]
                        {
                            *state.related_history_reads.entry(run).or_default() += 1;
                        }
                        Some(token)
                    })
                    .ok()
                    .flatten();
                let blocked = token.is_none()
                    && state
                        .update(cx, |state, _| state.observations.is_reading(run))
                        .unwrap_or(false);
                (token, blocked)
            });
            let Some(token) = token else {
                blocked |= was_blocked;
                continue;
            };
            let history = match read_history(&scope.home, run).await {
                Ok(Some(events)) => bound_related_at_home(&record, run, &events, &scope.home)
                    .await
                    .map(|()| Some(events)),
                other => other,
            };
            histories.push((token, history));
        }
        before_recheck.await;
        let current = observation
            .recheck()
            .await
            .map_err(|error| error.to_string());
        if current != Ok(true) {
            retired = cx.update(|cx| {
                state
                    .update(cx, |state, _| {
                        let mut retired = Vec::new();
                        for (token, _) in &histories {
                            retired.push(state.retire_history_read(token));
                        }
                        retired
                    })
                    .unwrap_or_default()
            });
            return Err(current
                .err()
                .unwrap_or_else(|| "operation changed during inspection; retry required".into()));
        }
        Ok(Some((record, histories, blocked)))
    }
    .await;
    (result, retired)
}
#[derive(Clone, Copy)]
pub(super) enum FallbackCompletion {
    Recovering,
    Nonterminal,
    Terminal,
    Unconfirmed,
}
pub(super) struct FallbackOwner {
    pub(super) token: Token,
    pub(super) completion: FallbackCompletion,
    pub(super) renewed: bool,
}
impl SurgeApp {
    pub(super) fn recover_rejected_subscription(&mut self, token: Token, cx: &mut Context<Self>) {
        let current = self.state.update(cx, |state, cx| {
            if !state.accepts_observation(&token, false)
                || !state.observations.reject_unattached(&token)
            {
                return false;
            }
            let stream = state.run_streams.entry(token.run).or_default();
            let revision = stream.observation_revision();
            stream.history_unconfirmed_if_unchanged(
                "Live attachment was unavailable; restoring recorded history",
                revision,
            );
            cx.notify();
            true
        });
        if !current {
            return;
        }
        self.stream_subscribed.remove(&token.run);
        self.fallback_owners.insert(
            token.run,
            FallbackOwner {
                token: token.clone(),
                completion: FallbackCompletion::Recovering,
                renewed: false,
            },
        );
        #[cfg(test)]
        let controlled_reads = self.controlled_history_reads.clone();
        let state = self.state.downgrade();
        cx.spawn(async move |this:WeakEntity<Self>,cx:&mut AsyncApp| {
            for attempt in 0..=3 {
                let read=cx.update(|cx|state.update(cx,|state,_| {
                    if !state.accepts_observation(&token,false){return None;}
                    let read=state.begin_history_read(&token)?;
                    #[cfg(test)] { *state.rejected_history_reads.entry(token.run).or_default()+=1; }
                    Some(read)
                }).ok().flatten());
                let Some(read)=read else{return;};
                let result=read_history(&read.scope.home,read.run).await;
                #[cfg(test)]
                let barrier=controlled_reads.as_ref().and_then(|reads|reads.borrow_mut().pop_front());
                #[cfg(test)]
                if let Some((loaded,release))=barrier { let _=loaded.send(());let _=release.await; }
                let completion=publish_read(&state,&read,result,cx);
                let confirmed=completion.confirmed();
                let _=this.update(cx,|app,cx| {
                    app.settle_current_reads(std::slice::from_ref(&completion),cx);
                    if !confirmed && attempt==3 {
                        app.mark_fallback_exhausted(&read,cx);
                    }
                    app.drain_pending_operations(cx);
                });
                if confirmed{return;}
                let current=cx.update(|cx|state.update(cx,|state,_|state.accepts_observation(&read,false)&&state.observations.retry_ready(&read)).unwrap_or(false));
                if !current{return;}
                if attempt==3 {
                    cx.update(|cx|{let _=state.update(cx,|state,cx| {
                        if state.accepts_observation(&read,false) && state.observations.last_read_is_current(&read){state.rejected_inspections.insert(read.run,(read.clone(),"Recorded history recovery retries exhausted; inspect the current source".into()));cx.notify();}
                    });});return;
                }
                cx.background_executor().timer(history_retry_delay(attempt)).await;
                let current=cx.update(|cx|state.update(cx,|state,_|state.accepts_observation(&read,false)&&state.observations.retry_ready(&read)).unwrap_or(false));
                if !current{return;}
            }
        }).detach();
    }

    pub(super) fn settle_current_reads(
        &mut self,
        completions: &[crate::app_state::ReadCompletion],
        cx: &mut Context<Self>,
    ) {
        use crate::app_state::ReadDisposition;
        let mut renewal = false;
        for completion in completions {
            let token = &completion.token;
            if completion.disposition == ReadDisposition::Obsolete
                || !self.state.update(cx, |state, _| {
                    state.accepts_observation(token, false)
                        && state.observations.last_read_is_current(token)
                })
            {
                continue;
            }
            #[cfg(test)]
            self.current_read_settlements
                .push((token.clone(), completion.disposition));
            let Some(owner) = self.fallback_owners.get_mut(&token.run) else {
                continue;
            };
            if !self.state.update(cx, |state, _| {
                state.accepts_observation(&owner.token, false)
            }) {
                continue;
            }
            let terminal = self
                .state
                .read(cx)
                .run_streams
                .get(&token.run)
                .is_some_and(|stream| {
                    matches!(
                        stream.display(),
                        surge_core::run_display::RunDisplayState::Done(_)
                    )
                });
            owner.completion = if terminal {
                FallbackCompletion::Terminal
            } else if completion.confirmed() {
                FallbackCompletion::Nonterminal
            } else {
                FallbackCompletion::Recovering
            };
            renewal |= owner.renewed;
        }
        if renewal {
            self.sync_run_subscriptions(cx);
        }
    }
    fn mark_fallback_exhausted(&mut self, token: &Token, cx: &mut Context<Self>) {
        if self.state.update(cx, |state, _| {
            state.accepts_observation(token, false)
                && state.observations.last_read_is_current(token)
        }) && let Some(owner) = self.fallback_owners.get_mut(&token.run)
            && self.state.update(cx, |state, _| {
                state.accepts_observation(&owner.token, false)
            })
        {
            owner.completion = FallbackCompletion::Unconfirmed;
        }
    }

    pub(super) fn restore_indexed_operations(&mut self, cx: &mut Context<Self>) {
        let scope = self.state.update(cx, |state, _| state.observation_scope());
        let Some(scope) = scope else {
            return;
        };
        for entry in self.bootstrap_index.clone().into_iter().filter(|entry| {
            entry
                .project_path
                .canonicalize()
                .unwrap_or_else(|_| entry.project_path.clone())
                == scope.project
        }) {
            self.read_operation(scope.clone(), entry.operation_id, cx);
        }
    }
    fn read_operation(
        &mut self,
        scope: Scope,
        operation: surge_core::RunId,
        cx: &mut Context<Self>,
    ) {
        self.read_operation_observed(scope, operation, cx, std::future::ready(()));
    }
    pub(super) fn read_operation_observed(
        &mut self,
        scope: Scope,
        operation: surge_core::RunId,
        cx: &mut Context<Self>,
        before_publish: impl std::future::Future<Output = ()> + 'static,
    ) {
        self.read_operation_selected(scope, operation, cx, before_publish, None);
    }
    fn read_operation_selected(
        &mut self,
        scope: Scope,
        operation: surge_core::RunId,
        cx: &mut Context<Self>,
        before_publish: impl std::future::Future<Output = ()> + 'static,
        eligible: Option<Vec<Token>>,
    ) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let key = (scope.clone(), operation);
        let expected = self
            .bootstrap_operations
            .get(&operation)
            .map(|(_, status)| status.clone());
        if self.offline_operations.contains_key(&key) {
            self.remember_operation_demand(scope, operation, expected, eligible);
            return;
        }
        self.next_operation_request = self.next_operation_request.saturating_add(1);
        let batch = self.next_operation_request;
        self.offline_operations.insert(key.clone(), batch);
        self.pending_operations.remove(&key);
        let state = self.state.downgrade();
        #[cfg(test)]
        let controlled = self
            .controlled_operation_reads
            .as_ref()
            .and_then(|reads| reads.borrow_mut().pop_front());
        #[cfg(test)]
        let recheck = self
            .controlled_operation_rechecks
            .as_ref()
            .and_then(|reads| reads.borrow_mut().pop_front());
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            #[cfg(test)]
            let before_recheck = async move {
                if let Some((loaded, release)) = recheck {
                    let _ = loaded.send(());
                    let _ = release.await;
                }
            };
            #[cfg(not(test))]
            let before_recheck = std::future::ready(());
            let result = inspect_operation(
                &scope,
                operation,
                &state,
                cx,
                eligible.as_deref(),
                before_recheck,
            )
            .await;
            #[cfg(test)]
            if let Some((loaded, release)) = controlled {
                let _ = loaded.send(());
                let _ = release.await;
            }
            before_publish.await;
            let _ = this.update(cx, |app, cx| {
                let publication = app.publish_operation_observation(
                    scope.clone(),
                    operation,
                    expected,
                    result,
                    cx,
                );
                app.settle_operation_batch(scope.clone(), operation, batch, publication.coverage);
                app.settle_current_reads(&publication.completions, cx);
                if let Some((tokens, record)) = publication.retry {
                    let tokens = app.state.update(cx, |state, _| {
                        tokens
                            .into_iter()
                            .filter(|token| state.observations.retry_ready(token))
                            .collect::<Vec<_>>()
                    });
                    if record || !tokens.is_empty() {
                        app.queue_operation_retry(scope, operation, tokens, record, cx);
                    }
                }
                app.drain_pending_operations(cx);
            });
        })
        .detach();
    }
    fn publish_operation_observation(
        &mut self,
        scope: Scope,
        operation: surge_core::RunId,
        expected: Option<surge_core::bootstrap_operation::BootstrapOperationStatus>,
        result: InspectionResult,
        cx: &mut Context<Self>,
    ) -> OperationPublication {
        let (result, retired) = result;
        let mut completions = retired;
        let mut covered = None;
        let mut blocked = false;
        let key = (scope.clone(), operation);
        let current = self.state.update(cx, |state, _| {
            state.observation_scope().as_ref() == Some(&scope)
        });
        if !current {
            return OperationPublication {
                coverage: (covered, blocked),
                completions,
                retry: None,
            };
        }
        if !matches!(result, Ok(Some(_)))
            && self
                .bootstrap_operations
                .get(&operation)
                .map(|(_, status)| status)
                != expected.as_ref()
        {
            return OperationPublication {
                coverage: (covered, blocked),
                completions,
                retry: None,
            };
        }
        let mut failures = Vec::new();
        let mut record_failed = false;
        let mut accepted = false;
        match result {
            Ok(Some((record, histories, was_blocked))) => {
                if self
                    .bootstrap_operations
                    .get(&operation)
                    .is_some_and(|(_, old)| {
                        old.revision > record.status.revision
                            || old.planning_run != record.status.planning_run
                            || old.implementation_run != record.status.implementation_run
                            || (old.revision == record.status.revision && *old != record.status)
                    })
                {
                    self.state.update(cx, |state, _| {
                        for (token, _) in &histories {
                            completions.push(state.retire_history_read(token));
                        }
                    });
                    return OperationPublication {
                        coverage: (covered, blocked),
                        completions,
                        retry: None,
                    };
                }
                covered = Some(record.status.clone());
                blocked = was_blocked;
                if let Some(episode) = self.offline_retries.get_mut(&key) {
                    accepted = episode.record;
                    episode.record = false; // The checked record recovered; sibling/read budgets remain intact.
                }
                self.state
                    .update(cx, |state, _| state.confirm_operation_record(operation));
                self.update_bootstrap_view(operation, record.status.clone(), &scope.project, cx);
                for (token, history) in histories {
                    let expected_started = expects_related_history(&record, token.run);
                    let completion = self.state.update(cx, |state, cx| {
                        let completion = state.complete_related_read(
                            operation,
                            &token,
                            history,
                            expected_started,
                        );
                        cx.notify();
                        completion
                    });
                    match completion.disposition {
                        crate::app_state::ReadDisposition::Obsolete => {},
                        crate::app_state::ReadDisposition::Incomplete => {
                            accepted = true;
                            failures.push(completion.token.clone());
                        },
                        _ => accepted = true,
                    }
                    completions.push(completion);
                }
            },
            Ok(None) => self.state.update(cx, |state, cx| {
                state.set_operation_refusal(operation, "Operation record is absent".into());
                cx.notify();
            }),
            Err(error) => {
                record_failed = true;
                accepted = true;
                self.state.update(cx, |state, cx| {
                    state.set_operation_refusal(operation, error);
                    cx.notify();
                });
            },
        }
        if !accepted {
            return OperationPublication {
                coverage: (covered, blocked),
                completions,
                retry: None,
            };
        } // Obsolete completions are counter-neutral.
        if let Some(episode) = self.offline_retries.get(&key) {
            let retained = self.state.update(cx, |state, _| {
                episode
                    .eligible
                    .iter()
                    .filter(|token| state.observations.retry_ready(token))
                    .cloned()
                    .collect::<Vec<_>>()
            });
            for token in retained {
                if !failures.iter().any(|failed| failed.run == token.run) {
                    failures.push(token);
                }
            }
        }
        if !record_failed && failures.is_empty() {
            if !blocked {
                self.offline_retries.remove(&key);
            }
            return OperationPublication {
                coverage: (covered, blocked),
                completions,
                retry: None,
            };
        }
        OperationPublication {
            coverage: (covered, blocked),
            completions,
            retry: Some((failures, record_failed)),
        }
    }
    fn remember_operation_demand(
        &mut self,
        scope: Scope,
        operation: surge_core::RunId,
        status: Option<surge_core::bootstrap_operation::BootstrapOperationStatus>,
        eligible: Option<Vec<Token>>,
    ) {
        let key = (scope, operation);
        if let Some(demand) = self.pending_operations.get_mut(&key)
            && demand.status == status
        {
            if eligible.is_some() {
                demand.eligible = eligible;
            }
            return;
        }
        self.next_operation_request = self.next_operation_request.saturating_add(1);
        self.pending_operations.insert(
            key,
            OperationDemand {
                request: self.next_operation_request,
                status,
                eligible,
            },
        );
    }
    fn settle_operation_batch(
        &mut self,
        scope: Scope,
        operation: surge_core::RunId,
        batch: u64,
        coverage: (
            Option<surge_core::bootstrap_operation::BootstrapOperationStatus>,
            bool,
        ),
    ) {
        let key = (scope.clone(), operation);
        if self.offline_operations.get(&key) != Some(&batch) {
            return;
        }
        self.offline_operations.remove(&key);
        if coverage.1 {
            let status = self
                .bootstrap_operations
                .get(&operation)
                .map(|(_, status)| status.clone());
            if !self.pending_operations.contains_key(&key) {
                self.remember_operation_demand(scope, operation, status, None);
            }
        } else if self.pending_operations.get(&key).is_some_and(|demand| {
            coverage
                .0
                .as_ref()
                .is_some_and(|status| demand.status.as_ref() == Some(status))
        }) {
            self.pending_operations.remove(&key);
        }
    }
    pub(super) fn drain_pending_operations(&mut self, cx: &mut Context<Self>) {
        let scope = self.state.update(cx, |state, _| state.observation_scope());
        let keys: Vec<_> = self.pending_operations.keys().cloned().collect();
        for key in keys {
            let Some(demand) = self.pending_operations.get(&key) else {
                continue;
            };
            if scope.as_ref() != Some(&key.0)
                || self
                    .bootstrap_operations
                    .get(&key.1)
                    .map(|(_, status)| status)
                    != demand.status.as_ref()
            {
                self.pending_operations.remove(&key);
                continue;
            }
            if self.offline_operations.contains_key(&key) {
                continue;
            }
            let reading = self
                .state
                .read(cx)
                .bootstrap_operations
                .get(&key.1)
                .is_some_and(|operation| {
                    crate::app_state::bootstrap_related_runs(operation)
                        .unwrap_or_default()
                        .into_iter()
                        .any(|run| self.state.read(cx).observations.is_reading(run))
                });
            if reading {
                continue;
            }
            // A same-episode pending request must use its already queued bounded timer.
            let record_only = if demand.eligible.is_none() {
                if let Some(episode) = self
                    .offline_retries
                    .get(&key)
                    .filter(|episode| episode.status == demand.status)
                {
                    if episode.record
                        || episode
                            .eligible
                            .iter()
                            .any(|token| self.state.read(cx).observations.retry_ready(token))
                    {
                        continue;
                    }
                    true
                } else {
                    false
                }
            } else {
                false
            };
            let request = demand.request;
            let eligible = if record_only {
                Some(Vec::new())
            } else {
                demand.eligible.as_ref().map(|tokens| {
                    tokens
                        .iter()
                        .filter(|token| self.state.read(cx).observations.retry_ready(token))
                        .cloned()
                        .collect::<Vec<_>>()
                })
            };
            if !record_only && eligible.as_ref().is_some_and(Vec::is_empty) {
                self.pending_operations.remove(&key);
                continue;
            }
            if self
                .pending_operations
                .get(&key)
                .is_some_and(|pending| pending.request == request)
            {
                self.read_operation_selected(key.0, key.1, cx, std::future::ready(()), eligible);
            }
        }
    }

    fn queue_operation_retry(
        &mut self,
        scope: Scope,
        operation: surge_core::RunId,
        eligible: Vec<Token>,
        record: bool,
        cx: &mut Context<Self>,
    ) {
        let key = (scope.clone(), operation);
        let status = self
            .bootstrap_operations
            .get(&operation)
            .map(|(_, status)| status.clone());
        if self
            .offline_retries
            .get(&key)
            .is_none_or(|episode| episode.status != status)
        {
            self.next_offline_retry = self.next_offline_retry.saturating_add(1);
            self.offline_retries.insert(
                key.clone(),
                RetryEpisode {
                    serial: self.next_offline_retry,
                    attempt: 0,
                    status: status.clone(),
                    eligible: Vec::new(),
                    record: false,
                },
            );
        }
        let episode = self.offline_retries.get_mut(&key);
        let Some(episode) = episode else {
            return;
        };
        episode.eligible = eligible;
        episode.record = record;
        if episode.attempt >= 3 {
            self.state.update(cx, |state, cx| {
                state.exhaust_related_retries(operation, &episode.eligible);
                if record && let Some(reason) = state.operation_inspection_reason(operation) {
                    state.set_operation_refusal(
                        operation,
                        format!("{reason}; inspection retries exhausted"),
                    );
                }
                cx.notify();
            });
            return;
        }
        let delay = history_retry_delay(episode.attempt);
        episode.attempt += 1;
        let serial = episode.serial;
        let attempt = episode.attempt;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |app, cx| {
                let key = (scope.clone(), operation);
                let Some(episode) = app.offline_retries.get(&key) else {
                    return;
                };
                if episode.serial != serial
                    || episode.attempt != attempt
                    || episode.status != status
                    || app
                        .bootstrap_operations
                        .get(&operation)
                        .map(|(_, status)| status)
                        != status.as_ref()
                {
                    return;
                }
                let eligible = app.state.update(cx, |state, _| {
                    if state.observation_scope().as_ref() != Some(&scope) {
                        return None;
                    }
                    Some(
                        episode
                            .eligible
                            .iter()
                            .filter(|token| state.observations.retry_ready(token))
                            .cloned()
                            .collect::<Vec<_>>(),
                    )
                });
                let Some(eligible) = eligible else {
                    return;
                };
                if !episode.record && eligible.is_empty() {
                    return;
                }
                let selected = if episode.record { None } else { Some(eligible) };
                app.read_operation_selected(
                    scope.clone(),
                    operation,
                    cx,
                    std::future::ready(()),
                    selected,
                );
            });
        })
        .detach();
    }
    pub(super) fn hydrate_related(&mut self, operation: surge_core::RunId, cx: &mut Context<Self>) {
        let scope = self.state.update(cx, |state, _| state.observation_scope());
        if let Some(scope) = scope {
            self.read_operation(scope, operation, cx);
        }
    }
}
pub(super) fn publish_read(
    state: &WeakEntity<AppState>,
    token: &Token,
    result: Result<Option<Vec<ReadEvent>>, String>,
    cx: &mut AsyncApp,
) -> crate::app_state::ReadCompletion {
    cx.update(|cx| {
        state
            .update(cx, |state, cx| {
                let completion = state.complete_history_read(token, result);
                cx.notify();
                completion
            })
            .unwrap_or_else(|_| crate::app_state::ReadCompletion::obsolete(token))
    })
}
