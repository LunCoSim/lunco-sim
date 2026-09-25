//! Typed scene-transition intents and lifecycle edges.
//!
//! This module is the dependency-light contract between scene consumers and
//! the USD scene owner. Consumers request a transition without knowing which
//! command handler mounts the stage; the owner publishes lifecycle edges from
//! the same command boundary that performs teardown and mounting.

use bevy::prelude::*;

/// Monotonic identity for one admitted scene lifecycle transaction.
///
/// Scene paths are descriptive input, not transaction identities: the same
/// scene may be loaded more than once while stale async results are still in
/// flight. Every lifecycle edge carries this id so a late terminal event
/// cannot close a newer transaction with the same path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SceneTransitionId(u64);

impl SceneTransitionId {
    /// Stable process-local transaction sequence number.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// The complete identity of a scene transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SceneTransition {
    /// Mount a stage at the requested root. An empty root means the stage's
    /// authored default root, resolved by the USD owner.
    Load { path: String, root_prim: String },
    /// Remove the active scene and leave the viewport empty.
    Clear,
    /// Re-read the currently mounted stage from its authoritative source.
    Restart {
        path: String,
        root_prim: String,
        reset_document: bool,
    },
}

/// A request for a scene transition before the scene owner has resolved its
/// concrete transaction identity.
///
/// In particular, restart deliberately carries no path: it means "restart the
/// scene that is active when this request is admitted". This distinction keeps
/// a restart queued behind an asynchronous load from accidentally targeting the
/// outgoing scene that happened to be visible when the request was made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SceneTransitionRequest {
    Load { path: String, root_prim: String },
    Clear,
    Restart { reset_document: bool },
}

impl SceneTransitionRequest {
    pub fn load(path: impl Into<String>, root_prim: impl Into<String>) -> Self {
        Self::Load {
            path: path.into(),
            root_prim: root_prim.into(),
        }
    }

    pub const fn clear() -> Self {
        Self::Clear
    }

    pub const fn restart(reset_document: bool) -> Self {
        Self::Restart { reset_document }
    }

    fn matches(&self, active: &SceneTransition) -> bool {
        match (self, active) {
            (
                Self::Load { path, root_prim },
                SceneTransition::Load {
                    path: active_path,
                    root_prim: active_root,
                },
            ) => path == active_path && root_prim == active_root,
            (Self::Clear, SceneTransition::Clear) => true,
            (
                Self::Restart { reset_document },
                SceneTransition::Restart {
                    reset_document: active_reset,
                    ..
                },
            ) => reset_document == active_reset,
            _ => false,
        }
    }
}

/// Result of submitting a request to the scene transaction owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneTransitionAdmission {
    /// No transaction is active; this request is admitted for the next
    /// lifecycle execution phase.
    Admitted,
    /// The same transition is already active. No second lifecycle is required.
    AlreadyActive,
    /// Another transition is active. This request is now the one pending request.
    Queued,
}

/// Serializes scene replacement transactions.
///
/// This resource is the sole admission boundary. An active transaction is never
/// torn down by a second request while asset/projection work still owns entities
/// from it. The newest request is retained and admitted only from the active
/// transaction's completed/failed edge. There is no frame polling or retry path.
#[derive(Resource, Debug, Default)]
pub struct SceneTransitionCoordinator {
    active: Option<(SceneTransitionId, SceneTransition)>,
    admitted: Option<SceneTransitionRequest>,
    pending: Option<SceneTransitionRequest>,
    next_id: u64,
    completed_generation: Option<SceneTransitionId>,
}

impl SceneTransitionCoordinator {
    pub fn admit(&mut self, request: SceneTransitionRequest) -> SceneTransitionAdmission {
        if let Some(active) = self.active.as_ref() {
            if request.matches(&active.1) {
                self.pending = None;
                return SceneTransitionAdmission::AlreadyActive;
            }
            self.pending = Some(request);
            return SceneTransitionAdmission::Queued;
        }

        if let Some(admitted) = self.admitted.as_ref() {
            if admitted == &request {
                self.pending = None;
                return SceneTransitionAdmission::AlreadyActive;
            }
            self.pending = Some(request);
            return SceneTransitionAdmission::Queued;
        }

        debug_assert!(self.pending.is_none());
        self.admitted = Some(request);
        SceneTransitionAdmission::Admitted
    }

    /// Take the one request admitted for execution at the lifecycle phase.
    pub fn take_admitted(&mut self) -> Option<SceneTransitionRequest> {
        self.admitted.take()
    }

