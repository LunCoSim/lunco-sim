//! Glue between the Modelica UI and `lunco-viz`.
//!
//! Keeps the "default plot" identity in one place so the Telemetry
//! panel, the Graphs panel, and the worker's auto-bind-on-compile
//! system all reference the same `VizId` and the same auto-bind
//! policy. Without this module each call site would invent its own
//! sentinel id and drift apart.

use bevy::prelude::*;
use lunco_viz::{
    SignalBinding, SignalRef, ViewTarget, VisualizationConfig, VisualizationRegistry, VizId,
    LINE_PLOT_KIND,
};

/// Stable id of the singleton "Modelica" time-series plot. Reserved
/// at the bottom of the [`VizId`] space so user-opened plots
/// allocated via `VizId::next()` (which starts at 1 and ascends)
/// don't collide. Re-using a fixed value also means a saved
/// workspace that opens this plot will round-trip cleanly.
pub const DEFAULT_MODELICA_GRAPH: VizId = VizId(1);

/// Look up — or, on first call, create — the singleton Modelica
/// plot. Returns a mutable handle so callers can edit `inputs`
/// directly.
pub fn ensure_default_modelica_graph(
    registry: &mut VisualizationRegistry,
) -> &mut VisualizationConfig {
    if registry.get(DEFAULT_MODELICA_GRAPH).is_none() {
        registry.insert(VisualizationConfig {
            id: DEFAULT_MODELICA_GRAPH,
            title: "Modelica".into(),
            kind: LINE_PLOT_KIND,
            view: ViewTarget::Panel2D,
            inputs: Vec::new(),
            style: serde_json::Value::Null,
        });
    }
    registry.get_mut(DEFAULT_MODELICA_GRAPH).unwrap()
}

/// Add (or remove) one signal in a specific plot. Idempotent — safe
/// to call from a checkbox handler each frame.
pub fn set_signal_plotted(
    registry: &mut VisualizationRegistry,
    viz_id: VizId,
    signal: SignalRef,
    plotted: bool,
) {
    if registry.get(viz_id).is_none() {
        // Create if missing (matches ensure_default_modelica_graph behavior)
        registry.insert(VisualizationConfig {
            id: viz_id,
            title: if viz_id == DEFAULT_MODELICA_GRAPH {
                "Modelica".into()
            } else {
                format!("Plot #{}", viz_id.0)
            },
            kind: LINE_PLOT_KIND,
            view: ViewTarget::Panel2D,
            inputs: Vec::new(),
            style: serde_json::Value::Null,
        });
    }
    let cfg = registry.get_mut(viz_id).unwrap();
    if plotted {
        if !cfg.inputs.iter().any(|b| b.source == signal) {
            cfg.inputs.push(SignalBinding {
                source: signal,
                role: "y".into(),
                label: None,
                color: None,
                visible: true,
            });
        }
    } else {
        cfg.inputs.retain(|b| b.source != signal);
    }
}

/// Whether `signal` is currently a binding of the specified plot.
pub fn is_signal_plotted(
    registry: &VisualizationRegistry,
    viz_id: VizId,
    signal: &SignalRef,
) -> bool {
    registry
        .get(viz_id)
        .is_some_and(|cfg| cfg.inputs.iter().any(|b| b.source == *signal))
}

/// Drop every binding tied to `entity` across ALL plots in the
/// registry. Called when a model entity despawns so stale-source
/// bindings are cleared globally.
pub fn drop_entity_bindings(registry: &mut VisualizationRegistry, entity: Entity) {
    for cfg in registry.values_mut() {
        cfg.inputs.retain(|b| b.source.entity != entity);
    }
}

/// Seed the default plot with every observable from a freshly-compiled
/// model — preserves the old "first compile populates the graph"
/// behavior. Inputs / parameters / time are excluded; only true
/// observables are auto-added.
///
/// Called from the worker's compile-result handler. Bindings already
/// present (from a prior compile) are not duplicated.
pub fn auto_bind_observables(
    registry: &mut VisualizationRegistry,
    entity: Entity,
    detected: &[(String, f64)],
    skip_names: impl Fn(&str) -> bool,
) {
    let cfg = ensure_default_modelica_graph(registry);
    for (name, _) in detected {
        if name.ends_with("_in") || skip_names(name) {
            continue;
        }
        let sig = SignalRef::new(entity, name.clone());
        if !cfg.inputs.iter().any(|b| b.source == sig) {
            cfg.inputs.push(SignalBinding {
                source: sig,
                role: "y".into(),
                label: None,
                color: None,
                visible: true,
            });
        }
    }
}
