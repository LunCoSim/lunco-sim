//! The clock tree (architecture doc 19 — T5): many clocks as a tree of affine
//! children of the master, plus per-object/selection/project playback.
//!
//! A **`TimeDomain`** is an affine child of a parent clock — `local_t = offset +
//! scale·parent_t` (USD `LayerOffset`). Floating clocks are debt; *rooted* clocks
//! are free — independently controllable yet always convertible back to the
//! master (the world clock, [`WorldTime::sim_secs`](crate::WorldTime)).
//!
//! Two node kinds (doc §3d):
//! * **Derived** — `TimeDomain` alone. `local_t = offset + scale·parent_t`. Rigidly
//!   follows the parent. *"Speed only the factory" = a derived domain, `scale = 100`.*
//! * **Driven** — `TimeDomain` + [`Playback`]. Its own **playhead** that advances by
//!   the world delta when playing, but seek/pause/replay/loop independently. *"Replay
//!   this object" = a driven domain, `head = start`, `mode = Playing`.*
//!
//! Bindings (doc §3d): an entity carries a [`TimeBinding`] to a domain entity;
//! absent ⇒ the world domain. Per-project / per-selection / per-object are just
//! different bound sets of the same machinery.
//!
//! Resolution is split into pure functions ([`derived_local_t`], [`step_playhead`],
//! [`resolve_snapshot`]) so the math is unit-tested headless; the Bevy system
//! [`advance_and_resolve_domains`] snapshots the domain components once per frame,
//! advances driven heads, and fills [`ResolvedDomains`] for the sampler to read.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::{TransportMode, WorldTime};

/// Coupling class of a domain (doc §5). Informational in v1 (the sampler is a pure
/// Tier-1 consumer); a future co-sim layer keys causal domains on communication
/// points rather than free rate-scaling.
#[derive(Reflect, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DomainRegime {
    /// State is a pure function of time — rate-scale freely (baked / `timeSamples`).
    #[default]
    Kinematic,
    /// Integrates — independent rate is bounded by solver stability / co-sim.
    Causal,
}

/// A clock node: an affine child of `parent` (or the world clock when `parent` is
/// `None`). `local_t = offset + scale·parent_t`. A domain entity always carries
/// this; adding [`Playback`] makes it a *driven* domain.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
pub struct TimeDomain {
    /// Parent domain entity; `None` = child of the world clock.
    pub parent: Option<Entity>,
    /// Affine offset over the parent (seconds).
    pub offset: f64,
    /// Affine scale over the parent (rate multiplier; `100` = the factory at 100×).
    pub scale: f64,
    /// Coupling class.
    pub regime: DomainRegime,
}

impl Default for TimeDomain {
    fn default() -> Self {
        Self { parent: None, offset: 0.0, scale: 1.0, regime: DomainRegime::Kinematic }
    }
}

impl TimeDomain {
    /// A derived domain: `local_t = offset + scale·parent_t`.
    pub fn derived(parent: Option<Entity>, offset: f64, scale: f64) -> Self {
        Self { parent, offset, scale, regime: DomainRegime::Kinematic }
    }
}

/// A driven domain's independent playhead. Present ⇒ the domain's resolved time is
/// `head` (authoritative), advanced by the world delta when `Playing`, optionally
/// clamped/looped to `[start, end]`.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
pub struct Playback {
    /// Current local time (the playhead).
    pub head: f64,
    /// Play / pause.
    pub mode: TransportMode,
    /// Playback rate relative to the world delta (1.0 = realtime, 2.0 = double).
    pub rate: f64,
    /// Range start (seconds). If `end <= start` the range is unbounded.
    pub start: f64,
    /// Range end (seconds). If `end <= start` the range is unbounded.
    pub end: f64,
    /// Wrap to `start` at `end` (loop) vs clamp at `end` (one-shot).
    pub looping: bool,
}

impl Default for Playback {
    fn default() -> Self {
        Self { head: 0.0, mode: TransportMode::Playing, rate: 1.0, start: 0.0, end: 0.0, looping: false }
    }
}

