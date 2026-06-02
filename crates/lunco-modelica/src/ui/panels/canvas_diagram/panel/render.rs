//! Canvas scene rendering and event routing.

use bevy::prelude::*;
use bevy_egui::egui;
use crate::ui::panels::model_view::TabRenderContext;
use super::super::{CanvasDiagramState, CanvasSnapSettings, ops, overlays};
use super::util::{mark, log_frame_times};
use super::snapshots::stash_snapshots;
use super::interaction::{handle_context_menu, handle_drag_and_drop, handle_node_double_click};

pub(crate) fn render_diagram_canvas(
    _panel: &super::CanvasDiagramPanel,
    ui: &mut egui::Ui,
    world: &mut World,
) {
    let _frame_t0 = web_time::Instant::now();
    let render_tab_id = world.resource::<TabRenderContext>().tab_id;
    let trace_phases = std::env::var_os("RENDER_CANVAS_TRACE").is_some();
    let mut phase_t = web_time::Instant::now();
    let mut phase_log = Vec::new();

    let (doc_id, editing_class) = ops::resolve_doc_context(world);
    mark("resolve_doc_context", &mut phase_t, &mut phase_log);

    let active_doc = doc_id;
    let tab_read_only = active_doc.map(|d| crate::ui::state::read_only_for(world, d)).unwrap_or(false);

    let snap_settings = world.get_resource::<CanvasSnapSettings>().filter(|s| s.enabled).map(|s| lunco_canvas::SnapSettings { step: s.step });

    {
        // Publish the active theme once per frame. Every egui paint
        // helper (canvas built-in layers, node/edge painters, icon
        // remap) reads it back via `lunco_theme::active(ctx)` — one
        // theme, one transport, no per-consumer projection caches.
        let theme = world.get_resource::<lunco_theme::Theme>().cloned().unwrap_or_else(lunco_theme::Theme::dark);
        lunco_theme::store_active(ui.ctx(), &theme);
    }

    stash_snapshots(ui.ctx(), world, doc_id);
    mark("snapshots+sigreg", &mut phase_t, &mut phase_log);

    let (response, events) = {
        let mut state = world.resource_mut::<CanvasDiagramState>();
        let docstate = match (render_tab_id, active_doc) { (Some(t), Some(d)) => state.get_mut_for_tab(t, d), _ => state.get_mut(active_doc) };
        docstate.canvas.read_only = tab_read_only;
        docstate.canvas.snap = snap_settings;
        docstate.canvas.ui(ui)
    };
    mark("canvas.ui (scene render)", &mut phase_t, &mut phase_log);

    if let Some(mut active) = world.get_resource_mut::<crate::ui::wasm_autosave::IsGestureActive>() {
        active.canvas = response.is_pointer_button_down_on();
    }

    // ─── Overlays ───
    //
    // Single derivation: `StatusBus::lifecycle(scope, has_content)`
    // collapses the prior `loading || parse_pending || projecting`
    // OR and the separate empty/error branches into one match. Every
    // async stage that contributes to the canvas (file-load, drill-in,
    // duplicate, projection, AST reparse) holds a `BusyHandle`
    // scoped to `Document(doc_id)` for its lifetime; the parse→project
    // handoff overlaps handles via
    // `CanvasDiagramState::pending_projection_handoff`, and AST
    // reparse runs through `track_ast_reparse_busy`. The bus is
    // never momentarily empty mid-flight, so the overlay needs no
    // local fallback predicates.
    {
        let theme = world.get_resource::<lunco_theme::Theme>().cloned().unwrap_or_else(lunco_theme::Theme::dark);
        let drilled_class = render_tab_id
            .and_then(|tid| {
                let tabs = world.resource::<crate::ui::panels::model_view::ModelTabs>();
                tabs.get(tid).and_then(|t| t.drilled_class.clone())
            })
            .unwrap_or_default();
        let lifecycle = {
            let state = world.resource::<CanvasDiagramState>();
            let docstate = match render_tab_id { Some(t) => state.get_for_tab(t), None => state.get(active_doc) };
            let has_content = docstate.canvas.scene.node_count() > 0;
            let bus = world.resource::<lunco_workbench::status_bus::StatusBus>();
            active_doc
                .map(|d| bus.lifecycle(lunco_workbench::status_bus::BusyScope::Document(d.0), has_content))
                .unwrap_or(lunco_workbench::status_bus::LifecycleState::Empty)
        };

        use lunco_workbench::status_bus::LifecycleState;
        match lifecycle {
            LifecycleState::Loading => {
                let bus = world.resource::<lunco_workbench::status_bus::StatusBus>();
                if let Some(doc_id) = active_doc {
                    lunco_ui::busy::LoadingIndicator::for_scope(lunco_workbench::status_bus::BusyScope::Document(doc_id.0))
                        .overlay_on(ui, response.rect, bus, &theme);
                }
                ui.ctx().request_repaint();
            }
            LifecycleState::Failed(msg) => {
                overlays::render_drill_in_error_overlay(ui, response.rect, &drilled_class, &msg, &theme);
            }
            LifecycleState::Empty => {
                overlays::render_empty_diagram_overlay(ui, response.rect, world);
            }
            LifecycleState::Content => {}
        }
    }

    handle_drag_and_drop(ui, world, &response, active_doc, render_tab_id, tab_read_only, editing_class.clone());
    let menu_ops = handle_context_menu(ui, world, &response, active_doc, render_tab_id, tab_read_only, editing_class.as_deref());
    handle_node_double_click(world, &events, active_doc);

    if let (Some(doc_id), Some(class)) = (doc_id, editing_class.as_ref()) {
        let mut all_ops = ops::build_ops_from_events(world, &events, class);
        all_ops.extend(menu_ops);
        if !all_ops.is_empty() {
            #[cfg(feature = "lunco-api")]
            crate::api::trigger_apply_ops(world, doc_id, all_ops);
            #[cfg(not(feature = "lunco-api"))]
            super::super::ops::apply_ops_public(world, doc_id, all_ops);
        }
    }

    // Apply in-canvas input-control widget writes. The control widget
    // (sliders rendered next to component icons) queues writes during
    // paint; we drain after the canvas finishes rendering so the
    // simulator's `model.inputs` map (and worker `set_input`) update
    // continuously while the user drags the slider.
    if let Some(doc_id) = active_doc {
        let writes = lunco_viz::kinds::canvas_plot_node::drain_input_writes(ui.ctx());
        for (name, value) in writes {
            if let Err(err) = crate::ui::commands::sim::apply_set_model_input(
                world, doc_id, &name, value,
            ) {
                bevy::log::warn!(
                    "[CanvasDiagram] in-canvas input write failed: name={name} value={value} err={err:?}"
                );
            }
        }
    }
    
    mark("tail (events/menu/fit)", &mut phase_t, &mut phase_log);
    log_frame_times(_frame_t0.elapsed().as_secs_f64() * 1000.0, 0.0);

    if trace_phases && !phase_log.is_empty() {
        let total: f64 = phase_log.iter().map(|(_, ms)| *ms).sum();
        if total > 30.0 {
            let breakdown = phase_log.iter().map(|(name, ms)| format!("{name}={ms:.1}ms")).collect::<Vec<_>>().join(" ");
            bevy::log::info!("[CanvasDiagram] render_canvas phases (sum={total:.1}ms): {breakdown}");
        }
    }
}
