//! # Telemetry & Monitoring Standards
//!
//! This module defines the common data structures for the simulation's
//! monitoring fabric. It adheres to **XTCE/YAMCS** standards to ensure
//! compatibility with real-world mission control toolchains.
//!
//! ## Domain Standards
//! 1. **Parameters**: Continuous data points (e.g., Temperature, Voltage)
//!    sampled at a specific frequency. These are typically broadcast as
//!    `SampledParameter` packets.
//! 2. **Events**: Discrete notifications of system state changes (e.g.,
//!    "Battery Low", "Command Ack"). These are typically broadcast as
//!    `TelemetryEvent` packets.
//! 3. **Timekeeping**: All telemetry is timestamped using the `WorldTime`
//!    epoch (Julian Date, TDB; from the `lunco-time` spine) to allow for precise
//!    post-mission analysis and correlation with ephemeris data.

use bevy::prelude::*;

/// Severity of a telemetry incident, ordered by urgency.
///
/// **Mapping**: Aligns with the 5-tier YAMCS alert hierarchy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Reflect, Default)]
#[reflect(Default, Debug, PartialEq)]
pub enum Severity {
    #[default]
    Debug,
    Info,
    Warning,
    Error,
    Critical,
}

/// A polymorphic container for telemetry values.
///
/// **Why**: Ensures that the telemetry transport layer is agnostic of the
/// internal Rust type (f32, i32, bool), allowing external subscribers to
/// deserialize data into a unified variant type.
#[derive(Debug, Clone, PartialEq, Reflect, serde::Serialize, serde::Deserialize)]
#[reflect(Debug, PartialEq, Default)]
pub enum TelemetryValue {
    F64(f64),
    I64(i64),
    Bool(bool),
    String(String),
}

impl Default for TelemetryValue {
    fn default() -> Self {
        Self::F64(0.0)
    }
}

/// A discrete notification pulse.
///
/// **Example**: A "BATTERY_MINIMUM_REACHED" message with severity `Warning`.
#[derive(Event, Debug, Clone, Reflect)]
#[reflect(Debug, Default)]
pub struct TelemetryEvent {
    /// The unique mnemonic for the event.
    pub name: String,
    /// The `GlobalEntityId` of the **emitter** — which sensor/script/source
    /// fired this. `0` = no entity (global, e.g. raw input). Lets a consumer
    /// apply different logic per source ("which of my ten checkpoints?"),
    /// independent of the `name`. Exposed to scripts as `evt.source`.
    pub source: u64,
    /// Alert level.
    pub severity: Severity,
    /// Associated data payload (e.g. for a zone enter, the ENTRANT's gid).
    pub data: TelemetryValue,
    /// The simulation TDB epoch of the event.
    pub timestamp: f64,
}

impl Default for TelemetryEvent {
    fn default() -> Self {
        Self {
            name: "UNKNOWN".to_string(),
            source: 0,
            severity: Severity::Info,
            data: TelemetryValue::F64(0.0),
            timestamp: 0.0,
        }
    }
}

/// Where a telemetry channel reads its value from.
///
/// Two address spaces, deliberately not collapsed into one — they are genuinely
/// different, and forcing either through the other costs more code, not less.
#[derive(Debug, Clone, Reflect, PartialEq)]
#[reflect(Debug, PartialEq, Default)]
pub enum ChannelSource {
    /// **Fast path.** A named port on this entity, resolved ONCE to a
    /// [`ResolvedPort`](crate::ports::ResolvedPort) and thereafter read by slot with
    /// no name lookup. Uniformly covers every simulated subsystem that exposes ports
    /// — Modelica variables, Avian rigid bodies, joints, FSW signals, and the USD
    /// sensors (`lunco:sensor:imu` / `:range` / `:contact`), which are already ports.
    ///
    /// `f64` only — that is the port currency.
    Port(String),
    /// **Escape hatch.** An arbitrary component field by reflection path
    /// (`"Port.value"`). The only source that can reach a non-port field, and
    /// the only one that can carry `Bool`/`String`. Slower: it needs exclusive world
    /// access and a type-registry lookup per sample.
    Reflect(String),
    /// **The engine's own health.** A `bevy::diagnostic::Diagnostic` by its path string —
    /// `"fps"`, `"frame_time"`, `"entity_count"`, and anything else the app registers.
    ///
    /// This is what makes FPS a first-class telemetry channel rather than a number that
    /// only ever reaches a HUD: it can be subscribed to, retained, plotted, and queried by
    /// a ground system exactly like a motor current. `f64` only — a `Diagnostic` is a
    /// float and nothing else.
    ///
    /// It is a *string* rather than a `DiagnosticPath` so this crate (and every domain
    /// crate that authors a channel) stays free of a `bevy_diagnostic` type in its public
    /// surface; the sampler resolves it.
    Diagnostic(String),
}

impl Default for ChannelSource {
    fn default() -> Self {
        Self::Reflect(String::new())
    }
}

