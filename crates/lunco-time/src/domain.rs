//! The clock tree (architecture doc 19 — T5 / §11): many clocks as a tree of affine
//! children of a few roots, plus per-object/selection/project playback.
//!
//! A **`TimeDomain`** is an affine child of a parent clock — `local_t = offset +
//! scale·parent_t` (USD `LayerOffset`). Floating clocks are debt; *rooted* clocks are
//! free — independently controllable yet always convertible back to a root.
//!
//! Node kinds:
//! * **Root** ([`ClockRoot`]) — where raw time ENTERS the tree: `Tick` (the
//!   deterministic `SimTick` master — freezes on pause), `Wall` (`Time<Real>` — never
//!   freezes).
//! * **Derived** — `TimeDomain` alone. `local_t = offset + scale·parent_t`. Rigidly
//!   follows the parent. *"Speed only the factory" = a derived clock, `scale = 100`.*
//! * **Driven** — `TimeDomain` + [`Playback`]. Its own **playhead**, advanced by the
//!   *parent's* delta when playing, but seek/pause/replay/loop independently. *"Replay
//!   this object" = a driven clock, `head = start`, `mode = Playing`.*
//!
//! **Pause propagates for free, with no flag** (doc §11a): if a parent stops advancing,
//! `parent_t` is constant, so the whole subtree is constant. That is also why "run the
//! Scene presentation samples the physical tick with fixed-step interpolation and
//! has no independent clock or rate.
//!
//! Bindings: an entity carries a [`TimeBinding`] to a clock entity; absent ⇒ the sim
//! clock. Per-project / per-selection / per-object are just different bound sets of the
//! same machinery.
//!
//! Resolution is split into pure functions ([`derived_local_t`], [`step_playhead`],
//! [`resolve_clocks`]) so the math is unit-tested headless; the Bevy system
//! [`advance_and_resolve_domains`] snapshots the clock components once per frame,
//! resolves the tree, and fills [`ResolvedDomains`] with a `{ t, dt }` sample per clock.

use std::collections::HashMap;

use bevy::prelude::*;

use lunco_core::{on_command, register_commands, Command};

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
    /// Affine scale over the parent (rate multiplier; `100` means 100×).
    pub scale: f64,
    /// Coupling class.
    pub regime: DomainRegime,
}

impl Default for TimeDomain {
    fn default() -> Self {
        Self {
            parent: None,
            offset: 0.0,
            scale: 1.0,
            regime: DomainRegime::Kinematic,
        }
    }
}

impl TimeDomain {
    /// A derived domain: `local_t = offset + scale·parent_t`.
    pub fn derived(parent: Option<Entity>, offset: f64, scale: f64) -> Self {
        Self {
            parent,
            offset,
            scale,
            regime: DomainRegime::Kinematic,
        }
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
        Self {
            head: 0.0,
            mode: TransportMode::Playing,
            rate: 1.0,
            start: 0.0,
            end: 0.0,
            looping: false,
        }
    }
}

impl Playback {
    /// A replay playhead over `[start, end]` at `rate`, starting at `start`.
    pub fn replay(start: f64, end: f64, rate: f64, looping: bool) -> Self {
        Self {
            head: start,
            mode: TransportMode::Playing,
            rate,
            start,
            end,
            looping,
        }
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

/// What a **root** clock reads its time from. Root nodes are the only place raw
/// time enters the tree; every other node is an affine child of one of them.
///
/// The two roots are not interchangeable, and the difference is the whole point:
///
/// * [`Tick`](ClockRoot::Tick) — the deterministic master. `t = WorldTime.sim_secs`,
///   derived from the integer [`SimTick`](lunco_core_runtime::SimTick) and gated by
///   [`TimeTransport`](crate::TimeTransport). Replicated, seekable, replayable.
///   **Freezes on pause** — and so does everything hanging under it.
/// * [`Wall`](ClockRoot::Wall) — `t = Time<Real>`. Free-running, never pauses,
///   non-deterministic by construction. It is for interaction such as camera and
///   UI easing. Scene state remains on the tick root.
#[derive(Component, Reflect, Debug, Clone, Copy, PartialEq, Eq)]
#[reflect(Component)]
pub enum ClockRoot {
    /// Deterministic tick master (`WorldTime.sim_secs`). Freezes on pause.
    Tick,
    /// Wall clock (`Time<Real>`). Never freezes.
    Wall,
}

/// One clock's resolved sample for this frame.
///
/// `dt` is what makes a clock *usable by a movement system*: without it, anything
/// that integrates (avatar fly, camera smoothing) has to reach past the clock tree
/// for a raw `Time<Real>`/`Time<Virtual>` delta — which is exactly why `lunco-avatar`
/// bypassed the domains entirely before this existed.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ClockSample {
    /// Resolved local time (seconds).
    pub t: f64,
    /// Local time advanced this frame (`t - t_prev`). Zero on a frozen clock, and
    /// on the first frame a clock is seen.
    pub dt: f64,
}

/// The well-known clocks, spawned once at startup. Handles, not state — the state
/// lives on the entities as [`TimeDomain`] / [`Playback`] / [`ClockRoot`].
///
/// The standing clock roots (doc 19 §11b):
///
/// ```text
///   real ── ClockRoot::Wall                sim ── ClockRoot::Tick
///    └── interaction                        └── <animation / per-object domains>
/// ```
#[derive(Resource, Debug, Clone, Copy)]
pub struct Clocks {
    /// Wall root. Never pauses.
    pub real: Entity,
    /// Tick root — the deterministic sim master. Pausing this freezes its subtree.
    pub sim: Entity,
    /// Wall-rooted interaction clock: avatar, camera smoothing, UI easing. Keeps
    /// running while the sim is paused (that is its entire reason to exist).
    pub interaction: Entity,
}

/// A [`SystemParam`](bevy::ecs::system::SystemParam) for "seconds of *interaction*
/// time that passed this frame".
///
/// This is the clock the avatar, the cameras and UI easing run on: wall-rooted, so
/// it **keeps advancing while the simulation is paused**. Systems used to reach for
/// a raw `Res<Time<Real>>` to get that behaviour, which worked but put the camera
/// outside the clock system entirely — it could not be inspected, rate-scaled, or
/// slowed for a cinematic. Read it through here instead.
#[derive(bevy::ecs::system::SystemParam)]
pub struct InteractionTime<'w> {
    clocks: Res<'w, Clocks>,
    resolved: Res<'w, ResolvedDomains>,
}

impl InteractionTime<'_> {
    /// Seconds the interaction clock advanced this frame.
    #[inline]
    pub fn delta_secs(&self) -> f32 {
        self.delta_secs_f64() as f32
    }

    /// Seconds the interaction clock advanced this frame, double precision (the
    /// avatar integrates its position in `f64` — big_space grid coordinates).
    #[inline]
    pub fn delta_secs_f64(&self) -> f64 {
        self.resolved.delta(self.clocks.interaction)
    }

    /// The interaction clock's local time, seconds.
    #[inline]
    pub fn elapsed_secs(&self) -> f64 {
        self.resolved.get(self.clocks.interaction).unwrap_or(0.0)
    }
}

