//! Runtime projection of Modelica state into the shared signal registry.
//!
//! A Modelica solver exposes its complete current state through
//! [`ModelicaVariables::values`]. Retaining that state is a simulation concern,
//! not a plot concern: an inspector, API client, recorder, or graph may all
//! consume the same history.  This module therefore owns the render-free
//! projection and applies the shared telemetry rate, retention, and
//! channel-limit policy.  Operator deadband is a display/notification concern;
//! it must not remove simulation-time samples from the retained model history.
//! It does not add attributes to USD or require a visualization binding.

use bevy::prelude::*;
use lunco_core::GlobalEntityId;
use lunco_signal::{SignalMeta, SignalRef, SignalRegistry, SignalSource};
use lunco_telemetry::TelemetrySettings;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use lunco_modelica_runtime::ModelicaScalarMetadata;
#[cfg(test)]
use lunco_modelica_runtime::ModelicaSignalProvenance;
use lunco_modelica_runtime::{ModelicaModel, ModelicaSignalLayout, ModelicaVariables};

/// Plugin that retains landed Modelica state in the shared telemetry registry.
///
/// The execution host installs this capability after it has configured the
/// Modelica response set. Keeping the projection here means compiler and
/// document consumers do not carry telemetry implementation code.
pub struct ModelicaTelemetryPlugin;

impl Plugin for ModelicaTelemetryPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<lunco_telemetry::TelemetryRetentionPlugin>() {
            app.add_plugins(lunco_telemetry::TelemetryRetentionPlugin);
        }
        app.init_resource::<RuntimeTelemetrySessions>().add_systems(
            Update,
            retain_modelica_runtime_state
                .after(lunco_core::RuntimeCycleSet::Lifecycle)
                .run_if(modelica_telemetry_inputs_changed),
        );
    }
}

/// Modelica telemetry changes only when its source sample or metadata contract
/// changes. Keep that projection off render-only frames while still observing
/// compiled metadata, projected identity, and settings independently of samples.
fn modelica_telemetry_inputs_changed(
    models: Query<
        (),
        Or<(
            Changed<ModelicaModel>,
            Changed<ModelicaVariables>,
            Changed<ModelicaSignalLayout>,
        )>,
    >,
    settings: Option<Res<TelemetrySettings>>,
) -> bool {
    !models.is_empty() || settings.is_some_and(|settings| settings.is_changed())
}

/// Runtime state retained for each Modelica participant.
///
/// The session id is the solver's authoritative lifecycle boundary.  When it
/// changes, histories from the previous solver session are cleared before the
/// new initial state is retained.  Signal identity remains `(entity, name)` so
/// UI and API consumers do not need a second Modelica-specific address scheme.
#[derive(Resource, Default)]
pub struct RuntimeTelemetrySessions {
    sessions: HashMap<Entity, RuntimeTelemetrySession>,
}

#[derive(Default)]
struct RuntimeTelemetrySession {
    session_id: u64,
    signals: HashMap<String, RuntimeSignalCache>,
    metadata: Option<Arc<BTreeMap<String, ModelicaScalarMetadata>>>,
    /// Last model-time sample attempted for this solver session. The shared
    /// registry remains the final per-channel authority; this batch cursor
    /// only avoids rebuilding the same variable list between due samples.
    last_sample_time: Option<f64>,
    /// The registry owns archived wire identity. This cursor admits reuse only
    /// while its catalog and the source's live owner remain unchanged.
    global_owner: Option<GlobalEntityId>,
    owner_catalog_revision: Option<u64>,
}

struct RuntimeSignalCache {
    signal: SignalRef,
    retained: bool,
    owner_published: bool,
    /// The registry owns metadata; the producer only tracks its publication.
    metadata_published: bool,
}

