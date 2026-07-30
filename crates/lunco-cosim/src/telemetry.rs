//! Co-simulation variables → retained telemetry, with no per-variable authoring.
//!
//! # Why this exists
//!
//! A Modelica model's *internal* variables (`soc`, `omega`, `T_case`, every
//! `Real` the solver observes) are already on the entity: the worker reports them
//! as [`ModelicaModel::variables`](lunco_modelica::worker::ModelicaModel), and
//! [`sync_modelica_outputs`](lunco_usd_sim) copies the whole map into
//! [`SimComponent::outputs`]. So they were *readable* — but nothing kept a
//! HISTORY of them, which is what a plot needs.
//!
//! The authored path ([`lunco_core::telemetry::Parameter`], `lunco:telemetry` in
//! USD) cannot cover them: `Parameter` is a Component, so an entity carries at
//! most ONE channel, and a model has dozens of variables. Authoring a channel
//! entity per variable of every model on every vessel is not a thing anyone will
//! do, and the moment a model gains a state it would be missing from the plots.
//!
//! So this publishes the model's own variables **wholesale**, at a fixed rate,
//! into the same [`SignalRegistry`] ring buffers every plot surface already reads.
//! Generic over [`SimComponent`], so a scripted or FMU-backed model gets the same
//! treatment as a Modelica one — this layer never names Modelica.
//!
//! # It cannot clobber an authored channel
//!
//! Auto-published variables use the [`VARIABLE_NAMESPACE`] prefix as a safe
//! fallback (`sim.soc`, `sim.omega`). Generated USD networks replace that
//! fallback with a canonical authored component path (for example
//! `Battery.soc_out`) using the synthesis mapping. Authored channels retain
//! their own mnemonic, so independent producers never silently interleave.
//!
//! # Rate, retention, and what it costs
//!
//! [`CosimTelemetrySettings`] defaults to **5 Hz for 5 minutes** — 1500 samples
//! per variable, which is the depth at which the ring buffer does NOT wrap inside
//! the window you asked to see. (Retention is in SAMPLES because that is what
//! bounds memory; seconds are `retention / rate_hz`.)
//!
//! One sample is a `(f64, f64)` = 16 B, so:
//!
//! ```text
//!   per variable : 1500 × 16 B          ≈  24 KB
//!   one model    : ~20 variables        ≈ 480 KB
//!   a rover      : ~7 models            ≈ 3.4 MB
//!   the cap      : 4096 variables       ≈  98 MB   ← never reached in practice
//! ```
//!
//! plus per-signal `HashMap` overhead (a `SignalRef` key: entity + a short
//! `String`), which is tens of bytes against 24 KB of samples — noise.
//!
//! The cap exists so a pathological scene (hundreds of models) degrades loudly
//! rather than eating the heap: past [`CosimTelemetrySettings::max_channels`] no
//! NEW variable is admitted, and it says so once. Already-admitted variables keep
//! streaming — truncating a running plot is worse than refusing a new one.
//!
//! # Clock
//!
//! Paced and stamped on `Time<Fixed>`, the simulation clock: a paused sim
//! publishes nothing, and a warped one publishes at 5 samples per SIM second, not
//! per wall second. Sampling a co-simulated model off the wall clock would alias
//! against the very stepper it is watching.

use bevy::prelude::*;
use lunco_signal::{SignalMeta, SignalRef, SignalRegistry, SignalSource};

use crate::component::{CosimOutputMetadata, SimComponent};

/// Prefix every auto-published co-sim variable carries in the registry. See the
/// module docs: it is what keeps this producer out of the authored channels'
/// buffers.
pub const VARIABLE_NAMESPACE: &str = "sim.";

/// Rate / depth / cap for the wholesale co-sim variable publisher.
#[derive(Resource, Debug, Clone)]
pub struct CosimTelemetrySettings {
    /// Master switch. Off ⇒ the system returns immediately and no history exists;
    /// authored channels are unaffected (they are a separate producer).
    pub enabled: bool,
    /// Samples per SIM second, per variable.
    pub rate_hz: f64,
    /// Ring-buffer depth per variable, in samples. `rate_hz × window_seconds`.
    pub retention: usize,
    /// Refuse to admit new variables past this many auto-published signals.
    pub max_channels: usize,
}

impl Default for CosimTelemetrySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            // 5 Hz for 5 minutes: 1500 samples ≈ 24 KB per variable. See the
            // module docs for the memory arithmetic — the pair is chosen
            // together, and raising the rate without raising the depth silently
            // shortens the window the plot can show.
            rate_hz: 5.0,
            retention: 1500,
            max_channels: 4096,
        }
    }
}

/// Pacing state for [`publish_cosim_variables`] — kept in a resource rather than
/// a local so a `Time<Fixed>` reset (scene reload) can't leave a due-time in the
/// future that stalls publishing forever.
#[derive(Resource, Debug, Default)]
pub struct CosimTelemetryClock {
    /// Next sim-time at which to sample. `0.0` ⇒ sample on the next tick.
    next_due: f64,
    /// Signals this producer has admitted, so the cap is O(1) instead of a
    /// registry scan at the sample rate.
    admitted: usize,
    /// The cap has been reported once.
    capped: bool,
}

