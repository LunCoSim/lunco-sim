//! Runtime projection of Modelica state into the shared signal registry.
//!
//! A Modelica solver exposes its complete current state through
//! [`ModelicaModel::variables`].  Retaining that state is a simulation concern,
//! not a plot concern: an inspector, API client, recorder, or graph may all
//! consume the same history.  This module therefore owns the render-free
//! projection and applies the shared telemetry rate, deadband, retention, and
//! channel-limit policy.  It does not add attributes to USD or require a
//! visualization binding.

use bevy::prelude::*;
use lunco_signal::{SignalMeta, SignalRef, SignalRegistry, SignalSource};
use lunco_telemetry::TelemetrySettings;
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::{state::ModelicaDocumentRegistry, ModelicaModel};

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
    if !settings.default_deadband.is_valid() {
        warn_once!(
            "modelica telemetry: invalid default deadband; runtime state retention is skipped until it is corrected"
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

            // The shared signal registry owns due-time, time-reversal, and
            // deadband policy for every runtime producer. Modelica contributes
            // only the value and its solver time here.
            if signals.retain_scalar_if_changed(
                signal.clone(),
                model.current_time,
                value,
                settings.default_rate_hz,
                settings.default_deadband,
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
    let entry = documents
        .and_then(|registry| registry.host(model.document))
        .and_then(|host| host.document().index().find_component_by_leaf(name));
    let unit = entry
        .and_then(|entry| entry.modifications.get("unit"))
        .map(|unit| unit.trim_matches('"').to_string())
        .filter(|unit| !unit.is_empty());

    SignalMeta {
        description: entry
            .map(|entry| entry.description.clone())
            .filter(|description| !description.is_empty()),
        unit,
        provenance: Some("modelica".to_string()),
        group_path: layout
            .and_then(|layout| layout.group_path(name))
            .map(str::to_owned),
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
        assert!(app.world().entity(entity).contains::<SignalSource>());
    }

    #[test]
    fn runtime_state_uses_rate_and_deadband_policy() {
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
            2,
            "deadband should suppress an unchanged value"
        );

        {
            let mut model = app.world_mut().entity_mut(entity);
            let mut value = model.get_mut::<ModelicaModel>().unwrap();
            value.current_time = 0.6;
            value.variables.insert("speed".to_string(), 2.1);
        }
        app.update();
        assert_eq!(
            app.world()
                .resource::<SignalRegistry>()
                .scalar_history(&signal)
                .unwrap()
                .len(),
            3
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
