//! Canvas scene rendering and event routing.

use bevy::prelude::*;
use bevy_egui::egui;
use crate::model_tabs_types::TabRenderContext;
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
    let tab_read_only = active_doc.map(|d| crate::state::read_only_for(world, d)).unwrap_or(false);

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
                let tabs = world.resource::<crate::model_tabs::ModelTabs>();
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

    // ─── "Icons update when MSL loaded" hint ───
    //
    // On web the MSL bundle decodes in the background (~tens of
    // seconds) while the diagram already renders with provisional
    // partial types — standard-library components resolve to gray
    // placeholder boxes (`node.rs` `!drew_icon` path) until MSL
    // installs and the `MslBecameReady` observer reprojects every
    // open tab with real icons. Surface that so the gray boxes read
    // as "still loading", not "broken render".
    //
    // Visible until icons actually resolve, not just until MSL bytes
    // land: `MslLoadState` flips to `Ready` a step *before* the
    // reproject that swaps the gray boxes for icons. `MslBecameReady`
    // then sets each tab's `force_reproject`; we keep the hint while
    // that one-shot flag is still pending so there's no bare
    // gray-boxes-with-no-text frame between Ready and reproject. Once
    // the reproject spawns, `force_reproject` clears and the canvas
    // enters its `Loading` lifecycle (the centred spinner), so the
    // window is fully covered. `force_reproject` is set *only* by
    // `request_reproject_all` (the MSL-ready handler), so it stays
    // MSL-specific.
    {
        let msl_state = world.get_resource::<lunco_assets::msl::MslLoadState>();
        let msl_pending = msl_state.map(|s| s.is_pending()).unwrap_or(true);
        // Live load detail (phase + %) while the bundle is still arriving,
        // so the diagram shows *why* the icons are gray and how far along
        // the download/parse is — not just a static "loading" string.
        let msl_detail = msl_state.and_then(|s| match s {
            lunco_assets::msl::MslLoadState::Loading {
                phase,
                bytes_done,
                bytes_total,
            } => Some(format_msl_loading_hint(*phase, *bytes_done, *bytes_total)),
            _ => None,
        });
        let (has_content, reproject_pending) = {
            let state = world.resource::<CanvasDiagramState>();
            let docstate = match render_tab_id {
                Some(t) => state.get_for_tab(t),
                None => state.get(active_doc),
            };
            (
                docstate.canvas.scene.node_count() > 0,
                docstate.force_reproject,
            )
        };
        if (msl_pending || reproject_pending) && has_content {
            // A compile or run dispatched while MSL is still loading can't
            // finish until the standard library installs (the worker parse
            // path needs MSL resident — but the worker queues it and runs it
            // on its `MslReady`, see worker_transport.rs). When one is
            // pending, extend the hint with a second line so the user knows
            // the action wasn't lost.
            //
            // Compile is doc-scoped (`is_compiling(d)`). A Fast Run is tracked
            // only by the process-global runner, so we light the run line
            // whenever it has queued/in-flight work — acceptable here because
            // this hint only shows *while MSL is still loading*, when the one
            // queued thing is what the user just clicked.
            let compile_pending = active_doc
                .and_then(|d| {
                    world
                        .get_resource::<lunco_doc_bevy::DocumentDiagnostics>()
                        .map(|cs| cs.is_compiling(d))
                })
                .unwrap_or(false);
            let run_pending = world
                .get_resource::<crate::ModelicaRunnerResource>()
                .map(|r| r.0.in_flight_count() > 0 || r.0.queued_count() > 0)
                .unwrap_or(false);
            let color = world
                .get_resource::<lunco_theme::Theme>()
                .map(|t| t.tokens.warning)
                .unwrap_or_else(|| lunco_theme::Theme::dark().tokens.warning);
            let painter = ui
                .painter()
                .clone()
                .with_clip_rect(ui.clip_rect().intersect(response.rect));
            let font = egui::FontId::proportional(11.0);
            let mut y = response.rect.bottom() - 8.0;
            // First line: icons-pending + live load progress when available.
            let first_line = match &msl_detail {
                Some(d) => format!("⏳ {d} · icons update when ready"),
                None => "⏳ Icons will be updated when MSL loaded".to_string(),
            };
            painter.text(
                egui::pos2(response.rect.left() + 10.0, y),
                egui::Align2::LEFT_BOTTOM,
                first_line,
                font.clone(),
                color,
            );
            // Second line: deferred-action notice (compile takes priority
            // over run since it's doc-scoped and more specific).
            let deferred = if compile_pending {
                Some("⏳ Compilation will run when MSL is ready")
            } else if run_pending {
                Some("⏳ Simulation will start when MSL is ready")
            } else {
                None
            };
            if let Some(line) = deferred {
                y -= 16.0;
                painter.text(
                    egui::pos2(response.rect.left() + 10.0, y),
                    egui::Align2::LEFT_BOTTOM,
                    line,
                    font,
                    color,
                );
            }
            // Coarse poll, not a per-frame spin: MSL readiness flips on a
            // background task, so a ~250ms re-check clears the hint promptly
            // without pinning the whole workbench to full-rate redraw for the
            // entire (slow, resource-constrained) decode window.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
    mark("overlays", &mut phase_t, &mut phase_log);

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
    let frame_ms = _frame_t0.elapsed().as_secs_f64() * 1000.0;
    log_frame_times(frame_ms, 0.0);

    // Emit the per-phase breakdown on slow frames. Previously gated behind
    // the `RENDER_CANVAS_TRACE` env var, which is always absent on wasm
    // (no process env) — so the browser never got the breakdown that
    // localises a stall to a phase. Now any slow frame self-reports.
    if (trace_phases || frame_ms > 16.0) && !phase_log.is_empty() {
        let breakdown = phase_log.iter().map(|(name, ms)| format!("{name}={ms:.1}ms")).collect::<Vec<_>>().join(" ");
        bevy::log::warn!("[CanvasDiagram] slow-frame phases (total={frame_ms:.1}ms): {breakdown}");
    }
}

/// Human-readable one-liner for the in-diagram MSL-loading hint, e.g.
/// `Loading Modelica library — downloading MSL 47%` or
/// `Loading Modelica library — parsing MSL 1200/2555`. The `Parsing`
/// phase carries file counts in `done`/`total`; other phases carry bytes.
/// Falls back to a bare phase label when `total` is unknown (`0`).
fn format_msl_loading_hint(
    phase: lunco_assets::msl::MslLoadPhase,
    done: u64,
    total: u64,
) -> String {
    use lunco_assets::msl::MslLoadPhase;
    let label = phase.as_str();
    match phase {
        MslLoadPhase::Parsing if total > 0 => {
            format!("Loading Modelica library — {label} {done}/{total}")
        }
        _ if total > 0 => {
            let pct = (done as f64 / total as f64 * 100.0).clamp(0.0, 100.0);
            format!("Loading Modelica library — {label} {pct:.0}%")
        }
        _ => format!("Loading Modelica library — {label}"),
    }
}