/// Retain the current variables of every live Modelica solver.
///
/// The system runs in `Update`, after worker admission in `First` and the
/// lifecycle cycle, where
/// [`ModelicaModel::current_time`] and [`ModelicaVariables::values`] describe the
/// same landed solver result.  Sampling is paced by model time rather than by
/// render frames, so a fast UI cannot inflate the recorded rate and a headless
/// run follows the same policy.
pub fn retain_modelica_runtime_state(
    mut commands: Commands,
    settings: Option<Res<TelemetrySettings>>,
    mut signals: Option<ResMut<SignalRegistry>>,
    mut sessions: ResMut<RuntimeTelemetrySessions>,
    models: Query<(
        Entity,
        &ModelicaModel,
        &ModelicaVariables,
        Option<Ref<ModelicaSignalLayout>>,
        Option<&GlobalEntityId>,
        Option<&SignalSource>,
    )>,
) {
    let Some(settings) = settings else {
        return;
    };
    if !settings.enabled {
        return;
    }
    if !settings.default_rate_hz.is_finite() || settings.default_rate_hz <= 0.0 {
        warn_once!(
            "modelica telemetry: invalid default rate {}; runtime state retention is skipped until it is corrected",
            settings.default_rate_hz
        );
        return;
    }
    let Some(signals) = signals.as_deref_mut() else {
        // The projection is optional for a host that intentionally does not
        // install telemetry.  It must not fabricate a private fallback store.
        return;
    };

    // The registry owns the channel catalog. Snapshot its size once per pass;
    // the registry provides this count without walking every retained history.
    let mut channel_count = signals.scalar_count();

    for (entity, model, variables, layout, global_owner, signal_source) in &models {
        let session = sessions.sessions.entry(entity).or_default();
        if session.session_id != model.session_id {
            for cached in session.signals.values().filter(|cached| cached.retained) {
                signals.clear_history(&cached.signal);
            }
            session.signals.clear();
            session.session_id = model.session_id;
            session.metadata = None;
            session.last_sample_time = None;
            session.global_owner = None;
            session.owner_catalog_revision = None;
        }

        let metadata_dirty = match (&session.metadata, &model.variable_metadata) {
            (Some(previous), Some(current)) => !Arc::ptr_eq(previous, current),
            (None, None) => false,
            _ => true,
        } || layout.as_ref().is_some_and(|layout| layout.is_changed());
        if metadata_dirty {
            session.metadata = model.variable_metadata.clone();
        }

        let sample_due = session.last_sample_time.is_none_or(|last| {
            model.current_time < last || model.current_time - last >= 1.0 / settings.default_rate_hz
        });
        if !sample_due && !metadata_dirty {
            continue;
        }

        let owner_dirty = session.global_owner != global_owner.copied()
            || session.owner_catalog_revision != Some(signals.catalog_revision());
        if owner_dirty {
            for cached in session.signals.values_mut() {
                cached.owner_published = false;
            }
        }
        let mut retained_any = false;
        for (name, &value) in &variables.values {
            if !value.is_finite() || name.is_empty() {
                continue;
            }

            let cached = match session.signals.get_mut(name) {
                Some(cached) => cached,
                None => {
                    let signal = SignalRef::new(entity, name.as_str());
                    if channel_count >= settings.max_channels
                        && signals.scalar_history(&signal).is_none()
                    {
                        warn_once!(
                            "modelica telemetry: max_channels ({}) reached; additional runtime variables are not retained",
                            settings.max_channels
                        );
                        continue;
                    }
                    session.signals.insert(
                        name.clone(),
                        RuntimeSignalCache {
                            signal,
                            retained: false,
                            owner_published: false,
                            metadata_published: false,
                        },
                    );
                    session
                        .signals
                        .get_mut(name)
                        .expect("runtime signal identity was just retained")
                }
            };
            let signal = &cached.signal;
            // Below the catalog limit every channel can record. Only a full
            // catalog needs to distinguish existing histories from admissions;
            // read its live count after append instead of hashing every path.
            if channel_count >= settings.max_channels && signals.scalar_history(signal).is_none() {
                warn_once!(
                    "modelica telemetry: max_channels ({}) reached; additional runtime variables are not retained",
                    settings.max_channels
                );
                continue;
            }
            if let Some(owner) = global_owner {
                if !cached.owner_published || !cached.retained {
                    signals.associate_global_owner(signal, *owner);
                    cached.owner_published = true;
                }
            }

            // A checked FMI inventory changes only when the worker installs
            // a new component. Draft edits never alter a live solver's facts.
            if metadata_dirty || !cached.metadata_published {
                let _span = bevy::log::debug_span!("modelica_telemetry_channel_metadata").entered();
                let meta = model_signal_meta(model, layout.as_deref(), name);
                signals.update_meta(signal.clone(), meta);
                cached.metadata_published = true;
            }

            // The shared signal registry owns due-time and time-reversal for
            // every runtime producer. Modelica histories are complete at the
            // recording rate; notification deadband must not remove elapsed
            // simulation-time samples from a graph.
            let retained = signals.record_scalar_at_rate(
                signal,
                model.current_time,
                value,
                settings.default_rate_hz,
                settings.default_retention,
            );
            if retained {
                cached.retained = true;
                retained_any = true;
                channel_count = signals.scalar_count();
            }
        }

        session.global_owner = global_owner.copied();
        session.owner_catalog_revision = Some(signals.catalog_revision());
        if retained_any && signal_source.is_none() {
            commands.entity(entity).try_insert(SignalSource);
        }
        if sample_due && model.current_time.is_finite() {
            session.last_sample_time = Some(model.current_time);
        }
    }
}