    /// Publish the concrete identity resolved by the admitted request.
    pub fn start(&mut self, transition: SceneTransition) -> SceneTransitionId {
        assert!(
            self.admitted.is_none(),
            "scene transition started before its admitted request was dispatched"
        );
        assert!(
            self.active.is_none(),
            "scene transition started while another transaction is active"
        );
        let id = SceneTransitionId(self.next_id.max(1));
        self.next_id =
            id.0.checked_add(1)
                .expect("scene transition identity sequence exhausted");
        self.active = Some((id, transition));
        id
    }

    /// Commit a successfully completed transition and admit the pending request
    /// for the next lifecycle phase.
    pub fn complete(&mut self, id: SceneTransitionId) -> bool {
        if !self.finish_active(id) {
            return false;
        }
        self.completed_generation = Some(id);
        true
    }

    /// Close a failed transaction without changing the generation of the last
    /// successfully composed scene.
    pub fn fail(&mut self, id: SceneTransitionId) -> bool {
        self.finish_active(id)
    }

    fn finish_active(&mut self, id: SceneTransitionId) -> bool {
        if self.active.as_ref().map(|(active_id, _)| *active_id) != Some(id) {
            return false;
        }
        assert!(
            self.admitted.is_none(),
            "scene transaction reached a terminal edge while another request was already admitted"
        );
        self.active = None;
        self.admitted = self.pending.take();
        true
    }

    /// Generation of the latest successfully completed scene transition.
    /// Failed, stale, and no-op requests do not advance it.
    pub const fn completed_generation(&self) -> Option<u64> {
        match self.completed_generation {
            Some(id) => Some(id.get()),
            None => None,
        }
    }

    /// Generation owned by lifecycle work currently admitting a scene.
    /// While a transaction is active, its identity owns projected entities;
    /// outside a transition, the latest successfully committed scene owns
    /// new lifecycle work. A failed transition never replaces that generation.
    pub fn lifecycle_generation(&self) -> Option<u64> {
        self.active_id()
            .map(SceneTransitionId::get)
            .or_else(|| self.completed_generation())
    }

    /// Advance after an admitted request resolves to a semantic no-op before a
    /// concrete transaction starts (for example, restart with no active scene).
    ///
    /// This is not a failure-recovery path: the admitted request has been
    /// consumed at the lifecycle boundary and owns no scene state. Any request
    /// submitted behind it becomes the next admitted request.
    pub fn finish_noop(&mut self) {
        assert!(
            self.active.is_none() && self.admitted.is_none(),
            "only a dispatched request that started no transaction can finish as a no-op"
        );
        self.admitted = self.pending.take();
    }

    pub fn active(&self) -> Option<&SceneTransition> {
        self.active.as_ref().map(|(_, transition)| transition)
    }

    /// Identity of the active scene transaction, if one has started.
    pub fn active_id(&self) -> Option<SceneTransitionId> {
        self.active.as_ref().map(|(id, _)| *id)
    }

    pub fn has_admitted(&self) -> bool {
        self.admitted.is_some()
    }
}

impl SceneTransition {
    /// Construct a stage-load intent.
    pub fn load(path: impl Into<String>, root_prim: impl Into<String>) -> Self {
        Self::Load {
            path: path.into(),
            root_prim: root_prim.into(),
        }
    }

    /// Construct a clear intent.
    pub const fn clear() -> Self {
        Self::Clear
    }
}

/// A typed request for the authoritative scene owner to perform a transition.
///
/// This is deliberately separate from the public API command envelope. A
/// tutorial or another in-process domain can request a scene without encoding
/// a command name and JSON parameters, while the USD command owner remains the
/// only code that resolves paths and performs the transition.
#[derive(Event, Debug, Clone, PartialEq, Eq)]
pub struct SceneTransitionIntent {
    pub request: SceneTransitionRequest,
}

/// Published by the scene transaction owner at the deterministic lifecycle
/// execution boundary after a request has won admission.
///
/// Public commands submit requests; only consumers of this edge may mutate
/// scene-owned state. Keeping submission and execution as separate event types
/// makes arbitrary caller schedules unable to tear down a scene mid-frame.
#[derive(Event, Debug, Clone, PartialEq, Eq)]
pub struct SceneTransitionAdmitted {
    pub request: SceneTransitionRequest,
}

impl SceneTransitionIntent {
    pub fn load(path: impl Into<String>, root_prim: impl Into<String>) -> Self {
        Self {
            request: SceneTransitionRequest::load(path, root_prim),
        }
    }

    pub const fn clear() -> Self {
        Self {
            request: SceneTransitionRequest::Clear,
        }
    }

    pub const fn restart(reset_document: bool) -> Self {
        Self {
            request: SceneTransitionRequest::Restart { reset_document },
        }
    }
}

