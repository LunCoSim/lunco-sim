//! Runtime projection of Modelica state into the shared signal registry.
//!
//! A Modelica solver exposes its complete current state through
//! [`ModelicaModel::variables`].  Retaining that state is a simulation concern,
//! not a plot concern: an inspector, API client, recorder, or graph may all
//! consume the same history.  This module therefore owns the render-free
//! projection and applies the shared telemetry rate, retention, and
//! channel-limit policy.  Operator deadband is a display/notification concern;
//! it must not remove simulation-time samples from the retained model history.
//! It does not add attributes to USD or require a visualization binding.

use bevy::prelude::*;
use lunco_signal::{SignalExposure, SignalMeta, SignalRef, SignalRegistry, SignalSource};
use lunco_telemetry::TelemetrySettings;
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::{
    ast_extract::ModelicaVariableMetadata, state::ModelicaDocumentRegistry, ModelicaModel,
};

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

/// Authored identity of a generated Modelica value.
///
/// The generated wrapper is a compiler artifact and may rename every member
/// to make the combined model legal.  Keeping this identity beside the solver
/// namespace means telemetry consumers never have to reverse-engineer those
/// names (or guess a component from a string prefix).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelicaSignalProvenance {
    /// Asset that declared the member class.
    pub source_asset: Option<String>,
    /// Fully-qualified class declared by that asset.
    pub model_class: Option<String>,
    /// Variable name in the member class.
    pub model_variable: Option<String>,
    /// Boundary name, when this value also has a canonical USD-facing output.
    pub canonical_name: Option<String>,
}

/// Authored-structure address map for a generated Modelica participant.
///
/// A generated network has one solver entity, but its variables still describe
/// values owned by the composed USD members.  The domain projector creates this
/// map from the composed network; the telemetry producer only applies it.  This
/// keeps the solver namespace private to the Modelica backend while preserving
/// the USD ownership tree for every consumer of [`SignalRegistry`].
#[derive(Component, Clone, Debug, Default)]
pub struct ModelicaSignalLayout {
    /// Exact solver variable → composed USD prim path mappings.  Boundary
    /// outputs and promoted member outputs use this form.
    pub exact_paths: BTreeMap<String, String>,
    /// Solver namespace prefix → composed USD prim path mappings.  Generated
    /// unit/member instance variables use this form so newly exposed internal
    /// variables do not require another explicit telemetry declaration.
    pub prefixes: Vec<(String, String)>,
    /// Exact solver variable → authored Modelica identity mappings.
    pub exact_provenance: BTreeMap<String, ModelicaSignalProvenance>,
    /// Solver namespace prefix → authored Modelica identity mappings. The
    /// variable name is completed from the suffix at lookup time.
    pub provenance_prefixes: Vec<(String, ModelicaSignalProvenance)>,
    /// Generated wrapper variables that correspond to authored outputs of the
    /// composed USD network. A member alias is public when it is the only
    /// runtime representation of that authored output; aliases already
    /// represented by a public network output remain internal to avoid a
    /// duplicate row for the same physical value. Every other variable remains
    /// available through the explicit model-variable inspection view.
    pub public_exact_paths: HashSet<String>,
    /// Modelica source metadata for projected solver names.  Generated wrapper
    /// declarations are intentionally plain `Real`s, so the member declaration
    /// is the only authoritative place to recover units and descriptions for
    /// promoted outputs.
    pub metadata: BTreeMap<String, ModelicaVariableMetadata>,
    /// Owner of a generated value for which the topology has no more specific
    /// member mapping.  This is the composed network scope, not a fabricated
    /// telemetry entity.
    pub root_path: String,
}

impl ModelicaSignalLayout {
    /// Resolve a solver variable to its composed USD owner.
    pub fn group_path(&self, variable: &str) -> Option<&str> {
        if let Some(path) = self.exact_paths.get(variable) {
            return Some(path);
        }
        self.prefixes
            .iter()
            .filter(|(prefix, _)| variable.starts_with(prefix))
            .max_by_key(|(prefix, _)| prefix.len())
            .map(|(_, path)| path.as_str())
            .or_else(|| (!self.root_path.is_empty()).then_some(self.root_path.as_str()))
    }

    /// Classify a generated solver variable without parsing its generated
    /// spelling in a consumer.  The projection records the authored network
    /// boundary once; copied unit variables and internal connector/state
    /// variables are therefore unambiguously implementation values.
    pub fn exposure(&self, variable: &str) -> SignalExposure {
        self.public_exact_paths
            .contains(variable)
            .then_some(SignalExposure::Public)
            .unwrap_or(SignalExposure::Internal)
    }

