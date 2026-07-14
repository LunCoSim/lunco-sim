# Telemetry subsystem — design

**Status: Phases 0 and 1 have LANDED (2026-07-13).** Phases 2–5 are proposal.

Landed in Phase 1: `ChannelSource::{Port, Reflect}`, **per-channel `rate_hz`**, `enabled`,
`deadband`, clock binding via `TimeBinding`, `TelemetrySettings` (a persisted
`SettingsSection`), rate clamping, backpressure warning, `SampledParameter::sim_secs`,
`SampledParameter::source`, and `UnsubscribeTelemetry`.

The one-line thesis: **almost every part of this already exists and is wired to something
else.** The work is not writing a telemetry engine — it is *connecting four subsystems that
were each built for one caller* and adding the single thing genuinely absent: **per-channel
rate**.

---

## 0. What is true today (verify before trusting)

| | today |
|---|---|
| Channel declaration | `lunco_core::telemetry::Parameter { name, unit, path }` — a `Reflect` Component with `ReflectDefault`, so scripts can author it via `add(id, "Parameter", #{…})` |
| Sampling | `lunco-telemetry::sample_parameters` — reflection-driven, exclusive `&mut World`, `FixedUpdate` |
| **Rate** | **`FIXED_HZ` = 60. Globally. For every channel. There is no per-channel rate anywhere in the codebase** (`grep sample_rate\|rate_limit\|min_interval` → 0 hits) |
| Transport | `SampledParameter` (pull/continuous) and `TelemetryEvent` (push/discrete) — Bevy events |
| Subscription | `lunco_api::subscription` — filters by **name allowlist + min_severity only**. No rate, no decimation |
| Retention | **none in the telemetry path.** `lunco_core::log` is a "black box logger" that only `info!`s |
| Unsubscribe | `TelemetrySubscriptions::unsubscribe()` exists, but **no `UnsubscribeTelemetry` request reaches it** |
| The only "subscribe at a rate" in the product | **a JS `setTimeout` poll loop in the MCP server** (`mcp/src/index.js:810` `watch_ports`, 50–5000 ms, ≤120 samples) |

**Phase 0 (landed 2026-07-13):** `LunCoTelemetryPlugin` is now added in `lunco-sandbox`. It
had never been added, so `SubscribeTelemetry` — whose consumer half (`sampled_param_observer`,
`TelemetryResponse::from_sampled`, `lunco_core::log`) was fully shipped — advertised parameter
telemetry that **could never arrive**. Sampling is `run_if`-gated on a `Parameter` existing and
runs on the fixed clock, so it costs nothing until a channel is authored.

---

## 1. Reuse map — do NOT rebuild these

The single biggest risk in this subsystem is reinventing something that already ships. Four
ring buffers, a clock tree, and a timeseries type already exist.

