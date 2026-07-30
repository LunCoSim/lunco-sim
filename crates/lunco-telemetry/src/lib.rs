//! # Telemetry sampling — the producer half of parameter telemetry
//!
//! Samples every entity tagged with a [`lunco_core::telemetry::Parameter`] and emits
//! a [`SampledParameter`](lunco_core::telemetry::SampledParameter) per sample.
//!
//! ## The channel
//!
//! A channel is a named, **rate-limited**, **clock-bound** view of one live value:
//!
//! ```ignore
//! Parameter {
//!     name: "motor_current", unit: "A",
//!     source:   ChannelSource::Port("left_wheel.torque".into()),  // or Reflect(..)
//!     rate_hz:  Some(10.0),      // None ⇒ TelemetrySettings::default_rate_hz
//!     enabled:  true,
//!     deadband: Some(0.01),      // don't emit unless it moved
//! }
//! ```
//!
//! Authoring needs no new API: `Parameter` is `Reflect` + `ReflectDefault`, so a
//! script adds one with `add(id, "Parameter", #{…})`
//! (`lunco_scripting::bridge_core::add_component`).
//!
//! ## Rate is measured on the channel's own clock, NOT the wall clock
//!
//! Each channel keeps an accumulator against the time domain its entity is bound to
//! ([`lunco_time::TimeBinding`] → [`lunco_time::domain_time`]; absent ⇒ the world
//! domain). That one decision buys pause, warp, `TimeDomain::scale`, and `Playback`
//! seek/loop **for free**, because those already live in the domain: a channel on a
//! `scale = 100` domain samples 100× the sim-seconds per wall-second, and a paused sim
//! samples nothing.
//!
//! This is deliberately **not** bevy's `on_timer` run-condition. `on_timer` is
//! wall-clock: it would keep firing while the sim is frozen and would ignore warp
//! entirely. That is the same mistake as pacing the co-simulation off the render frame
//! — a sampled signal must ride the clock it is sampling.
//!
//! Sampling runs in `FixedUpdate`, so the ceiling is `FIXED_HZ` and a replay produces
//! the same samples. **Do not move this to `Update`** — it would make telemetry
//! frame-rate-dependent and non-deterministic.
//!
//! ## Where the samples go — the consumer half was already shipped
//!
//! `SampledParameter` is observed by `lunco_api::subscription::sampled_param_observer`
//! (this is what `SubscribeTelemetry` delivers), mapped by
//! `TelemetryResponse::from_sampled`, and logged by `lunco_core::log`. **All of that
//! already existed while this crate sat unwired**, so the API advertised parameter
//! telemetry that could never arrive. Adding `LunCoTelemetryPlugin` was the whole fix.
//!
//! Distinct from `TelemetryEvent`, which is the *push* channel (something explicitly
//! emits an event). This is the *pull* channel: it samples state nobody emitted.
//!
//! See `docs/architecture/telemetry-subsystem.md`.

mod api;

use bevy::prelude::*;
use lunco_core::ports::{PortDirection, PortRegistry, ResolvedPort};
use lunco_core::telemetry::{ChannelSource, Parameter, SampledParameter, TelemetryValue};
use lunco_core::{on_command, register_commands, Command};
use lunco_settings::{AppSettingsExt, SettingsSection};
use lunco_time::{domain_time, ResolvedDomains, TimeBinding, WorldTime};
use serde::{Deserialize, Serialize};

/// Persisted telemetry defaults. Stored under the `"telemetry"` key of
/// `settings.json`.
#[derive(Resource, Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct TelemetrySettings {
    /// Rate for a channel that doesn't specify one.
    ///
    /// **5 Hz, not 60.** A channel is a network packet per sample per subscriber; the
    /// fixed rate is a ceiling, not a sensible default. Anything that genuinely needs
    /// per-tick fidelity asks for it.
    pub default_rate_hz: f64,
    /// Backpressure guard: refuse to sample beyond this many live channels, and say so
    /// once. A silently-truncated telemetry feed is worse than a loud one.
    pub max_channels: usize,
    /// Ring-buffer depth for a channel that doesn't specify one, in SAMPLES.
    ///
    /// **Chosen WITH the rate, never alone**: the pair is what decides how long a
    /// window a plot can show without the buffer wrapping. At the default 5 Hz,
    /// 1500 samples is exactly 5 minutes. Raising the rate without raising this
    /// silently shortens that window instead of costing memory, which is the
    /// failure mode that looks like "the plot keeps eating my history".
    ///
    /// Costs 16 B per sample: 24 KB per channel, ~24 MB at the 1024-channel cap.
    pub default_retention: usize,
    /// Master switch.
    pub enabled: bool,
}

impl Default for TelemetrySettings {
    fn default() -> Self {
        Self {
            // 5 Hz for 5 minutes — the same window the wholesale co-sim publisher
            // keeps (`lunco_cosim::telemetry`), so an authored channel and a model
            // variable plotted side by side show the same span of history.
            default_rate_hz: 5.0,
            max_channels: 1024,
            default_retention: 1500,
            enabled: true,
        }
    }
}

impl SettingsSection for TelemetrySettings {
    const KEY: &'static str = "telemetry";
}

/// Control the telemetry subsystem at runtime.
///
/// **One verb, all-`Option` fields** — the [`ControlAnimation`](lunco_time::ControlAnimation)
/// idiom. `None` means "leave unchanged". Five separate `StartTelemetry` /
/// `SetTelemetryRate` / `SetRetention` / … commands would be five things to discover,
/// document, journal, and keep in sync; this is one.
///
/// `channel: None` addresses the **subsystem** (the master switch). `channel: Some(name)`
/// addresses every channel with that name — names are not unique across entities, and
/// "turn off `motor_current` everywhere" is the useful operation. To address exactly one
/// entity's channel, edit its `Parameter` component directly (the Inspector, a script).
#[Command(default)]
pub struct ControlTelemetry {
    /// Channel name, or `None` for the whole subsystem.
    pub channel: Option<String>,
    /// **Create** the channel on this entity if it does not exist.
    ///
    /// Without this there was NO way to author a telemetry channel through the API at all —
    /// only from rhai or USD. That left an external client (an agent, OpenMCT, a dashboard)
    /// able to *read* channels but never to *ask for* one, so the only way to watch an
    /// arbitrary port was to poll it from the client. `port` (or `reflect`) names what to
    /// sample; both absent ⇒ this is a retune of an existing channel, not a create.
    #[authz_target]
    pub entity: Option<Entity>,
    /// Source for a created channel: a port name on `entity` (the fast path — this is what
    /// makes any Modelica variable, Avian body value, joint, FSW signal, or USD sensor
    /// watchable without authoring anything in the scene).
    pub port: Option<String>,
    /// Source for a created channel: a reflection path (`"Port.value"`). The escape
    /// hatch, for a field no port exposes.
    pub reflect: Option<String>,
    /// Engineering unit for a created channel.
    pub unit: Option<String>,
    pub enabled: Option<bool>,
    pub rate_hz: Option<f64>,
    pub retention: Option<usize>,
    pub deadband: Option<f64>,
}