    /// Resolve authored Modelica identity for a solver variable.
    pub fn provenance(&self, variable: &str) -> Option<ModelicaSignalProvenance> {
        if let Some(identity) = self.exact_provenance.get(variable) {
            return Some(identity.clone());
        }
        self.provenance_prefixes
            .iter()
            .filter(|(prefix, _)| variable.starts_with(prefix))
            .max_by_key(|(prefix, _)| prefix.len())
            .map(|(prefix, identity)| {
                let mut resolved = identity.clone();
                let suffix = variable.strip_prefix(prefix).unwrap_or_default();
                if !suffix.is_empty() {
                    resolved.model_variable = Some(suffix.to_string());
                }
                resolved
            })
    }
}

#[derive(Default)]
struct RuntimeTelemetrySession {
    session_id: u64,
    signals: HashSet<SignalRef>,
}

/// Retain the current variables of every live Modelica solver.
///
/// The system runs after worker responses in `Update`, where
/// [`ModelicaModel::current_time`] and [`ModelicaModel::variables`] describe the
/// same landed solver result.  Sampling is paced by model time rather than by
/// render frames, so a fast UI cannot inflate the recorded rate and a headless
/// run follows the same policy.
pub fn retain_modelica_runtime_state(
    mut commands: Commands,
    settings: Option<Res<TelemetrySettings>>,
    mut signals: Option<ResMut<SignalRegistry>>,
    documents: Option<Res<ModelicaDocumentRegistry>>,
    mut sessions: ResMut<RuntimeTelemetrySessions>,
    models: Query<(Entity, &ModelicaModel, Option<&ModelicaSignalLayout>)>,
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

    for (entity, model, layout) in &models {
        let session = sessions.sessions.entry(entity).or_default();
        if session.session_id != model.session_id {
            for signal in session.signals.drain() {
                signals.clear_history(&signal);
            }
            session.session_id = model.session_id;
        }

        let mut retained_any = false;
        for (name, &value) in &model.variables {
            if !value.is_finite() || name.is_empty() {
                continue;
            }

            let signal = SignalRef::new(entity, name.clone());
            // Generated-document metadata can become available after the
            // first solver response when its asynchronous index build lands.
            // Refresh metadata independently of sampling so a channel does
            // not remain permanently unitless just because it was not due on
            // that first pass.
            signals.update_meta(
                signal.clone(),
                model_signal_meta(documents.as_deref(), model, layout, name),
            );
            let known = signals.scalar_history(&signal).is_some();
            if !known && signals.iter_scalar().count() >= settings.max_channels {
                warn_once!(
                    "modelica telemetry: max_channels ({}) reached; additional runtime variables are not retained",
                    settings.max_channels
                );
                continue;
            }

            // The shared signal registry owns due-time and time-reversal for
            // every runtime producer. Modelica histories are complete at the
            // recording rate; notification deadband must not remove elapsed
            // simulation-time samples from a graph.
            if signals.record_scalar_at_rate(
                signal.clone(),
                model.current_time,
                value,
                settings.default_rate_hz,
                settings.default_retention,
            ) {
                session.signals.insert(signal);
                retained_any = true;
            }
        }

        if retained_any {
            commands.entity(entity).try_insert(SignalSource);
        }
    }
}