/// A telemetry channel: a named, rate-limited, clock-bound view of one live value.
///
/// **Usage**: attach to the entity that owns the value. Authorable with no new API —
/// it is `Reflect` + `ReflectDefault`, so a script adds one directly:
/// `add(id, "Parameter", #{name: "motor_current", unit: "A", ...})`.
///
/// **Clock**: not a field here. The channel samples on the time domain of the entity's
/// [`TimeBinding`](lunco_time::TimeBinding) — the component that already governs how
/// everything else in the sim reads time. Absent ⇒ the world domain. That is what
/// makes "sample this channel on another clock" free.
#[derive(Component, Debug, Clone, Reflect, PartialEq)]
#[reflect(Component, Default, Debug, PartialEq)]
pub struct Parameter {
    /// Mnemonic name (e.g., "OBC_TEMP"). **Not unique** — two entities may both
    /// declare `"motor_current"`, so a channel is keyed by `(entity, name)`. That is
    /// why [`SampledParameter`] carries its source entity.
    pub name: String,
    /// Engineering units (e.g., "degC").
    pub unit: String,
    /// Where the value comes from.
    pub source: ChannelSource,
    /// The entity whose value this channel measures. `None` ⇒ the entity carrying this
    /// component (how USD authors it: the tag sits on the prim it measures).
    ///
    /// **This indirection is why a rover can have more than one channel.** `Parameter` is a
    /// Component, so an entity carries at most ONE — without a target, "watch three ports on
    /// this rover" would be unrepresentable. A channel created through the API is therefore
    /// its own entity pointing AT the thing it measures.
    pub target: Option<Entity>,
    /// Samples per second, **in this channel's own time domain**. `None` ⇒
    /// `TelemetrySettings::default_rate_hz`.
    ///
    /// Clamped to [`FIXED_HZ`](crate::FIXED_HZ): the fixed step is the sampling
    /// ceiling, and asking for more aliases rather than oversamples.
    pub rate_hz: Option<f64>,
    /// Whether this channel emits. **Defaults to `true`** — a channel you bothered to
    /// author is on.
    pub enabled: bool,
    /// Emit only when the value has moved by at least this much since the last
    /// *emitted* sample (absolute epsilon). `None` ⇒ emit every due sample.
    ///
    /// The single biggest bandwidth win available: a value that isn't moving costs
    /// nothing. Applies to numeric values only; `Bool`/`String` always emit when due.
    pub deadband: Option<f64>,
    /// How many samples of history to keep for this channel — the depth of its ring
    /// buffer in the `SignalRegistry`. `None` ⇒ `TelemetrySettings::default_retention`.
    ///
    /// This is what "how much time to store" means in practice: at `rate_hz` samples per
    /// second, `retention` samples is `retention / rate_hz` seconds of history. Retention
    /// is in SAMPLES, not seconds, because that is what bounds the memory — a channel
    /// can't be allowed to blow up just because someone raised its rate.
    ///
    /// Scalars only. `Bool`/`String` channels have no plot buffer; they ride the event
    /// lane.
    pub retention: Option<usize>,
}

impl Default for Parameter {
    fn default() -> Self {
        Self {
            name: String::new(),
            unit: String::new(),
            source: ChannelSource::default(),
            target: None,
            rate_hz: None,
            // A channel that was explicitly authored is live. This default matters:
            // `ReflectDefault` builds the component from `Default` and *then* patches
            // the named fields, so a script that omits `enabled` gets an ON channel.
            enabled: true,
            deadband: None,
            retention: None,
        }
    }
}

/// A captured snapshot of a [`Parameter`].
#[derive(Event, Debug, Clone, Reflect)]
#[reflect(Debug)]
pub struct SampledParameter {
    /// The mnemonic name of the parameter. **Not unique across entities** — pair it
    /// with [`source`](Self::source) to identify a channel.
    pub name: String,
    /// The engineering value at the time of sampling.
    pub value: TelemetryValue,
    /// Engineering units for scale context.
    pub unit: String,
    /// The absolute TDB epoch (Julian Date) of the sample — for correlating with
    /// ephemeris and for wall-clock labelling.
    ///
    /// **Do NOT difference two of these to get a Δt.** JD is ~2.46e6, so an `f64`
    /// has only ~86 µs of resolution left at that magnitude; subtracting two of them
    /// throws away nearly all of the precision. Use [`sim_secs`](Self::sim_secs).
    pub timestamp: f64,
    /// Seconds on the channel's own time domain — starts near zero, so it keeps full
    /// `f64` precision. **This is the timebase for Δt, plotting, and recording.**
    pub sim_secs: f64,
    /// The entity that owns the channel. Names collide; entities don't.
    pub source: Entity,
}

/// Extension for projecting native/foreign Bevy **messages** onto the neutral
/// [`TelemetryEvent`] script bus — the discrete-event analog of a port wire.
///
/// Every "something happened" source (input, networking, a Modelica `when` edge,
/// a foreign physics message) can land on the SAME bus that rhai scenarios read
/// via `on_event` / `wait_for`, instead of each inventing its own delivery to
/// scripts. Use it for sources whose projection is PURE
/// (`&E -> Option<TelemetryEvent>`; return `None` to drop an event). Context-
/// heavy sources (e.g. collision zones needing the entity registry) keep a
/// dedicated system that still ends at `commands.trigger(TelemetryEvent)` — the
/// unification is "every event lands on ONE bus", not "one identical registrar".
pub trait ScriptEventAppExt {
    /// Add an `Update` system that turns every `E` message into 0..1
    /// [`TelemetryEvent`]s via `project`, fired onto the shared bus.
    fn project_events<E, F>(&mut self, project: F) -> &mut Self
    where
        E: bevy::ecs::message::Message,
        F: Fn(&E) -> Option<TelemetryEvent> + Send + Sync + 'static;
}

impl ScriptEventAppExt for App {
    fn project_events<E, F>(&mut self, project: F) -> &mut Self
    where
        E: bevy::ecs::message::Message,
        F: Fn(&E) -> Option<TelemetryEvent> + Send + Sync + 'static,
    {
        self.add_systems(
            Update,
            move |mut reader: MessageReader<E>, mut commands: Commands| {
                for e in reader.read() {
                    if let Some(ev) = project(e) {
                        commands.trigger(ev);
                    }
                }
            },
        )
    }
}