/// Per-frame resolved sample for every clock entity. The animation sampler reads
/// this (via [`domain_time`]) rather than re-resolving the chain itself.
#[derive(Resource, Default, Debug)]
pub struct ResolvedDomains(pub HashMap<Entity, ClockSample>);

impl ResolvedDomains {
    /// Resolved local time for `domain`, or `None` if unknown this frame.
    #[inline]
    pub fn get(&self, domain: Entity) -> Option<f64> {
        self.0.get(&domain).map(|s| s.t)
    }

    /// Full sample (`t` **and** `dt`) for `domain`, or `None` if unknown this frame.
    #[inline]
    pub fn sample(&self, domain: Entity) -> Option<ClockSample> {
        self.0.get(&domain).copied()
    }

    /// Local seconds advanced by `domain` this frame — `0.0` if the clock is frozen
    /// or unknown. The entry point for any system that integrates on a clock.
    #[inline]
    pub fn delta(&self, domain: Entity) -> f64 {
        self.0.get(&domain).map(|s| s.dt).unwrap_or(0.0)
    }
}

/// Previous frame's resolved `t` per clock — the source of each clock's `dt`, and
/// of the *parent* delta that advances a driven playhead.
/// Previous frame's resolved times. Public because it appears in the (public)
/// [`advance_and_resolve_domains`] system signature; it is spine bookkeeping, not an
/// API — read [`ResolvedDomains`] instead.
#[derive(Resource, Default)]
pub struct LastClockT {
    /// Previous frame's resolved `t` per clock.
    per_clock: HashMap<Entity, f64>,
    /// Previous frame's root times — the parent delta for a clock with no parent
    /// entity, and the reason an unrooted driven head still advances.
    roots: RootTimes,
}

/// System set wrapping [`advance_and_resolve_domains`] so cross-crate consumers
/// (the USD sampler in `lunco-usd-bevy-animation`) order their reads `.after` it.
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

/// One clock's component data, snapshotted for pure resolution.
#[derive(Debug, Clone, Copy)]
pub struct DomainSnapshot {
    /// Parent clock entity (`None` = the `sim` root).
    pub parent: Option<Entity>,
    /// Affine offset over the parent.
    pub offset: f64,
    /// Affine scale over the parent.
    pub scale: f64,
    /// Playback, if this is a *driven* clock (own seekable head).
    pub playback: Option<Playback>,
    /// Root source, if this is a *root* clock. Roots ignore `parent`.
    pub root: Option<ClockRoot>,
}

/// The raw time entering the tree at the roots this frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct RootTimes {
    /// `WorldTime.sim_secs` — the deterministic tick master. Frozen on pause.
    pub sim_secs: f64,
    /// `Time<Real>` elapsed seconds. Never frozen.
    pub wall_secs: f64,
}

/// Resolve every clock in one memoized walk, stepping driven playheads by their
/// **parent's** delta as it goes.
///
/// Three node kinds, in precedence order:
/// 1. **Root** — `t = offset + scale · (sim_secs | wall_secs)`. The only entry point
///    for raw time.
/// 2. **Driven** (has [`Playback`]) — `t = head`, where the head advanced by the
///    *parent's* delta this frame. A frozen parent hands it `dt = 0`, so the head
///    holds: pause propagates into driven clocks too, without a flag.
/// 3. **Derived** — `t = offset + scale · parent_t`. A frozen parent freezes it
///    structurally, because `parent_t` simply stops changing.
///
/// `last` is the previous frame's resolved `t` per clock, which is what makes the
/// parent delta available without a second pass. Depth-capped against cycles;
/// unknown/missing parents fall back to the sim root.
pub fn resolve_clocks(
    snap: &HashMap<Entity, DomainSnapshot>,
    last: &HashMap<Entity, f64>,
    roots: RootTimes,
    last_roots: RootTimes,
    sim_root: Option<Entity>,
) -> HashMap<Entity, ClockSample> {
    let mut out: HashMap<Entity, ClockSample> = HashMap::new();
    for &e in snap.keys() {
        resolve_one(snap, last, roots, last_roots, sim_root, e, 0, &mut out);
    }
    out
}

