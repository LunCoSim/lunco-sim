# Telemetry subsystem

> Status: Active · Audience: contributors on telemetry channels, plots, and the status bus
>
> Phases 0–5 are built. This document describes the current ownership and invariants.

The built subsystem provides explicit `ChannelSource` declarations, per-channel rate and
deadband, clock binding via `TimeBinding`, persisted `TelemetrySettings`, retained scalar
history, the query/subscription API, engine diagnostics admitted as channels, and
`SampledParameter::sim_secs`/`source` identity.

Sampling reads remain tick-bound so every value carries its source `sim_tick` and domain
time. The sampler queues immutable samples; bounded subscriber, logging, and history
fan-out runs in the plugin-owned `Telemetry` cycle in `PostUpdate`, after the fixed
simulation. `max_sample_deliveries_per_update` caps observer work per frame. Its queue is
bounded by that budget times the shared maximum fixed-step burst. When sustained load fills
the queue, the oldest non-authoritative samples are dropped, the dropped total is retained,
and one warning is emitted for the overload episode. Scene transitions also discard pending
samples from the outgoing Twin and count them as drops, so delayed delivery cannot leak data
into the next Twin. These boundaries can leave visible gaps in telemetry history while the
physics tick continues independently. The sampler itself still reads live ECS/port state in
an exclusive fixed pass; moving immutable source preparation off-thread does not permit
reading live simulation state asynchronously. An unbound channel uses the mission's
`SimTick` time. A channel with `TimeBinding` samples only when that exact domain is present
in `ResolvedDomains`; the owner reports an unavailable domain and skips the channel rather
than substituting world time.

The one-line thesis: telemetry history is shared, bounded, and policy-driven. Modelica runtime
state is retained by the render-free Modelica projection; authored channels use the same
registry, so inspectors, APIs, recorders, and plots never depend on one another's UI state.

### Identity, ownership, and labels

`SignalRef { entity, path }` is the identity used by the registry, history, APIs, and saved
bindings. It is never replaced by a display name. Producers attach `SignalMeta::group_path`
from the composed USD ownership graph, so a generated solver variable can be shown under the
authored prim that owns it without copying or renaming the model's state. `SignalExposure` then
controls the normal operator catalog: canonical network values and authored member outputs are
public, while complete runtime/model state remains available through the explicit model-variable
inspection view. Generated Modelica channels also carry the source asset, fully qualified class,
member variable, and canonical USD-facing name in `SignalMeta`; the browser and
`ListTelemetryChannels` expose those fields rather than asking a consumer to infer a component
from a generated solver spelling. When a member output is also promoted to a network boundary,
the boundary channel is the single public representation; its generated member alias stays
internal so one physical value cannot appear twice. The alias remains retained and queryable for
diagnostics, with the canonical relationship shown in its metadata.

Compound values use the same producer-owned metadata. `SignalMeta::presentation` is
`SignalPresentation::Component { group, component }` for an axis, tensor, or quaternion member,
or `SignalPresentation::Summary { group, label, formula }` for a derived headline such as speed.
The scalar `SignalRef` addresses and histories remain unchanged; the telemetry browser uses the
typed relationship to place components and summaries under one semantic group, while the query
API exposes the metadata for other clients. Producers therefore define the unit, frame, and
summary meaning once, and no UI parses channel names to reconstruct vectors.

Retained history whose publisher has disappeared is not part of the normal live telemetry
tree. The browser defaults to current publishers and exposes archived history only through
an explicit display setting; `ListTelemetryChannels` keeps the `active` field so API clients
can make the same live-versus-history choice without guessing from names.

For a producer with a `GlobalEntityId`, `SignalRegistry` captures that owner when the signal is
first retained. The query and export surfaces use the captured owner before consulting the live
ECS component, so an archived network channel keeps its original `api/<GlobalEntityId>:<name>`
key after the source despawns. Session-local producers remain keyed by their live
`Entity::to_bits()` identity. Producers must associate the owner at the retention boundary;
consumers must not reconstruct archived ownership from a missing entity or a display label.

### Archived history is not automatically stale

An archived row means that its publisher was removed during this process (for example, a
scene replacement); it does not mean that its samples are orphaned or safe to delete. The
`SignalRegistry` is an in-memory, bounded mission-history store: it is not persisted across
application restarts, and each row retains its `(entity, path)` identity, metadata, and
sample-capacity boundary. A valid archived history therefore remains queryable for review
after its live publisher is gone, while the normal browser hides it by default.