#[on_command(ControlTelemetry)]
fn on_control_telemetry(
    trigger: On<ControlTelemetry>,
    mut settings: ResMut<TelemetrySettings>,
    // ONE query: a second `Query<&Parameter>` alongside this `&mut` one is a conflicting
    // access and panics at run time (B0001).
    mut channels: Query<(Entity, &mut Parameter, Option<&mut ChannelClock>)>,
    mut commands: Commands,
) {
    let cmd = trigger.event().clone();

    // CREATE: an entity + a source + a name ⇒ author the channel (or re-point an existing
    // one on that entity). This is what lets a client say "watch this port at 20 Hz" instead
    // of polling it from outside.
    if let (Some(entity), Some(name)) = (cmd.entity, cmd.channel.clone()) {
        let source = match (&cmd.port, &cmd.reflect) {
            (Some(p), _) => Some(ChannelSource::Port(p.clone())),
            (None, Some(r)) => Some(ChannelSource::Reflect(r.clone())),
            (None, None) => None,
        };
        if let Some(source) = source {
            let param = Parameter {
                name,
                unit: cmd.unit.clone().unwrap_or_default(),
                description: None,
                source,
                target: Some(entity),
                rate_hz: cmd.rate_hz,
                enabled: cmd.enabled.unwrap_or(true),
                deadband: cmd.deadband,
                retention: cmd.retention,
            };
            // A DEDICATED channel entity targeting the measured one. Not a component on the
            // rover: `Parameter` is a Component, so putting it there would cap the rover at
            // ONE channel — "watch three ports on this rover" must be representable.
            //
            // Re-point instead of duplicating if a channel of this name already watches this
            // target, and drop the stale `ChannelClock` with it (a cached `ResolvedPort` slot
            // from the previous source would read the wrong value).
            let existing = channels
                .iter()
                .find(|(_, p, _)| p.name == param.name && p.target == Some(entity))
                .map(|(e, _, _)| e);
            match existing {
                Some(chan) => {
                    commands
                        .entity(chan)
                        .remove::<ChannelClock>()
                        .try_insert(param);
                }
                None => {
                    commands.spawn((Name::new(format!("telemetry:{}", param.name)), param));
                }
            }
            return;
        }
    }

    let Some(name) = cmd.channel.clone() else {
        // Subsystem-level: the master switch and the defaults.
        if let Some(enabled) = cmd.enabled {
            settings.enabled = enabled;
        }
        if let Some(rate) = cmd.rate_hz {
            settings.default_rate_hz = rate;
        }
        if let Some(retention) = cmd.retention {
            settings.default_retention = retention;
        }
        return;
    };

    for (_, mut param, clock) in channels.iter_mut().filter(|(_, p, _)| p.name == name) {
        if let Some(enabled) = cmd.enabled {
            param.enabled = enabled;
        }
        if let Some(rate) = cmd.rate_hz {
            param.rate_hz = Some(rate);
            // A rate change must take effect NOW, not after the old (possibly very long)
            // period elapses: dropping a channel from 0.01 Hz to 10 Hz would otherwise
            // sit silent for 100 s before honouring the new rate.
            if let Some(mut clock) = clock {
                clock.next_due_t = f64::NEG_INFINITY;
            }
        }
        if let Some(retention) = cmd.retention {
            param.retention = Some(retention);
        }
        if let Some(deadband) = cmd.deadband {
            param.deadband = Some(deadband);
        }
    }
}

register_commands!(on_control_telemetry);

/// Marks the entity carrying an engine-health channel (FPS, frame time), so the set is
/// identifiable and a second `Startup` never duplicates it.
#[derive(Component, Debug)]
pub struct EngineHealthChannel;

/// Publish the engine's own health as telemetry channels.
///
/// FPS was previously a number that could only ever reach a HUD. As a channel it is
/// subscribable, retained, plottable, and queryable by a ground system — exactly like a
/// motor current. That is the "reuse FPS" the perf HUD's hand-rolled ring buffer was
/// standing in the way of.
///
/// **Self-gating:** only spawns a channel whose `Diagnostic` actually exists. A headless
/// server links `bevy_diagnostic` but nobody adds `FrameTimeDiagnosticsPlugin` there (it
/// comes with the perf HUD), so a `--no-ui` run publishes no FPS channel rather than an
/// always-silent one that clutters the catalog.
///
/// Rate is deliberately low (2 Hz). Frame time is already smoothed by the `Diagnostic`;
/// sampling it at 60 Hz would spend 30× the bandwidth to convey the same trend.
fn spawn_engine_health_channels(
    diags: Option<Res<bevy::diagnostic::DiagnosticsStore>>,
    existing: Query<(), With<EngineHealthChannel>>,
    mut commands: Commands,
) {
    let Some(diags) = diags else { return };
    if !existing.is_empty() {
        return;
    }
    for (path, name, unit) in [
        ("fps", "engine.fps", "1/s"),
        ("frame_time", "engine.frame_time", "ms"),
    ] {
        if !diags.iter().any(|d| d.path().as_str() == path) {
            continue;
        }
        commands.spawn((
            Name::new(name),
            EngineHealthChannel,
            Parameter {
                name: name.to_string(),
                unit: unit.to_string(),
                description: None,
                source: ChannelSource::Diagnostic(path.to_string()),
                rate_hz: Some(2.0),
                target: None,
                enabled: true,
                deadband: None,
                retention: None,
            },
        ));
    }
}

/// Per-channel sampling state. Added lazily by the sampler — never authored.
#[derive(Component, Debug, Default)]
struct ChannelClock {
    /// Next due time, in the channel's domain seconds.
    next_due_t: f64,
    /// Value at the last *emitted* sample — the deadband reference.
    last_emitted: Option<f64>,
    /// Port handle, resolved once. Resolving by name every sample is exactly what
    /// `ResolvedPort` exists to avoid.
    resolved: Option<ResolvedPort>,
    /// True once we've tried and failed to resolve, so we don't re-scan every backend
    /// at the sample rate for a port that doesn't exist.
    resolve_failed: bool,
}

/// The cached sampling plan: which entities carry a channel. Rebuilt only when
/// the channel set changes (see [`mark_sampling_plan_dirty`]), NOT per tick —
/// the per-tick pass walks this list and reads `Parameter`/`TimeBinding` in
/// place, so the old full-snapshot clone of every `Parameter` (heap Strings)
/// per fixed tick is gone.
#[derive(Resource, Default)]
struct SamplingPlan {
    /// Channel entities, in query order. May briefly contain despawned
    /// entities (removal marks the plan dirty the same tick, and the sampler
    /// skips dead entities), never miss live ones.
    channels: Vec<Entity>,
    /// Set by [`mark_sampling_plan_dirty`]; consumed by the sampler, which
    /// rebuilds `channels` before the pass.
    dirty: bool,
}

/// Marks a channel generated from a live public output.  It carries no authored
/// data: the port backend remains the source for existence and value, while the
/// normal [`Parameter`] sampler supplies cadence, retention, API, and history.
#[derive(Component, Debug, Clone)]
struct RuntimePortChannel;

/// Last metadata applied to a retained signal by a telemetry channel.
///
/// Channel policy changes are rare while samples are continuous. Keeping this
/// tiny cache on the channel means [`retain_sample`] can update plot metadata
/// exactly when that policy changes, rather than allocating and replacing the
/// same strings for every sample.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
struct RetainedSignalMeta {
    description: Option<String>,
    unit: String,
}

/// Bounded discovery cadence. Port values sample at the fixed telemetry rate;
/// topology need only be rediscovered shortly after a spawn/despawn, not every
/// fixed step. At 64 Hz this is a quarter-second maximum attach delay.
#[derive(Resource)]
struct RuntimePortDiscovery {
    ticks_until_scan: u8,
}

impl Default for RuntimePortDiscovery {
    fn default() -> Self {
        Self {
            ticks_until_scan: 0,
        }
    }
}