/// Published immediately before an accepted scene transition tears down the
/// outgoing scene. All consumers use this edge to wind down their own state.
#[derive(Event, Debug, Clone, PartialEq, Eq)]
pub struct SceneTransitionStarted {
    pub id: SceneTransitionId,
    pub transition: SceneTransition,
}

/// Published after a transition has reached its authoritative completion edge.
#[derive(Event, Debug, Clone, PartialEq, Eq)]
pub struct SceneTransitionCompleted {
    pub id: SceneTransitionId,
    pub transition: SceneTransition,
}

/// Published only after the transaction owner accepts a matching completed
/// edge. Runtime cycles use this edge to arm work for the new scene generation.
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub struct SceneTransitionCommitted {
    pub id: SceneTransitionId,
}

/// Published when a requested stage cannot reach its completion edge.
#[derive(Event, Debug, Clone, PartialEq, Eq)]
pub struct SceneTransitionFailed {
    pub id: SceneTransitionId,
    pub transition: SceneTransition,
    pub error: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_transitions_are_serialized_at_terminal_edges() {
        let mut coordinator = SceneTransitionCoordinator::default();
        let first = SceneTransition::load("first.usda", "/World");

        assert_eq!(
            coordinator.admit(SceneTransitionRequest::load("first.usda", "/World")),
            SceneTransitionAdmission::Admitted
        );
        assert_eq!(
            coordinator.take_admitted(),
            Some(SceneTransitionRequest::load("first.usda", "/World"))
        );
        let first_id = coordinator.start(first.clone());
        assert_eq!(
            coordinator.admit(SceneTransitionRequest::load("second.usda", "/World")),
            SceneTransitionAdmission::Queued
        );

        assert!(!coordinator.complete(SceneTransitionId(first_id.get() + 1)));
        assert_eq!(coordinator.active_id(), Some(first_id));
        assert!(coordinator.complete(first_id));
        assert_eq!(coordinator.completed_generation(), Some(first_id.get()));
        assert!(coordinator.active().is_none());
        assert!(coordinator.has_admitted());
    }

    #[test]
    fn stale_or_failed_edges_preserve_the_last_successful_scene_generation() {
        let mut coordinator = SceneTransitionCoordinator::default();
        coordinator.admit(SceneTransitionRequest::load("first.usda", "/World"));
        coordinator.take_admitted();
        let first_id = coordinator.start(SceneTransition::load("first.usda", "/World"));
        assert_eq!(coordinator.lifecycle_generation(), Some(first_id.get()));
        assert!(coordinator.complete(first_id));
        assert_eq!(coordinator.completed_generation(), Some(first_id.get()));
        assert_eq!(coordinator.lifecycle_generation(), Some(first_id.get()));

        coordinator.admit(SceneTransitionRequest::load("second.usda", "/World"));
        coordinator.take_admitted();
        let second_id = coordinator.start(SceneTransition::load("second.usda", "/World"));
        assert_eq!(coordinator.lifecycle_generation(), Some(second_id.get()));

        assert!(!coordinator.complete(first_id));
        assert_eq!(coordinator.active_id(), Some(second_id));
        assert_eq!(coordinator.completed_generation(), Some(first_id.get()));
        assert!(coordinator.fail(second_id));
        assert_eq!(coordinator.completed_generation(), Some(first_id.get()));
        assert_eq!(coordinator.lifecycle_generation(), Some(first_id.get()));

        coordinator.admit(SceneTransitionRequest::load("third.usda", "/World"));
        coordinator.take_admitted();
        let third_id = coordinator.start(SceneTransition::load("third.usda", "/World"));
        assert_eq!(coordinator.lifecycle_generation(), Some(third_id.get()));
        assert!(coordinator.complete(third_id));
        assert_eq!(coordinator.completed_generation(), Some(third_id.get()));
        assert_eq!(coordinator.lifecycle_generation(), Some(third_id.get()));
    }

    #[test]
    fn restart_request_is_resolved_only_after_admission() {
        let request = SceneTransitionRequest::restart(true);
        assert_eq!(
            request,
            SceneTransitionRequest::Restart {
                reset_document: true
            }
        );
    }

    #[test]
    fn admitted_noop_promotes_the_request_queued_behind_it() {
        let mut coordinator = SceneTransitionCoordinator::default();
        let restart = SceneTransitionRequest::restart(false);
        let load = SceneTransitionRequest::load("next.usda", "/World");

        assert_eq!(
            coordinator.admit(restart.clone()),
            SceneTransitionAdmission::Admitted
        );
        assert_eq!(
            coordinator.admit(load.clone()),
            SceneTransitionAdmission::Queued
        );
        assert_eq!(coordinator.take_admitted(), Some(restart));

        coordinator.finish_noop();

        assert_eq!(coordinator.take_admitted(), Some(load));
    }
}