Treat an archived row as stale only when the owning lifecycle supplies an explicit reason to
discard that signal (such as an authoritative channel removal or a documented run-retention
policy). Absence of the current ECS publisher, an old session-local entity number, or a
missing optional display label is not sufficient evidence. Cleanup must call the registry's
owner-scoped `remove_signal`/`drop_entity` path; there is no name-only or age-only bulk purge.
When no such owner policy exists, inventory the row and preserve it rather than deleting a
valid mission trace.

The telemetry browser, plot controls, legends, and exports all label channels through
`lunco_viz::signal::display_channel_label`, which uses the same identifier humanizer and
ownership-relative shortening. Unit metadata removes redundant `_v`, `_a`, `_w`, and `_ah`
suffixes from operator labels, while standard Modelica electrical connector fields are presented
as `pin voltage` and `pin current`; their exact `p.v`/`p.i` identities remain in the tooltip,
API `name`, and model-variable metadata. Generated member variables receive the declaration's
units and descriptions through the existing Modelica source projection before they enter the
shared registry. The generated-name setting only selects the presentation projection; it never
changes identity, history, bindings, or the USD/Modelica model. `SignalExposure` is the
authoritative internal/public distinction: the telemetry browser applies the shared theme's
internal-state styling once in its legend and to internal row labels, without appending a
category suffix to every label. Exact generated names remain available through the existing
detail and generated-name views. A new producer therefore supplies
ownership and source metadata once and does not need a second naming table in those surfaces. A
runtime-authored compact surface may additionally opt a declaration into its
operator summary with the standard USD `ui:displayName`; the USD projector carries that explicit
label through the existing generic `Callsign` marker. This is membership and authoring data, not a
second name heuristic: declarations without it remain in the full telemetry catalog, and the
catalog's labels still use `display_channel_label`.

---

## 0. What is true today (verify before trusting)

| | today |
|---|---|
| Channel declaration | `lunco_telemetry_core::Parameter { name, unit, source, target, rate_hz, enabled, deadband, retention }` — a `Reflect` Component with `ReflectDefault`, so scripts can author it via `add(id, "Parameter", #{…})`; USD uses `LunCoTelemetryAPI`. A declaration on a prim with its own sampled port may omit `lunco:telemetry:target` and self-target; otherwise its optional target relationship names exactly one composed measured prim. Missing, multiple, or unresolved targets are runtime diagnostics and USD lint errors. Referenced component relationships stay on the stable assembly prim, while a variant deactivates an absent realization through standard USD `active` authoring. |
| Sampling | `lunco-telemetry::sample_parameters` — reflection-driven, exclusive `&mut World`, `FixedUpdate` |
| **Rate** | Per-channel `rate_hz` in the channel's bound simulation clock; `FIXED_HZ` is the execution ceiling |
| Transport | `SampledParameter` (pull/continuous) and `TelemetryEvent` (push/discrete) — Bevy events; both carry the simulation seconds and fixed tick used to correlate records |
| Subscription | `lunco_api::subscription` with explicit name/severity/rate filtering |
| Retention | `lunco_signal::SignalRegistry` scalar histories, with per-channel retention and deadband |
| Unsubscribe | `UnsubscribeTelemetry` owns subscription lifecycle explicitly |

`LunCoTelemetryPlugin` is registered in the shared simulation composition.
Sampling is `run_if`-gated on a `Parameter` existing and runs on the fixed
clock. When channels exist, the exclusive sampler checks one deadline heap per
active clock on each fixed tick and pops only due channels. It reads live
ECS/port state only for those channels, then queues immutable, tick-stamped
samples for bounded `PostUpdate` delivery, so retention, subscriber, and logging
observers do not run inline with physics. Plan rebuilds sort declarations by
stable source/channel identity; due records are merged in that same order before
they enter the bounded queue. A backward clock move resets that lane's cadence
and emits at most once for a resolved time snapshot. The fixed pass still visits
active clock lanes, and end-to-end observer throughput and physics tick cost
remain to be measured.

`lunco-telemetry-core` owns the transport-neutral telemetry contracts
(`TelemetryEvent`, `SampledParameter`, `Parameter`, `ChannelSource`, and their
typed values), the shared reflection registrations, and the black-box event
logger. It also projects generic core facts such as command occurrences and
runtime errors into the telemetry bus. `lunco-telemetry` owns sampling cadence,
settings, retention, and query/history integration. The dependency direction is
deliberate: `lunco-core` emits generic facts and does not depend on telemetry;
the telemetry crates consume those facts.

