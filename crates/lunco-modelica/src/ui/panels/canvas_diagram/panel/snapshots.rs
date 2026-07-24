//! Data snapshots for canvas-visual consumption.

use crate::state::ModelicaDocumentRegistry;
use crate::ModelicaModel;
use bevy_egui::egui;
use lunco_workbench::PanelCtx;

/// Resolve the `(min, max)` attributes of an input `member` on a
/// component of type `ty`, by building (once, cached) a `ModelicaIndex`
/// over the bundled package `pkg`'s source. This recovers real input
/// bounds (e.g. `Valve.opening` → 0..100) for a standalone `within P;`
/// duplicate whose own index can't see the package's class internals —
/// without it the on-canvas input slider has no range to anchor to and
/// falls back to a value-derived range that the user's drag can run away
/// (the `valve.opening` → 3.4e21 crash). Cached per package; bundled
/// sources are immutable so the index is parsed at most once per session.
fn bundled_member_bounds(pkg: &str, ty: &str, member: &str) -> (Option<f64>, Option<f64>) {
    thread_local! {
        static CACHE: std::cell::RefCell<
            std::collections::HashMap<String, std::rc::Rc<crate::index::ModelicaIndex>>,
        > = std::cell::RefCell::new(std::collections::HashMap::new());
    }
    let idx = CACHE.with(|c| {
        if let Some(i) = c.borrow().get(pkg) {
            return Some(i.clone());
        }
        let src = crate::ui::class_source::bundled_source_for(pkg)?;
        let ast = rumoca_phase_parse::parse_to_ast(src, "input-bounds.mo").ok()?;
        let mut index = crate::index::ModelicaIndex::new();
        index.rebuild_with_errors(&ast, src, false);
        let rc = std::rc::Rc::new(index);
        c.borrow_mut().insert(pkg.to_string(), rc.clone());
        Some(rc)
    });
    let Some(idx) = idx else { return (None, None) };
    // Match by member name AND owning class (exact or qualified suffix) so
    // a member name shared across classes (e.g. `availability`) resolves to
    // the right one.
    let entry = idx
        .components
        .iter()
        .find(|e| e.name == member && (e.class == ty || e.class.ends_with(&format!(".{ty}"))));
    match entry {
        Some(e) => (
            e.modifications.get("min").and_then(|s| s.parse().ok()),
            e.modifications.get("max").and_then(|s| s.parse().ok()),
        ),
        None => (None, None),
    }
}

pub(crate) fn stash_snapshots(
    ui: &egui::Context,
    ctx: &PanelCtx,
    doc_id: Option<lunco_doc::DocumentId>,
) {
    // ─── Signals ───
    if let Some(sig_reg) = ctx.resource::<lunco_viz::SignalRegistry>() {
        let mut snapshot = lunco_viz::kinds::canvas_plot_node::SignalSnapshot::default();
        for (sig_ref, hist) in sig_reg.iter_scalar() {
            let pts: Vec<[f64; 2]> = hist.samples.iter().map(|s| [s.time, s.value]).collect();
            snapshot
                .samples
                .insert((sig_ref.entity, sig_ref.path.clone()), pts);
        }
        // Seed doc → playback entity first, then overwrite with the
        // live cosim entity (if any) so live wins for docs that have
        // both. The playback entity holds the latest Fast Run's
        // series in `SignalRegistry` (published by
        // `drain_pending_handles`), keeping the lookup uniform
        // — `(entity, path) → samples` — across live and historical.
        if let Some(playback) = ctx.resource::<crate::experiments_runner::PlaybackEntities>() {
            for (d, e) in &playback.0 {
                snapshot.doc_to_entity.insert(d.raw(), *e);
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
        if let Some(reg) = ctx.resource::<ModelicaDocumentRegistry>() {
            for (e, d) in reg.iter_doc_for_entity() {
                snapshot
                    .doc_to_entity
                    .entry(d.raw())
                    .and_modify(|cur| {
                        if e.to_bits() < cur.to_bits() {
                            *cur = e;
                        }
                    })
                    .or_insert(e);
            }
        }
        lunco_viz::kinds::canvas_plot_node::stash_signal_snapshot(ui, snapshot);
    }

    let canvas_sim = doc_id.and_then(|d| crate::state::simulator_for_ctx(ctx, d));

    // ─── Live Values ───
    {
        let mut state = lunco_viz::kinds::canvas_plot_node::NodeStateSnapshot::default();
        if let Some(d) = doc_id {
            seed_state_from_latest_experiment(ctx, &mut state, d);
        }
        if let Some(entity) = canvas_sim {
            if let Some(model) = ctx.get::<ModelicaModel>(entity) {
                for (k, v) in &model.parameters {
                    state.values.insert(k.to_string(), *v);
                }
                for (k, v) in &model.inputs {
                    state.values.insert(k.to_string(), *v);
                }
                for (k, v) in &model.variables {
                    state.values.insert(k.to_string(), *v);
                }
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
                let host = ctx
                    .resource::<ModelicaDocumentRegistry>()
                    .and_then(|r| r.host(model.document));
                let index_ref = host.map(|h| h.document().index());
                // A standalone `within P;` duplicate's own index can't see
                // the sibling component classes (`Valve` etc.), so an input
                // like `valve.opening` has no min/max → the slider falls back
                // to a value-derived range that runs away. Recover the real
                // bounds (0..100) from the bundled package P.
                let within_pkg = host.and_then(|h| {
                    crate::ui::duplicate::within_package(h.document().source())
                });
                for (qualified, value) in &model.inputs {
                    let (mut mn, mut mx) = index_ref
                        .and_then(|idx| idx.find_component_by_leaf(qualified))
                        .map(|entry| {
                            (
                                entry.modifications.get("min").and_then(|s| s.parse().ok()),
                                entry.modifications.get("max").and_then(|s| s.parse().ok()),
                            )
                        })
                        .unwrap_or((None, None));
                    if mn.is_none() && mx.is_none() {
                        if let (Some(pkg), Some((inst, member))) =
                            (within_pkg.as_deref(), qualified.split_once('.'))
                        {
                            if let Some(ty) = index_ref
                                .and_then(|idx| idx.find_component_by_leaf(inst))
                                .map(|e| e.type_name.clone())
                            {
                                let (bmn, bmx) = bundled_member_bounds(pkg, &ty, member);
                                mn = bmn;
                                mx = bmx;
                            }
                        }
                    }
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
        .resource::<crate::ui::panels::experiments::ActivePlot>()
        .copied()
        .unwrap_or_default()
        .or_default();
    let plot_states = ctx.resource::<crate::ui::panels::experiments::PlotPanelStates>();
    let visible_in_active = plot_states.map(|s| s.visible(active_plot));
    let Some(registry) = ctx.resource::<ExperimentRegistry>() else {
        return;
    };
    let exps = registry.list_for_twin(&twin);
    let chosen = exps.iter().rev().find(|e| {
        e.result.is_some()
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
    for (name, samples) in &result.series {
        if let Some(v) = samples.get(idx) {
            if v.is_finite() {
                state.values.insert(name.clone(), *v);
            }
        }
    }
}