impl Playback {
    /// A replay playhead over `[start, end]` at `rate`, starting at `start`.
    pub fn replay(start: f64, end: f64, rate: f64, looping: bool) -> Self {
        Self { head: start, mode: TransportMode::Playing, rate, start, end, looping }
    }

    /// Whether the range `[start, end]` is bounded (clamp/loop applies).
    #[inline]
    pub fn bounded(&self) -> bool {
        self.end > self.start
    }
}

/// Binds an entity to a [`TimeDomain`]. Absent ⇒ the world domain
/// ([`WorldTime::sim_secs`](crate::WorldTime)). "New domain from selection" =
/// spawn a domain entity and add this to each selected entity.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
pub struct TimeBinding {
    /// The domain entity whose resolved local time drives this entity.
    pub domain: Entity,
}

/// Per-frame resolved local time for every domain entity. The animation sampler
/// reads this (via [`domain_time`]) rather than re-resolving the chain itself.
#[derive(Resource, Default, Debug)]
pub struct ResolvedDomains(pub HashMap<Entity, f64>);

impl ResolvedDomains {
    /// Resolved local time for `domain`, or `None` if unknown this frame.
    #[inline]
    pub fn get(&self, domain: Entity) -> Option<f64> {
        self.0.get(&domain).copied()
    }
}

/// Previous frame's `WorldTime.sim_secs`, to derive the world delta that advances
/// driven playheads.
#[derive(Resource, Default)]
struct LastWorldSecs(f64);

/// System set wrapping [`advance_and_resolve_domains`] so cross-crate consumers
/// (the USD sampler in `lunco-usd-bevy`) order their reads `.after` it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DomainResolveSet;

// --- pure resolution math (headless-testable) --------------------------------

/// Derived-domain affine map: `offset + scale·parent_t`.
#[inline]
pub fn derived_local_t(offset: f64, scale: f64, parent_t: f64) -> f64 {
    offset + scale * parent_t
}

/// Advance a driven playhead by `world_delta` and apply the range policy. Pure —
/// returns the new head, does not mutate.
pub fn step_playhead(pb: &Playback, world_delta: f64) -> f64 {
    if !matches!(pb.mode, TransportMode::Playing) {
        return pb.head;
    }
    let mut h = pb.head + world_delta * pb.rate;
    if pb.bounded() {
        let span = pb.end - pb.start;
        if pb.looping {
            // Wrap into [start, end) — rem_euclid handles negative rates too.
            h = pb.start + (h - pb.start).rem_euclid(span);
        } else {
            h = h.clamp(pb.start, pb.end);
        }
    }
    h
}

/// One domain's component data, snapshotted for pure resolution.
#[derive(Debug, Clone, Copy)]
pub struct DomainSnapshot {
    /// Parent domain entity (`None` = world clock).
    pub parent: Option<Entity>,
    /// Affine offset over the parent.
    pub offset: f64,
    /// Affine scale over the parent.
    pub scale: f64,
    /// Driven head (already advanced this frame), or `None` for a derived domain.
    pub head: Option<f64>,
}

/// Resolve one domain's local time from a snapshot map. Driven domains return
/// their head; derived domains compose `offset + scale·parent_t` up the chain to
/// the world clock (`world_secs`). Depth-capped against cycles / missing parents.
pub fn resolve_snapshot(
    snap: &HashMap<Entity, DomainSnapshot>,
    domain: Entity,
    world_secs: f64,
    depth: u32,
) -> f64 {
    if depth > 16 {
        return world_secs;
    }
    let Some(s) = snap.get(&domain) else {
        return world_secs;
    };
    if let Some(head) = s.head {
        return head; // driven: head is authoritative
    }
    let parent_t = match s.parent {
        Some(p) => resolve_snapshot(snap, p, world_secs, depth + 1),
        None => world_secs,
    };
    derived_local_t(s.offset, s.scale, parent_t)
}

