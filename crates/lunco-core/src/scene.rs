//! Typed scene-transition intents and lifecycle edges.
//!
//! This module is the dependency-light contract between scene consumers and
//! the USD scene owner. Consumers request a transition without knowing which
//! command handler mounts the stage; the owner publishes lifecycle edges from
//! the same command boundary that performs teardown and mounting.

use bevy::ecs::schedule::ScheduleLabel;
use bevy::prelude::*;

/// Runs once while an outgoing scene still exists and before its replacement is
/// projected.
///
/// Scene-owned entities are despawned by the scene owner. Subsystems register
/// resource resets and derived-entity retirement here, beside the state they
/// own. Keeping this label in `lunco-core` lets lower-level domains participate
/// in the transaction without depending on the USD projection layer.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct SceneTeardown;

/// Execute the one scene-teardown transaction.
///
/// This is an exclusive-world command so the schedule, its deferred writes,
/// and the following entity reclamation are one ordered command-queue phase.
pub fn run_scene_teardown(world: &mut World) {
    if world.try_run_schedule(SceneTeardown).is_err() {
        debug!("[clear-scene] no SceneTeardown schedule registered");
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
    active: Option<SceneTransition>,
    admitted: Option<SceneTransitionRequest>,
    pending: Option<SceneTransitionRequest>,
}

impl SceneTransitionCoordinator {
    pub fn admit(&mut self, request: SceneTransitionRequest) -> SceneTransitionAdmission {
        if let Some(active) = self.active.as_ref() {
            if request.matches(active) {
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
    pub fn start(&mut self, transition: SceneTransition) {
        assert!(
            self.admitted.is_none(),
            "scene transition started before its admitted request was dispatched"
        );
        assert!(
            self.active.replace(transition).is_none(),
            "scene transition started while another transaction is active"
        );
    }

    /// Close the active transaction and admit the pending request for the next
    /// lifecycle phase.
    pub fn finish(&mut self, transition: &SceneTransition) {
        assert_eq!(
            self.active.as_ref(),
            Some(transition),
            "scene terminal edge does not match the active transaction"
        );
        assert!(
            self.admitted.is_none(),
            "scene transaction reached a terminal edge while another request was already admitted"
        );
        self.active = None;
        self.admitted = self.pending.take();
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
        self.active.as_ref()
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
    pub transition: SceneTransition,
}

/// Published after a transition has reached its authoritative completion edge.
#[derive(Event, Debug, Clone, PartialEq, Eq)]
pub struct SceneTransitionCompleted {
    pub transition: SceneTransition,
}

/// Published when a requested stage cannot reach its completion edge.
#[derive(Event, Debug, Clone, PartialEq, Eq)]
pub struct SceneTransitionFailed {
    pub transition: SceneTransition,
    pub error: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Clone, PartialEq, Debug)]
    struct SceneOnly(u32);

    #[derive(Resource, Clone, PartialEq, Debug)]
    struct AppOwned(u32);

    #[test]
    fn teardown_removes_scene_state_and_restores_app_state() {
        let mut app = App::new();
        app.add_systems(SceneTeardown, |mut commands: Commands| {
            commands.remove_resource::<SceneOnly>();
            commands.insert_resource(AppOwned(1));
        });
        app.insert_resource(SceneOnly(42));
        app.insert_resource(AppOwned(99));

        run_scene_teardown(app.world_mut());

        assert!(app.world().get_resource::<SceneOnly>().is_none());
        assert_eq!(app.world().get_resource::<AppOwned>(), Some(&AppOwned(1)));
    }

    #[test]
    fn teardown_is_repeatable() {
        let mut app = App::new();
        app.add_systems(SceneTeardown, |mut commands: Commands| {
            commands.remove_resource::<SceneOnly>();
        });

        for value in [1u32, 2, 3] {
            app.insert_resource(SceneOnly(value));
            run_scene_teardown(app.world_mut());
            assert!(app.world().get_resource::<SceneOnly>().is_none());
        }
    }

    #[test]
    fn missing_schedule_is_a_valid_empty_transaction() {
        let mut app = App::new();
        run_scene_teardown(app.world_mut());
    }

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
        coordinator.start(first.clone());
        assert_eq!(
            coordinator.admit(SceneTransitionRequest::load("second.usda", "/World")),
            SceneTransitionAdmission::Queued
        );

        coordinator.finish(&first);
        assert!(coordinator.active().is_none());
        assert!(coordinator.has_admitted());
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
