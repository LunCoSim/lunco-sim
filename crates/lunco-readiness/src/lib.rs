//! Readiness: what the world is still waiting on, and what that waiting freezes.
//!
//! # The problem
//!
//! A scene does not become real all at once. A `.usda` loads, entities appear,
//! their programs (Modelica models, rhai scripts) are handed to a compiler, and
//! the compiler answers some frames — sometimes some *seconds* — later. Physics,
//! meanwhile, starts integrating immediately.
//!
//! The gap is not theoretical. A descent lander spawned with a guidance model
//! fell **55 m** before `begin: Compile model=DescentGuidance` appeared in the
//! log: by the time the thing that was supposed to fly it existed, it was
//! already somewhere else, at speed. Every "the vehicle was fine yesterday"
//! report of this shape is the same race.
//!
//! # The model
//!
//! Anything that will make an object controllable, collidable or correct
//! **declares itself** before it starts and clears when it finishes:
//!
//! ```ignore
//! let ticket = readiness.begin(Subject::Entity(e), kinds::PROGRAM_COMPILE, "DescentGuidance");
//! // …compile off-thread…
//! readiness.finish(ticket);
//! ```
//!
//! Three concepts, and keeping them apart is the whole design:
//!
//! | | Owner | Question |
//! |---|---|---|
//! | **Item** | the producer | "I am not ready yet" |
//! | **Action** | [policy](#policy) | "so what?" |
//! | **Effect** | the effector | "then freeze this" |
//!
//! A producer never decides whether the world stops — it cannot know whether a
//! given compile matters. It states a fact; policy interprets it.
//!
//! # Policy
//!
//! Every pending item is mapped to an [`Action`] — [`Proceed`](Action::Proceed),
//! [`HoldEntity`](Action::HoldEntity) or [`HoldWorld`](Action::HoldWorld) — by the
//! `readiness.action` hook ([`READINESS_HOOK`]), authored in
//! `assets/scripting/policy/readiness.rhai`. The action set is **closed**: policy
//! chooses between behaviours the engine already implements, so a bad rule can
//! pick the wrong one but cannot invent an unsafe one.
//!
//! Actions are re-evaluated **every frame**, not decided once at `begin`. That is
//! what makes a rule a function of `elapsed_s` — the shipped policy escalates a
//! slow boot and, past a deadline, gives up and lets the world run rather than
//! hanging on a compile that is never going to answer. It is also what makes the
//! policy hot-swappable: re-register the hook and the next frame obeys it.
//!
//! With no hook registered, [`Action::builtin`] decides — the same rule the
//! shipped policy states, so an app with no scripting behaves identically.
//!
//! # Effect
//!
//! This crate computes [`ReadinessState`] and marks held entities with
//! [`HeldForReadiness`]; it does not enforce anything and does not depend on a
//! physics engine. `lunco_physics::readiness` is the effector that turns
//! `world_hold` into a `PhysicsHolds` reason and [`HeldForReadiness`] into
//! disabled bodies and colliders.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::collections::BTreeMap;

use bevy::prelude::*;
use lunco_hooks::HookValue as H;

/// Hook id for the readiness policy: `(context) -> action name`.
///
/// Authored in `assets/scripting/policy/readiness.rhai`, entry `readiness_action`.
/// Local-only — never mark it deterministic; it reads wall-clock elapsed time and
/// two peers may legitimately disagree.
pub const READINESS_HOOK: &str = "readiness.action";

/// What a pending item is waiting for. A `&'static str` so it can be matched in
/// Rust and read by name in policy; the constants below are the vocabulary the
/// shipped policy knows.
pub mod kinds {
    /// The scene itself is still composing — entities, colliders and terrain are
    /// still arriving. Nothing in the world is trustworthy yet.
    pub const SCENE_LOAD: &str = "scene_load";
    /// A program (Modelica model, rhai script, …) that will drive a subject is
    /// being compiled or loaded.
    pub const PROGRAM_COMPILE: &str = "program_compile";
    /// A subject's simulation participant is being initialised — solver
    /// allocated, ports bound, first step not yet taken.
    pub const PARTICIPANT_INIT: &str = "participant_init";
}