/// Resolve `binding`'s domain time from the per-frame [`ResolvedDomains`], falling
/// back to the world clock when unbound or unresolved. This is the one entry point
/// the sampler uses to turn an entity into its `local_t`.
#[inline]
pub fn domain_time(
    resolved: &ResolvedDomains,
    binding: Option<&TimeBinding>,
    world: &WorldTime,
) -> f64 {
    match binding {
        Some(b) => resolved.get(b.domain).unwrap_or(world.sim_secs),
        None => world.sim_secs,
    }
}

// --- the Bevy system ---------------------------------------------------------

/// Advance driven playheads by the world delta, then resolve every domain's local
/// time into [`ResolvedDomains`]. One query, iterated once (snapshot), then pure
/// resolution — so there is no mutable/immutable `Playback` aliasing.
pub fn advance_and_resolve_domains(
    world: Res<WorldTime>,
    mut last: ResMut<LastWorldSecs>,
    mut q: Query<(Entity, &TimeDomain, Option<&mut Playback>)>,
    mut resolved: ResMut<ResolvedDomains>,
) {
    let world_delta = world.sim_secs - last.0;
    last.0 = world.sim_secs;

    // Pass 1: advance driven heads + snapshot all domain components.
    let mut snap: HashMap<Entity, DomainSnapshot> = HashMap::new();
    for (e, d, pb) in &mut q {
        let head = pb.map(|mut pb| {
            pb.head = step_playhead(&pb, world_delta);
            pb.head
        });
        snap.insert(e, DomainSnapshot { parent: d.parent, offset: d.offset, scale: d.scale, head });
    }

    // Pass 2: resolve each domain's local time (pure over the snapshot).
    resolved.0.clear();
    for &e in snap.keys() {
        let t = resolve_snapshot(&snap, e, world.sim_secs, 0);
        resolved.0.insert(e, t);
    }
}

/// Spawn a **derived** domain entity (`local_t = offset + scale·parent_t`).
pub fn spawn_derived_domain(
    commands: &mut Commands,
    parent: Option<Entity>,
    offset: f64,
    scale: f64,
) -> Entity {
    commands
        .spawn((TimeDomain::derived(parent, offset, scale), Name::new("DerivedTimeDomain")))
        .id()
}

/// Spawn a **driven** domain entity (own playhead). `parent` feeds the affine
/// chain for any *derived* children; the driven head itself advances on the world
/// delta (v1).
pub fn spawn_driven_domain(commands: &mut Commands, parent: Option<Entity>, playback: Playback) -> Entity {
    commands
        .spawn((
            TimeDomain::derived(parent, 0.0, 1.0),
            playback,
            Name::new("DrivenTimeDomain"),
        ))
        .id()
}