impl CosimTelemetryClock {
    /// Number of variables currently admitted — for tests and diagnostics.
    pub fn admitted(&self) -> usize {
        self.admitted
    }
}

/// Publish every co-simulated component's variables into the [`SignalRegistry`].
///
/// Runs in `FixedUpdate` (the sim clock's schedule) and self-paces to
/// [`CosimTelemetrySettings::rate_hz`]; the fixed rate is the ceiling.
pub fn publish_cosim_variables(
    time: Res<Time<Fixed>>,
    settings: Res<CosimTelemetrySettings>,
    mut clock: ResMut<CosimTelemetryClock>,
    mut signals: ResMut<SignalRegistry>,
    q: Query<(Entity, &SimComponent, Option<&CosimOutputMetadata>)>,
    q_unmarked: Query<(), Without<SignalSource>>,
    mut commands: Commands,
) {
    if !settings.enabled || settings.rate_hz <= 0.0 {
        return;
    }
    let now = time.elapsed_secs_f64();
    // A clock that went backwards (scene reload, sim reset) must not leave the
    // producer waiting out the old timeline.
    if clock.next_due > now + 1.0 / settings.rate_hz {
        clock.next_due = 0.0;
    }
    if now < clock.next_due {
        return;
    }
    let period = 1.0 / settings.rate_hz;
    // Advance on the grid rather than from `now`, so the cadence doesn't drift
    // with the fixed-step remainder; catch up in one jump after a long stall
    // instead of firing a burst of back-dated samples.
    clock.next_due = (clock.next_due + period).max(now + period);

    for (entity, comp, output_metadata) in &q {
        if comp.outputs.is_empty() {
            continue;
        }
        // Tag the owner ONCE so `drop_signals_of_removed_source` frees these
        // buffers when the vessel despawns — this producer must not be the reason
        // a dead rover's history lingers.
        for (name, value) in &comp.outputs {
            if q_unmarked.get(entity).is_ok() {
                commands.entity(entity).try_insert(SignalSource);
            }
            let metadata = output_metadata.and_then(|metadata| metadata.outputs.get(name));
            let signal_name = metadata
                .and_then(|entry| entry.canonical_name.clone())
                .unwrap_or_else(|| format!("{VARIABLE_NAMESPACE}{name}"));
            // The runtime projection owns the signal lifecycle.  Its metadata
            // carries the composed USD presentation path, which the browser
            // uses to place it under the authored part without rebinding the
            // signal to a potentially duplicated instance entity.
            let sig = SignalRef::new(entity, signal_name);
            let known = signals.scalar_history(&sig).is_some();
            if !known {
                // `admitted` is a RATCHET, and the buffers it counts are not:
                // `drop_signals_of_removed_source` frees a despawned vessel's
                // signals without telling this counter, so every scene reload
                // leaves it overstating the truth. Left alone it reaches the cap
                // after enough reloads and the publisher refuses every new
                // variable FOREVER, in a session whose registry is nearly empty —
                // one log line, then no cosim history at all.
                //
                // Recount from the registry, but only at the boundary: the O(1)
                // counter still carries the steady path, and the scan happens at
                // most once per sample tick while genuinely at the cap.
                if clock.admitted >= settings.max_channels {
                    clock.admitted = signals
                        .iter_scalar()
                        .filter(|(sig, _)| sig.path.starts_with(VARIABLE_NAMESPACE))
                        .count();
                    if clock.admitted < settings.max_channels {
                        clock.capped = false;
                    }
                }
                if clock.admitted >= settings.max_channels {
                    if !clock.capped {
                        clock.capped = true;
                        warn!(
                            "cosim telemetry: {} auto-published variables reached — \
                             new variables will not be recorded. Raise \
                             CosimTelemetrySettings::max_channels or disable the \
                             wholesale publisher.",
                            settings.max_channels
                        );
                    }
                    continue;
                }
                clock.admitted += 1;
                signals.update_meta(
                    sig.clone(),
                    SignalMeta {
                        description: metadata.and_then(|entry| entry.description.clone()),
                        unit: metadata.and_then(|entry| entry.unit.clone()),
                        provenance: Some(
                            metadata
                                .map(|entry| entry.provenance.clone())
                                .unwrap_or_else(|| "cosim".into()),
                        ),
                        group_path: metadata.and_then(|entry| entry.group_path.clone()),
                    },
                );
            }
            signals.push_scalar_with_capacity(sig, now, *value, settings.retention);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    /// Wall-clock ms banked per `app.update()`. Larger than one fixed step so a
    /// handful of updates crosses several `FixedUpdate` boundaries.
    const UPDATE_MS: u64 = 20;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        // Without a manual strategy the test's `Time` barely advances, so
        // `FixedUpdate` — where this system lives — may never run at all.
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            UPDATE_MS,
        )));
        app.init_resource::<SignalRegistry>();
        app.init_resource::<CosimTelemetrySettings>();
        app.init_resource::<CosimTelemetryClock>();
        app.add_systems(FixedUpdate, publish_cosim_variables);
        app
    }

    /// Run enough updates to cross at least `n` fixed-step boundaries.
    ///
    /// NOT the same as `n` calls to `app.update()`: the fixed accumulator starts
    /// empty, so the first update banks time without necessarily crossing a
    /// boundary and runs `FixedUpdate` zero times (the same trap
    /// `lunco-telemetry`'s tests document).
    fn step_fixed(app: &mut App, n: usize) {
        for _ in 0..=n {
            app.update();
        }
    }

    fn with_outputs(app: &mut App, pairs: &[(&str, f64)]) -> Entity {
        let mut comp = SimComponent::default();
        for (k, v) in pairs {
            comp.outputs.insert((*k).to_string(), *v);
        }
        app.world_mut().spawn(comp).id()
    }

    #[test]
    fn variables_land_under_the_sim_namespace() {
        let mut app = app();
        let e = with_outputs(&mut app, &[("soc", 0.9)]);
        step_fixed(&mut app, 2);

        let reg = app.world().resource::<SignalRegistry>();
        assert!(
            reg.scalar_history(&SignalRef::new(e, "sim.soc")).is_some(),
            "a model variable must be published under the namespace"
        );
        assert!(
            reg.scalar_history(&SignalRef::new(e, "soc")).is_none(),
            "the bare name belongs to authored channels — this producer must not take it"
        );
    }

    #[test]
    fn publisher_uses_authored_output_metadata_without_inventing_a_description() {
        let mut app = app();
        let entity = with_outputs(&mut app, &[("temperature", 280.0), ("undocumented", 1.0)]);
        app.world_mut()
            .entity_mut(entity)
            .insert(CosimOutputMetadata {
                outputs: std::collections::HashMap::from([(
                    "temperature".to_string(),
                    crate::component::CosimOutputDescriptor {
                        description: Some("Motor case temperature".to_string()),
                        unit: Some("K".to_string()),
                        provenance: "modelica".to_string(),
                        canonical_name: None,
                        group_path: None,
                    },
                )]),
            });
        step_fixed(&mut app, 2);

        let registry = app.world().resource::<SignalRegistry>();
        assert_eq!(
            registry.meta(&SignalRef::new(entity, "sim.temperature")),
            Some(&SignalMeta {
                description: Some("Motor case temperature".to_string()),
                unit: Some("K".to_string()),
                provenance: Some("modelica".to_string()),
                group_path: None,
            })
        );
        assert_eq!(
            registry
                .meta(&SignalRef::new(entity, "sim.undocumented"))
                .and_then(|meta| meta.description.as_deref()),
            None,
            "an undocumented model output must remain undocumented"
        );
    }

    #[test]
    fn an_authored_channels_buffer_is_never_touched() {
        let mut app = app();
        let e = with_outputs(&mut app, &[("torque", 1.0)]);
        // Stand in for the authored `Parameter` channel: same entity, same
        // mnemonic as a model variable.
        app.world_mut()
            .resource_mut::<SignalRegistry>()
            .push_scalar(SignalRef::new(e, "torque"), 0.0, 42.0);
        step_fixed(&mut app, 2);

        let reg = app.world().resource::<SignalRegistry>();
        let authored = reg.scalar_history(&SignalRef::new(e, "torque")).unwrap();
        assert_eq!(authored.len(), 1, "the authored buffer must be untouched");
        assert_eq!(authored.samples.back().unwrap().value, 42.0);
    }

    #[test]
    fn retention_is_deep_enough_that_the_window_does_not_wrap() {
        let s = CosimTelemetrySettings::default();
        let window_s = s.retention as f64 / s.rate_hz;
        assert!(
            window_s >= 300.0,
            "default retention must hold the full 5-minute window, got {window_s}s"
        );
    }

    #[test]
    fn the_cap_refuses_new_variables_but_keeps_the_admitted_ones_streaming() {
        let mut app = app();
        app.world_mut()
            .resource_mut::<CosimTelemetrySettings>()
            .max_channels = 1;
        let e = with_outputs(&mut app, &[("a", 1.0)]);
        step_fixed(&mut app, 2);
        // A second variable appears after the cap is reached.
        app.world_mut()
            .entity_mut(e)
            .get_mut::<SimComponent>()
            .unwrap()
            .outputs
            .insert("b".into(), 2.0);
        // Force the next sample to be due.
        app.world_mut()
            .resource_mut::<CosimTelemetryClock>()
            .next_due = 0.0;
        step_fixed(&mut app, 2);

        let reg = app.world().resource::<SignalRegistry>();
        assert!(reg.scalar_history(&SignalRef::new(e, "sim.a")).is_some());
        assert!(
            reg.scalar_history(&SignalRef::new(e, "sim.b")).is_none(),
            "past the cap a NEW variable is refused"
        );
    }

    #[test]
    fn the_owner_is_tagged_so_its_history_dies_with_it() {
        let mut app = app();
        let e = with_outputs(&mut app, &[("x", 1.0)]);
        step_fixed(&mut app, 2);
        assert!(
            app.world().entity(e).contains::<SignalSource>(),
            "the publisher must mark the owner for despawn cleanup"
        );
    }
}