/// Who is not ready.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Subject {
    /// The scene as a whole.
    World,
    /// One object. Only this object need be frozen — the rest of the world can
    /// carry on around it.
    Entity(Entity),
}

/// What the engine does about a pending item.
///
/// A **closed** set: policy picks one of these, so an authored rule can be wrong
/// but cannot be unsafe.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Action {
    /// Let the world run. The item is still tracked and still reported — this
    /// says the wait does not justify freezing anything.
    #[default]
    Proceed,
    /// Freeze the subject alone: it neither moves nor collides until it is ready,
    /// while everything around it keeps simulating. `Subject::World` cannot be
    /// frozen this way and escalates to [`Action::HoldWorld`].
    HoldEntity,
    /// Freeze all physics until the item clears.
    HoldWorld,
}

impl Action {
    /// Parse a policy's answer. Unknown text is [`None`] so the caller can warn
    /// and fall back rather than silently choosing something.
    pub fn parse(s: &str) -> Option<Action> {
        match s {
            "proceed" => Some(Action::Proceed),
            "hold_entity" => Some(Action::HoldEntity),
            "hold_world" => Some(Action::HoldWorld),
            _ => None,
        }
    }

    /// The name policy uses for this action.
    pub fn name(self) -> &'static str {
        match self {
            Action::Proceed => "proceed",
            Action::HoldEntity => "hold_entity",
            Action::HoldWorld => "hold_world",
        }
    }

    /// The engine's own rule, used when no policy hook is registered (and when
    /// one is registered but faults). `assets/scripting/policy/readiness.rhai`
    /// states the same rule, so scripted and unscripted hosts agree.
    ///
    /// - A loading scene holds the world: nothing in it is trustworthy yet.
    /// - A program or participant that a *specific object* is waiting on freezes
    ///   that object only. A rover whose script has not compiled must not roll
    ///   away, but a second rover that is ready has no reason to wait for it.
    /// - The same wait for the world as a whole holds the world.
    /// - Past [`Self::DEADLINE_S`], nothing holds. A hold exists to protect a
    ///   world that is about to become correct; a wait this long is a failure,
    ///   and a frozen app hides it where a moving one shows it.
    pub fn builtin(kind: &str, subject: Subject, elapsed_s: f64) -> Action {
        if elapsed_s >= Self::DEADLINE_S {
            return Action::Proceed;
        }
        match (kind, subject) {
            (kinds::SCENE_LOAD, _) => Action::HoldWorld,
            (kinds::PROGRAM_COMPILE | kinds::PARTICIPANT_INIT, Subject::Entity(_)) => {
                Action::HoldEntity
            }
            (kinds::PROGRAM_COMPILE | kinds::PARTICIPANT_INIT, Subject::World) => Action::HoldWorld,
            _ => Action::Proceed,
        }
    }

    /// How long any single item may hold before the engine stops waiting on it.
    /// Generous enough for a cold Modelica compile; short enough that a wedged
    /// one is a visibly moving world rather than a hung app.
    pub const DEADLINE_S: f64 = 60.0;
}

/// Handle to one declared wait. Returned by [`ReadinessRegistry::begin`] and
/// spent by [`ReadinessRegistry::finish`].
///
/// Opaque and non-`Copy` on purpose: a ticket is a resource that must be handed
/// back exactly once, and a copyable one invites finishing the same wait twice
/// from two code paths.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReadinessTicket(u64);

/// One declared, not-yet-finished wait.
#[derive(Clone, Debug)]
pub struct PendingItem {
    /// Who is waiting.
    pub subject: Subject,
    /// What is being waited on — one of [`kinds`].
    pub kind: &'static str,
    /// Human-readable detail for diagnostics (a model name, a file stem).
    pub label: String,
    /// Seconds since this item was declared.
    pub elapsed_s: f64,
    /// The action currently in force, as decided by policy this frame.
    pub action: Action,
    /// Set once the item has been reported as overdue, so the warning is emitted
    /// exactly once per item rather than every frame.
    warned: bool,
}