/// Discover every currently readable public output and materialize its sampling
/// channel.  This is deliberately runtime-only: spawned systems begin exposing
/// telemetry on the next fixed step and no USD scene needs a telemetry tag.
///
/// A dedicated channel entity lets one physical component expose every output
/// despite `Parameter` itself being a one-per-entity component.  Existing
/// explicitly-created channels win by `(target, port)` so API users retain a
/// deliberate rate/retention override without duplicated samples.
fn discover_runtime_port_channels(world: &mut World) {
    let due = {
        let mut discovery = world.resource_mut::<RuntimePortDiscovery>();
        if discovery.ticks_until_scan > 0 {
            discovery.ticks_until_scan -= 1;
            false
        } else {
            discovery.ticks_until_scan = 15;
            true
        }
    };
    if !due {
        return;
    }
    let Some(ports) = world.get_resource::<PortRegistry>().cloned() else {
        return;
    };
    let existing: std::collections::HashSet<(Entity, String)> = world
        .query::<&Parameter>()
        .iter(world)
        .filter_map(|p| match (&p.target, &p.source) {
            (Some(target), ChannelSource::Port(port)) => Some((*target, port.clone())),
            _ => None,
        })
        .collect();
    let entities: Vec<Entity> = world.query::<Entity>().iter(world).collect();
    let mut discovered = Vec::new();
    for entity in entities {
        // Streamed terrain tiles, collider rings, and scatter are implementation
        // detail. Publishing every internal output makes their lifecycle look like
        // user telemetry and turns LOD churn into a growing sampling plan. An
        // authored or explicitly-created `Parameter` remains available when a
        // caller genuinely wants to observe a system-owned entity.
        if world.get::<lunco_core::SystemManaged>(entity).is_some() {
            continue;
        }
        for port in ports.entity_ports(world, entity) {
            if !matches!(port.direction, PortDirection::Out | PortDirection::InOut)
                || existing.contains(&(entity, port.name.clone()))
            {
                continue;
            }
            discovered.push((entity, port.name));
        }
    }
    for (target, port) in discovered {
        // Source lifecycle owns the telemetry lifecycle, regardless of where
        // this dedicated sampling entity lives.
        if let Ok(mut source) = world.get_entity_mut(target) {
            source.insert(lunco_signal::SignalSource);
        }
        world.spawn((
            RuntimePortChannel,
            Name::new(format!("telemetry:{port}")),
            Parameter {
                name: port.clone(),
                unit: String::new(),
                description: None,
                source: ChannelSource::Port(port),
                target: Some(target),
                rate_hz: None,
                enabled: true,
                deadband: None,
                retention: None,
            },
        ));
    }
}

/// Cheap change detector in front of the exclusive sampler: any added/changed/
/// removed `Parameter` or `TimeBinding` invalidates the plan. Runs on normal
/// (parallel) system params — the archetype-level `Changed` check and the
/// removal inboxes are near-free when nothing changed, which is every tick of
/// steady state.
fn mark_sampling_plan_dirty(
    changed: Query<(), Or<(Changed<Parameter>, Changed<TimeBinding>)>>,
    mut removed_params: RemovedComponents<Parameter>,
    mut removed_bindings: RemovedComponents<TimeBinding>,
    mut plan: ResMut<SamplingPlan>,
) {
    let removed = removed_params.read().next().is_some() | removed_bindings.read().next().is_some();
    if removed || !changed.is_empty() {
        plan.dirty = true;
    }
}

pub struct LunCoTelemetryPlugin;

impl Plugin for LunCoTelemetryPlugin {
    fn build(&self, app: &mut App) {
        app.register_settings_section::<TelemetrySettings>();
        // The retention plane. `SignalRegistry` is the ring buffer every plot surface
        // already reads — routing samples into it is what makes telemetry both *retained*
        // and *plottable*, with no new storage type and no new renderer.
        //
        // `init_resource` is idempotent: `lunco-viz` also initialises it in a GUI build,
        // and a headless run initialises it here. Deliberately NOT gated on the UI —
        // a `--no-ui` run wants history just as much (that is the whole point of a black
        // box), and `lunco-signal` is render-free precisely so it can.
        app.init_resource::<lunco_signal::SignalRegistry>();
        // What the user is looking at, so a telemetry surface can narrow to it.
        // Initialised beside the registry (and for the same reason): the resource is
        // render-free intent, written by whichever app owns selection and read by
        // whatever displays channels. Empty here in a headless run — nothing selects.
        app.init_resource::<lunco_signal::TelemetryFocus>();
        app.init_resource::<RuntimePortDiscovery>();
        app.add_observer(retain_sample);
        app.add_observer(drop_signal_of_removed_channel);
        app.add_observer(lunco_signal::drop_signals_of_removed_source);
        register_all_commands(app);
        // Engine health (FPS, frame time) as real telemetry channels — see
        // `spawn_engine_health_channels`.
        app.add_systems(Startup, spawn_engine_health_channels);
        // The QUERY surface — channel catalog + history range query. Subscription alone
        // gives a client a firehose it cannot interpret: no way to ask what channels
        // exist, no way to see anything from before it connected. OpenMCT (and any
        // ground-system UI) needs all three. See `api.rs`.
        api::build(app);
        // The plan starts dirty so the first sampler pass builds it.
        app.insert_resource(SamplingPlan {
            channels: Vec::new(),
            dirty: true,
        });
        app.add_systems(
            // FIXED step — see the module docs. Not `Update`: telemetry would then be
            // paced by the frame rate (different sample counts on a fast vs slow
            // machine, a flood on an uncapped headless loop) and replay would diverge.
            FixedUpdate,
            (
                discover_runtime_port_channels,
                // Change-driven plan maintenance — the sampler itself never scans
                // the channel set; it walks the cached plan.
                mark_sampling_plan_dirty,
                // The sampler is EXCLUSIVE (`&mut World`) — it forces a sync point
                // whenever it runs. Don't run it when there's nothing to sample,
                // which is the overwhelmingly common case: a scene has no channels
                // until one is authored.
                sample_parameters_system.run_if(any_with_component::<Parameter>),
            )
                .chain(),
        );
    }
}

fn sample_parameters_system(world: &mut World) {
    sample_parameters(world);
}

/// Retain every sample in the `SignalRegistry` ring buffer — the plot/plane of record.
///
/// Keyed by `(entity, name)` via `SignalRef`, because parameter names are NOT unique:
/// two rovers both report `"motor_current"`, and folding them into one buffer would
/// interleave two vehicles' data into a single nonsense trace.
///
/// **Scalars only.** `Bool` and `String` samples have no place in an `f64` ring buffer;
/// they are carried by the discrete `TelemetryEvent` lane instead. Silently coercing a
/// bool to 0.0/1.0 here would make a plot that lies about its type.
fn retain_sample(
    trigger: On<SampledParameter>,
    settings: Res<TelemetrySettings>,
    channels: Query<(&Parameter, Option<&RetainedSignalMeta>)>,
    mut commands: Commands,
    mut signals: ResMut<lunco_signal::SignalRegistry>,
) {
    let s = trigger.event();
    let Some(value) = numeric_of(&s.value) else {
        return;
    };
    // `SampledParameter` carries the producer's entity.  Looking it up directly
    // keeps the continuous retention path O(1); the old `(source, name)` scan
    // was O(samples × channels), and terrain-created channels made it dominate
    // Traverse's frame time.
    let Ok((channel, applied_meta)) = channels.get(s.channel) else {
        // A channel can be removed between collection and observer dispatch.
        // Its sample is no longer authoritative, so retain neither data nor
        // stale metadata.
        return;
    };
    let retention = channel.retention.unwrap_or(settings.default_retention);
    let signal = lunco_signal::SignalRef::new(s.source, s.name.clone());

    signals.push_scalar_with_capacity(
        signal.clone(),
        // `sim_secs`, NOT `timestamp`. The Julian-Date epoch has ~86 µs of f64 resolution
        // left, so a plot axis built from it would quantise into visible stair-steps and
        // any Δt would be garbage.
        s.sim_secs,
        value,
        retention,
    );

    let expected_meta = RetainedSignalMeta {
        description: channel.description.clone(),
        unit: channel.unit.clone(),
    };
    if applied_meta != Some(&expected_meta) {
        signals.update_meta(
            signal,
            lunco_signal::SignalMeta {
                description: expected_meta.description.clone(),
                unit: (!expected_meta.unit.is_empty()).then_some(expected_meta.unit.clone()),
                provenance: Some("telemetry".to_string()),
            },
        );
        commands.entity(s.channel).try_insert(expected_meta);
    }
}