fn model_signal_meta(
    model: &ModelicaModel,
    layout: Option<&ModelicaSignalLayout>,
    name: &str,
) -> SignalMeta {
    let compiled = model
        .variable_metadata
        .as_ref()
        .and_then(|metadata| metadata.get(name));
    let modelica_provenance = layout.and_then(|layout| layout.provenance(name));
    let model_variable = modelica_provenance
        .as_ref()
        .and_then(|identity| identity.model_variable.clone())
        .or_else(|| (!name.is_empty()).then(|| name.to_string()));

    let model_class = modelica_provenance
        .as_ref()
        .and_then(|identity| identity.model_class.clone())
        .or_else(|| (!model.model_name.is_empty()).then(|| model.model_name.clone()));
    let source_asset = modelica_provenance
        .as_ref()
        .and_then(|identity| identity.source_asset.clone())
        .or_else(|| (!model.source_uri.is_empty()).then(|| model.source_uri.clone()));

    SignalMeta {
        description: compiled.and_then(|entry| entry.description.clone()),
        unit: compiled.and_then(|entry| entry.unit.clone()),
        provenance: Some("modelica".to_string()),
        group_path: layout
            .and_then(|layout| layout.group_path(name))
            .map(str::to_owned),
        exposure: layout
            .map(|layout| layout.exposure(name))
            .unwrap_or_default(),
        // Generated participants provide authored member provenance through
        // `ModelicaSignalLayout`. Standalone Modelica participants do not
        // have that layout, but their runtime component still owns the
        // authoritative model name, source URI, and solver variable name.
        // Retain those facts here so every Modelica producer has inspectable
        // metadata without a model-specific registration path.
        model_class,
        model_variable,
        source_asset,
        canonical_name: modelica_provenance.and_then(|identity| identity.canonical_name),
        presentation: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::app::Update;

    fn app_with_model(model: ModelicaModel, variables: ModelicaVariables) -> (App, Entity) {
        let mut app = App::new();
        app.insert_resource(TelemetrySettings::default());
        app.insert_resource(SignalRegistry::default());
        app.init_resource::<RuntimeTelemetrySessions>();
        app.add_systems(
            Update,
            retain_modelica_runtime_state.run_if(modelica_telemetry_inputs_changed),
        );
        let entity = app
            .world_mut()
            .spawn((model, variables, ModelicaSignalLayout::default()))
            .id();
        (app, entity)
    }

    #[test]
    fn runtime_state_is_retained_without_a_plot_binding() {
        let model = ModelicaModel {
            model_name: "LunCo.Test.Sensor".into(),
            source_uri: "lunco://models/LunCo/Test/Sensor.mo".into(),
            current_time: 0.0,
            ..Default::default()
        };
        let mut variables = ModelicaVariables::default();
        variables.values.insert("soc".to_string(), 0.95);
        let (mut app, entity) = app_with_model(model, variables);

        app.update();

        let signal = SignalRef::new(entity, "soc");
        let history = app
            .world()
            .resource::<SignalRegistry>()
            .scalar_history(&signal)
            .expect("runtime Modelica state must be inspectable");
        let sample = history.back().expect("initial state sample");
        assert_eq!((sample.time, sample.value), (0.0, 0.95));
        assert_eq!(
            app.world()
                .resource::<SignalRegistry>()
                .meta(&signal)
                .unwrap()
                .provenance
                .as_deref(),
            Some("modelica")
        );
        let meta = app
            .world()
            .resource::<SignalRegistry>()
            .meta(&signal)
            .unwrap();
        assert_eq!(meta.model_class.as_deref(), Some("LunCo.Test.Sensor"));
        assert_eq!(meta.model_variable.as_deref(), Some("soc"));
        assert_eq!(
            meta.source_asset.as_deref(),
            Some("lunco://models/LunCo/Test/Sensor.mo")
        );
        assert!(app.world().entity(entity).contains::<SignalSource>());
    }

    #[test]
    fn checked_scalar_metadata_is_used_for_generated_channel_names() {
        let mut model = ModelicaModel::default();
        let mut metadata = BTreeMap::new();
        metadata.insert(
            "member_current".to_string(),
            ModelicaScalarMetadata {
                description: Some("Current delivered by the member".to_string()),
                unit: Some("A".to_string()),
                ..Default::default()
            },
        );

        model.variable_metadata = Some(Arc::new(metadata));
        let meta = model_signal_meta(&model, None, "member_current");
        assert_eq!(meta.unit.as_deref(), Some("A"));
        assert_eq!(
            meta.description.as_deref(),
            Some("Current delivered by the member")
        );
    }

    #[test]
    fn projected_modelica_identity_is_retained_for_solver_names() {
        let model = ModelicaModel::default();
        let mut layout = ModelicaSignalLayout::default();
        layout.provenance_prefixes.push((
            "unit.Camera.".to_string(),
            ModelicaSignalProvenance {
                source_asset: Some("lunco://models/LunCo/Electrical/CameraPayload.mo".into()),
                model_class: Some("LunCo.Electrical.CameraPayload".into()),
                ..default()
            },
        ));

        let meta = model_signal_meta(&model, Some(&layout), "unit.Camera.power_draw_w");
        assert_eq!(
            meta.model_class.as_deref(),
            Some("LunCo.Electrical.CameraPayload")
        );
        assert_eq!(meta.model_variable.as_deref(), Some("power_draw_w"));
        assert_eq!(
            meta.source_asset.as_deref(),
            Some("lunco://models/LunCo/Electrical/CameraPayload.mo")
        );
    }

    #[test]
    fn runtime_state_uses_rate_and_retains_steady_time_samples() {
        let model = ModelicaModel::default();
        let mut variables = ModelicaVariables::default();
        variables.values.insert("speed".to_string(), 1.0);
        let (mut app, entity) = app_with_model(model, variables);
        let signal = SignalRef::new(entity, "speed");

        app.update();
        {
            let mut model = app.world_mut().entity_mut(entity);
            let mut value = model.get_mut::<ModelicaModel>().unwrap();
            value.current_time = 0.01;
            drop(value);
            model
                .get_mut::<ModelicaVariables>()
                .unwrap()
                .values
                .insert("speed".to_string(), 2.0);
        }
        app.update();
        assert_eq!(
            app.world()
                .resource::<SignalRegistry>()
                .scalar_history(&signal)
                .unwrap()
                .len(),
            1
        );

        {
            let mut model = app.world_mut().entity_mut(entity);
            let mut value = model.get_mut::<ModelicaModel>().unwrap();
            value.current_time = 0.2;
            drop(value);
            model
                .get_mut::<ModelicaVariables>()
                .unwrap()
                .values
                .insert("speed".to_string(), 2.0);
        }
        app.update();
        let history = app
            .world()
            .resource::<SignalRegistry>()
            .scalar_history(&signal)
            .unwrap();
        assert_eq!(
            history.len(),
            2,
            "a meaningful change is retained once the rate is due"
        );

        {
            let mut model = app.world_mut().entity_mut(entity);
            let mut value = model.get_mut::<ModelicaModel>().unwrap();
            value.current_time = 0.4;
            drop(value);
            model
                .get_mut::<ModelicaVariables>()
                .unwrap()
                .values
                .insert("speed".to_string(), 2.0);
        }
        app.update();
        assert_eq!(
            app.world()
                .resource::<SignalRegistry>()
                .scalar_history(&signal)
                .unwrap()
                .len(),
            3,
            "a steady value still needs a simulation-time sample for graphs"
        );

        {
            let mut model = app.world_mut().entity_mut(entity);
            let mut value = model.get_mut::<ModelicaModel>().unwrap();
            value.current_time = 0.61;
            drop(value);
            model
                .get_mut::<ModelicaVariables>()
                .unwrap()
                .values
                .insert("speed".to_string(), 2.1);
        }
        app.update();
        assert_eq!(
            app.world()
                .resource::<SignalRegistry>()
                .scalar_history(&signal)
                .unwrap()
                .len(),
            4
        );
    }

    #[test]
    fn a_new_solver_session_clears_previous_history() {
        let model = ModelicaModel {
            session_id: 1,
            current_time: 1.0,
            ..Default::default()
        };
        let mut variables = ModelicaVariables::default();
        variables.values.insert("x".to_string(), 4.0);
        let (mut app, entity) = app_with_model(model, variables);
        let signal = SignalRef::new(entity, "x");
        app.update();

        {
            let mut model = app.world_mut().entity_mut(entity);
            let mut value = model.get_mut::<ModelicaModel>().unwrap();
            value.session_id = 2;
            value.current_time = 0.0;
            drop(value);
            model
                .get_mut::<ModelicaVariables>()
                .unwrap()
                .values
                .insert("x".to_string(), 8.0);
        }
        app.update();

        let history = app
            .world()
            .resource::<SignalRegistry>()
            .scalar_history(&signal)
            .unwrap();
        assert_eq!(history.len(), 1);
        let sample = history.back().expect("new session sample");
        assert_eq!((sample.time, sample.value), (0.0, 8.0));
    }

    #[test]
    fn owner_publication_tracks_channel_lifecycle_and_source_identity() {
        let mut variables = ModelicaVariables::default();
        variables.values.insert("x".into(), 1.0);
        let mut model = ModelicaModel::default();
        model.current_time = f64::NAN;
        let (mut app, entity) = app_with_model(model, variables);
        let first_owner = GlobalEntityId::from_raw(42);
        app.world_mut().entity_mut(entity).insert(first_owner);
        app.update();
        let x = SignalRef::new(entity, "x");
        assert_eq!(
            app.world().resource::<SignalRegistry>().global_owner(&x),
            Some(first_owner)
        );

        assert!(
            app.world()
                .resource::<SignalRegistry>()
                .scalar_history(&x)
                .is_none()
        );
        app.world_mut()
            .resource_mut::<SignalRegistry>()
            .remove_signal(&x);
        app.world_mut()
            .get_mut::<ModelicaModel>(entity)
            .unwrap()
            .current_time = 0.0;
        app.update();
        assert_eq!(
            app.world().resource::<SignalRegistry>().global_owner(&x),
            Some(first_owner)
        );
        assert!(
            app.world()
                .resource::<SignalRegistry>()
                .scalar_history(&x)
                .is_some()
        );

        app.world_mut()
            .resource_mut::<SignalRegistry>()
            .remove_signal(&x);
        assert_eq!(
            app.world().resource::<SignalRegistry>().global_owner(&x),
            None
        );
        app.world_mut()
            .get_mut::<ModelicaModel>(entity)
            .unwrap()
            .current_time = 0.25;
        app.update();
        assert_eq!(
            app.world().resource::<SignalRegistry>().global_owner(&x),
            Some(first_owner)
        );

        app.world_mut()
            .get_mut::<ModelicaVariables>(entity)
            .unwrap()
            .values
            .insert("y".into(), 2.0);
        app.world_mut()
            .get_mut::<ModelicaModel>(entity)
            .unwrap()
            .current_time = 0.5;
        app.update();
        let y = SignalRef::new(entity, "y");
        assert_eq!(
            app.world().resource::<SignalRegistry>().global_owner(&y),
            Some(first_owner)
        );

        let next_owner = GlobalEntityId::from_raw(99);
        app.world_mut().entity_mut(entity).insert(next_owner);
        app.world_mut()
            .get_mut::<ModelicaModel>(entity)
            .unwrap()
            .current_time = 0.75;
        app.update();
        for signal in [&x, &y] {
            assert_eq!(
                app.world()
                    .resource::<SignalRegistry>()
                    .global_owner(signal),
                Some(next_owner)
            );
        }

        // A skipped value must not acknowledge a batch's owner/catalog change.
        app.world_mut()
            .get_mut::<ModelicaVariables>(entity)
            .unwrap()
            .values
            .insert("x".into(), f64::NAN);
        let final_owner = GlobalEntityId::from_raw(123);
        app.world_mut().entity_mut(entity).insert(final_owner);
        app.world_mut()
            .get_mut::<ModelicaModel>(entity)
            .unwrap()
            .current_time = 1.0;
        app.update();
        assert_eq!(
            app.world().resource::<SignalRegistry>().global_owner(&x),
            Some(next_owner)
        );
        app.world_mut()
            .get_mut::<ModelicaVariables>(entity)
            .unwrap()
            .values
            .insert("x".into(), 3.0);
        app.world_mut()
            .get_mut::<ModelicaModel>(entity)
            .unwrap()
            .current_time = 1.25;
        app.update();
        assert_eq!(
            app.world().resource::<SignalRegistry>().global_owner(&x),
            Some(final_owner)
        );

        app.world_mut()
            .get_mut::<ModelicaVariables>(entity)
            .unwrap()
            .values
            .insert("x".into(), f64::NAN);
        app.world_mut()
            .resource_mut::<SignalRegistry>()
            .remove_signal(&x);
        app.world_mut()
            .get_mut::<ModelicaModel>(entity)
            .unwrap()
            .current_time = 1.5;
        app.update();
        app.world_mut()
            .get_mut::<ModelicaVariables>(entity)
            .unwrap()
            .values
            .insert("x".into(), 4.0);
        app.world_mut()
            .get_mut::<ModelicaModel>(entity)
            .unwrap()
            .current_time = 1.75;
        app.update();
        assert_eq!(
            app.world().resource::<SignalRegistry>().global_owner(&x),
            Some(final_owner)
        );

        app.world_mut()
            .entity_mut(entity)
            .remove::<GlobalEntityId>();
        app.world_mut()
            .get_mut::<ModelicaModel>(entity)
            .unwrap()
            .current_time = 2.0;
        app.update();
        for signal in [&x, &y] {
            assert_eq!(
                app.world()
                    .resource::<SignalRegistry>()
                    .global_owner(signal),
                Some(final_owner)
            );
        }
    }

    #[test]
    fn channel_limit_uses_live_registry_admission_after_removal() {
        let model = ModelicaModel::default();
        let mut variables = ModelicaVariables::default();
        variables.values.insert("a".into(), 1.0);
        variables.values.insert("b".into(), 2.0);
        let (mut app, entity) = app_with_model(model, variables);
        app.world_mut()
            .resource_mut::<TelemetrySettings>()
            .max_channels = 1;
        app.update();
        let signal = {
            let registry = app.world().resource::<SignalRegistry>();
            assert_eq!(registry.scalar_count(), 1);
            registry.iter_scalar().next().unwrap().0.clone()
        };

        // Lowering the limit stops admissions, not the existing recorder.
        app.world_mut()
            .resource_mut::<TelemetrySettings>()
            .max_channels = 0;
        app.world_mut()
            .get_mut::<ModelicaModel>(entity)
            .unwrap()
            .current_time = 0.2;
        app.update();
        assert_eq!(app.world().resource::<SignalRegistry>().scalar_count(), 1);
        assert_eq!(
            app.world()
                .resource::<SignalRegistry>()
                .scalar_history(&signal)
                .unwrap()
                .len(),
            2
        );

        // A cached producer identity must not bypass a removed history's cap.
        app.world_mut()
            .resource_mut::<SignalRegistry>()
            .remove_signal(&signal);
        app.world_mut()
            .get_mut::<ModelicaModel>(entity)
            .unwrap()
            .current_time = 0.4;
        app.update();
        assert_eq!(app.world().resource::<SignalRegistry>().scalar_count(), 0);

        app.world_mut()
            .resource_mut::<TelemetrySettings>()
            .max_channels = 1;
        app.world_mut()
            .get_mut::<ModelicaModel>(entity)
            .unwrap()
            .current_time = 0.61;
        app.update();
        let registry = app.world().resource::<SignalRegistry>();
        assert_eq!(registry.scalar_count(), 1);
        let history = registry.iter_scalar().next().unwrap().1;
        assert_eq!(history.len(), 1);
        assert_eq!(history.back().unwrap().time, 0.61);
    }
}