fn model_signal_meta(
    documents: Option<&ModelicaDocumentRegistry>,
    model: &ModelicaModel,
    layout: Option<&ModelicaSignalLayout>,
    name: &str,
) -> SignalMeta {
    let projected = layout.and_then(|layout| layout.metadata.get(name));
    let modelica_provenance = layout.and_then(|layout| layout.provenance(name));
    let entry = documents
        .and_then(|registry| registry.host(model.document))
        .and_then(|host| host.document().index().find_component_by_leaf(name));
    let unit = projected.and_then(|entry| entry.unit.clone()).or_else(|| {
        entry
            .and_then(|entry| entry.modifications.get("unit"))
            .map(|unit| unit.trim_matches('"').to_string())
            .filter(|unit| !unit.is_empty())
    });

    let model_class = modelica_provenance
        .as_ref()
        .and_then(|identity| identity.model_class.clone())
        .or_else(|| (!model.model_name.is_empty()).then(|| model.model_name.clone()));
    let model_variable = modelica_provenance
        .as_ref()
        .and_then(|identity| identity.model_variable.clone())
        .or_else(|| (!name.is_empty()).then(|| name.to_string()));
    let source_asset = modelica_provenance
        .as_ref()
        .and_then(|identity| identity.source_asset.clone())
        .or_else(|| (!model.source_uri.is_empty()).then(|| model.source_uri.clone()));

    SignalMeta {
        description: projected
            .and_then(|entry| entry.description.clone())
            .filter(|description| !description.is_empty())
            .or_else(|| {
                entry
                    .map(|entry| entry.description.clone())
                    .filter(|description| !description.is_empty())
            }),
        unit,
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::app::Update;

    fn app_with_model(model: ModelicaModel) -> (App, Entity) {
        let mut app = App::new();
        app.insert_resource(TelemetrySettings::default());
        app.insert_resource(SignalRegistry::default());
        app.init_resource::<RuntimeTelemetrySessions>();
        app.add_systems(Update, retain_modelica_runtime_state);
        let entity = app
            .world_mut()
            .spawn((model, ModelicaSignalLayout::default()))
            .id();
        (app, entity)
    }

    #[test]
    fn runtime_state_is_retained_without_a_plot_binding() {
        let mut model = ModelicaModel::default();
        model.model_name = "LunCo.Test.Sensor".into();
        model.source_uri = "lunco://models/LunCo/Test/Sensor.mo".into();
        model.current_time = 0.0;
        model.variables.insert("soc".to_string(), 0.95);
        let (mut app, entity) = app_with_model(model);

        app.update();

        let signal = SignalRef::new(entity, "soc");
        let history = app
            .world()
            .resource::<SignalRegistry>()
            .scalar_history(&signal)
            .expect("runtime Modelica state must be inspectable");
        let sample = history.samples.back().expect("initial state sample");
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
    fn projected_member_metadata_is_used_for_generated_channel_names() {
        let mut model = ModelicaModel::default();
        model.variables.insert("member_current".to_string(), 3.0);
        let mut layout = ModelicaSignalLayout::default();
        layout.metadata.insert(
            "member_current".to_string(),
            ModelicaVariableMetadata {
                description: Some("Current delivered by the member".to_string()),
                unit: Some("A".to_string()),
            },
        );

        let meta = model_signal_meta(None, &model, Some(&layout), "member_current");
        assert_eq!(meta.unit.as_deref(), Some("A"));
        assert_eq!(
            meta.description.as_deref(),
            Some("Current delivered by the member")
        );
    }

    #[test]
    fn projected_modelica_identity_is_retained_for_solver_names() {
        let mut model = ModelicaModel::default();
        model
            .variables
            .insert("unit.Camera.power_draw_w".to_string(), 3.0);
        let mut layout = ModelicaSignalLayout::default();
        layout.provenance_prefixes.push((
            "unit.Camera.".to_string(),
            ModelicaSignalProvenance {
                source_asset: Some("lunco://models/LunCo/Electrical/CameraPayload.mo".into()),
                model_class: Some("LunCo.Electrical.CameraPayload".into()),
                ..default()
            },
        ));

        let meta = model_signal_meta(None, &model, Some(&layout), "unit.Camera.power_draw_w");
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
        let mut model = ModelicaModel::default();
        model.variables.insert("speed".to_string(), 1.0);
        let (mut app, entity) = app_with_model(model);
        let signal = SignalRef::new(entity, "speed");

        app.update();
        {
            let mut model = app.world_mut().entity_mut(entity);
            let mut value = model.get_mut::<ModelicaModel>().unwrap();
            value.current_time = 0.01;
            value.variables.insert("speed".to_string(), 2.0);
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
            value.variables.insert("speed".to_string(), 2.0);
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
            value.variables.insert("speed".to_string(), 2.0);
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
            value.variables.insert("speed".to_string(), 2.1);
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
        let mut model = ModelicaModel::default();
        model.session_id = 1;
        model.current_time = 1.0;
        model.variables.insert("x".to_string(), 4.0);
        let (mut app, entity) = app_with_model(model);
        let signal = SignalRef::new(entity, "x");
        app.update();

        {
            let mut model = app.world_mut().entity_mut(entity);
            let mut value = model.get_mut::<ModelicaModel>().unwrap();
            value.session_id = 2;
            value.current_time = 0.0;
            value.variables.insert("x".to_string(), 8.0);
        }
        app.update();

        let history = app
            .world()
            .resource::<SignalRegistry>()
            .scalar_history(&signal)
            .unwrap();
        assert_eq!(history.len(), 1);
        let sample = history.samples.back().expect("new session sample");
        assert_eq!((sample.time, sample.value), (0.0, 8.0));
    }
}