/// A channel declaration may disappear while its sampled history is still a
/// mission artifact. Mark only this channel inactive; do not erase the trace.
fn drop_signal_of_removed_channel(
    trigger: On<Remove, Parameter>,
    channels: Query<&Parameter>,
    mut signals: ResMut<lunco_signal::SignalRegistry>,
) {
    let channel_entity = trigger.entity;
    let Ok(param) = channels.get(channel_entity) else {
        return;
    };
    let measured = param.target.unwrap_or(channel_entity);
    signals.deactivate_signal(&lunco_signal::SignalRef::new(measured, param.name.clone()));
}

/// One sampling pass. Public so a host can drive it directly (tests, a batch runner).
pub fn sample_parameters(world: &mut World) {
    let settings = world
        .get_resource::<TelemetrySettings>()
        .copied()
        .unwrap_or_default();
    if !settings.enabled {
        return;
    }

    // Absolute epoch for wall-clock labelling; the per-channel domain gives the
    // precise timebase (see `SampledParameter::sim_secs`). Graceful without the time
    // spine so this plugin is safe in a spine-less app.
    // The clock. Normally the `lunco-time` spine; without it, fall back to the fixed
    // schedule's own accumulated time.
    //
    // The fallback is NOT cosmetic. `WorldTime::default()` has `sim_secs == 0`, so a
    // spine-less app would see a clock that never advances — every channel would fire
    // exactly once and then never come due again. That failure is silent (telemetry
    // just stops), which is the worst kind.
    let world_time = world
        .get_resource::<WorldTime>()
        .cloned()
        .unwrap_or_else(|| {
            let elapsed = world
                .get_resource::<Time<Fixed>>()
                .map(|t| t.elapsed_secs_f64())
                .unwrap_or(0.0);
            WorldTime {
                sim_secs: elapsed,
                ..Default::default()
            }
        });
    // The sampling plan: which entities to visit. Rebuilt ONLY when the channel
    // set changed (see `mark_sampling_plan_dirty`) — steady state pays a Vec
    // walk, not a query + per-channel `Parameter` clone. The plan is taken OUT
    // of the world for the pass (reinserted below) so the loop can borrow
    // `&World` freely. A host driving this function directly without the plugin
    // has no plan resource and gets a fresh scan every call — the old behavior.
    let taken_plan = world.remove_resource::<SamplingPlan>();
    // Whether to give the plan back afterwards: only when the plugin owns it —
    // a plugin-less world has no dirty-marker, so caching there would go stale.
    let plan_is_owned = taken_plan.is_some();
    let mut plan = taken_plan.unwrap_or(SamplingPlan {
        channels: Vec::new(),
        dirty: true,
    });
    if plan.dirty {
        plan.channels.clear();
        plan.channels.extend(
            world
                .query_filtered::<Entity, With<Parameter>>()
                .iter(world),
        );
        plan.dirty = false;
    }

    // The resolver runs once per frame in `Update`. Take the resource OUT for the
    // pass instead of cloning its HashMap every fixed tick — the loop only reads
    // it, and it is reinserted before any event fires. (The resolver rewrites it
    // every frame anyway, so the remove/insert changes no change-detection story.)
    let resolved_taken = world.remove_resource::<ResolvedDomains>();
    let resolved_empty = ResolvedDomains::default();
    let resolved_domains = resolved_taken.as_ref().unwrap_or(&resolved_empty);

    if plan.channels.len() > settings.max_channels {
        warn_once!(
            "telemetry: {} channels exceeds max_channels ({}); sampling the first {} \
             and DROPPING the rest — raise TelemetrySettings::max_channels",
            plan.channels.len(),
            settings.max_channels,
            settings.max_channels
        );
    }

    let mut samples: Vec<SampledParameter> = Vec::new();
    let mut clock_writes: Vec<(Entity, ChannelClock)> = Vec::new();

    for &entity in plan.channels.iter().take(settings.max_channels) {
        // The plan may be a tick stale on removal — a dead entity or a stripped
        // `Parameter` is simply skipped (the dirty flag is already set).
        let Ok(entity_ref) = world.get_entity(entity) else {
            continue;
        };
        let Some(param) = entity_ref.get::<Parameter>() else {
            continue;
        };
        if !param.enabled || param.name.is_empty() {
            continue;
        }
        let binding = entity_ref.get::<TimeBinding>();

        // The channel's OWN time. This is the whole clock-binding feature.
        let t = domain_time(resolved_domains, binding, &world_time);

        // Due check BEFORE any clone or read: a not-due channel costs two
        // component lookups and nothing else.
        if let Some(clock) = entity_ref.get::<ChannelClock>() {
            if t < clock.next_due_t {
                continue;
            }
        }

        let mut clock = entity_ref
            .get::<ChannelClock>()
            .map(clone_clock)
            .unwrap_or_else(|| {
                // First sight of this channel: due immediately.
                ChannelClock {
                    next_due_t: t,
                    ..Default::default()
                }
            });

        let rate = effective_rate(param, &settings);
        let measured = param.target.unwrap_or(entity);
        let Some(value) = read_value(world, measured, param, &mut clock) else {
            // Unreadable (port not resolvable, bad reflect path, unsupported type).
            // Still advance the clock so a broken channel doesn't retry every tick.
            advance(&mut clock, t, rate);
            clock_writes.push((entity, clock));
            continue;
        };

        // Deadband: numeric values that haven't moved don't get sent.
        let numeric = numeric_of(&value);
        let suppressed = match (param.deadband, numeric, clock.last_emitted) {
            (Some(db), Some(v), Some(last)) => (v - last).abs() < db,
            _ => false,
        };

        if !suppressed {
            if let Some(v) = numeric {
                clock.last_emitted = Some(v);
            }
            samples.push(SampledParameter {
                channel: entity,
                name: param.name.clone(),
                value,
                unit: param.unit.clone(),
                timestamp: world_time.epoch_jd,
                sim_secs: t,
                // The MEASURED entity, not the channel entity — "whose value is this" is what
                // a subscriber needs to tell two rovers' `motor_current` apart.
                source: measured,
            });
        }

        advance(&mut clock, t, rate);
        clock_writes.push((entity, clock));
    }

    if let Some(r) = resolved_taken {
        world.insert_resource(r);
    }
    if plan_is_owned {
        world.insert_resource(plan);
    }

    for (entity, clock) in clock_writes {
        if let Ok(mut e) = world.get_entity_mut(entity) {
            e.insert(clock);
        }
    }

    for sample in samples {
        world.trigger(sample);
    }
}

/// Requested rate, clamped to the fixed step.
///
/// You cannot sample faster than the schedule that does the sampling. Asking for more
/// doesn't oversample — it aliases, silently, which is worse than being told.
fn effective_rate(param: &Parameter, settings: &TelemetrySettings) -> f64 {
    let requested = param.rate_hz.unwrap_or(settings.default_rate_hz);
    if !requested.is_finite() || requested <= 0.0 {
        return settings.default_rate_hz;
    }
    if requested > lunco_core::FIXED_HZ {
        warn_once!(
            "telemetry: channel '{}' requested {} Hz but the fixed step is {} Hz — \
             clamping. A faster rate would alias, not oversample.",
            param.name,
            requested,
            lunco_core::FIXED_HZ
        );
        return lunco_core::FIXED_HZ;
    }
    requested
}

/// Advance the due time by one period, never into the past.
///
/// The `max(t)` clamp is load-bearing: after a pause, a seek, or a warp the domain time
/// can jump far ahead, and a naive `next += period` would then fire a burst of catch-up
/// samples for time that never elapsed. A sampled signal has no backlog.
fn advance(clock: &mut ChannelClock, t: f64, rate: f64) {
    clock.next_due_t = (clock.next_due_t + 1.0 / rate).max(t);
}

fn clone_clock(c: &ChannelClock) -> ChannelClock {
    ChannelClock {
        next_due_t: c.next_due_t,
        last_emitted: c.last_emitted,
        resolved: c.resolved,
        resolve_failed: c.resolve_failed,
    }
}

