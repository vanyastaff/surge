//! One publisher lease per actual run. Scope generations never revive old callbacks.
use std::{collections::HashMap, path::PathBuf};
use surge_core::RunId;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Scope {
    pub(crate) home: PathBuf,
    pub(crate) project: PathBuf,
    pub(crate) repository: Option<PathBuf>,
    generation: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Token {
    pub(crate) scope: Scope,
    pub(crate) run: RunId,
    owner: u64,
    read: u64,
    pub(crate) error_revision: u64,
}
#[derive(Default)]
pub(crate) struct Observations {
    generation: u64,
    next_owner: u64,
    scope: Option<Scope>,
    owners: HashMap<RunId, Owner>,
}
struct Owner {
    token: Token,
    live: bool,
    in_flight: bool,
    complete: bool,
    recovering: bool,
}
impl Observations {
    pub(crate) fn clear(&mut self) {
        self.generation = self.generation.saturating_add(1);
        self.scope = None;
        self.owners.clear();
    }
    pub(crate) fn scope(
        &mut self,
        home: PathBuf,
        project: PathBuf,
        repository: Option<PathBuf>,
    ) -> Scope {
        let mut scope = Scope {
            home,
            project,
            repository,
            generation: self.generation,
        };
        if self.scope.as_ref() != Some(&scope) {
            self.clear();
            scope.generation = self.generation;
            self.scope = Some(scope.clone());
        }
        scope
    }
    pub(crate) fn accepts_scope(&self, scope: &Scope) -> bool {
        self.scope.as_ref() == Some(scope)
    }
    pub(crate) fn retain_runs(&mut self, known: &std::collections::HashSet<RunId>) {
        self.owners.retain(|run, _| known.contains(run));
    }
    pub(crate) fn claim(&mut self, scope: &Scope, run: RunId, live: bool) -> Option<Token> {
        if !self.accepts_scope(scope) {
            return None;
        }
        if self
            .owners
            .get(&run)
            .is_some_and(|owner| owner.live || (!live && (owner.in_flight || owner.complete)))
        {
            return None;
        }
        if !live
            && let Some(owner) = self.owners.get(&run)
            && owner.recovering
        {
            return Some(owner.token.clone());
        }
        self.next_owner = self.next_owner.saturating_add(1);
        let token = Token {
            scope: scope.clone(),
            run,
            owner: self.next_owner,
            read: 0,
            error_revision: 0,
        };
        self.owners.insert(
            run,
            Owner {
                token: token.clone(),
                live,
                in_flight: false,
                complete: false,
                recovering: false,
            },
        );
        Some(token)
    }
    pub(crate) fn accepts(&self, token: &Token) -> bool {
        self.accepts_scope(&token.scope)
            && self
                .owners
                .get(&token.run)
                .is_some_and(|owner| owner.token.owner == token.owner)
    }
    pub(crate) fn begin_read(&mut self, token: &Token, error_revision: u64) -> Option<Token> {
        if !self.accepts(token) {
            return None;
        }
        let owner = self.owners.get_mut(&token.run)?;
        if owner.in_flight {
            return None;
        }
        owner.token.read = owner.token.read.saturating_add(1);
        owner.token.error_revision = error_revision;
        owner.in_flight = true;
        Some(owner.token.clone())
    }
    pub(crate) fn accepts_read(&self, token: &Token) -> bool {
        self.accepts(token)
            && self.owners.get(&token.run).is_some_and(|owner| {
                owner.in_flight
                    && owner.token.read == token.read
                    && owner.token.error_revision == token.error_revision
            })
    }
    pub(crate) fn reject_unattached(&mut self, token: &Token) -> bool {
        if !self.accepts(token) {
            return false;
        }
        let Some(owner) = self.owners.get_mut(&token.run) else {
            return false;
        };
        if !owner.live || owner.in_flight {
            return false;
        }
        owner.live = false;
        owner.recovering = true;
        true
    }
    pub(crate) fn is_reading(&self, run: RunId) -> bool {
        self.owners.get(&run).is_some_and(|owner| owner.in_flight)
    }

    pub(crate) fn last_read_is_current(&self, token: &Token) -> bool {
        self.accepts(token)
            && self.owners.get(&token.run).is_some_and(|owner| {
                owner.token.read == token.read && owner.token.error_revision == token.error_revision
            })
    }
    pub(crate) fn retry_ready(&self, token: &Token) -> bool {
        self.last_read_is_current(token)
            && self
                .owners
                .get(&token.run)
                .is_some_and(|owner| !owner.live && !owner.in_flight && !owner.complete)
    }

    pub(crate) fn finish_read(&mut self, token: &Token, complete: bool) {
        if self.accepts_read(token)
            && let Some(owner) = self.owners.get_mut(&token.run)
        {
            owner.in_flight = false;
            owner.complete = complete;
        }
    }
    pub(crate) fn retire(&mut self, token: &Token) -> bool {
        if !self.accepts(token) {
            return false;
        }
        self.owners.remove(&token.run);
        true
    }
}
