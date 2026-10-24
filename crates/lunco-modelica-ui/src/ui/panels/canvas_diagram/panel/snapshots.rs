//! Data snapshots for canvas-visual consumption.

use crate::ui::document_context::ModelicaDocuments;
use bevy_egui::egui;
use lunco_modelica_runtime::{ModelicaModel, ModelicaVariables};
use lunco_workbench_core::PanelCtx;

/// Publish the active diagram's namespace into the canvas snapshot. This is
/// an explicit Modelica root-scope projection, not a renderer fallback: all
/// views of a drilled class consume the same unqualified class-local keys.
fn project_target_unit_scope(
    values: &mut std::collections::HashMap<String, f64>,
    instance: Option<&str>,
) {
    let Some(instance) = instance.filter(|name| !name.is_empty()) else {
        return;
    };
    let prefix = format!("{instance}.");
    let scoped: Vec<(String, f64)> = values
        .iter()
        .filter_map(|(name, value)| {
            name.strip_prefix(&prefix)
                .map(|local| (local.to_string(), *value))
        })
        .collect();
    for (name, value) in scoped {
        values.insert(name, value);
    }
}

pub(crate) fn stash_snapshots(
    ui: &egui::Context,
    ctx: &PanelCtx,
    doc_id: Option<lunco_doc::DocumentId>,
    target_unit_instance: Option<&str>,
) {
    // ─── Signals ───
    //
    // Change-driven: we no longer copy every registered signal's ring
    // here. `stash_signal_snapshot_from_registry` copies only the
    // signals plot visuals registered interest in last frame, and only
    // when a signal's history actually changed (Arc-cached otherwise).
    // This function's job shrinks to building the `doc → entity` table
    // the producer needs to resolve Doc-bound tiles.
    if let Some(sig_reg) = ctx.resource::<lunco_viz::SignalRegistry>() {
        let mut doc_to_entity: std::collections::HashMap<u64, bevy::prelude::Entity> =
            std::collections::HashMap::new();
        // Seed doc → playback entity first, then overwrite with the
        // live cosim entity (if any) so live wins for docs that have
        // both. The playback entity holds the latest Fast Run's
        // series in `SignalRegistry` (published by
        // `drain_pending_handles`), keeping the lookup uniform
        // — `(entity, path) → samples` — across live and historical.
        if let Some(playback) = ctx.resource::<lunco_modelica_runner::PlaybackEntities>() {
            for (d, e) in &playback.0 {
                doc_to_entity.insert(d.raw(), *e);
            }
        }
        // Source-backed plot tiles store a `doc_id` instead of a
        // pinned sim entity (the runtime entity isn't known at
        // parse / projection time). Populate the per-frame
        // `doc → entity` table from the document registry so those
        // tiles can resolve at fetch time.
        //
        // Cosim caveat: a single doc can be linked to multiple sim
        // entities (>1 element in `entities_linked_to`). A plain
        // `insert` would let HashMap iteration order decide which
        // sim wins, flipping the bound entity frame-to-frame. We
        // pick the lowest entity bits as a *deterministic* tie-
        // break — not necessarily the right one in cosim, but
        // stable. When real cosim plot scenarios land, extend
        // `PlotBinding::Doc` with a role/index and resolve
        // `(doc, role) → entity` instead.
        if let Some(reg) = ctx.resource::<ModelicaDocuments>() {
            for (e, d) in reg.iter_doc_for_entity() {
                doc_to_entity
                    .entry(d.raw())
                    .and_modify(|cur| {
                        if e.to_bits() < cur.to_bits() {
                            *cur = e;
                        }
                    })
                    .or_insert(e);
            }
        }
        lunco_viz::kinds::canvas_plot_node::stash_signal_snapshot_from_registry(
            ui,
            sig_reg,
            doc_to_entity,
        );
    }

    let canvas_sim = doc_id.and_then(|d| crate::ui::context::simulator_for(ctx, d));

    // ─── Live Values ───
    {
        let mut state = lunco_viz::kinds::canvas_plot_node::NodeStateSnapshot::default();
        if let Some(d) = doc_id {
            seed_state_from_latest_experiment(ctx, &mut state, d);
        }
        if let Some(entity) = canvas_sim {
            if let Some(model) = ctx.get::<ModelicaModel>(entity) {
                let Some(variables) = ctx.get::<ModelicaVariables>(entity) else {
                    bevy::log::warn_once!(
                        "Modelica canvas snapshot unavailable: participant {entity:?} has no required variable state"
                    );
                    return;
                };
                state.values.clear();
                for (k, v) in &model.parameters {
                    state.values.insert(k.to_string(), *v);
                }
                for (k, v) in &model.inputs {
                    state.values.insert(k.to_string(), *v);
                }
                for (k, v) in &variables.values {
                    state.values.insert(k.to_string(), *v);
                }
                project_target_unit_scope(&mut state.values, target_unit_instance);
                state.values.insert("time".to_string(), model.current_time);
            }
        }
        lunco_viz::kinds::canvas_plot_node::stash_node_state(ui, state);

        let any_unpaused = canvas_sim
            .and_then(|e| ctx.get::<ModelicaModel>(e))
            .map(|m| !m.paused)
            .unwrap_or(false);
        let dt = ui.input(|i| i.stable_dt as f64);
        let prev = ui
            .data(|d| d.get_temp::<f64>(egui::Id::new("lunco_modelica_flow_anim_time")))
            .unwrap_or(0.0);
        let next = if any_unpaused { prev + dt } else { prev };
        ui.data_mut(|d| {
            d.insert_temp(egui::Id::new("lunco_modelica_flow_anim_time"), next);
            d.insert_temp(egui::Id::new("lunco_modelica_sim_stepping"), any_unpaused);
        });
    }

    // ─── Input Controls ───
    {
        let mut control_snapshot =
            lunco_viz::kinds::canvas_plot_node::InputControlSnapshot::default();
        if let Some(entity) = canvas_sim {
            if let Some(model) = ctx.get::<ModelicaModel>(entity) {
                for (qualified, value) in &model.inputs {
                    let metadata = model
                        .variable_metadata
                        .as_ref()
                        .and_then(|facts| facts.get(qualified));
                    let mn = metadata.and_then(|facts| facts.minimum);
                    let mx = metadata.and_then(|facts| facts.maximum);
                    control_snapshot
                        .inputs
                        .insert(qualified.to_string(), (*value, mn, mx));
                }
            }
        }
        lunco_viz::kinds::canvas_plot_node::stash_input_control_snapshot(ui, control_snapshot);
    }
}