/// The memoized recursion behind [`resolve_clocks`]. Returns the clock's `t`.
#[allow(clippy::too_many_arguments)]
fn resolve_one(
    snap: &HashMap<Entity, DomainSnapshot>,
    last: &HashMap<Entity, f64>,
    roots: RootTimes,
    last_roots: RootTimes,
    sim_root: Option<Entity>,
    domain: Entity,
    depth: u32,
    out: &mut HashMap<Entity, ClockSample>,
) -> f64 {
    if let Some(s) = out.get(&domain) {
        return s.t;
    }
    // Cycle / depth guard: fall back to the deterministic master rather than spin.
    if depth > 16 {
        return roots.sim_secs;
    }
    let Some(s) = snap.get(&domain) else {
        return roots.sim_secs;
    };

    let t = if let Some(root) = s.root {
        let src = match root {
            ClockRoot::Tick => roots.sim_secs,
            ClockRoot::Wall => roots.wall_secs,
        };
        derived_local_t(s.offset, s.scale, src)
    } else {
        // Parent defaults to the sim root.
        let parent = s.parent.or(sim_root);
        let parent_t = match parent {
            Some(p) => resolve_one(snap, last, roots, last_roots, sim_root, p, depth + 1, out),
            None => roots.sim_secs,
        };
        match s.playback {
            Some(pb) => {
                // Driven: advance the head by the PARENT's delta. A frozen parent
                // yields 0 → the head holds. This is how a pause reaches a driven
                // clock without anyone propagating a flag.
                //
                // With no parent ENTITY to look up (an unrooted domain), fall back to
                // the previous frame's sim root time — otherwise the delta would be a
                // constant 0 and the head would never advance at all.
                let parent_last = match parent {
                    Some(p) => last.get(&p).copied().unwrap_or(parent_t),
                    None => last_roots.sim_secs,
                };
                step_playhead(&pb, parent_t - parent_last)
            }
            None => derived_local_t(s.offset, s.scale, parent_t),
        }
    };

    let dt = t - last.get(&domain).copied().unwrap_or(t);
    out.insert(domain, ClockSample { t, dt });
    t
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

/// Resolve the whole clock tree once per frame into [`ResolvedDomains`].
///
/// Snapshot → pure resolve → write back. The snapshot is what lets the pure
/// resolver step driven heads mid-walk without a mutable/immutable `Playback`
/// aliasing conflict.
///
/// Runs in `PreUpdate` from the `WorldTime` published after the preceding fixed
/// loop. Nothing downstream recomputes a clock; it only reads the resolved
/// sample.
pub fn advance_and_resolve_domains(
    world: Res<WorldTime>,
    real: Res<Time<bevy::time::Real>>,
    clocks: Option<Res<Clocks>>,
    mut last: ResMut<LastClockT>,
    mut q: Query<(
        Entity,
        &TimeDomain,
        Option<&mut Playback>,
        Option<&ClockRoot>,
    )>,
    mut resolved: ResMut<ResolvedDomains>,
) {
    let roots = RootTimes {
        sim_secs: world.sim_secs,
        wall_secs: real.elapsed_secs_f64(),
    };
    let sim_root = clocks.map(|c| c.sim);

    let mut snap: HashMap<Entity, DomainSnapshot> = HashMap::new();
    for (e, d, pb, root) in &q {
        snap.insert(
            e,
            DomainSnapshot {
                parent: d.parent,
                offset: d.offset,
                scale: d.scale,
                playback: pb.copied(),
                root: root.copied(),
            },
        );
    }

    let samples = resolve_clocks(&snap, &last.per_clock, roots, last.roots, sim_root);

    // Write the advanced heads back onto the driven clocks — the `Playback` component
    // stays the authority for the head, so a seek/pause lands on it directly.
    for (e, _, pb, _) in &mut q {
        if let Some(mut pb) = pb {
            if let Some(s) = samples.get(&e) {
                if pb.head != s.t {
                    pb.head = s.t;
                }
            }
        }
    }

    last.per_clock.clear();
    last.per_clock
        .extend(samples.iter().map(|(&e, s)| (e, s.t)));
    last.roots = roots;
    resolved.0 = samples;
}

/// Startup: spawn the well-known roots and publish [`Clocks`].
fn spawn_well_known_clocks(mut commands: Commands) {
    let real = commands
        .spawn((
            Name::new("Clock:Real"),
            TimeDomain::default(),
            ClockRoot::Wall,
        ))
        .id();
    let sim = commands
        .spawn((
            Name::new("Clock:Sim"),
            TimeDomain::default(),
            ClockRoot::Tick,
        ))
        .id();
    // Wall-rooted: the avatar and the camera keep moving while the sim is paused.
    let interaction = commands
        .spawn((
            Name::new("Clock:Interaction"),
            TimeDomain::derived(Some(real), 0.0, 1.0),
        ))
        .id();
    commands.insert_resource(Clocks {
        real,
        sim,
        interaction,
    });
}

/// Spawn a **derived** domain entity (`local_t = offset + scale·parent_t`).
pub fn spawn_derived_domain(
    commands: &mut Commands,
    parent: Option<Entity>,
    offset: f64,
    scale: f64,
) -> Entity {
    commands
        .spawn((
            TimeDomain::derived(parent, offset, scale),
            Name::new("DerivedTimeDomain"),
        ))
        .id()
}

/// Spawn a **driven** domain entity (own playhead). `parent` feeds the affine
/// chain for any *derived* children; the driven head itself advances on the world
/// delta (v1).
pub fn spawn_driven_domain(
    commands: &mut Commands,
    parent: Option<Entity>,
    playback: Playback,
) -> Entity {
    commands
        .spawn((
            TimeDomain::derived(parent, 0.0, 1.0),
            playback,
            Name::new("DrivenTimeDomain"),
        ))
        .id()
}

// --- animation preview transport (doc 19 — T7) -------------------------------

/// The singleton **animation preview** domain for entities explicitly bound
/// by an editor or cinematic workflow. Scene-authored animation without a
/// [`TimeBinding`] follows [`crate::SimulationPresentationTime`]. This domain's
/// [`Playback`] head can be paused, seeked, or rate-scaled without touching the
/// physics clock, which is gated by [`TimeTransport`](crate::TimeTransport).
/// The [`ControlAnimation`] command and Inspector transport widget drive it.
#[derive(Resource, Debug, Clone, Copy)]
pub struct AnimationPreview {
    /// The driven domain entity (carries the [`Playback`] head).
    pub domain: Entity,
}

/// Startup: spawn the singleton [`AnimationPreview`] domain (rate 1, playing,
/// unbounded). Idempotent-by-construction — guarded `TimePlugin` adds run once.
fn spawn_animation_preview(mut commands: Commands) {
    let domain = commands
        .spawn((
            Name::new("AnimationPreview"),
            TimeDomain::default(),
            Playback::default(),
        ))
        .id();
    commands.insert_resource(AnimationPreview { domain });
}

/// Drive the [`AnimationPreview`] transport. Each field is optional so one verb
/// covers run / pause / scroll(seek) / rate / loop — `{"type":"ExecuteCommand","command":"ControlAnimation",
/// "params":{"playing":false}}` pauses, `{"seek_secs":3.0}` scrubs to 3 s,
/// `{"rate":2.0}` doubles speed, `{"looping":true}` loops. Headless-safe: it only
/// writes the preview domain's [`Playback`], never any UI or render resource.
///
/// Fields are orthogonal, so a **restart** is one trigger:
/// `{"playing":true,"seek_secs":0.0}` — seek to the range start and run. Seek to
/// [`Playback::start`] rather than a literal `0.0`: [`step_playhead`] clamps to
/// `[start, end]`, so on a clip whose range starts late a hardcoded 0 lands
/// outside the range and snaps forward on the next step.
#[Command(default)]
pub struct ControlAnimation {
    /// Which driven domain to control. `None` = the shared [`AnimationPreview`].
    ///
    /// A per-object driven clock (a camera path's, say) is otherwise unreachable:
    /// it owns its own [`Playback`], so the preview transport does not touch it and
    /// the shot cannot be paused, scrubbed or replayed at all. Point this at the
    /// domain entity to drive it with the same one verb.
    pub target: Option<Entity>,
    /// Play (`Some(true)`) / pause (`Some(false)`) the animation; `None` leaves it.
    pub playing: Option<bool>,
    /// Seek the playhead to this time in **seconds**; `None` leaves it.
    pub seek_secs: Option<f64>,
    /// Playback rate (1.0 = realtime); `None` leaves it.
    pub rate: Option<f64>,
    /// Wrap at the range end instead of clamping (`None` leaves it). Honoured by
    /// [`step_playhead`], and only meaningful once the range is bounded — an
/// unbounded `Playback` ignores it, so a looping camera path needs an authored
/// clip span from its camera-track binding.
    pub looping: Option<bool>,
}

#[on_command(ControlAnimation)]
fn on_control_animation(
    trigger: On<ControlAnimation>,
    preview: Option<Res<AnimationPreview>>,
    mut q: Query<&mut Playback>,
) {
    let cmd = trigger.event();
    // An explicit target drives that per-object domain; otherwise the shared
    // preview. Same verb either way.
    let Some(domain) = cmd.target.or_else(|| preview.map(|p| p.domain)) else {
        return;
    };
    let Ok(mut pb) = q.get_mut(domain) else {
        return;
    };
    apply_control_animation(&mut pb, cmd);
}

/// Pure transport edit — apply a [`ControlAnimation`] to a [`Playback`]. Split
/// out so the verb is unit-tested headless without an observer / world.
pub fn apply_control_animation(pb: &mut Playback, cmd: &ControlAnimation) {
    if let Some(playing) = cmd.playing {
        pb.mode = if playing {
            TransportMode::Playing
        } else {
            TransportMode::Paused
        };
    }
    if let Some(secs) = cmd.seek_secs {
        pb.head = secs;
    }
    if let Some(rate) = cmd.rate {
        pb.rate = rate;
    }
    if let Some(looping) = cmd.looping {
        pb.looping = looping;
    }
}

/// Select how the host advances the application loop, independently of the
/// live simulation rate controlled by [`SetTimeTransport`]. `Realtime` keeps
/// the normal wall-clock cadence; `MaxSpeed` lets headless, recording, and test
/// hosts run the fixed lattice without an intentional wall-clock wait.
#[Command(default)]
pub struct SetSimulationExecutionMode {
    /// Host execution policy.
    pub mode: lunco_core_runtime::SimulationExecutionMode,
}

#[on_command(SetSimulationExecutionMode)]
fn on_set_simulation_execution_mode(
    trigger: On<SetSimulationExecutionMode>,
    mut mode: ResMut<lunco_core_runtime::SimulationExecutionMode>,
) {
    let requested = trigger.event().mode;
    if *mode != requested {
        bevy::log::info!(
            "[pacing] execution mode changed: {:?} -> {:?}",
            *mode,
            requested
        );
        *mode = requested;
    }
}

/// Drive the LIVE-WORLD transport (physics/tick clock), distinct from
/// [`ControlAnimation`] which drives the keyframe preview. Each field optional so
/// one verb covers pause / play / rate — `{"type":"ExecuteCommand","command":"SetTimeTransport",
/// "params":{"playing":false}}` PAUSES the whole simulation (tick + physics),
/// `{"rate":4.0}` runs it 4× realtime, and the bounded causal ladder ends at
/// 64×. Rates below 0.1× or above 64× are rejected. Celestial presentation uses
/// this same physical tick and cannot be detached or independently rate-scaled.
/// This is THE pause command:
/// exposed on the API/MCP and wrapped by the rhai prelude verbs
/// `pause()`/`play()`/`set_rate()`, so a cutscene or a "reload-then-pause"
/// one-liner can freeze the world.
#[Command(default)]
pub struct SetTimeTransport {
    /// Play (`Some(true)`) / pause (`Some(false)`); `None` leaves it.
    #[serde(default)]
    #[reflect(default)]
    pub playing: Option<bool>,
    /// Speed multiplier vs realtime (1.0 = realtime, bounded to 0.1–64.0 for
    /// the causal live transport); `None` leaves it.
    #[serde(default)]
    #[reflect(default)]
    pub rate: Option<f64>,
}

#[on_command(SetTimeTransport)]
fn on_set_time_transport(
    trigger: On<SetTimeTransport>,
    mut transport: ResMut<crate::TimeTransport>,
    virtual_time: Option<ResMut<Time<Virtual>>>,
    mut fixed_time: Option<ResMut<Time<Fixed>>>,
    mut pending_scene_pause: Option<ResMut<crate::PendingScenePause>>,
    coordinator: Option<Res<lunco_core::SceneTransitionCoordinator>>,
    scene_time: Option<Res<crate::SceneTimeState>>,
) {
    let before = *transport;
    let command = trigger.event();
    apply_time_transport(&mut transport, command);
    if command.playing == Some(false)
        && coordinator.is_some_and(|state| state.active().is_some() || state.has_admitted())
    {
        if let Some(pending_scene_pause) = pending_scene_pause.as_deref_mut() {
            pending_scene_pause.0 = true;
        }
    }
    if before.mode != transport.mode || before.rate.to_bits() != transport.rate.to_bits() {
        if let Some(mut virtual_time) = virtual_time {
            if scene_time.is_some_and(|state| !state.is_ready()) {
                hold_virtual_time(&mut virtual_time, fixed_time.as_deref_mut());
            } else {
                crate::project_transport_state(
                    &transport,
                    &mut virtual_time,
                    fixed_time.as_deref_mut(),
                );
            }
        }
    }
    if before.mode != transport.mode || before.rate.to_bits() != transport.rate.to_bits() {
        info!(
            "[time] transport changed: mode {:?} -> {:?}, rate {:.3} -> {:.3}",
            before.mode, transport.mode, before.rate, transport.rate
        );
    }
}

fn hold_virtual_time(virtual_time: &mut Time<Virtual>, fixed_time: Option<&mut Time<Fixed>>) {
    if !virtual_time.is_paused() {
        virtual_time.pause();
    }
    if let Some(fixed_time) = fixed_time {
        crate::discard_fixed_overstep(fixed_time);
    }
}

pub(crate) fn on_scene_transition_started(
    _trigger: On<lunco_core::SceneTransitionStarted>,
    mut scene_time: Option<ResMut<crate::SceneTimeState>>,
    virtual_time: Option<ResMut<Time<Virtual>>>,
    mut fixed_time: Option<ResMut<Time<Fixed>>>,
) {
    let Some(state) = scene_time.as_deref_mut() else {
        return;
    };
    state.begin_scene_load();
    if let Some(mut virtual_time) = virtual_time {
        hold_virtual_time(&mut virtual_time, fixed_time.as_deref_mut());
    }
}

pub(crate) fn on_scene_transition_failed(
    _trigger: On<lunco_core::SceneTransitionFailed>,
    mut scene_time: Option<ResMut<crate::SceneTimeState>>,
    mut transport: Option<ResMut<crate::TimeTransport>>,
    mut pending_scene_pause: Option<ResMut<crate::PendingScenePause>>,
    virtual_time: Option<ResMut<Time<Virtual>>>,
    mut fixed_time: Option<ResMut<Time<Fixed>>>,
) {
    let Some(state) = scene_time.as_deref_mut() else {
        return;
    };
    state.clear_scene();
    if let Some(transport) = transport.as_deref_mut() {
        transport.mode = crate::TransportMode::Paused;
        if let Some(mut virtual_time) = virtual_time {
            crate::project_transport_state(
                &transport,
                &mut virtual_time,
                fixed_time.as_deref_mut(),
            );
        }
    }
    if let Some(pending_scene_pause) = pending_scene_pause.as_deref_mut() {
        pending_scene_pause.0 = false;
    }
}

pub(crate) fn on_scene_transition_completed(
    trigger: On<lunco_core::SceneTransitionCompleted>,
    scene_time: Option<ResMut<crate::SceneTimeState>>,
    mut transport: Option<ResMut<crate::TimeTransport>>,
    mut pending_scene_pause: Option<ResMut<crate::PendingScenePause>>,
    virtual_time: Option<ResMut<Time<Virtual>>>,
    mut fixed_time: Option<ResMut<Time<Fixed>>>,
) {
    if trigger.event().transition != lunco_core::SceneTransition::Clear {
        return;
    }
    if let Some(mut state) = scene_time {
        state.clear_scene();
    }
    if let Some(transport) = transport.as_deref_mut() {
        transport.mode = crate::TransportMode::Paused;
        if let Some(mut virtual_time) = virtual_time {
            crate::project_transport_state(
                &transport,
                &mut virtual_time,
                fixed_time.as_deref_mut(),
            );
        }
    }
    if let Some(pending_scene_pause) = pending_scene_pause.as_deref_mut() {
        pending_scene_pause.0 = false;
    }
}

fn apply_time_transport(transport: &mut crate::TimeTransport, cmd: &SetTimeTransport) {
    if let Some(playing) = cmd.playing {
        transport.mode = if playing {
            crate::TransportMode::Playing
        } else {
            crate::TransportMode::Paused
        };
    }
    if let Some(rate) = cmd.rate {
        if rate.is_finite() && (crate::MIN_REALTIME_RATE..=crate::MAX_REALTIME_RATE).contains(&rate)
        {
            transport.rate = rate;
        } else {
            bevy::log::warn!(
                "[time] rejected live transport rate {rate:?}; supported range is {}..={}x",
                crate::MIN_REALTIME_RATE,
                crate::MAX_REALTIME_RATE
            );
        }
    }
}

/// Re-anchor the world clock at an absolute epoch (Julian Date, TDB) —
/// `{"type":"ExecuteCommand","command":"SetMissionEpoch","params":{"epoch_jd":2461253.0}}`. Sets both
/// the mission origin and the calendar anchor at the CURRENT tick, so the sim
/// jumps to that date without a tick discontinuity. Scene-time policy applies
/// its selected epoch through [`crate::ApplySceneTimeSelection`].
#[Command(default)]
pub struct SetMissionEpoch {
    /// Absolute epoch, Julian Date (TDB).
    pub epoch_jd: f64,
}

#[on_command(SetMissionEpoch)]
fn on_set_mission_epoch(
    trigger: On<SetMissionEpoch>,
    tick: Res<crate::SimTick>,
    mut clock: ResMut<crate::MissionClock>,
) {
    let jd = trigger.event().epoch_jd;
    if !jd.is_finite() || jd == 0.0 {
        bevy::log::warn!("[time] rejected invalid mission epoch JD {jd:?}");
        return;
    }
    *clock = crate::MissionClock::anchored(jd, tick.0);
    bevy::log::info!("[time] mission epoch re-anchored to JD {jd:.4}");
}

pub(crate) fn on_apply_scene_time_selection(
    trigger: On<crate::ApplySceneTimeSelection>,
    mut scene_time: ResMut<crate::SceneTimeState>,
    mut transport: ResMut<crate::TimeTransport>,
    mut faults: ResMut<lunco_core::RuntimeFaults>,
    mut commands: Commands,
) {
    let selection = &trigger.event().selection;
    if !selection.epoch_jd.is_finite()
        || selection.epoch_jd == 0.0
        || !matches!(selection.source, "authored" | "computer_time")
        || scene_time.phase != crate::SceneTimePhase::Loading
    {
        transport.mode = crate::TransportMode::Paused;
        let detail = format!(
            "scene time selection cannot be applied (source={}, epoch_jd={}, phase={:?})",
            selection.source, selection.epoch_jd, scene_time.phase
        );
        faults.raise(
            "scene-time-policy-invalid",
            None,
            "scene.time.select",
            detail.clone(),
        );
        bevy::log::error!("[time] {detail}");
        return;
    }
    scene_time.begin_selection_application(selection);
    commands.trigger(ResetTime {});
}

/// Reset the **entire clock tree** to defaults from the retained scene epoch.
///
/// This command restores the standing clock shape across scene reloads (doc 19
/// §11b):
///
/// * **interaction** → wall-rooted identity (its default);
/// * **animation preview** → playhead 0, playing, 1×;
/// * **transport** → Playing at 1×, except for an explicit pause requested while
///   the scene transition was pending, which is applied once to the replacement;
/// * **mission calendar** → the selected scene epoch retained from the last
///   completed `scene.time.select` decision.
#[Command(default)]
pub struct ResetTime {}

#[on_command(ResetTime)]
fn on_reset_time(
    _trigger: On<ResetTime>,
    mut mission: ResMut<crate::MissionClock>,
    clocks: Option<Res<Clocks>>,
    mut q_domain: Query<&mut TimeDomain>,
    mut q_playback: Query<&mut Playback>,
    preview: Option<Res<AnimationPreview>>,
    mut tick: Option<ResMut<lunco_core_runtime::SimTick>>,
    mut transport: ResMut<crate::TimeTransport>,
    virtual_time: Option<ResMut<Time<Virtual>>>,
    fixed_time: Option<ResMut<Time<Fixed>>>,
    mut pending_scene_pause: Option<ResMut<crate::PendingScenePause>>,
    mut resolved: ResMut<ResolvedDomains>,
    mut last: ResMut<LastClockT>,
    mut scene_time: Option<ResMut<crate::SceneTimeState>>,
) {
    let (epoch_jd, source) = match scene_time.as_deref() {
        Some(state) if state.phase == crate::SceneTimePhase::Applying => {
            let Some(selection) = state.selection else {
                bevy::log::error!("[time] scene time selection is applying without a selected epoch");
                return;
            };
            (selection.epoch_jd, selection.source)
        }
        Some(state) if state.phase == crate::SceneTimePhase::Ready => {
            let Some(selection) = state.selection else {
                bevy::log::error!("[time] settled scene has no retained epoch selection");
                return;
            };
            (selection.epoch_jd, selection.source)
        }
        Some(state) if state.phase == crate::SceneTimePhase::Loading => {
            bevy::log::warn!("[time] ResetTime waits for the settled scene-time selection");
            return;
        }
        Some(_) => {
            if let Some(pending_scene_pause) = pending_scene_pause.as_deref_mut() {
                pending_scene_pause.0 = false;
            }
            return;
        }
        None => (mission.anchor.epoch0_jd, "existing_anchor"),
    };

    if let Some(clocks) = clocks {
        // Interaction: wall-rooted identity (what `spawn_well_known_clocks`
        // builds).
        if let Ok(mut d) = q_domain.get_mut(clocks.interaction) {
            *d = TimeDomain::derived(Some(clocks.real), 0.0, 1.0);
        }

        // Animation preview: rewind and play at 1x.
        if let Some(preview) = preview {
            if let Ok(mut pb) = q_playback.get_mut(preview.domain) {
                *pb = Playback::default();
            }
        }
    }

    // Transport: a reloaded scene starts playing at realtime unless the caller
    // explicitly requested a pause while this scene transaction was pending.
    // The latter is the deterministic ordering contract behind
    // `restart_scene(); pause();`: the reset is deferred, so the request must
    // survive this boundary without preserving an unrelated earlier pause.
    let pause_replacement = pending_scene_pause
        .as_deref()
        .is_some_and(|pending| pending.0);
    *transport = crate::TimeTransport::default();
    transport.mode = if pause_replacement {
        crate::TransportMode::Paused
    } else {
        crate::TransportMode::Playing
    };

    if let Some(tick) = tick.as_deref_mut() {
        tick.0 = 0;
    }
    *mission = crate::MissionClock::anchored(epoch_jd, 0);
    bevy::log::info!(
        "[time] scene reset to retained {} epoch JD {:.8}",
        source,
        epoch_jd
    );
    if let Some(state) = scene_time.as_deref_mut() {
        if state.phase == crate::SceneTimePhase::Applying {
            state.finish_selection_application();
        }
    }
    if let Some(pending_scene_pause) = pending_scene_pause.as_deref_mut() {
        pending_scene_pause.0 = false;
    }
    if let Some(mut virtual_time) = virtual_time {
        crate::project_transport_state(&transport, &mut virtual_time, None);
    }
    if let Some(mut fixed_time) = fixed_time {
        let timestep = fixed_time.timestep();
        *fixed_time = Time::<Fixed>::from_duration(timestep);
    }

    // Clock entities persist across scene loads, so clear their sample history
    // before resolving the replacement scene.
    *resolved = ResolvedDomains::default();
    *last = LastClockT::default();

    bevy::log::info!("[time] clock tree reset for scene replacement");
}

register_commands!(
    on_control_animation,
    on_set_simulation_execution_mode,
    on_set_time_transport,
    on_set_mission_epoch,
    on_reset_time
);

/// Plugin wiring for the clock tree: components, [`ResolvedDomains`], the resolve
/// system in [`DomainResolveSet`] (`Update`), the [`AnimationPreview`] domain, and
/// the [`ControlAnimation`] command. Added by [`TimePlugin`](crate::TimePlugin).
pub(crate) fn build_domain_tree(app: &mut App) {
    app.register_type::<TimeDomain>()
        .register_type::<Playback>()
        .register_type::<TimeBinding>()
        .register_type::<DomainRegime>()
        .register_type::<ClockRoot>()
        .init_resource::<ResolvedDomains>()
        .init_resource::<LastClockT>()
        // The tree resolves in `PreUpdate` from the `WorldTime` published after
        // the preceding fixed loop, before domain consumers read their samples.
        .add_systems(
            PreUpdate,
            advance_and_resolve_domains
                .in_set(DomainResolveSet)
                .after(crate::TimeSpineSet),
        )
        .add_systems(
            Startup,
            (spawn_well_known_clocks, spawn_animation_preview).chain(),
        );
    register_all_commands(app);
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
        DomainSnapshot {
            parent,
            offset,
            scale,
            playback: head.map(|h| Playback {
                head: h,
                ..default()
            }),
            root: None,
        }
    }

    fn root(kind: ClockRoot) -> DomainSnapshot {
        DomainSnapshot {
            parent: None,
            offset: 0.0,
            scale: 1.0,
            playback: None,
            root: Some(kind),
        }
    }

    /// Resolve `domain`'s `t` with no prior frame (so every `dt` starts from `t`).
    fn t_of(m: &HashMap<Entity, DomainSnapshot>, domain: Entity, sim: f64) -> f64 {
        let roots = RootTimes {
            sim_secs: sim,
            wall_secs: 0.0,
        };
        resolve_clocks(m, &HashMap::new(), roots, roots, None)
            .get(&domain)
            .map(|s| s.t)
            .unwrap_or(sim)
    }

    #[test]
    fn control_animation_pauses_seeks_and_rates_independently() {
        let mut pb = Playback::default(); // playing, head 0, rate 1
                                          // Pause only.
        apply_control_animation(
            &mut pb,
            &ControlAnimation {
                playing: Some(false),
                ..default()
            },
        );
        assert!(matches!(pb.mode, TransportMode::Paused));
        assert_eq!(pb.head, 0.0);
        assert_eq!(pb.rate, 1.0);
        // Seek only — leaves the paused mode untouched.
        apply_control_animation(
            &mut pb,
            &ControlAnimation {
                seek_secs: Some(3.5),
                ..default()
            },
        );
        assert!(matches!(pb.mode, TransportMode::Paused));
        assert!((pb.head - 3.5).abs() < EPS);
        // Play + rate in one verb.
        apply_control_animation(
            &mut pb,
            &ControlAnimation {
                playing: Some(true),
                rate: Some(2.0),
                ..default()
            },
        );
        assert!(matches!(pb.mode, TransportMode::Playing));
        assert!((pb.rate - 2.0).abs() < EPS);
        assert!((pb.head - 3.5).abs() < EPS); // seek preserved

        // A paused preview head does NOT advance with the world delta (scrub holds).
        let held = Playback {
            mode: TransportMode::Paused,
            head: 3.5,
            ..default()
        };
        assert!((step_playhead(&held, 10.0) - 3.5).abs() < EPS);
    }

    #[test]
    fn transport_rate_command_resumes_a_paused_transport() {
        let mut transport = crate::TimeTransport {
            mode: TransportMode::Paused,
            rate: 1.0,
        };
        apply_time_transport(
            &mut transport,
            &SetTimeTransport {
                playing: Some(true),
                rate: Some(8.0),
            },
        );
        assert_eq!(transport.mode, TransportMode::Playing);
        assert_eq!(transport.rate, 8.0);
    }

    #[test]
    fn transport_pause_command_closes_the_current_fixed_burst() {
        let mut app = App::new();
        app.insert_resource(crate::TimeTransport::default())
            .insert_resource(Time::<Virtual>::default())
            .insert_resource(Time::<Fixed>::from_hz(60.0));
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .accumulate_overstep(std::time::Duration::from_millis(20));
        app.add_observer(on_set_time_transport);

        app.world_mut().trigger(SetTimeTransport {
            playing: Some(false),
            rate: None,
        });

        assert!(app.world().resource::<Time<Virtual>>().is_paused());
        assert_eq!(
            app.world().resource::<Time<Fixed>>().overstep(),
            std::time::Duration::ZERO
        );
    }

    #[test]
    fn execution_mode_command_is_independent_of_transport_rate() {
        let mut app = App::new();
        app.insert_resource(lunco_core_runtime::SimulationExecutionMode::Realtime)
            .insert_resource(crate::TimeTransport {
                mode: TransportMode::Playing,
                rate: 4.0,
            })
            .add_observer(on_set_simulation_execution_mode);

        app.world_mut().trigger(SetSimulationExecutionMode {
            mode: lunco_core_runtime::SimulationExecutionMode::MaxSpeed,
        });

        assert_eq!(
            *app.world()
                .resource::<lunco_core_runtime::SimulationExecutionMode>(),
            lunco_core_runtime::SimulationExecutionMode::MaxSpeed
        );
        assert_eq!(app.world().resource::<crate::TimeTransport>().rate, 4.0);
    }

    #[test]
    fn reset_time_restores_clock_projection_and_fixed_admission_state() {
        let selected_epoch = 2_461_234.5;
        let mut app = App::new();
        app.insert_resource(crate::MissionClock::default())
            .insert_resource(lunco_core_runtime::SimTick(11))
            .insert_resource(crate::SceneTimeState {
                phase: crate::SceneTimePhase::Ready,
                selection: Some(crate::SceneTimeSelectionRecord {
                    source: "computer_time",
                    epoch_jd: selected_epoch,
                }),
            })
            .insert_resource(crate::TimeTransport {
                mode: TransportMode::Paused,
                rate: 4.0,
            })
            .insert_resource(Time::<Virtual>::default())
            .insert_resource(Time::<Fixed>::from_hz(60.0))
            .init_resource::<ResolvedDomains>()
            .init_resource::<LastClockT>();
        app.world_mut().resource_mut::<Time<Virtual>>().pause();
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .accumulate_overstep(std::time::Duration::from_millis(20));
        app.add_observer(on_reset_time);

        app.world_mut().trigger(ResetTime {});

        let transport = app.world().resource::<crate::TimeTransport>();
        assert_eq!(transport.mode, TransportMode::Playing);
        assert_eq!(transport.rate, 1.0);
        assert_eq!(app.world().resource::<lunco_core_runtime::SimTick>().0, 0);
        assert_eq!(
            app.world().resource::<crate::MissionClock>().mission_epoch0_jd,
            selected_epoch
        );
        assert!(!app.world().resource::<Time<Virtual>>().is_paused());
        assert_eq!(
            app.world().resource::<Time<Fixed>>().overstep(),
            std::time::Duration::ZERO
        );
    }

    #[test]
    fn pause_requested_behind_scene_restart_survives_reset() {
        let mut app = App::new();
        app.insert_resource(crate::TimeTransport::default())
            .insert_resource(crate::PendingScenePause::default())
            .insert_resource(lunco_core::SceneTransitionCoordinator::default())
            .insert_resource(crate::SceneTimeState {
                phase: crate::SceneTimePhase::Loading,
                selection: None,
            })
            .insert_resource(lunco_core::RuntimeFaults::default())
            .insert_resource(Time::<Virtual>::default())
            .insert_resource(Time::<Fixed>::from_hz(60.0))
            .insert_resource(crate::MissionClock::default())
            .init_resource::<ResolvedDomains>()
            .init_resource::<LastClockT>();
        app.world_mut()
            .resource_mut::<lunco_core::SceneTransitionCoordinator>()
            .admit(lunco_core::SceneTransitionRequest::restart(false));
        app.add_observer(on_set_time_transport);
        app.add_observer(on_reset_time);

        app.world_mut().trigger(SetTimeTransport {
            playing: Some(false),
            rate: None,
        });
        app.add_observer(on_apply_scene_time_selection);
        app.world_mut().trigger(crate::ApplySceneTimeSelection {
            selection: crate::SceneTimeSelection {
                source: "authored",
                epoch_jd: 2_461_234.5,
                warning: None,
            },
        });
        app.update();

        assert!(app.world().resource::<Time<Virtual>>().is_paused());
        assert!(!app.world().resource::<crate::PendingScenePause>().0);
        assert_eq!(
            app.world().resource::<crate::SceneTimeState>().phase,
            crate::SceneTimePhase::Ready
        );
    }

    #[test]
    fn transport_rate_command_rejects_rates_outside_supported_range() {
        let mut transport = crate::TimeTransport::default();

        apply_time_transport(
            &mut transport,
            &SetTimeTransport {
                rate: Some(crate::MIN_REALTIME_RATE),
                ..default()
            },
        );
        assert_eq!(transport.rate, crate::MIN_REALTIME_RATE);

        apply_time_transport(
            &mut transport,
            &SetTimeTransport {
                rate: Some(crate::MIN_REALTIME_RATE / 2.0),
                ..default()
            },
        );
        assert_eq!(transport.rate, crate::MIN_REALTIME_RATE);

        apply_time_transport(
            &mut transport,
            &SetTimeTransport {
                rate: Some(crate::MAX_REALTIME_RATE),
                ..default()
            },
        );
        assert_eq!(transport.rate, crate::MAX_REALTIME_RATE);

        apply_time_transport(
            &mut transport,
            &SetTimeTransport {
                rate: Some(crate::MAX_REALTIME_RATE + 1.0),
                ..default()
            },
        );
        assert_eq!(transport.rate, crate::MAX_REALTIME_RATE);

        apply_time_transport(
            &mut transport,
            &SetTimeTransport {
                rate: Some(f64::NAN),
                ..default()
            },
        );
        assert_eq!(transport.rate, crate::MAX_REALTIME_RATE);
    }

    #[test]
    fn driven_playhead_freezes_with_a_frozen_parent_and_runs_on_a_live_one() {
        // WHY the animation clock is re-parentable at all: a driven playhead
        // advances by its PARENT's delta, so `mode = Playing` is not enough. On a
        // paused sim the parent delta is 0 and the camera sits still with the
        // transport insisting it is playing — which reads as "play is broken".
        let pb = Playback {
            mode: TransportMode::Playing,
            head: 4.0,
            rate: 1.0,
            ..default()
        };
        // Sim-rooted + sim paused ⇒ parent delta 0 ⇒ frozen (the standing shape).
        assert!((step_playhead(&pb, 0.0) - 4.0).abs() < EPS);
        // Wall-rooted ⇒ the parent keeps advancing ⇒ the move runs regardless.
        assert!((step_playhead(&pb, 0.5) - 4.5).abs() < EPS);
    }

    #[test]
    fn control_animation_toggles_looping_and_restarts() {
        // `looping` is reachable as a verb: the field was honoured by
        // `step_playhead` long before anything could set it.
        let mut pb = Playback {
            start: 0.0,
            end: 10.0,
            ..default()
        };
        assert!(!pb.looping);
        apply_control_animation(
            &mut pb,
            &ControlAnimation {
                looping: Some(true),
                ..default()
            },
        );
        assert!(pb.looping);
        // ...and it reaches the stepper: past `end` wraps instead of clamping.
        let wrapped = Playback { head: 9.0, ..pb };
        assert!((step_playhead(&wrapped, 2.0) - 1.0).abs() < EPS);
        apply_control_animation(
            &mut pb,
            &ControlAnimation {
                looping: Some(false),
                ..default()
            },
        );
        let clamped = Playback { head: 9.0, ..pb };
        assert!((step_playhead(&clamped, 2.0) - 10.0).abs() < EPS);

        // Restart = seek-to-start + play in ONE verb (the HUD's ⏮ button).
        // Range starts late on purpose: seeking a literal 0.0 here would land
        // outside [start, end] and snap forward on the next step.
        let mut late = Playback {
            start: 5.0,
            end: 20.0,
            mode: TransportMode::Paused,
            head: 17.0,
            ..default()
        };
        let restart_to = late.start;
        apply_control_animation(
            &mut late,
            &ControlAnimation {
                playing: Some(true),
                seek_secs: Some(restart_to),
                ..default()
            },
        );
        assert!(matches!(late.mode, TransportMode::Playing));
        assert!((late.head - 5.0).abs() < EPS);
        assert!((step_playhead(&late, 1.0) - 6.0).abs() < EPS); // runs forward from start
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
        assert!((t_of(&m, inner, 10.0) - 60.0).abs() < EPS);
        assert!((t_of(&m, outer, 10.0) - 20.0).abs() < EPS);
    }

    #[test]
    fn driven_domain_returns_its_head_not_the_chain() {
        let d = e(7);
        let mut m = HashMap::new();
        // A driven head with no prior frame gets parent delta 0 → holds at its head,
        // regardless of world_secs.
        m.insert(d, snap(None, 0.0, 1.0, Some(42.0)));
        assert!((t_of(&m, d, 1000.0) - 42.0).abs() < EPS);
    }

    #[test]
    fn unknown_or_cyclic_domain_falls_back_to_world() {
        let a = e(8);
        let b = e(9);
        let mut m = HashMap::new();
        // a → b → a cycle: depth cap returns world_secs.
        m.insert(a, snap(Some(b), 0.0, 1.0, None));
        m.insert(b, snap(Some(a), 0.0, 1.0, None));
        assert!((t_of(&m, a, 5.0) - 5.0).abs() < 1e-6);
        // Missing domain → world_secs.
        assert!((t_of(&m, e(99), 5.0) - 5.0).abs() < EPS);
    }

    /// The load-bearing property of the whole design (doc 19 §11a): a frozen
    /// ancestor freezes its subtree **structurally**, with no `paused` flag and no
    /// propagation pass. `parent_t` simply stops changing, so `child_t` does too.
    #[test]
    fn a_frozen_parent_freezes_its_whole_subtree_with_no_flag() {
        let sim = e(1);
        let child = e(2);
        let grandchild = e(3);
        let mut m = HashMap::new();
        m.insert(sim, root(ClockRoot::Tick));
        m.insert(child, snap(Some(sim), 0.0, 1.0, None));
        m.insert(grandchild, snap(Some(child), 0.0, 60.0, None)); // 60× the parent

        let roots_running = RootTimes {
            sim_secs: 10.0,
            wall_secs: 99.0,
        };
        let a = resolve_clocks(&m, &HashMap::new(), roots_running, roots_running, Some(sim));
        assert!((a[&grandchild].t - 600.0).abs() < EPS);

        // Sim paused: sim_secs stops at 10 while WALL time keeps running (99 → 123).
        // Everything under `sim` must hold — including the 60× child.
        let last: HashMap<Entity, f64> = a.iter().map(|(&k, s)| (k, s.t)).collect();
        let roots_paused = RootTimes {
            sim_secs: 10.0,
            wall_secs: 123.0,
        };
        let b = resolve_clocks(&m, &last, roots_paused, roots_running, Some(sim));
        assert!((b[&grandchild].t - 600.0).abs() < EPS);
        assert!(
            b[&grandchild].dt.abs() < EPS,
            "a frozen subtree must report dt = 0"
        );
        assert!(b[&child].dt.abs() < EPS);
    }

    /// A WALL-rooted interaction clock keeps running while the sim is paused.
    #[test]
    fn a_wall_rooted_clock_survives_a_sim_pause() {
        let sim = e(1);
        let real = e(2);
        let interaction = e(3);
        let mut m = HashMap::new();
        m.insert(sim, root(ClockRoot::Tick));
        m.insert(real, root(ClockRoot::Wall));
        m.insert(interaction, snap(Some(real), 0.0, 1.0, None));

        let r1 = RootTimes {
            sim_secs: 10.0,
            wall_secs: 100.0,
        };
        let a = resolve_clocks(&m, &HashMap::new(), r1, r1, Some(sim));
        let last: HashMap<Entity, f64> = a.iter().map(|(&k, s)| (k, s.t)).collect();

        // Sim frozen (10 → 10), wall advances (100 → 100.25).
        let r2 = RootTimes {
            sim_secs: 10.0,
            wall_secs: 100.25,
        };
        let b = resolve_clocks(&m, &last, r2, r1, Some(sim));
        assert!((b[&sim].dt).abs() < EPS, "the sim clock is paused");
        assert!(
            (b[&interaction].dt - 0.25).abs() < EPS,
            "the interaction clock must keep ticking through a sim pause"
        );
    }

    /// A driven playhead advances by its PARENT's delta — so a pause reaches driven
    /// clocks too, without anyone propagating a flag into them.
    #[test]
    fn driven_head_advances_by_parent_delta_and_holds_when_parent_freezes() {
        let sim = e(1);
        let d = e(2);
        let mut m = HashMap::new();
        m.insert(sim, root(ClockRoot::Tick));
        m.insert(
            d,
            DomainSnapshot {
                parent: Some(sim),
                offset: 0.0,
                scale: 1.0,
                playback: Some(Playback::replay(0.0, 0.0, 2.0, false)), // 2×
                root: None,
            },
        );

        // Frame 1 establishes the baseline (no prior frame ⇒ parent delta 0).
        let r1 = RootTimes {
            sim_secs: 5.0,
            wall_secs: 0.0,
        };
        let a = resolve_clocks(&m, &HashMap::new(), r1, r1, Some(sim));
        let last: HashMap<Entity, f64> = a.iter().map(|(&k, s)| (k, s.t)).collect();

        // Sim advances 5 → 8 (delta 3); the 2× head advances 6.
        let r2 = RootTimes {
            sim_secs: 8.0,
            wall_secs: 0.0,
        };
        let b = resolve_clocks(&m, &last, r2, r1, Some(sim));
        assert!((b[&d].t - 6.0).abs() < EPS);

        // The live system writes the advanced head back onto the `Playback` component,
        // so the next frame's snapshot carries it. Model that here.
        m.get_mut(&d).unwrap().playback.as_mut().unwrap().head = b[&d].t;

        // Sim pauses (8 → 8): the head holds, even though it is "Playing".
        let last2: HashMap<Entity, f64> = b.iter().map(|(&k, s)| (k, s.t)).collect();
        let c = resolve_clocks(&m, &last2, r2, r2, Some(sim));
        assert!((c[&d].t - 6.0).abs() < EPS);
        assert!(c[&d].dt.abs() < EPS);
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
            .init_resource::<LastClockT>()
            .init_resource::<Time<bevy::time::Real>>()
            .add_systems(Update, advance_and_resolve_domains);

        app.world_mut().resource_mut::<WorldTime>().sim_secs = 10.0;
        let derived = app
            .world_mut()
            .spawn(TimeDomain::derived(None, 0.0, 2.0))
            .id();
        let driven = app
            .world_mut()
            .spawn((
                TimeDomain::default(),
                Playback::replay(0.0, 0.0, 1.0, false),
            ))
            .id();

        app.update();

        let resolved = app.world().resource::<ResolvedDomains>();
        // Derived: parent defaults to the sim root ⇒ 2·world = 20.
        assert_eq!(resolved.get(derived), Some(20.0));
        // Driven: head advanced by the sim delta (10 − 0)·rate = 10.
        assert_eq!(resolved.get(driven), Some(10.0));

        // Second frame: sim advances 10 → 15, so the 1× head advances by another 5.
        app.world_mut().resource_mut::<WorldTime>().sim_secs = 15.0;
        app.update();
        let resolved = app.world().resource::<ResolvedDomains>();
        assert_eq!(resolved.get(derived), Some(30.0));
        assert_eq!(resolved.get(driven), Some(15.0));
        assert_eq!(resolved.delta(driven), 5.0);

        // Third frame: the sim clock HOLDS ⇒ the driven head holds too, with dt = 0.
        // A pause reaches a driven clock with no flag, purely because its parent
        // stopped advancing.
        app.update();
        let resolved = app.world().resource::<ResolvedDomains>();
        assert_eq!(resolved.get(driven), Some(15.0));
        assert_eq!(resolved.delta(driven), 0.0);
    }

    /// The well-known clocks have only the physical tick and wall-time roots.
    #[test]
    fn well_known_clocks_spawn_in_the_documented_shape() {
        let mut app = App::new();
        app.add_systems(Startup, spawn_well_known_clocks);
        app.update();

        let clocks = *app.world().resource::<Clocks>();
        let w = app.world();
        assert_eq!(w.get::<ClockRoot>(clocks.real), Some(&ClockRoot::Wall));
        assert_eq!(w.get::<ClockRoot>(clocks.sim), Some(&ClockRoot::Tick));
        // The interaction clock hangs off the wall root — that is what keeps the
        // avatar moving while the sim is paused.
        assert_eq!(
            w.get::<TimeDomain>(clocks.interaction).unwrap().parent,
            Some(clocks.real)
        );
    }
}