/// Everything the world is currently waiting on.
///
/// Producers hold `ResMut<ReadinessRegistry>` and call [`begin`](Self::begin) /
/// [`finish`](Self::finish). Nothing else should write to it.
#[derive(Resource, Debug, Default)]
pub struct ReadinessRegistry {
    items: BTreeMap<u64, PendingItem>,
    next: u64,
}

impl ReadinessRegistry {
    /// Declare that `subject` is not ready, and why.
    ///
    /// The returned ticket must be spent with [`finish`](Self::finish) on every
    /// path out — including failure. An abandoned ticket is a wait that never
    /// clears; the deadline in [`Action::builtin`] keeps that from wedging the
    /// app, but it is still a bug, and the overdue warning names it.
    pub fn begin(
        &mut self,
        subject: Subject,
        kind: &'static str,
        label: impl Into<String>,
    ) -> ReadinessTicket {
        let id = self.next;
        self.next += 1;
        let label = label.into();
        let action = Action::builtin(kind, subject, 0.0);
        debug!("[readiness] begin {kind} {subject:?} {label}");
        self.items.insert(
            id,
            PendingItem { subject, kind, label, elapsed_s: 0.0, action, warned: false },
        );
        ReadinessTicket(id)
    }

    /// Clear a wait. Idempotent in effect: a ticket can only be spent once
    /// because it is moved in.
    pub fn finish(&mut self, ticket: ReadinessTicket) {
        if let Some(item) = self.items.remove(&ticket.0) {
            debug!(
                "[readiness] ready {} {:?} {} after {:.2}s",
                item.kind, item.subject, item.label, item.elapsed_s
            );
        }
    }

    /// Clear every wait declared for `entity` — the despawn path. An entity that
    /// no longer exists cannot become ready, so its items would otherwise hold
    /// forever (or until the deadline, which is the same bug with a timer).
    pub fn finish_all_for(&mut self, entity: Entity) {
        self.items
            .retain(|_, item| item.subject != Subject::Entity(entity));
    }

    /// Drop every wait. Called on scene teardown: the next scene's readiness is
    /// declared by the next scene's producers.
    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Everything still pending, oldest ticket first.
    pub fn pending(&self) -> impl Iterator<Item = &PendingItem> + '_ {
        self.items.values()
    }

    /// Is anything at all pending?
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Number of pending items.
    pub fn len(&self) -> usize {
        self.items.len()
    }
}

/// The current *decision*, derived from the registry each frame. This is what an
/// effector reads; the registry itself is the producers' surface.
#[derive(Resource, Debug, Default, PartialEq, Eq)]
pub struct ReadinessState {
    /// At least one pending item asked to freeze all physics.
    pub world_hold: bool,
    /// Entities that at least one pending item asked to freeze individually.
    pub held_entities: Vec<Entity>,
}

/// Marks an entity frozen because something it needs is not ready.
///
/// Maintained by [`evaluate_readiness`] from [`ReadinessState`]. The physics
/// effector watches this component; anything else that must stand still while an
/// object is not ready (an animation driver, an audio source) can watch it too,
/// which is why the mark is a component rather than a private detail of the
/// physics bridge.
#[derive(Component, Debug, Clone, Copy)]
pub struct HeldForReadiness;