fn seed_state_from_latest_experiment(
    ctx: &PanelCtx,
    state: &mut lunco_viz::kinds::canvas_plot_node::NodeStateSnapshot,
    doc_id: lunco_doc::DocumentId,
) {
    use lunco_experiments::ExperimentRegistry;
    let twin = crate::ui::doc_pin::twin_id_for_doc(doc_id);
    let active_plot = ctx
        .resource::<lunco_experiments_ui::ActivePlot>()
        .copied()
        .unwrap_or_default()
        .or_default(crate::ui::viz::DEFAULT_MODELICA_GRAPH);
    let plot_states = ctx.resource::<lunco_experiments_ui::PlotPanelStates>();
    // A plot's selection belongs to its document. A newly opened diagram
    // must not inherit an empty/uninitialized or another document's selection
    // simply because that graph has not been painted yet. Once initialized
    // for this document, explicit hidden-run choices remain authoritative.
    let visible_in_active = plot_states
        .and_then(|states| states.get(active_plot))
        .filter(|plot| plot.last_twin.as_ref() == Some(&twin) && plot.auto_show_attempted)
        .map(|plot| &plot.visible_experiments);
    let Some(registry) = ctx.resource::<ExperimentRegistry>() else {
        return;
    };
    let exps = registry.list_for_twin(&twin);
    let chosen = exps.iter().rev().find(|e| {
        e.result.is_some()
            && crate::ui::context::run_is_in_active_scope(ctx, e.id)
            && visible_in_active
                .as_ref()
                .map(|v| v.contains(&e.id))
                .unwrap_or(true)
    });
    let Some(exp) = chosen else { return };
    let Some(result) = &exp.result else { return };
    if result.times.is_empty() {
        return;
    }
    let scrub_time = plot_states.and_then(|s| s.scrub(active_plot));
    let idx = match scrub_time {
        Some(t) => {
            let mut best = 0usize;
            let mut best_d = f64::INFINITY;
            for (i, ti) in result.times.iter().enumerate() {
                let d = (ti - t).abs();
                if d < best_d {
                    best_d = d;
                    best = i;
                }
            }
            best
        }
        None => result.times.len() - 1,
    };
    state.values.insert("time".to_string(), result.times[idx]);
    for (name, samples) in &result.series {
        if let Some(v) = samples.get(idx) {
            if v.is_finite() {
                state.values.insert(name.clone(), *v);
            }
        }
    }
}