fn numeric_of(v: &TelemetryValue) -> Option<f64> {
    match v {
        TelemetryValue::F64(f) => Some(*f),
        TelemetryValue::I64(i) => Some(*i as f64),
        _ => None,
    }
}

fn read_value(
    world: &World,
    entity: Entity,
    param: &Parameter,
    clock: &mut ChannelClock,
) -> Option<TelemetryValue> {
    match &param.source {
        ChannelSource::Port(name) => read_port(world, entity, name, clock),
        ChannelSource::Reflect(path) => read_reflect(world, entity, path),
        ChannelSource::Diagnostic(path) => read_diagnostic(world, path),
    }
}

/// Diagnostic source — the engine's own health (FPS, frame time, entity count) as a
/// telemetry channel.
///
/// Reads the SMOOTHED value: a diagnostic's raw per-frame value is spiky by nature (one
/// slow frame is not a change in frame rate), and a subscriber plotting FPS wants the
/// trend. Anything that genuinely needs the spikes is looking at frame *time*, which the
/// perf HUD reads raw from the same store.
///
/// Not entity-scoped — a diagnostic is global. The channel still carries its owning entity
/// so it keys and plots like any other, but the entity is just where you hung the tag.
fn read_diagnostic(world: &World, path: &str) -> Option<TelemetryValue> {
    let diags = world.get_resource::<bevy::diagnostic::DiagnosticsStore>()?;
    // Matched by string, so a domain crate can name a diagnostic without depending on
    // whichever crate registered it. `iter()` is over a handful of entries.
    let d = diags.iter().find(|d| d.path().as_str() == path)?;
    d.smoothed().map(TelemetryValue::F64)
}

/// Port source — resolve the name ONCE, then read by slot forever.
fn read_port(
    world: &World,
    entity: Entity,
    name: &str,
    clock: &mut ChannelClock,
) -> Option<TelemetryValue> {
    let registry = world.get_resource::<PortRegistry>()?;

    if let Some(r) = clock.resolved {
        if let Some(v) = registry.read_resolved(world, entity, r) {
            return Some(TelemetryValue::F64(v));
        }
        // The slot went dead (component removed). Fall through and re-resolve once.
        clock.resolved = None;
    }

    if clock.resolve_failed {
        // Already scanned every backend and came up empty — don't do it again at the
        // sample rate. A re-authored `Parameter` (Changed) clears this, since the
        // clock is keyed to the entity and reset when the channel is re-added.
        return registry
            .read_port(world, entity, name)
            .map(TelemetryValue::F64);
    }

    if let Some(r) = registry.resolve_output(world, entity, name) {
        clock.resolved = Some(r);
        return registry
            .read_resolved(world, entity, r)
            .map(TelemetryValue::F64);
    }

    // Not a resolvable output — it may still be a readable input, or simply absent.
    match registry.read_port(world, entity, name) {
        Some(v) => Some(TelemetryValue::F64(v)),
        None => {
            warn_once!("telemetry: port '{name}' not found on {entity} — channel will stay silent");
            clock.resolve_failed = true;
            None
        }
    }
}