/// Plugin wiring for the clock tree: components, [`ResolvedDomains`], and the
/// resolve system in [`DomainResolveSet`] (`Update`). Added by [`TimePlugin`](crate::TimePlugin).
pub(crate) fn build_domain_tree(app: &mut App) {
    app.register_type::<TimeDomain>()
        .register_type::<Playback>()
        .register_type::<TimeBinding>()
        .register_type::<DomainRegime>()
        .init_resource::<ResolvedDomains>()
        .init_resource::<LastWorldSecs>()
        .add_systems(Update, advance_and_resolve_domains.in_set(DomainResolveSet));
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f64 = 1e-9;

    /// Test-only entity handle (`Entity::from_raw_u32` is fallible in this Bevy).
    fn e(n: u32) -> Entity {
        Entity::from_raw_u32(n).unwrap()
    }

    fn snap(parent: Option<Entity>, offset: f64, scale: f64, head: Option<f64>) -> DomainSnapshot {
        DomainSnapshot { parent, offset, scale, head }
    }

    #[test]
    fn derived_domain_scales_the_world_clock() {
        // factory at 100×: local = 100·world.
        assert!((derived_local_t(0.0, 100.0, 3.0) - 300.0).abs() < EPS);
        // with an offset.
        assert!((derived_local_t(5.0, 2.0, 10.0) - 25.0).abs() < EPS);
    }

    #[test]
    fn nested_derived_domains_compose_scales() {
        let world = e(1);
        let outer = e(2); // scale 2 of world
        let inner = e(3); // scale 3 of outer → 6× world
        let _ = world;
        let mut m = HashMap::new();
        m.insert(outer, snap(None, 0.0, 2.0, None));
        m.insert(inner, snap(Some(outer), 0.0, 3.0, None));
        // world_secs = 10 → outer 20 → inner 60.
        assert!((resolve_snapshot(&m, inner, 10.0, 0) - 60.0).abs() < EPS);
        assert!((resolve_snapshot(&m, outer, 10.0, 0) - 20.0).abs() < EPS);
    }

    #[test]
    fn driven_domain_returns_its_head_not_the_chain() {
        let d = e(7);
        let mut m = HashMap::new();
        m.insert(d, snap(None, 0.0, 1.0, Some(42.0)));
        // head is authoritative regardless of world_secs.
        assert!((resolve_snapshot(&m, d, 1000.0, 0) - 42.0).abs() < EPS);
    }

    #[test]
    fn unknown_or_cyclic_domain_falls_back_to_world() {
        let a = e(8);
        let b = e(9);
        let mut m = HashMap::new();
        // a → b → a cycle: depth cap returns world_secs.
        m.insert(a, snap(Some(b), 0.0, 1.0, None));
        m.insert(b, snap(Some(a), 0.0, 1.0, None));
        assert!((resolve_snapshot(&m, a, 5.0, 0) - 5.0).abs() < 1e-6);
        // Missing domain → world_secs.
        assert!((resolve_snapshot(&m, e(99), 5.0, 0) - 5.0).abs() < EPS);
    }

    #[test]
    fn playhead_advances_by_world_delta_times_rate() {
        let pb = Playback::replay(0.0, 0.0, 2.0, false); // unbounded, 2×
        assert!((step_playhead(&pb, 1.0) - 2.0).abs() < EPS);
    }

    #[test]
    fn paused_playhead_holds() {
        let mut pb = Playback::replay(0.0, 0.0, 2.0, false);
        pb.mode = TransportMode::Paused;
        pb.head = 5.0;
        assert!((step_playhead(&pb, 10.0) - 5.0).abs() < EPS);
    }

    #[test]
    fn looping_playhead_wraps_into_range() {
        let mut pb = Playback::replay(0.0, 10.0, 1.0, true);
        pb.head = 9.0;
        // 9 + 3 = 12 → wraps to 2.
        assert!((step_playhead(&pb, 3.0) - 2.0).abs() < EPS);
    }

    #[test]
    fn one_shot_playhead_clamps_at_end() {
        let mut pb = Playback::replay(0.0, 10.0, 1.0, false);
        pb.head = 9.0;
        assert!((step_playhead(&pb, 5.0) - 10.0).abs() < EPS);
    }

    /// End-to-end of the real Bevy system: a derived domain scales the world
    /// clock; a driven domain's head advances by the world delta — both land in
    /// `ResolvedDomains` for the sampler.
    #[test]
    fn resolve_system_populates_resolved_domains() {
        let mut app = App::new();
        app.init_resource::<WorldTime>()
            .init_resource::<ResolvedDomains>()
            .init_resource::<LastWorldSecs>()
            .add_systems(Update, advance_and_resolve_domains);

        app.world_mut().resource_mut::<WorldTime>().sim_secs = 10.0;
        let derived = app.world_mut().spawn(TimeDomain::derived(None, 0.0, 2.0)).id();
        let driven = app
            .world_mut()
            .spawn((TimeDomain::default(), Playback::replay(0.0, 0.0, 1.0, false)))
            .id();

        app.update();

        let resolved = app.world().resource::<ResolvedDomains>();
        // Derived: 2·world = 20.
        assert_eq!(resolved.get(derived), Some(20.0));
        // Driven: head advanced by world_delta (10−0)·rate = 10.
        assert_eq!(resolved.get(driven), Some(10.0));
    }
}