| Need | **Already exists** | Verdict |
|---|---|---|
| **Different clocks / cycles** | `lunco-time::domain` — `TimeDomain { parent, offset, scale, regime }` (affine child clock, USD `LayerOffset` semantics), `Playback { head, mode, rate, looping }` (independent playhead), **`TimeBinding { domain: Entity }` — a per-entity component**, `ResolvedDomains` resolved once per frame | **Use as-is.** "Sample this channel on another clock" = give the channel a `TimeBinding`. Nothing to build. |
| **Retention / ring buffer** | `lunco_viz::SignalRegistry` — `ScalarHistory { VecDeque<ScalarSample>, capacity }` **per signal** (default 2000), `push_scalar()` drops non-finite, `SignalMeta { unit, provenance }`, deterministic per-path plot colour | **Use as-is.** Only producer today is the Modelica worker; its docs already anticipate other producers. Routing `SampledParameter → push_scalar` buys retention **and plotting** in one wire-up. |
| **FPS / frame stats** | `bevy::diagnostic::Diagnostic` — named `DiagnosticPath` + ring buffer + `history_len` + `smoothed()` + `is_enabled`. Used in **exactly one file** (`perf_hud.rs`), which then **hand-rolls its own** `frame_history: VecDeque<f32>` (`FRAME_HISTORY_LEN = 240`) on top of it | **A `Diagnostic` IS a telemetry channel** (f64-only). Expose it as a channel *source*; delete perf_hud's duplicate buffer. |
| **Timeseries / experiments** | `RunResult { times: Vec<f64>, series: BTreeMap<String, Vec<f64>> }` (columnar), `RunUpdate::Progress { delta }` (incremental stream), `RunBounds { dt, n_intervals }` (**the codebase's existing vocabulary for output sample spacing**), `REGISTRY_CAP_PER_TWIN = 20` | A telemetry **recording** should *be* a `RunResult` — it then plots and retains through machinery that already works. Rate vocabulary should rhyme with `RunBounds::dt`. |
| **Sensors from USD** | `lunco-cosim/src/sensors.rs` — `ImuSensor` / `RangeSensor` / `ContactSensor`, authored from `lunco:sensor:imu` / `:range` / `:contact` (`lunco-usd-sim:594`), **and they already surface as ports** | **Nothing to build.** A USD sensor is *already* a telemetry source. |
| **Channel address space** | `lunco_core::ports` — `PortRegistry`, `PortRef { name, direction, value: f64 }`, and crucially **`ResolvedPort { backend, slot }` — resolve the name ONCE, then read every tick with one call**. Backends: Modelica vars, Avian bodies, joints, FSW signals, USD sensors | The fast path. **Do not re-resolve a name at 60 Hz.** |
| **Command shape** | `ControlAnimation { playing: Option<bool>, seek_secs: Option<f64>, rate: Option<f64> }` — one verb, all-`Option`, each field a distinct control | **Copy this idiom exactly.** One `ControlTelemetry`, not five verbs. |
| **Settings** | `SettingsSection` trait (`const KEY`) → `~/.lunco/settings.json` (see `PerfHudSettings`, `ExperimentSettings`) | Add one `TelemetrySettings` section. |
| **Journal** | `ExperimentOp` (`experiment_journal.rs`) — journals the *definition*, never the results | Journal channel **definitions** (undo/replay/network-sync). **Never journal samples** — `twin-journal/src/lib.rs:40` says so explicitly. |

---

## 2. The channel

One component. It already exists; it grows four fields.

```rust
pub struct Parameter {
    pub name: String,
    pub unit: String,
    pub source: ChannelSource,      // was: `path: String`
    pub rate_hz: Option<f64>,       // None ⇒ TelemetrySettings::default_rate_hz
    pub enabled: bool,
    pub retention: Option<usize>,   // ring-buffer depth; None ⇒ settings default
    // clock comes from the entity's `TimeBinding` — NOT a field here (§4)
}

pub enum ChannelSource {
    /// Fast path. Resolved ONCE to a `ResolvedPort`, then read by slot.
    /// Covers Modelica vars, Avian bodies, joints, FSW signals AND USD sensors uniformly.
    Port(String),
    /// Escape hatch: arbitrary component field by reflection path ("PhysicalPort.value").
    /// The only source that can carry Bool/String. Slower — exclusive world access.
    Reflect(String),
    /// A bevy `Diagnostic` (FPS, frame time, entity count). f64 only. Free ring buffer.
    Diagnostic(DiagnosticPath),
}
```

**Why three sources and not one.** They are genuinely different address spaces, and collapsing
them would lose something real: `Port` is the fast, uniform, `f64`-only space that already
covers every simulated subsystem; `Reflect` is the only way to reach a non-port field or a
`Bool`/`String`; `Diagnostic` is where the engine's own health already lives with a ring buffer
attached. The alternative — forcing FPS through a port backend — is more code, not less.

`Parameter.path: String` → `Parameter.source: ChannelSource` is a **breaking change to a
`Reflect` component**, which means USD/rhai/journal authoring of the old form breaks. It is
worth it now, while the only authored `Parameter`s in existence are in tests.

---

## 3. Per-channel rate — the one genuinely new mechanism

**Do not use bevy's `on_timer` run-condition.** It is wall-clock: it ignores pause, ignores
warp, and would keep firing while the sim is frozen. (Nothing in this repo uses `on_timer`
today, and this is why it shouldn't start.)

Instead: **an accumulator against the channel's own time domain.**

```rust
struct ChannelClock { next_due_t: f64 }     // in the channel's domain seconds

// each fixed step, for each enabled channel:
let t = domain_time(&resolved_domains, binding);   // lunco-time::domain, already exists
if t >= clock.next_due_t {
    emit(sample);
    clock.next_due_t = t + 1.0 / rate;
    // clamp so a paused/seeked/warped domain can't queue a burst of catch-up samples:
    clock.next_due_t = clock.next_due_t.max(t);
}
```

This inherits pause, warp, `TimeDomain::scale`, and `Playback` seek/loop **for free**, because
those already live in the domain. A channel bound to a `scale = 100` domain samples 100× the
sim-seconds per wall-second — which is exactly what "speed only the factory" means.

Rate ceiling is the fixed step: you cannot sample faster than `FIXED_HZ`. A requested
`rate_hz > FIXED_HZ` should **clamp and warn once**, not silently alias.

---

## 4. Clock binding

No `clock` field on `Parameter`. A channel entity carries `TimeBinding { domain }` — the
component that **already exists** and already governs how everything else reads time. Absent ⇒
the world domain. `ControlTelemetry.clock` sets it.

This is the whole answer to *"option to run it in different cycles/clock"*, and it costs one
component you already have.

---

## 5. Where samples go

Three lanes, and they are not interchangeable:

1. **Live push → subscribers.** `SampledParameter` → `sampled_param_observer` → API. Today it
   fans out at the sample rate. It needs **per-subscription decimation** (§7) so a 1 Hz
   dashboard cannot be forced to eat a 60 Hz channel.
2. **Retention → `SignalRegistry::push_scalar`.** Per-channel `ScalarHistory` ring buffer;
   this is what a plot reads, and what "how much history to store" means. **Scalars only** —
   `TelemetryValue::{Bool, String}` cannot enter a `ScalarHistory`.
3. **Discrete/eventful → `TelemetryEvent`.** The existing push bus, with `Severity`. Bool and
   String channels belong here, not in the ring buffer. *(This asymmetry is real and must be
   stated, not papered over: a `String` channel has no plot.)*

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
- **Add `UnsubscribeTelemetry`.** `TelemetrySubscriptions::unsubscribe()` already exists and is
  **unreachable** — subscriptions currently leak for the life of the process. This is a bug, not
  a feature request.

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
adapter (or a YAMCS bridge) is a thin shim over these, not a rewrite**; HTTP/WebSocket streaming
can be layered on later without touching this layer.

Two decisions that make that possible:

- **Channel key = `"<api_id>:<name>"`, never the name alone.** Names collide — two rovers both
  report `motor_current`. OpenMCT wants one opaque stable string per telemetry point; this is it,
  and it round-trips back to the owning entity.
- **Times are `sim_secs`, not `epoch_jd`.** Julian Date is ~2.46e6, so an `f64` has ~86 µs of
  resolution left there: a plot axis built on it quantises into visible stair-steps and a range
  query is sloppy at its edges. Responses carry `epoch_jd` separately for wall-clock labelling.

## 8. USD authoring

Follows the `lunco:sensor:*` convention exactly (`lunco-usd-sim:594`):

```usda
bool   lunco:telemetry           = true
token  lunco:telemetry:name      = "motor_current"        # defaults to the port/field name
token  lunco:telemetry:port      = "left_wheel.torque"    # ChannelSource::Port  (preferred)
token  lunco:telemetry:reflect   = "PhysicalPort.value"   # …or ChannelSource::Reflect
token  lunco:telemetry:unit      = "A"
double lunco:telemetry:rateHz    = 10                     # absent ⇒ settings default
bool   lunco:telemetry:enabled   = true                   # absent ⇒ TRUE (authored = live)
double lunco:telemetry:deadband  = 0.01
int    lunco:telemetry:retention = 2000                   # SAMPLES, not seconds
```

`lunco:telemetry` with neither `:port` nor `:reflect` warns and authors nothing — silently
creating a channel with no source would be a channel that can never speak.

`ChannelSource::Diagnostic` is deliberately **not** USD-authorable: a diagnostic is
engine-global, not a property of a prim. `lunco-telemetry` publishes those itself
(`spawn_engine_health_channels`), and only when the diagnostic actually exists — so a
`--no-ui` run, which links `bevy_diagnostic` but never adds `FrameTimeDiagnosticsPlugin`,
publishes no always-silent FPS channel to clutter the catalog.

USD sensors (`lunco:sensor:imu` etc.) **already emit ports**, so tagging one for telemetry is
just a `lunco:telemetry:port` pointing at it. No new sensor machinery.

---

## 9. Settings

```rust
struct TelemetrySettings {          // impl SettingsSection, KEY = "telemetry"
    default_rate_hz: f64,           // 10.0 — NOT 60; 60 is a firehose default
    default_retention: usize,       // 2000, matching ScalarHistory's default
    max_channels: usize,            // backpressure guard
    enabled: bool,
}
```

---

## 10. What else is needed — the things not asked for

These are the gaps the requirements didn't name and that will bite:

1. **Deadband / change-only sampling.** A channel that hasn't moved shouldn't spend bandwidth.
   `emit_on_change: Option<f64>` (absolute epsilon) is the single biggest bandwidth win
   available and is standard in real telemetry systems.
2. **Timestamp precision.** `SampledParameter.timestamp` is `epoch_jd: f64`. Julian Date is
   ~2.46e6 days, so an `f64` has ≈**86 µs** of resolution left — fine at 60 Hz, but it is *not*
   a high-rate timebase, and differencing two JDs to get a Δt loses most of the precision.
   Recordings should carry `sim_secs` (which starts near zero) and keep `epoch_jd` for absolute
   wall-time labelling. **This is a real defect waiting to happen.**
3. **Name collisions.** Two entities can both declare `"motor_current"`. Channels must be keyed
   by `(entity, name)` — `SignalRef { entity, path }` already has exactly this shape, and
   `SignalRef::global(path)` covers the un-owned case.
4. **Backpressure / drop policy.** What happens when a subscriber is slower than its channel?
   Decide explicitly: drop-oldest (lossy, bounded) vs. block (never). For a sim, drop-oldest.
   And **say so in a log line** — a silently-lossy telemetry feed is worse than none.
5. **Unsubscribe leak** (§7) — existing bug.
6. **Pause semantics.** On the fixed clock, sampling stops when the sim pauses. That is correct
   and comes for free — but it must be *documented*, or someone will "fix" it onto `Update`.
7. **Determinism.** Sampling on the fixed clock means a replay produces the same samples. Moving
   telemetry to `Update` would break that. This is the same failure the co-sim already had once
   (paced by the render frame); do not reintroduce it here.

---

## 11. Order

- **Phase 0 — DONE.** Plugin wired; `SubscribeTelemetry` can actually deliver.
- **Phase 1 — DONE.** `ChannelSource`, per-channel `rate_hz`, `enabled`, `deadband`, the
  domain accumulator, `TimeBinding`, `TelemetrySettings`. Closes the "60 Hz firehose, one
  global rate" gap. Plus the §10 defects: `sim_secs`, `(entity, name)` keying, the
  unreachable unsubscribe, rate clamping, and a loud backpressure cap.
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
- **Phase 4 — DONE.** `lunco:telemetry:*` USD authoring (same convention as
  `lunco:sensor:*`, and since USD sensors already emit ports, tagging one for telemetry is
  just `lunco:telemetry:port` naming it). Recording = `ExportTelemetryRecording`.
- **Phase 5 — DONE.** `ChannelSource::Diagnostic` makes FPS/frame-time real channels; the
  hand-rolled `frame_history` ring buffer in `perf_hud` is **deleted** — `bevy::Diagnostic`
  already IS a named ring buffer with a configurable depth, and `PerfStats` was shadowing
  it with a second `VecDeque` holding the identical values.

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

`register_settings_section` **auto-adds `SettingsPlugin`**, which loads `~/.lunco/settings.json`
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
write — unless it explicitly names a config dir via `LUNCOSIM_CONFIG`. A test binary is detected
by its parent directory being `deps/` (nothing legitimately runs an app from there), and
`a_test_binary_is_detected_as_such` asserts this from *inside* a test binary, so the guard fails
loudly rather than silently opening back up.

**The general rule: auto-persistence and test isolation are in direct conflict, and the default
must be the safe one.** Anything that writes to a user's home directory on a resource change must
prove it isn't a test first.

### A trap found while building Phase 1

`WorldTime::default()` has `sim_secs == 0`. An app without the `lunco-time` spine therefore
sees a clock that **never advances** — so every channel comes due once, fires, sets its next
due time, and is never due again. Telemetry silently stops after one sample. The sampler now
falls back to `Time<Fixed>`'s accumulated time when the spine is absent. Any future code that
paces itself off `WorldTime` must handle this — a defaulted clock is not a stopped clock, it
is a *frozen* one, and the two are indistinguishable at the call site.