Every event and continuous sample is stamped at the shared `SimTick` boundary
from `MissionClock`. `timestamp` is the derived TDB epoch for calendar labels;
`sim_secs` is the precise plotting/differencing timebase and `sim_tick` is the
integer correlation key. Producers may run in `Update`, `FixedUpdate`, or an
observer, but telemetry never derives time from render cadence or a wall-clock
accumulator. Parameter collection runs after the runtime's tick-advance set;
physics collection runs after the physics step, and Modelica collection uses the
solver's landed simulation time.

### Physics and UI cost contract

`TelemetryEvent` callbacks used by Rhai keep the deterministic producer stamp and
are delivered in the declared simulation event phase. Log formatting, API
subscription fan-out, serialization, and persistence consume an observation copy
outside the fixed physics transaction. Continuous samples capture only channels
due at that channel's resolved clock time; a fixed tick must not scan all
declarations just to discard not-due channels. The capture boundary records the
small typed value and source/tick identity. Retention, API delivery, logs, and
recording use bounded owner queues or in-memory rings and report overload
explicitly; physics never waits on telemetry I/O. UI plots derive decimated
summaries from `SignalRegistry` history revisions instead of rebuilding them on
every repaint. The sampler now indexes deadlines per clock and reads only due
channels; measured physics cost and end-to-end observer throughput remain open.

---

## 1. Reuse map — do NOT rebuild these

The single biggest risk in this subsystem is reinventing something that already ships. Four
ring buffers, a clock tree, and a timeseries type already exist.