/// Ask policy what to do about one pending item.
///
/// Consults [`READINESS_HOOK`] and falls back to [`Action::builtin`] when no hook
/// is registered, when it faults, or when it answers with something outside the
/// closed action set.
fn decide(kind: &str, subject: Subject, label: &str, elapsed_s: f64) -> Action {
    let entity_bits = match subject {
        Subject::Entity(e) => e.to_bits() as i64,
        Subject::World => -1,
    };
    let ctx = H::map([
        ("kind", H::str(kind)),
        (
            "subject",
            H::str(match subject {
                Subject::World => "world",
                Subject::Entity(_) => "entity",
            }),
        ),
        ("entity", H::Int(entity_bits)),
        ("label", H::str(label)),
        ("elapsed_s", H::Float(elapsed_s)),
        ("deadline_s", H::Float(Action::DEADLINE_S)),
    ]);

    let fallback = || Action::builtin(kind, subject, elapsed_s);
    let Some(result) = lunco_hooks::invoke(READINESS_HOOK, &[ctx]) else {
        return fallback();
    };
    match result {
        Ok(v) => match v.as_str().and_then(Action::parse) {
            Some(action) => action,
            None => {
                bevy::log::warn_once!(
                    "[readiness] policy returned {v:?}, which is not one of \
                     proceed / hold_entity / hold_world — using the built-in rule"
                );
                fallback()
            }
        },
        Err(err) => {
            bevy::log::warn_once!("[readiness] policy faulted ({err}); using the built-in rule");
            fallback()
        }
    }
}

/// Age every pending item, re-run policy over it, and publish [`ReadinessState`].
///
/// Runs on the **real** clock (`Time<Real>`), not virtual time: a compile takes
/// as long as it takes regardless of time warp, and a deadline measured in
/// paused-or-warped seconds would either never fire or fire immediately.
pub fn evaluate_readiness(
    time: Res<Time<Real>>,
    mut registry: ResMut<ReadinessRegistry>,
    mut state: ResMut<ReadinessState>,
) {
    let dt = time.delta_secs_f64();
    let mut world_hold = false;
    let mut held: Vec<Entity> = Vec::new();

    for item in registry.items.values_mut() {
        item.elapsed_s += dt;
        item.action = decide(item.kind, item.subject, &item.label, item.elapsed_s);

        if !item.warned && item.elapsed_s >= Action::DEADLINE_S {
            item.warned = true;
            warn!(
                "[readiness] {} {:?} '{}' has been pending {:.0}s — no longer holding. \
                 Either it never finished, or its ticket was dropped without finish().",
                item.kind, item.subject, item.label, item.elapsed_s,
            );
        }

        match (item.action, item.subject) {
            (Action::HoldWorld, _) => world_hold = true,
            // A world-subject item cannot freeze "just itself" — there is no
            // object to freeze — so the request escalates rather than evaporating.
            (Action::HoldEntity, Subject::World) => world_hold = true,
            (Action::HoldEntity, Subject::Entity(e)) => held.push(e),
            (Action::Proceed, _) => {}
        }
    }

    held.sort_unstable();
    held.dedup();

    let next = ReadinessState { world_hold, held_entities: held };
    if *state != next {
        *state = next;
    }
}

/// Add and remove [`HeldForReadiness`] to match [`ReadinessState`].
///
/// Split from [`evaluate_readiness`] so the decision is testable without a
/// command queue, and so the mark is applied in one place.
pub fn apply_readiness_marks(
    state: Res<ReadinessState>,
    marked: Query<Entity, With<HeldForReadiness>>,
    mut commands: Commands,
) {
    for entity in &state.held_entities {
        if !marked.contains(*entity) {
            if let Ok(mut e) = commands.get_entity(*entity) {
                e.insert(HeldForReadiness);
            }
        }
    }
    for entity in &marked {
        if !state.held_entities.contains(&entity) {
            commands.entity(entity).remove::<HeldForReadiness>();
        }
    }
}

/// Installs the readiness registry and its per-frame evaluation.
///
/// Enforcement is **not** here — see `lunco_physics::readiness` for the effector
/// that turns these decisions into frozen physics.
pub struct ReadinessPlugin;

/// System set holding readiness evaluation, so an effector can order itself
/// after the decision it consumes.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReadinessSet;

