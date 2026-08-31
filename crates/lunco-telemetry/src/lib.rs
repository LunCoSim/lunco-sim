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
use lunco_core::ports::{PortRegistry, ResolvedPort};
use lunco_core::telemetry::{ChannelSource, Parameter, SampledParameter, TelemetryValue};
use lunco_core::{on_command, register_commands, Command};
use lunco_settings::{AppSettingsExt, SettingsSection};
use lunco_signal::TelemetryDeadband;
use lunco_time::{domain_time, ResolvedDomains, TimeBinding, WorldTime};
use serde::{Deserialize, Serialize};

/// Persisted telemetry defaults. Stored under the `"telemetry"` key of
/// `settings.json`.
#[derive(Resource, Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(deny_unknown_fields)]
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
    /// Costs 16 B per sample: 24 KB per channel, ~96 MB at the 4096-channel cap.
    pub default_retention: usize,
    /// Master switch.
    pub enabled: bool,
    /// Default numeric visibility policy for channels without an explicit
    /// `Parameter::deadband`.  This is deliberately separate from Modelica's
    /// solver tolerances: it governs operator-facing samples, not convergence.
    pub default_deadband: TelemetryDeadband,
}

impl Default for TelemetrySettings {
    fn default() -> Self {
        Self {
            // 5 Hz for 5 minutes — the default window for an explicit telemetry
            // channel. Model state uses this policy through the Modelica runtime
            // projection; authored channels use it when they omit their own rate.
            default_rate_hz: 5.0,
            // The Traverse scene publishes more than 2048 physics and Modelica
            // channels once its remote stations and antenna mechanisms are live.
            // Keep the complete scene plus headroom so ordinary telemetry is not
            // silently truncated by enumeration order.
            max_channels: 4096,
            default_retention: 1500,
            enabled: true,
            default_deadband: TelemetryDeadband::default(),
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
    /// Absolute tolerance for the subsystem default numeric deadband. Applies
    /// only when `channel` is `None`; a named channel uses `deadband` as its
    /// explicit absolute override.
    pub atol: Option<f64>,
    /// Relative tolerance for the subsystem default numeric deadband. Applies
    /// only when `channel` is `None`.
    pub rtol: Option<f64>,
    pub deadband: Option<f64>,
}

#[on_command(ControlTelemetry)]
fn on_control_telemetry(
    trigger: On<ControlTelemetry>,
    mut settings: ResMut<TelemetrySettings>,
    // ONE query: a second `Query<&Parameter>` alongside this `&mut` one is a conflicting
    // access and panics at run time (B0001).
    mut channels: Query<(Entity, &mut Parameter)>,
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
            let rate_hz = match cmd.rate_hz {
                Some(rate) => {
                    let Some(rate) = accepted_command_rate(rate, "new channel") else {
                        return;
                    };
                    Some(rate)
                }
                None => None,
            };
            let deadband = match cmd.deadband {
                Some(deadband) => {
                    let Some(deadband) = accepted_channel_deadband(deadband, "new channel") else {
                        return;
                    };
                    Some(deadband)
                }
                None => None,
            };
            let param = Parameter {
                name,
                unit: cmd.unit.clone().unwrap_or_default(),
                description: None,
                source,
                target: Some(entity),
                rate_hz,
                enabled: cmd.enabled.unwrap_or(true),
                deadband,
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
                .find(|(_, p)| p.name == param.name && p.target == Some(entity))
                .map(|(e, _)| e);
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
            if let Some(rate) = accepted_command_rate(rate, "subsystem default") {
                settings.default_rate_hz = rate;
            }
        }
        if let Some(retention) = cmd.retention {
            settings.default_retention = retention;
        }
        if let Some(atol) = cmd.atol {
            if TelemetryDeadband::is_valid_tolerance(atol) {
                settings.default_deadband.atol = atol;
            } else {
                warn!(
                    atol,
                    "telemetry: ignoring invalid default absolute tolerance"
                );
            }
        }
        if let Some(rtol) = cmd.rtol {
            if TelemetryDeadband::is_valid_tolerance(rtol) {
                settings.default_deadband.rtol = rtol;
            } else {
                warn!(
                    rtol,
                    "telemetry: ignoring invalid default relative tolerance"
                );
            }
        }
        return;
    };

    let deadband = match cmd.deadband {
        Some(deadband) => {
            let Some(deadband) = accepted_channel_deadband(deadband, "named channel") else {
                return;
            };
            Some(deadband)
        }
        None => None,
    };

    let rate_hz = match cmd.rate_hz {
        Some(rate) => {
            let Some(rate) = accepted_command_rate(rate, "named channel") else {
                return;
            };
            Some(rate)
        }
        None => None,
    };

    for (_, mut param) in channels.iter_mut().filter(|(_, p)| p.name == name) {
        if let Some(enabled) = cmd.enabled {
            param.enabled = enabled;
        }
        if let Some(rate) = rate_hz {
            param.rate_hz = Some(rate);
        }
        if let Some(retention) = cmd.retention {
            param.retention = Some(retention);
        }
        if let Some(deadband) = deadband {
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
    /// Value at the last operator-notification sample — the deadband reference.
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

/// Rebuild the sampling plan from the unique signal identity `(measured entity,
/// channel name)`.  Several projections can observe the same port, but the
/// retained registry uses that pair as its key; sampling the same key twice
/// only duplicates work and makes the channel cap lie about useful data.
///
/// Every channel is an explicit recording declaration. Its `Parameter` owns
/// rate, retention, metadata, and source policy.
fn rebuild_sampling_plan(world: &mut World, plan: &mut SamplingPlan) -> usize {
    let mut selected = std::collections::HashMap::<(Entity, String), Entity>::new();
    let mut candidates = 0usize;

    for (entity, parameter) in world.query::<(Entity, &Parameter)>().iter(world) {
        if !parameter.enabled || parameter.name.is_empty() {
            continue;
        }
        candidates += 1;
        let measured = parameter.target.unwrap_or(entity);
        let key = (measured, parameter.name.clone());
        selected.entry(key).or_insert(entity);
    }

    plan.channels = selected.into_values().collect();
    plan.channels.sort_by_key(|entity| entity.to_bits());
    candidates.saturating_sub(plan.channels.len())
}

/// Last metadata applied to a retained signal by a telemetry channel.
///
/// Channel policy changes are rare while samples are continuous. Keeping this
/// tiny cache on the channel means [`retain_sample`] can update plot metadata
/// exactly when that policy changes, rather than allocating and replacing the
/// same strings for every sample.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
struct RetainedSignalMeta {
    signal: lunco_signal::SignalMeta,
}

/// Cheap change detector in front of the exclusive sampler: any added/changed/
/// removed `Parameter` or `TimeBinding` invalidates the plan. Runs on normal
/// (parallel) system params — the archetype-level `Changed` check and the
/// removal inboxes are near-free when nothing changed, which is every tick of
/// steady state.
fn mark_sampling_plan_dirty(
    mut clocks: ParamSet<(
        Query<&mut ChannelClock, Changed<Parameter>>,
        Query<&mut ChannelClock, (With<Parameter>, Changed<TimeBinding>)>,
    )>,
    changed: Query<(), Or<(Changed<Parameter>, Changed<TimeBinding>)>>,
    mut removed_params: RemovedComponents<Parameter>,
    mut removed_bindings: RemovedComponents<TimeBinding>,
    mut plan: ResMut<SamplingPlan>,
) {
    // `ChannelClock` caches source resolution, the due time, and the last emitted
    // value used by deadband. Any authored Parameter or clock-binding change makes
    // all of that state belong to the old declaration, so reset it before sampling.
    for mut clock in clocks.p0().iter_mut() {
        *clock = ChannelClock::default();
    }
    for mut clock in clocks.p1().iter_mut() {
        *clock = ChannelClock::default();
    }

    let removed = removed_params.read().next().is_some() | removed_bindings.read().next().is_some();
    if removed || !changed.is_empty() {
        plan.dirty = true;
    }
}

pub struct LunCoTelemetryPlugin;

impl Plugin for LunCoTelemetryPlugin {
    fn build(&self, app: &mut App) {
        // Telemetry is a simulation subsystem, so it requires the same unified
        // mission-time spine as every other fixed-clock consumer. Do not let a
        // missing clock turn into a second, implicit time contract.
        if !app.is_plugin_added::<lunco_time::TimePlugin>() {
            app.add_plugins(lunco_time::TimePlugin);
        }
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

    // A USD telemetry declaration is the operator-facing name for a measured
    // Modelica port.  Inherit the producer's authored ownership and Modelica
    // identity so the browser can group it with the same component state as
    // the generated solver channel; do not make the UI reverse-engineer the
    // generated wrapper spelling.
    let producer_meta = match &channel.source {
        ChannelSource::Port(port) => signals
            .meta(&lunco_signal::SignalRef::new(s.source, port.clone()))
            .cloned(),
        _ => None,
    };
    let signal_meta = lunco_signal::SignalMeta {
        description: channel.description.clone().or_else(|| {
            producer_meta
                .as_ref()
                .and_then(|meta| meta.description.clone())
        }),
        unit: (!channel.unit.is_empty())
            .then_some(channel.unit.clone())
            .or_else(|| producer_meta.as_ref().and_then(|meta| meta.unit.clone())),
        provenance: Some("telemetry".to_string()),
        group_path: producer_meta
            .as_ref()
            .and_then(|meta| meta.group_path.clone()),
        // An explicit telemetry declaration is a public canonical channel,
        // even when its source is an internal Modelica port.
        exposure: lunco_signal::SignalExposure::Public,
        model_class: producer_meta
            .as_ref()
            .and_then(|meta| meta.model_class.clone()),
        model_variable: producer_meta
            .as_ref()
            .and_then(|meta| meta.model_variable.clone()),
        source_asset: producer_meta
            .as_ref()
            .and_then(|meta| meta.source_asset.clone()),
        canonical_name: Some(channel.name.clone()),
    };
    let expected_meta = RetainedSignalMeta {
        signal: signal_meta.clone(),
    };
    if applied_meta != Some(&expected_meta) {
        signals.update_meta(signal, signal_meta);
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

/// One sampling pass owned by [`LunCoTelemetryPlugin`].
fn sample_parameters(world: &mut World) {
    // These resources are installed by the plugin. A missing one is an integration
    // error, not a reason to invent telemetry settings or a second simulation clock.
    let settings = *world.resource::<TelemetrySettings>();
    if !settings.enabled {
        return;
    }

    // Absolute epoch for wall-clock labelling; the per-channel domain gives the
    // precise timebase (see `SampledParameter::sim_secs`). Both come from the
    // unified mission-time spine installed above.
    let world_time = *world.resource::<WorldTime>();
    // The sampling plan: which entities to visit. Rebuilt ONLY when the channel
    // set changed (see `mark_sampling_plan_dirty`) — steady state pays a Vec
    // walk, not a query + per-channel `Parameter` clone. The plan is taken OUT
    // of the world for the pass (reinserted below) so the loop can borrow
    // `&World` freely. The plugin owns this cache; a missing plan is an
    // integration error rather than a reason to rescan under different semantics.
    let mut plan = world
        .remove_resource::<SamplingPlan>()
        .expect("LunCoTelemetryPlugin requires its SamplingPlan resource");
    if plan.dirty {
        let duplicate_count = rebuild_sampling_plan(world, &mut plan);
        if duplicate_count > 0 {
            debug!(
                duplicate_count,
                "telemetry: collapsed duplicate channel declarations"
            );
        }
        plan.dirty = false;
    }

    // The resolver runs once per frame in `Update`. Take the resource OUT for the
    // pass instead of cloning its HashMap every fixed tick — the loop only reads
    // it, and it is reinserted before any event fires. (The resolver rewrites it
    // every frame anyway, so the remove/insert changes no change-detection story.)
    let resolved_taken = world
        .remove_resource::<ResolvedDomains>()
        .expect("LunCoTelemetryPlugin requires lunco_time::ResolvedDomains");
    let resolved_domains = &resolved_taken;

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
        if let Some(deadband) = param.deadband {
            if !TelemetryDeadband::is_valid_tolerance(deadband) {
                warn_once!(
                    "telemetry: channel '{}' has invalid explicit deadband {}; sampling is skipped until a finite non-negative value is authored",
                    param.name,
                    deadband
                );
                continue;
            }
        } else if !settings.default_deadband.is_valid() {
            warn_once!(
                "telemetry: channel '{}' has no deadband and the subsystem default is invalid; sampling is skipped until the default is corrected",
                param.name
            );
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

        let Some(rate) = effective_rate(param, &settings) else {
            // An explicit invalid rate is an invalid channel declaration. Do not
            // replace it with the subsystem default: that would hide an authoring
            // error and silently substitute an implicit rate.
            continue;
        };
        let measured = param.target.unwrap_or(entity);
        let Some(value) = read_value(world, measured, param, &mut clock) else {
            // Unreadable (port not resolvable, bad reflect path, unsupported type).
            // Still advance the clock so a broken channel doesn't retry every tick.
            advance(&mut clock, t, rate);
            clock_writes.push((entity, clock));
            continue;
        };

        // An authored absolute deadband is an explicit per-channel override;
        // otherwise use the subsystem's shared absolute/relative policy.
        let numeric = numeric_of(&value);
        let changed = match (param.deadband, numeric, clock.last_emitted) {
            (Some(db), Some(v), Some(last)) => (v - last).abs() > db,
            (None, Some(v), Some(last)) => settings.default_deadband.changed(last, v),
            _ => true,
        };

        if changed {
            if let Some(v) = numeric {
                clock.last_emitted = Some(v);
            }
        }

        // Recording is clock-driven; `changed` is only the operator/API
        // notification decision. Keeping both on the sample prevents a
        // deadband from making a time-series appear frozen.
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
            changed,
        });

        advance(&mut clock, t, rate);
        clock_writes.push((entity, clock));
    }

    world.insert_resource(resolved_taken);
    world.insert_resource(plan);

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
fn effective_rate(param: &Parameter, settings: &TelemetrySettings) -> Option<f64> {
    let requested = param.rate_hz.unwrap_or(settings.default_rate_hz);
    if !requested.is_finite() || requested <= 0.0 {
        if param.rate_hz.is_some() {
            warn_once!(
                "telemetry: channel '{}' has invalid explicit rate {} Hz; sampling is skipped \
                 until a finite positive rate is authored",
                param.name,
                requested
            );
        } else {
            warn_once!(
                "telemetry: channel '{}' has no rate and TelemetrySettings::default_rate_hz \
                 ({}) is invalid; sampling is skipped until the setting is corrected",
                param.name,
                requested
            );
        }
        return None;
    }
    if requested > lunco_core::FIXED_HZ {
        warn_once!(
            "telemetry: channel '{}' requested {} Hz but the fixed step is {} Hz — \
             clamping. A faster rate would alias, not oversample.",
            param.name,
            requested,
            lunco_core::FIXED_HZ
        );
        return Some(lunco_core::FIXED_HZ);
    }
    Some(requested)
}

/// Validate a rate arriving through the runtime command surface.
///
/// `None` means the command is rejected and the previous setting remains intact.
/// A rate above the fixed schedule is representable input but cannot be executed,
/// so it is stored as the authoritative fixed-step ceiling after warning.
fn accepted_command_rate(rate: f64, subject: &str) -> Option<f64> {
    if !rate.is_finite() || rate <= 0.0 {
        warn!("telemetry: rejecting {subject} rate {rate} Hz; it must be finite and positive");
        return None;
    }
    if rate > lunco_core::FIXED_HZ {
        warn!(
            "telemetry: {subject} rate {rate} Hz exceeds the fixed-step ceiling of {} Hz; \
             storing the ceiling",
            lunco_core::FIXED_HZ
        );
        return Some(lunco_core::FIXED_HZ);
    }
    Some(rate)
}

/// Validate an absolute deadband arriving through the runtime command surface.
fn accepted_channel_deadband(deadband: f64, subject: &str) -> Option<f64> {
    if !TelemetryDeadband::is_valid_tolerance(deadband) {
        warn!(
            "telemetry: rejecting {subject} deadband {deadband}; it must be finite and non-negative"
        );
        return None;
    }
    Some(deadband)
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
        TelemetryValue::Bool(_)
        | TelemetryValue::String(_)
        | TelemetryValue::Array(_)
        | TelemetryValue::Map(_) => None,
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
            // A declared Modelica output has an authoritative port identity before
            // its first solver snapshot. `entity_ports` reports that contract, but
            // `read_port` quite correctly has no value to return yet. Do not turn
            // that normal compile-to-first-sample interval into a permanent failed
            // clock: the output will become readable when the worker publishes it.
            if registry.has_output_port(world, entity, name)
                || world
                    .get::<lunco_core::PortSurfacePending>(entity)
                    .is_some()
            {
                return None;
            }
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
    use lunco_core::ports::PortDirection;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[derive(Component)]
    struct TestOutput(f64);

    #[derive(Component)]
    struct TestDeclaredOutput;

    #[derive(Component)]
    struct AdvancesPerFixedStep;

    fn advance_test_ports(mut ports: Query<&mut Port, With<AdvancesPerFixedStep>>) {
        for mut port in &mut ports {
            port.value += 1.0;
        }
    }

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

    fn list_test_declared_output(
        world: &World,
        entity: Entity,
        out: &mut Vec<lunco_core::ports::PortRef>,
    ) {
        if world.get::<TestDeclaredOutput>(entity).is_some() {
            out.push(lunco_core::ports::PortRef {
                name: "value".to_string(),
                direction: PortDirection::Out,
                value: world.get::<TestOutput>(entity).map_or(0.0, |value| value.0),
            });
        }
    }

    fn read_test_declared_output(world: &World, entity: Entity, name: &str) -> Option<f64> {
        (name == "value")
            .then(|| world.get::<TestOutput>(entity).map(|value| value.0))
            .flatten()
    }

    const TEST_DECLARED_OUTPUT_BACKEND: lunco_core::ports::PortBackend =
        lunco_core::ports::PortBackend {
            list: list_test_declared_output,
            read_output: read_test_declared_output,
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
        // the user's settings file. Asserted belt-and-braces: the master-switch test below
        // writes `enabled: false`, and that once escaped into the real config.
        app.insert_resource(TelemetrySettings::default());
        app.add_systems(FixedUpdate, advance_test_ports);
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
            if trigger.event().changed {
                sink.lock().unwrap().push(trigger.event().clone());
            }
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
    fn default_channel_cap_covers_the_current_scene_budget() {
        assert!(
            TelemetrySettings::default().max_channels >= 4096,
            "the default must retain a full multi-rover scene with headroom"
        );
    }

    #[test]
    fn persisted_settings_require_the_current_shape() {
        let value = serde_json::to_value(TelemetrySettings::default()).unwrap();
        assert!(value.get("schema_version").is_none());
        assert!(serde_json::from_value::<TelemetrySettings>(value).is_ok());

        let missing_current_deadband = serde_json::json!({
            "default_rate_hz": 5.0,
            "max_channels": 4096,
            "default_retention": 1500,
            "enabled": true
        });
        assert!(
            serde_json::from_value::<TelemetrySettings>(missing_current_deadband).is_err(),
            "a persisted section missing a current field must be rejected, not migrated"
        );

        let invalid_deadband = serde_json::json!({
            "default_rate_hz": 5.0,
            "max_channels": 2048,
            "default_retention": 1500,
            "enabled": true,
            "default_deadband": { "atol": -1.0, "rtol": 0.001 }
        });
        assert!(
            serde_json::from_value::<TelemetrySettings>(invalid_deadband).is_err(),
            "a persisted deadband with an invalid tolerance must be rejected"
        );
    }

    #[test]
    fn obsolete_schema_marker_is_rejected() {
        let value = serde_json::json!({
            "default_rate_hz": 10.0,
            "max_channels": 1024,
            "default_retention": 2000,
            "enabled": true,
            "schema_version": 1
        });
        assert!(serde_json::from_value::<TelemetrySettings>(value).is_err());
    }

    #[test]
    fn a_declared_output_waits_for_its_first_sample_without_becoming_failed() {
        let mut app = app();
        let seen = capture(&mut app);
        app.init_resource::<PortRegistry>();
        app.world_mut()
            .resource_mut::<PortRegistry>()
            .register(TEST_DECLARED_OUTPUT_BACKEND);
        let source = app
            .world_mut()
            .spawn((
                TestDeclaredOutput,
                Parameter {
                    name: "fuel".to_string(),
                    source: ChannelSource::Port("value".to_string()),
                    target: None,
                    rate_hz: Some(lunco_core::FIXED_HZ),
                    ..Default::default()
                },
            ))
            .id();

        // The contract exists, but the producer has not published a value yet.
        step_fixed(&mut app, 2);
        assert!(seen.lock().unwrap().is_empty());
        assert!(
            !app.world()
                .entity(source)
                .get::<ChannelClock>()
                .expect("sampling creates a clock")
                .resolve_failed
        );

        // Once the first live value arrives, the same authored channel must
        // sample it; re-authoring the Parameter is not part of the lifecycle.
        app.world_mut().entity_mut(source).insert(TestOutput(3.25));
        step_fixed(&mut app, 2);
        assert_eq!(
            seen.lock().unwrap().last().map(|sample| &sample.value),
            Some(&TelemetryValue::F64(3.25))
        );
    }

    #[test]
    fn no_implicit_port_is_recorded() {
        let mut app = app();
        app.init_resource::<PortRegistry>();
        app.world_mut()
            .resource_mut::<PortRegistry>()
            .register(TEST_OUTPUT_BACKEND);
        let source = app.world_mut().spawn(TestOutput(7.5)).id();

        step_fixed(&mut app, 2);

        let signal = lunco_signal::SignalRef::new(source, "value");
        assert!(
            app.world()
                .resource::<lunco_signal::SignalRegistry>()
                .scalar_history(&signal)
                .is_none(),
            "a port becomes telemetry only after an explicit channel is requested"
        );
    }

    #[test]
    fn duplicate_channel_declarations_are_collapsed() {
        let mut world = World::new();
        let model = world.spawn_empty().id();
        world.spawn(Parameter {
            name: "soc".into(),
            target: Some(model),
            ..Default::default()
        });
        world.spawn(Parameter {
            name: "soc".into(),
            target: Some(model),
            rate_hz: Some(2.0),
            ..Default::default()
        });
        let mut plan = SamplingPlan {
            dirty: true,
            ..Default::default()
        };

        let duplicates = rebuild_sampling_plan(&mut world, &mut plan);

        assert_eq!(duplicates, 1);
        assert_eq!(plan.channels.len(), 1);
    }

    #[test]
    fn same_port_name_on_different_model_entities_remains_two_channels() {
        let mut world = World::new();
        let left = world.spawn_empty().id();
        let right = world.spawn_empty().id();
        world.spawn(Parameter {
            name: "contact_force".into(),
            target: Some(left),
            ..Default::default()
        });
        world.spawn(Parameter {
            name: "contact_force".into(),
            target: Some(right),
            ..Default::default()
        });
        let mut plan = SamplingPlan {
            dirty: true,
            ..Default::default()
        };

        let duplicates = rebuild_sampling_plan(&mut world, &mut plan);

        assert_eq!(duplicates, 0);
        assert_eq!(plan.channels.len(), 2);
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

    /// Rate is per channel: a fixed-step channel and a slower channel in the same
    /// world must produce different sample counts.
    #[test]
    fn each_channel_samples_at_its_own_rate() {
        let mut app = app();
        let seen = capture(&mut app);

        app.world_mut().spawn((
            Port { value: 1.0 },
            AdvancesPerFixedStep,
            Parameter {
                rate_hz: Some(lunco_core::FIXED_HZ),
                ..reflect_channel("fast")
            },
        ));
        app.world_mut().spawn((
            Port { value: 1.0 },
            AdvancesPerFixedStep,
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
        assert_eq!(rate, Some(lunco_core::FIXED_HZ));
    }

    /// A non-positive or non-finite explicit rate is rejected rather than silently
    /// replacing an authored value with the subsystem default.
    #[test]
    fn a_nonsense_rate_is_rejected() {
        let s = TelemetrySettings::default();
        for bad in [0.0, -5.0, f64::NAN, f64::INFINITY] {
            let p = Parameter {
                rate_hz: Some(bad),
                ..reflect_channel("bad")
            };
            assert_eq!(effective_rate(&p, &s), None, "rate {bad} must be rejected");
        }
    }

    #[test]
    fn invalid_authored_rate_produces_no_samples() {
        let mut app = app();
        let seen = capture(&mut app);
        app.world_mut().spawn((
            Port { value: 1.0 },
            Parameter {
                rate_hz: Some(0.0),
                ..reflect_channel("invalid")
            },
        ));

        step_fixed(&mut app, 4);

        assert!(
            seen.lock().unwrap().is_empty(),
            "an invalid explicit rate must not be replaced by the subsystem default"
        );
    }

    #[test]
    fn invalid_command_rate_does_not_replace_the_existing_setting() {
        let mut app = app();
        app.world_mut().trigger(ControlTelemetry {
            channel: None,
            rate_hz: Some(0.0),
            ..Default::default()
        });
        app.update();

        assert_eq!(
            app.world().resource::<TelemetrySettings>().default_rate_hz,
            TelemetrySettings::default().default_rate_hz
        );
    }

    #[test]
    fn invalid_command_rate_does_not_create_a_channel() {
        let mut app = app();
        let entity = app.world_mut().spawn(Port { value: 1.0 }).id();
        app.world_mut().trigger(ControlTelemetry {
            channel: Some("invalid".to_string()),
            entity: Some(entity),
            reflect: Some("Port.value".to_string()),
            rate_hz: Some(f64::NAN),
            ..Default::default()
        });
        app.update();

        let mut channels = app.world_mut().query::<&Parameter>();
        assert!(
            channels
                .iter(app.world())
                .all(|parameter| parameter.name != "invalid"),
            "an invalid explicit rate must reject channel creation"
        );
    }

    #[test]
    fn invalid_authored_deadband_produces_no_samples() {
        let mut app = app();
        let seen = capture(&mut app);
        app.world_mut().spawn((
            Port { value: 1.0 },
            Parameter {
                deadband: Some(-1.0),
                ..reflect_channel("invalid_deadband")
            },
        ));

        step_fixed(&mut app, 4);

        assert!(
            seen.lock().unwrap().is_empty(),
            "an invalid explicit deadband must not be treated as an implicit policy"
        );
    }

    #[test]
    fn invalid_command_deadband_does_not_create_a_channel() {
        let mut app = app();
        let entity = app.world_mut().spawn(Port { value: 1.0 }).id();
        app.world_mut().trigger(ControlTelemetry {
            channel: Some("invalid_deadband".to_string()),
            entity: Some(entity),
            reflect: Some("Port.value".to_string()),
            deadband: Some(f64::NAN),
            ..Default::default()
        });
        app.update();

        let mut channels = app.world_mut().query::<&Parameter>();
        assert!(
            channels
                .iter(app.world())
                .all(|parameter| parameter.name != "invalid_deadband"),
            "an invalid explicit deadband must reject channel creation"
        );
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

    #[test]
    fn the_default_deadband_suppresses_small_numeric_jitter() {
        let mut app = app();
        let seen = capture(&mut app);
        let e = app
            .world_mut()
            .spawn((
                Port { value: 100.0 },
                Parameter {
                    rate_hz: Some(lunco_core::FIXED_HZ),
                    ..reflect_channel("acceleration")
                },
            ))
            .id();

        step_fixed(&mut app, 4);
        let baseline = seen.lock().unwrap().len();

        app.world_mut()
            .entity_mut(e)
            .get_mut::<Port>()
            .unwrap()
            .value = 100.05;
        step_fixed(&mut app, 4);
        assert_eq!(
            seen.lock().unwrap().len(),
            baseline,
            "the shared default must suppress sub-threshold jitter"
        );

        app.world_mut()
            .entity_mut(e)
            .get_mut::<Port>()
            .unwrap()
            .value = 100.2;
        step_fixed(&mut app, 4);
        assert_eq!(
            seen.lock().unwrap().len(),
            baseline + 1,
            "the shared default must retain a meaningful change"
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
                AdvancesPerFixedStep,
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
        assert!(
            hist.iter().next().unwrap().value >= 3.0,
            "the advancing test source must retain its initial value range"
        );
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
                AdvancesPerFixedStep,
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

    #[test]
    fn control_telemetry_can_set_the_shared_deadband_defaults() {
        let mut app = app();
        app.world_mut().trigger(ControlTelemetry {
            channel: None,
            atol: Some(0.01),
            rtol: Some(0.02),
            ..Default::default()
        });
        app.update();

        let settings = app.world().resource::<TelemetrySettings>();
        assert_eq!(settings.default_deadband.atol, 0.01);
        assert_eq!(settings.default_deadband.rtol, 0.02);
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

    #[test]
    fn direct_parameter_changes_reset_sampling_state() {
        let mut app = app();
        let seen = capture(&mut app);
        let channel = app
            .world_mut()
            .spawn((
                Port { value: 1.0 },
                Parameter {
                    deadband: Some(1.0),
                    ..reflect_channel("old_name")
                },
            ))
            .id();

        step_fixed(&mut app, 2);
        assert!(
            seen.lock()
                .unwrap()
                .iter()
                .any(|sample| sample.name == "old_name"),
            "the original declaration must sample"
        );

        app.world_mut()
            .entity_mut(channel)
            .get_mut::<Parameter>()
            .expect("channel parameter")
            .name = "new_name".to_string();

        step_fixed(&mut app, 1);

        assert!(
            seen.lock()
                .unwrap()
                .iter()
                .any(|sample| sample.name == "new_name"),
            "a changed declaration must not inherit the old deadband reference"
        );
        let clock = app
            .world()
            .entity(channel)
            .get::<ChannelClock>()
            .expect("the changed channel must receive fresh sampling state");
        assert!(clock.last_emitted.is_some());
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
            AdvancesPerFixedStep,
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