| Need | **Already exists** | Verdict |
|---|---|---|
| **Different clocks / cycles** | `lunco-time::domain` — `TimeDomain { parent, offset, scale, regime }` (affine child clock, USD `LayerOffset` semantics), `Playback { head, mode, rate, looping }` (independent playhead), **`TimeBinding { domain: Entity }` — a per-entity component**, `ResolvedDomains` resolved once per frame | **Use as-is.** "Sample this channel on another clock" = give the channel a `TimeBinding`. A missing bound domain is an owner-visible error, never an implicit switch to world time. |
| **Retention / ring buffer** | `lunco_signal::SignalRegistry` — `ScalarHistory { VecDeque<ScalarSample>, capacity }` **per signal**, `push_scalar()` drops non-finite, and `SignalMeta { unit, provenance }` | **Use as-is.** Routing `SampledParameter → push_scalar` keeps retention and plotting on one path. |
| **FPS / frame stats** | Bevy diagnostics provide the engine-owned measurements and short diagnostic ring. `lunco-telemetry` admits the selected paths, samples the typed `EngineHealthSnapshot`, and retains them in the global `SignalRegistry` history | **Use the diagnostic as an input, not as a UI data store.** The retained `SignalRegistry` series is the canonical history for HUDs, plots, APIs, and recording. |
| **Timeseries / experiments** | `RunResult { times: Vec<f64>, series: BTreeMap<String, Vec<f64>> }` (columnar), `RunUpdate::Progress { delta }` (incremental stream), `RunBounds { dt, n_intervals }` (**the codebase's existing vocabulary for output sample spacing**), `REGISTRY_CAP_PER_TWIN = 20` | A telemetry **recording** should *be* a `RunResult` — it then plots and retains through machinery that already works. Rate vocabulary should rhyme with `RunBounds::dt`. |
| **Physics observations and conversions** | Native Avian ports plus LunCoRaycastAPI raw-query outputs; Modelica IMU/altimeter/attitude conversions are ordinary SimComponent ports | **Use the same telemetry path.** No semantic Rust sensor registry is needed. |
| **Channel address space** | `lunco_port_core::ports` — `PortRegistry`, `PortRef { name, direction, value: f64 }`, and crucially **`ResolvedPort { backend, slot }` — resolve the name ONCE, then read every tick with one call**. Backends: Modelica vars, Avian bodies, joints, FSW signals, USD sensors | The fast path. **Do not re-resolve a name at 60 Hz.** |
| **Command shape** | `ControlAnimation { playing: Option<bool>, seek_secs: Option<f64>, rate: Option<f64> }` — one verb, all-`Option`, each field a distinct control | **Copy this idiom exactly.** One `ControlTelemetry`, not five verbs. |
| **Settings** | `SettingsSection` trait (`const KEY`) → `<OS config dir>/lunco/settings.json` | `TelemetrySettings` owns telemetry defaults; its persisted section must have the current shape. |
| **Journal** | `ExperimentOp` (`experiment_journal.rs`) — journals the *definition*, never the results | Journal channel **definitions** (undo/replay/network-sync). **Never journal samples** — `twin-journal/src/lib.rs:40` says so explicitly. |

---

## 2. The channel

One component owns each channel declaration. `Parameter` is the existing reflected
contract used by USD, scripts, and telemetry commands.

```rust
pub struct Parameter {
    pub name: String,
    pub unit: String,
    pub source: ChannelSource,
    pub rate_hz: Option<f64>,       // None ⇒ TelemetrySettings::default_rate_hz
    pub enabled: bool,
    pub retention: Option<usize>,   // ring-buffer depth; None ⇒ settings default
    // clock comes from the entity's `TimeBinding` — NOT a field here (§4)
}

pub enum ChannelSource {
    /// Fast path. Resolved ONCE to a `ResolvedPort`, then read by slot.
    /// Covers Modelica vars, Avian bodies, joints, FSW signals AND USD sensors uniformly.
    Port(String),
    /// Escape hatch: arbitrary component field by reflection path ("Port.value").
    /// The only source that can carry Bool/String. Slower — exclusive world access.
    Reflect(String),
    /// A Bevy diagnostic path admitted by telemetry policy. f64 only.
    Diagnostic(String),
}
```

**Why three sources and not one.** They are genuinely different address spaces, and collapsing
them would lose something real: `Port` is the fast, uniform, `f64`-only space that already
covers every simulated subsystem; `Reflect` is the only way to reach a non-port field or a
`Bool`/`String`; `Diagnostic` is where the engine's own health already lives with a ring buffer
attached. The alternative — forcing FPS through a port backend — is more code, not less.

`Parameter` remains the explicit history declaration for non-Modelica and mission-semantic
channels. Modelica runtime variables are a separate generic producer: the solver already
publishes its complete current variable map, and the Modelica core projects that map into the
same `SignalRegistry` using `TelemetrySettings`. No USD output attribute, per-variable
`Parameter`, or plot binding is required.

### Engine diagnostics and generic visualization

Bevy's `DiagnosticsStore` and diagnostic rings are an engine-owned measurement source. They are
read once by the core health publisher and by the telemetry admission/sampling bridge; no HUD,
status widget, plot, or API surface reads the store directly. The default admitted engine
channels are FPS and frame time. Additional diagnostic paths require explicit channel metadata
or policy so an internal diagnostic cannot silently become a public catalog entry.

The bridge publishes the latest typed health facts through `EngineHealthSnapshot` and retains
the selected scalar samples as global signals (`engine.fps`, `engine.frame_time`) in the shared
`SignalRegistry`. The headline frame time is smoothed, while `engine.frame_time` retains the
latest raw diagnostic value so short hitches remain visible. `lunco-viz` owns the generic
telemetry sparkline: it consumes a `SignalRef` and the corresponding retained
`ScalarHistory`, caches only derived display points/statistics, and does not create another
history. The status bar merely selects the default frame-time signal; the same widget and
history are available to plots, API clients, recording, and future HUDs.

---

## 3. Per-channel rate — the one genuinely new mechanism

**Do not use bevy's `on_timer` run-condition.** It is wall-clock: it ignores pause, ignores
warp, and would keep firing while the sim is frozen. (Nothing in this repo uses `on_timer`
today, and this is why it shouldn't start.)

Instead: keep each channel's next deadline in its own time domain and index it in a
min-heap for that clock. A fixed pass checks the stable clock lanes and pops only
deadlines that have arrived; it never scans every channel just to reject not-due ones.

```rust
struct ChannelClock { next_due_t: f64 }     // in the channel's domain seconds

// each fixed step, once per active clock lane:
let Some(now) = lane.resolved_time() else {
    // A missing explicit domain holds its channels; it does not use world time.
    continue;
};
let rewound = lane.observe_time(now); // detects a seek/loop once per snapshot
for channel in lane.pop_due(now, rewound) {
    read_and_queue(channel, now);
    channel.next_due_t = if rewound {
        now + 1.0 / channel.rate
    } else {
        (channel.next_due_t + 1.0 / channel.rate).max(now + 1.0 / channel.rate)
    };
    lane.reinsert(channel);
}
```

Due channels from all lanes are merged by stable declaration identity before sampling and
bounded delivery. Forward time jumps do not create catch-up bursts; a backward move samples
at the new playhead once and starts a new cadence there. Unbound channels use the integer
simulation tick through `MissionClock`; explicitly bound channels use the exact resolved
`TimeBinding` domain or remain held.

This inherits pause, warp, `TimeDomain::scale`, and `Playback` seek/loop **for free**, because
those already live in the domain. A channel bound to a `scale = 100` domain samples 100× the
sim-seconds per wall-second — which is exactly what "speed only the factory" means.

Rate ceiling is the fixed step: you cannot sample faster than `FIXED_HZ`. A requested
`rate_hz > FIXED_HZ` is **clamped and warned**, not silently aliased. An omitted rate uses
`TelemetrySettings::default_rate_hz`; an explicit non-positive or non-finite rate is invalid,
warned, and skipped until corrected — it is never replaced by the subsystem default.

---

## 4. Clock binding

No `clock` field on `Parameter`. A channel entity carries `TimeBinding { domain }` — the
component that **already exists** and already governs how everything else reads time. Absent ⇒
the deterministic mission simulation clock. `ControlTelemetry` does not currently set a
binding; a caller that needs another domain authors `TimeBinding` on the channel entity.

This is the whole answer to *"option to run it in different cycles/clock"*, and it costs one
component you already have.

---

## 5. Where samples go

Three lanes, and they are not interchangeable:

1. **Live push → subscribers.** `SampledParameter` → `sampled_param_observer` → API, with
   explicit per-subscription decimation so a 1 Hz dashboard is not forced to eat a 60 Hz
   channel.
2. **Retention → `SignalRegistry::push_scalar`.** Per-channel `ScalarHistory` ring buffer;
   this is what a plot reads, and what "how much history to store" means. **Scalars only** —
   `TelemetryValue::{Bool, String}` cannot enter a `ScalarHistory`.
3. **Discrete/eventful → `TelemetryEvent`.** The existing push bus, with `Severity` and the
   authoritative `sim_secs`/`sim_tick` stamp. Bool and
   String channels belong here, not in the ring buffer. Event payloads preserve signed
   `i64` and unsigned `u64` values as distinct types for scripting and API consumers.
   *(This asymmetry is real and must be stated, not papered over: a `String` channel has no
   plot.)*

### 5b. Model state and explicit channels share one retention plane

`ModelicaModel::parameters`, `inputs`, and `variables` are the live model-inspector surface, and
`CosimStatus`/`SnapshotVariables` expose the same state to agents and API clients. After a solver
response lands, `lunco-modelica-telemetry` retains finite `variables` in the shared
`SignalRegistry` at `TelemetrySettings::default_rate_hz`, subject to the shared deadband,
retention, and channel cap. The projection clears a solver session's old history when the
authoritative Modelica `session_id` changes, so reloads cannot mix time bases.

This is runtime exploration, not authored schema: no USD output attribute and no per-variable
`Parameter` is synthesized. Explicit `Parameter`/`lunco:telemetry` channels remain the right
choice when an author wants a stable mission-facing name, custom rate/unit, or a non-Modelica
source. Both producers write the same signal identity and registry, so the telemetry browser,
API, recorder, and plot surfaces see one catalog. Plot bindings only choose what to display; they
do not decide whether model state exists. The Modelica inspector likewise lists runtime-published
variables, authored inputs, and experiment outputs; it does not infer observable variables from
static component types before a solver publishes them.

The model inspector answers "what is the model doing now?"; the telemetry browser answers "what
channels and history are available?". Both use the shared deadband to suppress numerical jitter,
with non-finite values excluded from scalar history and solver time resets handled explicitly.

The physics producer treats metadata as catalog state: it publishes metadata on first channel
discovery or when an entity's USD owner path changes, then only records samples on later fixed
steps. This keeps the fixed physics path from rebuilding identical descriptions while the shared
registry remains the single metadata owner.

Its transient per-entity cursors follow the same lifecycle boundary: `RemovedComponents` retires
state when the last physical source leaves an entity, while the shared registry deliberately keeps
the archived history. The producer does not rebuild a live-entity set or scan its state maps on
every fixed step, and a body-to-wheel source transition does not discard still-live state.

---

## 6. Recording → experiments

A **recording** is a bounded capture of N channels over a time window, exported as a
`RunResult { times, series }` — the type experiments already produce and plots already consume.
Start/stop via `ControlTelemetry`. This is where telemetry and experiments genuinely share
machinery, and it costs almost nothing because the sink type already exists.

The `RunBounds { dt, n_intervals }` vocabulary should be reused for a recording's output grid
rather than inventing a second spelling of "sample spacing".

---

## 7. Commands + API — one verb, not five

Follows the `ControlAnimation` idiom (one command, all-`Option` fields), which is what keeps
this from becoming five new verbs:

```rust
#[Command(default)]
pub struct ControlTelemetry {
    pub channel:   Option<String>,   // None ⇒ applies to the whole subsystem
    pub enabled:   Option<bool>,
    pub rate_hz:   Option<f64>,
    pub retention: Option<usize>,
    pub clock:     Option<Entity>,   // rebind TimeBinding
    pub record:    Option<bool>,     // start/stop a RunResult capture
}
```

API-side, two changes to existing types — **no new request verbs**:
- `TelemetryFilter { names, min_severity }` gains **`rate_hz: Option<f64>`** — per-subscription
  decimation, independent of the channel's own rate. This is what finally replaces the MCP
  `watch_ports` JS poll loop with a real server push.
- **`UnsubscribeTelemetry`** is the explicit lifecycle operation paired with
  `SubscribeTelemetry`; subscriptions do not rely on process teardown.

---

## 7b. The query surface — OpenMCT (and any ground system) needs THREE things

Subscription is only one of them. A client that can *only* subscribe gets a firehose it cannot
interpret: no way to ask what channels exist, and blind to everything that happened before it
connected — so every plot opens empty and stays that way until new data arrives.

| what a ground system asks for | surface | status |
|---|---|---|
| **dictionary** — what channels exist, names, units | `ListTelemetryChannels` | **built** |
| **history** — channel K between t0 and t1 (plot open, scroll back, zoom) | `QueryTelemetryHistory` | **built** |
| **realtime** — push me new values | `SubscribeTelemetry` | already existed |

Both are `ApiQueryProvider`s — the same extension point Modelica's `SnapshotVariables` uses — so
they are transport-agnostic and already reachable over the API and MCP. **An OpenMCT telemetry
adapter (or a YAMCS bridge) is a thin integration layer over these, not a rewrite**; HTTP/WebSocket streaming
can be layered on later without touching this layer.

`ListTelemetryChannels` also returns a `delivery` object with `pending_samples`,
`queue_capacity`, and cumulative `dropped_samples`. This makes overload visible
to a ground client; missing telemetry samples never hold the simulation clock.

Two decisions that make that possible:

- **Channel key = `"<owner>:<name>"`, never the name alone.** Names collide — two rovers both
  report `motor_current`. `owner` is `api/<GlobalEntityId>` for network-addressable model
  entities and `session/<Entity::to_bits()>` for deliberately local physics/model entities.
  The latter is explicit session identity, never the invalid `0` placeholder. The stable API
  owner is captured by `SignalRegistry` while the source is live, so this key remains unchanged
  when an archived network source is gone. The key is the wire form of the same
  `(SignalRef::entity, SignalRef::path)` identity rendered by the native telemetry window.
- **Times are `sim_secs`, not `epoch_jd`.** Julian Date is ~2.46e6, so an `f64` has ~86 µs of
  resolution left there: a plot axis built on it quantises into visible stair-steps and a range
  query is sloppy at its edges. Responses carry `epoch_jd` separately for wall-clock labelling.

## 8. USD authoring

Follows the ordinary USD port convention; raw ray configuration uses
LunCoRaycastAPI and semantic conversion is authored as Modelica:

```usda
bool   lunco:telemetry           = true
token  lunco:telemetry:name      = "motor_current"        # defaults to the port/field name
token  lunco:telemetry:port      = "left_wheel.torque"    # ChannelSource::Port  (preferred)
token  lunco:telemetry:reflect   = "Port.value"   # …or ChannelSource::Reflect
token  lunco:telemetry:unit      = "A"
double lunco:telemetry:rateHz    = 10                     # absent ⇒ settings default
bool   lunco:telemetry:enabled   = true                   # absent ⇒ TRUE (authored = live)
double lunco:telemetry:deadband  = 0.01
int    lunco:telemetry:retention = 2000                   # SAMPLES, not seconds
```

`lunco:telemetry` with neither `:port` nor `:reflect` is a terminal
`usd-telemetry` projection diagnostic and authors no channel — silently
creating a channel with no source would be a channel that can never speak.
When the composed USD stage revision changes, the telemetry projector removes
all previously derived channels and declaration markers before rebuilding the
index. A target that has no projected runtime entity is likewise terminal and
remains visible in the diagnostics/lint surface; it is never retried as a
silent first-pass candidate.

`ChannelSource::Diagnostic` is deliberately **not** USD-authorable: a diagnostic is
engine-global, not a property of a prim. `lunco-telemetry` publishes those itself
(`spawn_engine_health_channels`), and only when the diagnostic actually exists — so a
`--no-ui` run, which links `bevy_diagnostic` but never adds `FrameTimeDiagnosticsPlugin`,
publishes no always-silent FPS channel to clutter the catalog.

Avian observations and Modelica conversion outputs already emit ordinary ports,
so tagging one for telemetry is just a lunco:telemetry:port pointing at it.
No separate sensor telemetry machinery exists.

---

## 9. Settings

```rust
struct TelemetrySettings {          // impl SettingsSection, KEY = "telemetry"
    default_rate_hz: f64,           // 5.0 — the semantic default for omitted channel rates
    default_retention: usize,       // 1500 samples — five minutes at 5 Hz
    max_channels: usize,            // 8192 live channels by default; backpressure guard
    max_sample_deliveries_per_update: usize, // 1024 callbacks per app frame by default
    enabled: bool,
    default_deadband: TelemetryDeadband,
}
```

The resolution rules are deliberately narrow:

- `Parameter.rate_hz = None` selects the configured `default_rate_hz`.
- A rate above `FIXED_HZ` selects the fixed-step ceiling and emits a warning.
- A non-positive or non-finite explicit rate is rejected or skipped; no other rate is
  substituted.
- An explicit channel deadband must be finite and non-negative. Invalid authored or command
  values are rejected or skipped; the subsystem deadband is never substituted.
- `LunCoTelemetryPlugin` installs and owns the unified mission-time spine and telemetry
  settings required by the sampler. A host that calls the internal sampler without that
  plugin is misconfigured; it does not receive guessed settings or a second clock.
- The plugin installs the bounded `Telemetry` delivery cycle automatically. A queue
  overflow drops only old telemetry samples, reports the overload episode, and never holds
  `SimulationProgress` or the fixed physics cycle.
- The query and export providers use the plugin-owned `SignalRegistry`; a missing registry is
  an integration error, not an empty recording.
- A persisted telemetry section must contain the current fields. Missing fields or unknown
  fields invalidate that section and cause the settings owner to install current defaults;
  there is no field-level compatibility reader.

---

## 10. Operational invariants

1. **Explicit history.** Modelica parameters, inputs, and variables are live inspection state;
   only a selected binding or authored `Parameter` is retained.
2. **Bounded history.** Scalar retention is a ring buffer with an authored or configured sample
   capacity. It never grows to preserve an implicit stream.
3. **Fixed-clock sampling.** Sampling stops when the bound simulation clock pauses, which is
   correct; moving it to `Update` would make replay depend on render cadence.
4. **Numerical visibility.** The shared deadband suppresses ordinary jitter, while non-finite
   transitions are always retained so numerical faults remain visible.
5. **Identity.** Channels are keyed by `(measured entity, name)`, not by a name-only lookup.
6. **No invented values.** Missing samples in a multi-rate recording remain empty; history is
   not interpolated to make a slower channel look faster.

---

## 11. Order

- **Phase 0 — DONE.** Plugin wired; `SubscribeTelemetry` can actually deliver.
- **Phase 1 — DONE.** `ChannelSource`, per-channel `rate_hz`, `enabled`, `deadband`, the
  domain accumulator, `TimeBinding`, `TelemetrySettings`. Closes the "60 Hz firehose, one
  global rate" gap. Plus the §10 invariants: `sim_secs`, `(entity, name)` keying, rate
  clamping, and a loud backpressure cap.
- **Phase 2 — DONE.** `SampledParameter → SignalRegistry::push_scalar`, per-channel `retention`,
  history dropped with its entity. Required extracting **`lunco-signal`** (render-free) out of
  `lunco-viz` (which links `bevy_egui → bevy_render`), since a `--no-ui` run needs retention just
  as much as a plot does — same split as `lunco-render` / `lunco-render-bevy`. `lunco-viz`
  re-exports it, so all 15 existing `lunco_viz::SignalRegistry` callers are untouched.
  - **Plot colour now comes from the THEME.** `color_for_signal` had a hardcoded 12-entry Tab10
    palette — the only colours in the app that ignored the active theme. It is now
    `Theme.plot: PlotTokens`, palette-derived via `from_palette` like every other token group.
- **Phase 3 — DONE.** `ControlTelemetry` (one verb, all-`Option`), `TelemetryFilter.rate_hz`,
  `UnsubscribeTelemetry`, plus the **OpenMCT query surface** (below).
  - Decimation caveat, stated rather than hidden: telemetry is ONE shared stream, not a
    per-subscriber fan-out, so a rate cap throttles to the *fastest* matching subscriber. A slow
    dashboard cannot slow down a client that asked for full rate. True per-subscriber fan-out
    needs a routed transport.
  - Still open from this phase: retiring the MCP `watch_ports` JS poll loop
    (`mcp/src/index.js:810`) in favour of the real query surface.
- **Phase 4 — DONE.** `lunco:telemetry:*` USD authoring. Avian observations and
  Modelica conversion outputs are ordinary ports, so tagging one for telemetry
  is just `lunco:telemetry:port` naming it. Recording = `ExportTelemetryRecording`.
- **Phase 5 — DONE.** `ChannelSource::Diagnostic` admits FPS/frame-time as real channels. The
  status bar no longer owns a diagnostic-specific `VecDeque`: the typed engine-health publisher
  samples the engine facts once, `SignalRegistry` retains the canonical global series, and the
  generic `lunco-viz` sparkline reads that history. Bevy's diagnostic ring remains a bounded
  engine measurement source and smoothing aid; it is not the application's UI or recording
  history.

### There is no separate "recorder"

`ExportTelemetryRecording` reads the existing ring buffers and returns
`{ times, series }` — the shape `lunco_experiments::RunResult` already uses, so an
experiments plot or CSV export consumes a telemetry recording with no second code path.

**The ring buffer IS the recording.** A start/stop recorder with its own buffer would be a
second store of the same samples, with its own retention bug waiting to happen — the exact
duplication this subsystem was built to avoid.

The one subtlety: channels sample at *different rates* (that is the point of Phase 1), so
they share no time axis. The export builds the sorted **union** of sample times and fills a
channel's missing slots with `null` — the same NaN-padding `RunResult::merge_delta` does.
**Do not interpolate**: a hole is data the channel genuinely never reported, and inventing a
value would launder a 1 Hz channel into looking like a 60 Hz one.

### A trap found while building Phase 3 — tests were writing the developer's real config

`register_settings_section` **auto-adds `SettingsPlugin`**, which loads the shared LunCoSim settings file
and installs a flush system that writes it back on *any* change to the typed resource. Correct
for the app; actively dangerous in a test — a test app that merely installs a domain plugin
inherits real, persistent, **cross-process** state.

A `lunco-telemetry` test flipped `TelemetrySettings::enabled` to `false`. That `false` landed in
the real user config, and every subsequent test in the process — plus the developer's next run of
the actual application — read it back and sampled nothing. It presented as **a cluster of
unrelated failures whose membership changed with the test-thread count**, because the poison
travelled through the *filesystem* rather than through the code.

Nine crates register settings sections; two isolated their config dir. Rather than patch seven
test suites (which the next new test would forget), the gate now lives at the two I/O sites in
`lunco-settings`: `disk_backed()` makes a **cargo-test binary in-memory-only** — no read, no
write — unless it explicitly names a config dir via `LUNCOSIM_CONFIG` or
`isolate_config_dir_for_tests`. A test binary is detected
by its parent directory being `deps/` (nothing legitimately runs an app from there), and
`a_test_binary_is_detected_as_such` asserts this from *inside* a test binary, so the guard fails
loudly rather than silently opening back up.

**The general rule: auto-persistence and test isolation are in direct conflict, and the default
must be the safe one.** Anything that writes to a user's home directory on a resource change must
prove it isn't a test first.

### The clock contract

Telemetry is a fixed-clock subsystem and therefore requires `lunco_time::TimePlugin`.
`MissionClock(SimTick)` supplies the authoritative epoch and simulation seconds for the
default channel domain; an authored `TimeBinding` reads the same shared resolved domain tree.
A missing time resource is an integration error; falling back to a different clock would make
pause, warp, and replay semantics depend on how the host assembled the app.

Runtime projections use the same contract at their owning boundaries:

- fixed-step physics retains post-step state at `MissionClock::sim_secs(SimTick)`. It does not
  use `Time<Fixed>::elapsed_secs_f64()`, which is only the Bevy fixed-schedule accumulator and
  can repeat a timestamp when several fixed steps execute in one rendered frame;
- Modelica retains a solver result at the solver's landed `current_time`, which is the same
  simulation domain presented by the model participant;
- `WorldTime.epoch_jd` remains the absolute display/correlation timestamp. It is not used for
  finite differences because its large Julian-Date magnitude loses precision.

This gives every producer one semantic time value for history, rate, deadband, and derivatives,
while preserving the absolute epoch as a separate presentation field.