impl Plugin for ReadinessPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ReadinessRegistry>()
            .init_resource::<ReadinessState>()
            .add_systems(
                PreUpdate,
                (evaluate_readiness, apply_readiness_marks)
                    .chain()
                    .in_set(ReadinessSet),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hook registry is process-global, so tests that install a policy — and
    /// tests that rely on there being none — must not run concurrently.
    fn policy_lock() -> std::sync::MutexGuard<'static, ()> {
        static L: std::sync::Mutex<()> = std::sync::Mutex::new(());
        L.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(ReadinessPlugin);
        app.init_resource::<Time<Real>>();
        app
    }

    /// The default rule, stated as behaviour rather than as a table: a loading
    /// scene stops the world; one object's compile stops that object.
    #[test]
    fn builtin_rule_scopes_the_hold_to_who_is_waiting() {
        let e = Entity::from_raw_u32(7).unwrap();
        assert_eq!(
            Action::builtin(kinds::SCENE_LOAD, Subject::World, 0.0),
            Action::HoldWorld
        );
        assert_eq!(
            Action::builtin(kinds::PROGRAM_COMPILE, Subject::Entity(e), 0.0),
            Action::HoldEntity
        );
        assert_eq!(
            Action::builtin(kinds::PROGRAM_COMPILE, Subject::World, 0.0),
            Action::HoldWorld
        );
    }

    /// The app must never stall. A wait that outlives the deadline stops holding
    /// anything, so a wedged compile leaves a world that visibly runs (and warns)
    /// instead of an app that appears hung.
    #[test]
    fn nothing_holds_past_the_deadline() {
        let e = Entity::from_raw_u32(1).unwrap();
        assert_eq!(
            Action::builtin(kinds::SCENE_LOAD, Subject::World, Action::DEADLINE_S + 0.1),
            Action::Proceed
        );
        assert_eq!(
            Action::builtin(
                kinds::PROGRAM_COMPILE,
                Subject::Entity(e),
                Action::DEADLINE_S + 0.1
            ),
            Action::Proceed
        );
    }

    /// An entity waiting on its program is marked; the mark clears when the wait
    /// does. This is the whole per-object contract, end to end.
    #[test]
    fn a_waiting_entity_is_marked_and_unmarked() {
        let _guard = policy_lock();
        let mut app = app();
        let e = app.world_mut().spawn_empty().id();
        let ticket = app.world_mut().resource_mut::<ReadinessRegistry>().begin(
            Subject::Entity(e),
            kinds::PROGRAM_COMPILE,
            "guidance",
        );

        app.update();
        assert!(app.world().entity(e).contains::<HeldForReadiness>());
        assert!(
            !app.world().resource::<ReadinessState>().world_hold,
            "one object's compile must not stop the world"
        );

        app.world_mut()
            .resource_mut::<ReadinessRegistry>()
            .finish(ticket);
        app.update();
        assert!(!app.world().entity(e).contains::<HeldForReadiness>());
    }

    /// Two objects, one waiting: the other must not be frozen on its behalf.
    #[test]
    fn a_held_object_does_not_freeze_its_neighbour() {
        let _guard = policy_lock();
        let mut app = app();
        let waiting = app.world_mut().spawn_empty().id();
        let ready = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<ReadinessRegistry>().begin(
            Subject::Entity(waiting),
            kinds::PROGRAM_COMPILE,
            "x",
        );

        app.update();
        assert!(app.world().entity(waiting).contains::<HeldForReadiness>());
        assert!(!app.world().entity(ready).contains::<HeldForReadiness>());
    }

    /// A scene load holds the world, not an object.
    #[test]
    fn scene_load_holds_the_world() {
        let _guard = policy_lock();
        let mut app = app();
        app.world_mut().resource_mut::<ReadinessRegistry>().begin(
            Subject::World,
            kinds::SCENE_LOAD,
            "scene.usda",
        );
        app.update();
        let state = app.world().resource::<ReadinessState>();
        assert!(state.world_hold);
        assert!(state.held_entities.is_empty());
    }

    /// A despawned entity can never report ready, so its waits must go with it.
    #[test]
    fn despawn_clears_that_entitys_waits() {
        let _guard = policy_lock();
        let mut app = app();
        let e = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<ReadinessRegistry>().begin(
            Subject::Entity(e),
            kinds::PROGRAM_COMPILE,
            "x",
        );
        app.update();
        assert_eq!(app.world().resource::<ReadinessRegistry>().len(), 1);

        app.world_mut().resource_mut::<ReadinessRegistry>().finish_all_for(e);
        app.update();
        assert!(app.world().resource::<ReadinessRegistry>().is_empty());
        assert!(app.world().resource::<ReadinessState>().held_entities.is_empty());
    }

    /// Policy overrides the built-in rule, and re-registering it takes effect on
    /// the next frame — the "customisable in realtime" requirement.
    #[test]
    fn policy_overrides_the_builtin_and_can_be_swapped_live() {
        let _guard = policy_lock();
        use lunco_hooks::{HookResult, RegisteredHook, ScriptHook};
        use std::sync::Arc;

        struct Fixed(&'static str);
        impl ScriptHook for Fixed {
            fn invoke(&self, _args: &[H]) -> HookResult {
                Ok(H::str(self.0))
            }
        }
        let install = |answer: &'static str| {
            lunco_hooks::register(RegisteredHook {
                id: READINESS_HOOK.into(),
                backend: "rust".into(),
                deterministic: false,
                hook: Arc::new(Fixed(answer)),
            });
        };

        let mut app = app();
        let e = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<ReadinessRegistry>().begin(
            Subject::Entity(e),
            kinds::PROGRAM_COMPILE,
            "x",
        );

        // Built-in would freeze the object; policy says carry on.
        install("proceed");
        app.update();
        assert!(!app.world().entity(e).contains::<HeldForReadiness>());

        // Swap the rule with the item still pending — no restart, no re-declare.
        install("hold_world");
        app.update();
        assert!(app.world().resource::<ReadinessState>().world_hold);

        lunco_hooks::unregister(READINESS_HOOK);
    }

    /// A policy that answers with nonsense must not be obeyed, and must not take
    /// the engine down either — the built-in rule stands in.
    #[test]
    fn an_unparseable_policy_answer_falls_back_to_the_builtin() {
        let _guard = policy_lock();
        use lunco_hooks::{HookResult, RegisteredHook, ScriptHook};
        use std::sync::Arc;

        struct Nonsense;
        impl ScriptHook for Nonsense {
            fn invoke(&self, _args: &[H]) -> HookResult {
                Ok(H::str("melt_the_reactor"))
            }
        }
        lunco_hooks::register(RegisteredHook {
            id: READINESS_HOOK.into(),
            backend: "rust".into(),
            deterministic: false,
            hook: Arc::new(Nonsense),
        });

        let mut app = app();
        app.world_mut().resource_mut::<ReadinessRegistry>().begin(
            Subject::World,
            kinds::SCENE_LOAD,
            "s",
        );
        app.update();
        assert!(
            app.world().resource::<ReadinessState>().world_hold,
            "unknown action ⇒ built-in rule, which holds the world for a scene load"
        );

        lunco_hooks::unregister(READINESS_HOOK);
    }

    /// `Subject::World` has no object to freeze, so a `hold_entity` verdict on it
    /// escalates rather than quietly doing nothing.
    #[test]
    fn hold_entity_on_the_world_escalates_to_a_world_hold() {
        let _guard = policy_lock();
        use lunco_hooks::{HookResult, RegisteredHook, ScriptHook};
        use std::sync::Arc;

        struct HoldEntityAlways;
        impl ScriptHook for HoldEntityAlways {
            fn invoke(&self, _args: &[H]) -> HookResult {
                Ok(H::str("hold_entity"))
            }
        }
        lunco_hooks::register(RegisteredHook {
            id: READINESS_HOOK.into(),
            backend: "rust".into(),
            deterministic: false,
            hook: Arc::new(HoldEntityAlways),
        });

        let mut app = app();
        app.world_mut().resource_mut::<ReadinessRegistry>().begin(
            Subject::World,
            kinds::PARTICIPANT_INIT,
            "cosim",
        );
        app.update();
        assert!(app.world().resource::<ReadinessState>().world_hold);

        lunco_hooks::unregister(READINESS_HOOK);
    }
}