/// Reflection source — the escape hatch. Reaches any registered component field, and is
/// the only source that can carry `Bool`/`String`.
fn read_reflect(world: &World, entity: Entity, path: &str) -> Option<TelemetryValue> {
    if path.is_empty() {
        return None;
    }
    let registry = world.get_resource::<AppTypeRegistry>()?.read();

    let mut parts = path.split('.');
    let component_name = parts.next().unwrap_or("");
    let field_path = parts.collect::<Vec<&str>>().join(".");

    let reg = registry.get_with_short_type_path(component_name)?;
    let reflect_component = reg.data::<ReflectComponent>()?;
    let entity_ref = world.get_entity(entity).ok()?;
    let reflect_data = reflect_component.reflect(entity_ref)?;

    let target: &dyn PartialReflect = if field_path.is_empty() {
        reflect_data.as_partial_reflect()
    } else {
        reflect_data.reflect_path(field_path.as_str()).ok()?
    };

    if let Some(v) = target.try_downcast_ref::<f32>() {
        Some(TelemetryValue::F64(*v as f64))
    } else if let Some(v) = target.try_downcast_ref::<f64>() {
        Some(TelemetryValue::F64(*v))
    } else if let Some(v) = target.try_downcast_ref::<i16>() {
        Some(TelemetryValue::I64(*v as i64))
    } else if let Some(v) = target.try_downcast_ref::<i32>() {
        Some(TelemetryValue::I64(*v as i64))
    } else if let Some(v) = target.try_downcast_ref::<i64>() {
        Some(TelemetryValue::I64(*v))
    } else if let Some(v) = target.try_downcast_ref::<bool>() {
        Some(TelemetryValue::Bool(*v))
    } else {
        target
            .try_downcast_ref::<String>()
            .map(|v| TelemetryValue::String(v.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::time::TimeUpdateStrategy;
    use lunco_core::architecture::Port;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[derive(Component)]
    struct TestOutput(f64);

    fn list_test_output(world: &World, entity: Entity, out: &mut Vec<lunco_core::ports::PortRef>) {
        if let Some(source) = world.get::<TestOutput>(entity) {
            out.push(lunco_core::ports::PortRef {
                name: "value".to_string(),
                direction: PortDirection::Out,
                value: source.0,
            });
        }
    }

    fn read_test_output(world: &World, entity: Entity, name: &str) -> Option<f64> {
        (name == "value").then(|| world.get::<TestOutput>(entity).map(|source| source.0))?
    }

    const TEST_OUTPUT_BACKEND: lunco_core::ports::PortBackend = lunco_core::ports::PortBackend {
        list: list_test_output,
        read_output: read_test_output,
        read_input: |_, _, _| None,
        write_input: |_, _, _, _| false,
        resolve_output: None,
        resolve_input: None,
        read_slot: None,
        write_slot: None,
    };

    /// 20 ms per update against the 64 Hz default fixed step (~15.6 ms).
    const UPDATE_MS: u64 = 20;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            lunco_core::LunCoCorePlugin,
            LunCoTelemetryPlugin,
        ));
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            UPDATE_MS,
        )));
        // `lunco-settings` refuses disk I/O in a test binary (see its `disk_backed`), so
        // this app's settings are in-memory defaults and CANNOT reach the developer's real
        // `~/.lunco/settings.json`. Asserted belt-and-braces: the master-switch test below
        // writes `enabled: false`, and that once escaped into the real config.
        app.insert_resource(TelemetrySettings::default());
        app
    }

    /// Advance `n` FIXED steps.
    ///
    /// NOT the same as `n` calls to `app.update()`: the fixed accumulator starts empty,
    /// so the first update banks 20 ms without crossing the 15.6 ms boundary and runs
    /// `FixedUpdate` ZERO times. Asserting after one update tests nothing and looks
    /// like a product bug — it isn't.
    fn step_fixed(app: &mut App, n: usize) {
        for _ in 0..=n {
            app.update();
        }
    }

    fn capture(app: &mut App) -> Arc<Mutex<Vec<SampledParameter>>> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        app.add_observer(move |trigger: On<SampledParameter>| {
            sink.lock().unwrap().push(trigger.event().clone());
        });
        seen
    }

    fn reflect_channel(name: &str) -> Parameter {
        Parameter {
            name: name.to_string(),
            unit: "A".to_string(),
            source: ChannelSource::Reflect("Port.value".to_string()),
            ..Default::default()
        }
    }

    /// End to end: a tagged field becomes a `SampledParameter` — exactly what the API's
    /// `SubscribeTelemetry` observer is already wired to receive.
    #[test]
    fn a_parameter_tag_turns_a_live_field_into_telemetry() {
        let mut app = app();
        let seen = capture(&mut app);
        let e = app
            .world_mut()
            .spawn((
                Port { value: 42.0 },
                Parameter {
                    rate_hz: Some(lunco_core::FIXED_HZ),
                    ..reflect_channel("motor_current")
                },
            ))
            .id();

        step_fixed(&mut app, 1);

        let seen = seen.lock().unwrap();
        let s = seen
            .first()
            .expect("a tagged parameter must produce a sample");
        assert_eq!(s.name, "motor_current");
        assert_eq!(s.unit, "A");
        assert_eq!(s.value, TelemetryValue::F64(42.0));
        assert_eq!(
            s.source, e,
            "the sample must carry its owning entity — names collide"
        );
    }

    /// `enabled` defaults to TRUE. `ReflectDefault` builds the component from `Default`
    /// and then patches named fields, so a script that omits `enabled` must get a live
    /// channel — not a silently dead one.
    #[test]
    fn a_channel_is_enabled_by_default() {
        assert!(Parameter::default().enabled);
    }

    #[test]
    fn a_live_port_is_discovered_without_a_usd_channel_declaration() {
        let mut app = app();
        app.init_resource::<PortRegistry>();
        app.world_mut()
            .resource_mut::<PortRegistry>()
            .register(TEST_OUTPUT_BACKEND);
        let source = app.world_mut().spawn(TestOutput(7.5)).id();
        // Discovery runs before the normal cached sampler in the same fixed
        // schedule; a plain runtime port becomes observable without Parameter
        // or USD telemetry data on the source entity.
        step_fixed(&mut app, 2);
        let signal = lunco_signal::SignalRef::new(source, "value".to_string());
        let history = app
            .world()
            .resource::<lunco_signal::SignalRegistry>()
            .scalar_history(&signal)
            .expect("runtime port telemetry must exist");
        assert_eq!(history.samples.back().unwrap().value, 7.5);
    }

    /// Runtime discovery is for the user-facing simulation surface, not the
    /// thousands of transient terrain/collider implementation entities. Those
    /// entities remain observable when an author deliberately adds a
    /// `Parameter`, but must not silently grow the global sampling plan.
    #[test]
    fn system_managed_ports_are_not_auto_discovered_as_telemetry() {
        let mut app = app();
        app.init_resource::<PortRegistry>();
        app.world_mut()
            .resource_mut::<PortRegistry>()
            .register(TEST_OUTPUT_BACKEND);
        app.world_mut()
            .spawn((TestOutput(7.5), lunco_core::SystemManaged));

        step_fixed(&mut app, 2);

        let mut channels = app.world_mut().query::<&RuntimePortChannel>();
        assert_eq!(
            channels.iter(app.world()).count(),
            0,
            "system-managed entities must not become implicit telemetry channels"
        );
    }

    /// A sample distinguishes the measured source from the channel that chose
    /// its cadence and retention. The latter lets retention use direct lookup
    /// even when many channel entities target the same rover.
    #[test]
    fn samples_carry_their_producing_channel_entity() {
        let mut app = app();
        let seen = capture(&mut app);
        let rover = app.world_mut().spawn(Port { value: 3.0 }).id();
        let channel = app
            .world_mut()
            .spawn(Parameter {
                rate_hz: Some(lunco_core::FIXED_HZ),
                target: Some(rover),
                ..reflect_channel("direct")
            })
            .id();

        step_fixed(&mut app, 1);

        let sample = seen
            .lock()
            .unwrap()
            .first()
            .cloned()
            .expect("the channel must sample its target");
        assert_eq!(sample.source, rover);
        assert_eq!(sample.channel, channel);
    }

    #[test]
    fn a_disabled_channel_emits_nothing() {
        let mut app = app();
        let seen = capture(&mut app);
        app.world_mut().spawn((
            Port { value: 1.0 },
            Parameter {
                enabled: false,
                ..reflect_channel("off")
            },
        ));
        step_fixed(&mut app, 8);
        assert!(seen.lock().unwrap().is_empty());
    }

    /// The sampler is exclusive — a scene with no channels must not pay for it.
    #[test]
    fn a_world_with_no_parameters_never_runs_the_sampler() {
        let mut app = app();
        let seen = capture(&mut app);
        app.world_mut().spawn(Port { value: 42.0 });
        step_fixed(&mut app, 4);
        assert!(
            seen.lock().unwrap().is_empty(),
            "nothing tagged ⇒ nothing sampled"
        );
    }

    /// THE PHASE-1 PROPERTY: rate is PER CHANNEL. A 60 Hz channel and a 10 Hz channel in
    /// the same world must produce different sample counts. Before this, every channel
    /// sampled at FIXED_HZ and there was no rate field at all.
    #[test]
    fn each_channel_samples_at_its_own_rate() {
        let mut app = app();
        let seen = capture(&mut app);

        app.world_mut().spawn((
            Port { value: 1.0 },
            Parameter {
                rate_hz: Some(lunco_core::FIXED_HZ),
                ..reflect_channel("fast")
            },
        ));
        app.world_mut().spawn((
            Port { value: 1.0 },
            Parameter {
                rate_hz: Some(6.0),
                ..reflect_channel("slow")
            },
        ));

        // ~1 second of sim: 64 fixed steps.
        step_fixed(&mut app, 64);

        let seen = seen.lock().unwrap();
        let fast = seen.iter().filter(|s| s.name == "fast").count();
        let slow = seen.iter().filter(|s| s.name == "slow").count();

        assert!(
            fast > 50,
            "a FIXED_HZ channel should sample near every step, got {fast}"
        );
        assert!(
            (4..=9).contains(&slow),
            "a 6 Hz channel should sample ~6× in a sim-second, got {slow}"
        );
        assert!(
            fast > slow * 4,
            "the rates must actually differ: fast={fast} slow={slow}"
        );
    }

    /// A rate above the fixed step cannot be honoured — it must clamp, not alias.
    #[test]
    fn a_rate_above_the_fixed_step_is_clamped() {
        let p = Parameter {
            rate_hz: Some(10_000.0),
            ..reflect_channel("greedy")
        };
        let rate = effective_rate(&p, &TelemetrySettings::default());
        assert_eq!(rate, lunco_core::FIXED_HZ);
    }

    /// A non-positive or non-finite rate falls back to the default rather than dividing
    /// by zero into an infinite due-time.
    #[test]
    fn a_nonsense_rate_falls_back_to_the_default() {
        let s = TelemetrySettings::default();
        for bad in [0.0, -5.0, f64::NAN, f64::INFINITY] {
            let p = Parameter {
                rate_hz: Some(bad),
                ..reflect_channel("bad")
            };
            assert_eq!(
                effective_rate(&p, &s),
                s.default_rate_hz,
                "rate {bad} must fall back"
            );
        }
    }

    /// Deadband: a value that isn't moving costs nothing.
    #[test]
    fn a_deadband_suppresses_an_unchanged_value() {
        let mut app = app();
        let seen = capture(&mut app);
        let e = app
            .world_mut()
            .spawn((
                Port { value: 1.0 },
                Parameter {
                    rate_hz: Some(lunco_core::FIXED_HZ),
                    deadband: Some(0.5),
                    ..reflect_channel("steady")
                },
            ))
            .id();

        step_fixed(&mut app, 10);
        let after_steady = seen.lock().unwrap().len();
        assert_eq!(
            after_steady, 1,
            "an unchanging value emits ONCE, then goes quiet"
        );

        // Move it past the deadband.
        app.world_mut()
            .entity_mut(e)
            .get_mut::<Port>()
            .unwrap()
            .value = 9.0;
        step_fixed(&mut app, 2);
        assert!(
            seen.lock().unwrap().len() > after_steady,
            "a move beyond the deadband must emit again"
        );
    }

    /// A move SMALLER than the deadband stays suppressed — otherwise the deadband is
    /// just a first-sample filter and buys nothing.
    #[test]
    fn a_move_below_the_deadband_stays_suppressed() {
        let mut app = app();
        let seen = capture(&mut app);
        let e = app
            .world_mut()
            .spawn((
                Port { value: 1.0 },
                Parameter {
                    rate_hz: Some(lunco_core::FIXED_HZ),
                    deadband: Some(1.0),
                    ..reflect_channel("jitter")
                },
            ))
            .id();

        step_fixed(&mut app, 4);
        let baseline = seen.lock().unwrap().len();

        app.world_mut()
            .entity_mut(e)
            .get_mut::<Port>()
            .unwrap()
            .value = 1.2;
        step_fixed(&mut app, 4);
        assert_eq!(
            seen.lock().unwrap().len(),
            baseline,
            "a 0.2 move under a 1.0 deadband is noise"
        );
    }

    /// PHASE 2: samples land in the `SignalRegistry` ring buffer — the same store every
    /// plot surface already reads. Retention and plotting come from one wire-up.
    #[test]
    fn samples_are_retained_in_the_signal_ring_buffer() {
        let mut app = app();
        let e = app
            .world_mut()
            .spawn((
                Port { value: 3.0 },
                Parameter {
                    rate_hz: Some(lunco_core::FIXED_HZ),
                    ..reflect_channel("retained")
                },
            ))
            .id();

        step_fixed(&mut app, 5);

        let signals = app.world().resource::<lunco_signal::SignalRegistry>();
        let sig = lunco_signal::SignalRef::new(e, "retained".to_string());
        let hist = signals
            .scalar_history(&sig)
            .expect("the sample must be retained");
        assert!(
            hist.len() >= 2,
            "several fixed steps ⇒ several retained samples"
        );
        assert_eq!(hist.iter().next().unwrap().value, 3.0);
        // Unit metadata rides along so a plot can label its axis.
        assert_eq!(signals.meta(&sig).unwrap().unit.as_deref(), Some("A"));
    }

    /// Retention is PER CHANNEL and bounds memory: a channel capped at N samples must
    /// never hold more, however long it runs.
    #[test]
    fn retention_bounds_the_ring_buffer() {
        let mut app = app();
        let e = app
            .world_mut()
            .spawn((
                Port { value: 1.0 },
                Parameter {
                    rate_hz: Some(lunco_core::FIXED_HZ),
                    retention: Some(3),
                    ..reflect_channel("capped")
                },
            ))
            .id();

        step_fixed(&mut app, 20);

        let signals = app.world().resource::<lunco_signal::SignalRegistry>();
        let sig = lunco_signal::SignalRef::new(e, "capped".to_string());
        assert_eq!(
            signals.scalar_history(&sig).unwrap().len(),
            3,
            "a retention of 3 must hold exactly 3, no matter how many steps elapse"
        );
    }

    /// A despawn turns a channel inactive; mission history remains available for
    /// graphing and export after the live source is gone.
    #[test]
    fn despawning_a_channel_retains_inactive_history() {
        let mut app = app();
        let e = app
            .world_mut()
            .spawn((
                Port { value: 1.0 },
                Parameter {
                    rate_hz: Some(lunco_core::FIXED_HZ),
                    ..reflect_channel("doomed")
                },
            ))
            .id();
        step_fixed(&mut app, 3);
        let sig = lunco_signal::SignalRef::new(e, "doomed".to_string());
        assert!(app
            .world()
            .resource::<lunco_signal::SignalRegistry>()
            .scalar_history(&sig)
            .is_some());

        app.world_mut().entity_mut(e).despawn();
        app.update();

        assert!(
            app.world()
                .resource::<lunco_signal::SignalRegistry>()
                .scalar_history(&sig)
                .is_some(),
            "a despawned channel retains its mission history"
        );
        assert!(
            !app.world()
                .resource::<lunco_signal::SignalRegistry>()
                .is_active(&sig),
            "the source must be marked inactive"
        );
    }

    /// PHASE 3: one command controls the subsystem. `None` fields leave things unchanged.
    #[test]
    fn control_telemetry_retunes_a_named_channel() {
        let mut app = app();
        app.world_mut().spawn((
            Port { value: 1.0 },
            Parameter {
                rate_hz: Some(60.0),
                ..reflect_channel("tunable")
            },
        ));
        app.update();

        app.world_mut().trigger(ControlTelemetry {
            channel: Some("tunable".to_string()),
            rate_hz: Some(2.0),
            retention: Some(50),
            ..Default::default()
        });
        app.update();

        let mut q = app.world_mut().query::<&Parameter>();
        let p = q.iter(app.world()).find(|p| p.name == "tunable").unwrap();
        assert_eq!(p.rate_hz, Some(2.0));
        assert_eq!(p.retention, Some(50));
        assert!(p.enabled, "an untouched (None) field must be left alone");
    }

    /// `channel: None` addresses the SUBSYSTEM — the master switch, not a channel.
    #[test]
    fn control_telemetry_with_no_channel_sets_subsystem_defaults() {
        let mut app = app();
        app.update();

        app.world_mut().trigger(ControlTelemetry {
            channel: None,
            enabled: Some(false),
            rate_hz: Some(4.0),
            ..Default::default()
        });
        app.update();

        let s = app.world().resource::<TelemetrySettings>();
        assert!(!s.enabled);
        assert_eq!(s.default_rate_hz, 4.0);
    }

    /// The master switch actually stops sampling — not just a flag nobody reads.
    #[test]
    fn disabling_the_subsystem_stops_every_channel() {
        let mut app = app();
        let seen = capture(&mut app);
        app.world_mut().spawn((
            Port { value: 1.0 },
            Parameter {
                rate_hz: Some(lunco_core::FIXED_HZ),
                ..reflect_channel("live")
            },
        ));
        step_fixed(&mut app, 3);
        assert!(!seen.lock().unwrap().is_empty());

        app.world_mut().resource_mut::<TelemetrySettings>().enabled = false;
        let before = seen.lock().unwrap().len();
        step_fixed(&mut app, 8);
        assert_eq!(
            seen.lock().unwrap().len(),
            before,
            "the master switch must actually stop it"
        );
    }

    /// A client can now AUTHOR a channel through the API — the thing whose absence forced
    /// every external watcher to poll from outside (the MCP `watch_ports` loop).
    #[test]
    fn control_telemetry_can_create_a_channel_on_an_entity() {
        let mut app = app();
        let seen = capture(&mut app);
        let e = app.world_mut().spawn(Port { value: 7.0 }).id();

        app.world_mut().trigger(ControlTelemetry {
            channel: Some("watched".to_string()),
            entity: Some(e),
            reflect: Some("Port.value".to_string()),
            unit: Some("A".to_string()),
            rate_hz: Some(lunco_core::FIXED_HZ),
            ..Default::default()
        });
        step_fixed(&mut app, 2);

        // The channel is its OWN entity targeting the rover — not a component on it, because a
        // Component would cap the rover at one channel.
        let mut q = app.world_mut().query::<&Parameter>();
        let p = q
            .iter(app.world())
            .find(|p| p.name == "watched")
            .expect("the channel must be authored");
        assert_eq!(
            p.target,
            Some(e),
            "the channel must point at what it measures"
        );
        assert!(p.enabled, "a channel someone explicitly asked for is live");

        let seen = seen.lock().unwrap();
        let s = seen
            .first()
            .expect("the created channel must actually sample");
        assert_eq!(s.value, TelemetryValue::F64(7.0));
        assert_eq!(s.source, e);
    }

    /// Re-pointing a channel at a different source must NOT inherit the old one's cached
    /// port handle or deadband reference — a stale `ResolvedPort` would read the wrong slot.
    #[test]
    fn recreating_a_channel_drops_its_stale_clock_state() {
        let mut app = app();
        let e = app.world_mut().spawn(Port { value: 1.0 }).id();
        app.world_mut().trigger(ControlTelemetry {
            channel: Some("c".to_string()),
            entity: Some(e),
            reflect: Some("Port.value".to_string()),
            rate_hz: Some(lunco_core::FIXED_HZ),
            ..Default::default()
        });
        step_fixed(&mut app, 3);
        let chan = {
            let mut q = app.world_mut().query::<(Entity, &Parameter)>();
            q.iter(app.world())
                .find(|(_, p)| p.name == "c")
                .map(|(e, _)| e)
                .expect("channel entity")
        };
        assert!(app.world().entity(chan).contains::<ChannelClock>());

        // Re-point the SAME channel at a different source.
        app.world_mut().trigger(ControlTelemetry {
            channel: Some("c".to_string()),
            entity: Some(e),
            port: Some("some_port".to_string()),
            ..Default::default()
        });
        app.update();

        let mut q = app.world_mut().query::<(Entity, &Parameter)>();
        let n = q.iter(app.world()).filter(|(_, p)| p.name == "c").count();
        assert_eq!(
            n, 1,
            "a re-point must retune the channel, not spawn a second one"
        );

        // The sampler legitimately re-adds a FRESH clock — what must not survive is the OLD
        // one's state: a `ResolvedPort` pointing at the previous source's slot, and a deadband
        // reference taken from a value this channel no longer reads.
        if let Some(clock) = app.world().entity(chan).get::<ChannelClock>() {
            assert!(
                clock.resolved.is_none(),
                "a stale resolved port slot must not survive a re-point"
            );
            assert!(
                clock.last_emitted.is_none(),
                "a stale deadband reference must not survive a re-point"
            );
        }
        assert!(matches!(
            app.world().entity(chan).get::<Parameter>().unwrap().source,
            ChannelSource::Port(_)
        ));
    }

    /// THE REASON A CHANNEL CAN TARGET ANOTHER ENTITY. `Parameter` is a Component, so an
    /// entity carries at most ONE — putting the channel on the rover would cap the rover at a
    /// single watched value. "Watch three ports on this rover" must be representable.
    #[test]
    fn one_entity_can_carry_many_channels() {
        let mut app = app();
        let seen = capture(&mut app);
        let rover = app.world_mut().spawn(Port { value: 5.0 }).id();

        for name in ["a", "b", "c"] {
            app.world_mut().trigger(ControlTelemetry {
                channel: Some(name.to_string()),
                entity: Some(rover),
                reflect: Some("Port.value".to_string()),
                rate_hz: Some(lunco_core::FIXED_HZ),
                ..Default::default()
            });
        }
        step_fixed(&mut app, 2);

        let seen = seen.lock().unwrap();
        for name in ["a", "b", "c"] {
            let s = seen
                .iter()
                .find(|s| s.name == name)
                .unwrap_or_else(|| panic!("channel {name} must sample"));
            assert_eq!(
                s.source, rover,
                "every channel must report the MEASURED entity"
            );
            assert_eq!(s.value, TelemetryValue::F64(5.0));
        }

        // …and each keeps its own ring buffer, keyed by (measured entity, name).
        let signals = app.world().resource::<lunco_signal::SignalRegistry>();
        for name in ["a", "b", "c"] {
            assert!(
                signals
                    .scalar_history(&lunco_signal::SignalRef::new(rover, name.to_string()))
                    .is_some(),
                "channel {name} must retain its own history"
            );
        }
    }

    /// Removing one channel keeps its own history inactive and does not affect
    /// siblings on the same rover.
    #[test]
    fn removing_one_channel_leaves_its_siblings_alone() {
        let mut app = app();
        let rover = app.world_mut().spawn(Port { value: 2.0 }).id();
        for name in ["keep", "drop"] {
            app.world_mut().trigger(ControlTelemetry {
                channel: Some(name.to_string()),
                entity: Some(rover),
                reflect: Some("Port.value".to_string()),
                rate_hz: Some(lunco_core::FIXED_HZ),
                ..Default::default()
            });
        }
        step_fixed(&mut app, 2);

        let doomed = {
            let mut q = app.world_mut().query::<(Entity, &Parameter)>();
            q.iter(app.world())
                .find(|(_, p)| p.name == "drop")
                .map(|(e, _)| e)
                .expect("channel entity")
        };
        app.world_mut().entity_mut(doomed).despawn();
        app.update();

        let signals = app.world().resource::<lunco_signal::SignalRegistry>();
        assert!(
            signals.scalar_history(&lunco_signal::SignalRef::new(rover, "keep".to_string())).is_some(),
            "a sibling channel's history must survive — this is why removal is per-signal, not drop_entity"
        );
        assert!(
            signals
                .scalar_history(&lunco_signal::SignalRef::new(rover, "drop".to_string()))
                .is_some(),
            "the removed channel's history remains available"
        );
        assert!(
            !signals.is_active(&lunco_signal::SignalRef::new(rover, "drop".to_string())),
            "the removed channel is inactive rather than erased"
        );
    }

    /// PHASE 5: a bevy `Diagnostic` is a telemetry channel. FPS stops being a number that
    /// can only ever reach a HUD, and becomes subscribable / retained / plottable /
    /// queryable like any other channel.
    #[test]
    fn a_diagnostic_can_be_a_telemetry_channel() {
        use bevy::diagnostic::{Diagnostic, DiagnosticPath, DiagnosticsStore};

        let mut app = app();
        let seen = capture(&mut app);
        app.init_resource::<DiagnosticsStore>();
        const PATH: DiagnosticPath = DiagnosticPath::const_new("fps");
        {
            let mut store = app.world_mut().resource_mut::<DiagnosticsStore>();
            store.add(Diagnostic::new(PATH));
            store.get_mut(&PATH).unwrap().add_measurement(
                bevy::diagnostic::DiagnosticMeasurement {
                    time: std::time::Instant::now(),
                    value: 59.5,
                },
            );
        }
        app.world_mut().spawn(Parameter {
            name: "engine.fps".to_string(),
            unit: "1/s".to_string(),
            source: ChannelSource::Diagnostic("fps".to_string()),
            rate_hz: Some(lunco_core::FIXED_HZ),
            ..Default::default()
        });

        step_fixed(&mut app, 2);

        let seen = seen.lock().unwrap();
        let s = seen
            .first()
            .expect("a diagnostic-sourced channel must emit");
        assert_eq!(s.name, "engine.fps");
        assert_eq!(s.value, TelemetryValue::F64(59.5));
    }

    /// A diagnostic that doesn't exist must not spam or panic — the channel is simply
    /// silent. (A headless server links `bevy_diagnostic` but nobody adds
    /// `FrameTimeDiagnosticsPlugin` there.)
    #[test]
    fn a_missing_diagnostic_is_silent_not_fatal() {
        let mut app = app();
        let seen = capture(&mut app);
        app.world_mut().spawn(Parameter {
            name: "engine.fps".to_string(),
            source: ChannelSource::Diagnostic("fps".to_string()),
            rate_hz: Some(lunco_core::FIXED_HZ),
            ..Default::default()
        });
        step_fixed(&mut app, 4);
        assert!(seen.lock().unwrap().is_empty());
    }

    /// …and with no FPS diagnostic registered, no engine-health channel is published at
    /// all — a `--no-ui` run must not carry an always-silent channel in its catalog.
    #[test]
    fn engine_health_channels_are_not_published_without_diagnostics() {
        let mut app = app();
        app.update();
        let mut q = app.world_mut().query::<&EngineHealthChannel>();
        assert_eq!(q.iter(app.world()).count(), 0);
    }

    /// Samples carry `sim_secs` — the timebase you can actually difference. `timestamp`
    /// (Julian Date) has ~86 µs of f64 resolution left and must not be used for Δt.
    #[test]
    fn samples_carry_a_precise_simulation_timebase() {
        let mut app = app();
        let seen = capture(&mut app);
        app.world_mut().spawn((
            Port { value: 1.0 },
            Parameter {
                rate_hz: Some(lunco_core::FIXED_HZ),
                ..reflect_channel("t")
            },
        ));

        step_fixed(&mut app, 6);

        let seen = seen.lock().unwrap();
        assert!(seen.len() >= 2);
        let dt = seen[1].sim_secs - seen[0].sim_secs;
        assert!(
            dt > 0.0 && dt < 1.0,
            "consecutive samples must be separated by a real, positive Δt, got {dt}"
        );
    }
}
