//! Projection task management and polling.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_canvas::Scene;
use lunco_doc::Document;
use crate::state::ModelicaDocumentRegistry;
use crate::model_tabs::ModelTabs;
use super::super::{CanvasDiagramState, DiagramProjectionLimits, ProjectionTask, active_doc_from_world, decorations, render_target};
use super::super::projection::{project_scene, projection_relevant_source_hash, recover_edges_from_ast};

pub(crate) fn poll_and_swap_projection(
    ui: &mut egui::Ui,
    world: &mut World,
    render_tab_id: Option<crate::model_tabs_types::TabId>,
) {
    let active_doc = active_doc_from_world(world);
    let current_gen_for_deadline = active_doc.and_then(|d| {
        world.get_resource::<ModelicaDocumentRegistry>()
            .and_then(|r| r.host(d))
            .map(|h| h.document().generation())
    });
    
    let mut state = world.resource_mut::<CanvasDiagramState>();
    let docstate = match (render_tab_id, active_doc) { (Some(t), Some(d)) => state.get_mut_for_tab(t, d), _ => state.get_mut(active_doc) };
    let is_initial_projection = docstate.last_seen_gen == 0;

    let timed_out = docstate.projection_task.as_ref().map(|t| t.spawned_at.elapsed() > t.deadline).unwrap_or(false);
    if timed_out {
        use std::sync::atomic::Ordering;
        if let Some(t) = docstate.projection_task.as_ref() {
            t.cancel.store(true, Ordering::Relaxed);
            bevy::log::warn!("[CanvasDiagram] projection exceeded {:.1}s deadline — cancelled.", t.deadline.as_secs_f32());
        }
        docstate.projection_task = None;
        if let Some(g) = current_gen_for_deadline { docstate.last_seen_gen = g; }
    }

    let done_task = docstate.projection_task.as_mut().and_then(|t| {
        t.task.poll_once()
            .map(|scene| (t.gen_at_spawn, t.doc_at_spawn, t.target_at_spawn.clone(), t.source_hash, scene))
    });

    let done_task = done_task.and_then(|(gen, doc, target, source_hash, scene)| {
        if gen < docstate.canvas_acked_gen {
            docstate.projection_task = None;
            None
        } else {
            Some((gen, doc, target, source_hash, scene))
        }
    });

    if let Some((gen, task_doc, target, source_hash, scene)) = done_task {
        bevy::log::info!(
            "[CanvasDiagram] poll_done gen={} target={:?} new_scene_nodes={} old_scene_nodes={}",
            gen, target, scene.node_count(), docstate.canvas.scene.node_count(),
        );
        docstate.projection_task = None;
        if scene.node_count() == 0 && docstate.canvas.scene.node_count() > 0 {
            docstate.last_seen_gen = gen;
            docstate.last_seen_target = target;
            docstate.last_seen_source_hash = source_hash;
            return;
        }

        let preserved_origins: std::collections::HashSet<String> = docstate.canvas.selection.iter().filter_map(|sid| match sid {
            lunco_canvas::SelectItem::Node(nid) => docstate.canvas.scene.node(*nid).and_then(|n| n.origin.clone()),
            _ => None,
        }).collect();

        let old_origin_to_id: std::collections::HashMap<String, lunco_canvas::NodeId> = docstate.canvas.scene.nodes().filter_map(|(id, n)| n.origin.clone().map(|o| (o, *id))).collect();
        let new_origin_to_id: std::collections::HashMap<String, lunco_canvas::NodeId> = scene.nodes().filter_map(|(id, n)| n.origin.clone().map(|o| (o, *id))).collect();
        let id_remap: std::collections::HashMap<lunco_canvas::NodeId, lunco_canvas::NodeId> = old_origin_to_id.iter().filter_map(|(origin, old_id)| new_origin_to_id.get(origin).map(|new_id| (*old_id, *new_id))).collect();
        docstate.canvas.tool.remap_node_ids(&|old: lunco_canvas::NodeId| id_remap.get(&old).copied());

        let mut scene = scene;
        let (bg_graphics, bg_plot_nodes): (Vec<_>, Vec<_>) = docstate.background_diagram.read().ok().and_then(|g| g.as_ref().map(|(_, gfx, plots)| (gfx.clone(), plots.clone()))).unwrap_or_default();
        // Tag source-backed plot tiles with the doc this task was
        // spawned for — *not* the world's current active doc. The
        // task may have started in tab A and completed after the
        // user duplicated / switched to tab B; using the live
        // active doc would bind tile resolution to the wrong sim.
        let source_backed_origins = decorations::emit_diagram_decorations(&mut scene, &bg_graphics, &bg_plot_nodes, Some(task_doc));
        
        let scene_only_nodes: Vec<lunco_canvas::scene::Node> = docstate.canvas.scene.nodes().filter(|(_, n)| {
            n.kind == lunco_viz::kinds::canvas_plot_node::PLOT_NODE_KIND && n.origin.as_deref().map(|o| !source_backed_origins.contains(o)).unwrap_or(true)
        }).map(|(_, n)| n.clone()).collect();
        for mut node in scene_only_nodes { node.id = scene.alloc_node_id(); scene.insert_node(node); }

        docstate.canvas.scene = scene;
        docstate.canvas.selection.clear();
        if !preserved_origins.is_empty() {
            let new_ids: Vec<lunco_canvas::NodeId> = docstate.canvas.scene.nodes().filter_map(|(nid, n)| n.origin.as_deref().filter(|o| preserved_origins.contains(*o)).map(|_| *nid)).collect();
            for id in new_ids { docstate.canvas.selection.add(lunco_canvas::SelectItem::Node(id)); }
        }
        docstate.last_seen_gen = gen;
        docstate.last_seen_target = target;
        docstate.last_seen_source_hash = source_hash;

        if is_initial_projection {
          if let Some(saved) = docstate.pending_view.take() {
            // Hot-exit restore: honour the saved camera instead of
            // fitting, so a reopened diagram looks exactly as at exit.
            docstate.canvas.viewport.snap_to(saved.center, saved.zoom);
          } else {
            let physical_zoom = lunco_canvas::Viewport::physical_mm_zoom(ui.ctx());
            if let Some(world_rect) = docstate.canvas.scene.bounds() {
                let avail = ui.available_size();
                let origin = ui.cursor().min;
                let screen = lunco_canvas::Rect::from_min_max(lunco_canvas::Pos::new(origin.x, origin.y), lunco_canvas::Pos::new(origin.x + avail.x.max(1.0), origin.y + avail.y.max(1.0)));
                let (c, z) = docstate.canvas.viewport.fit_values(world_rect, screen, 40.0);
                let z = z.min(physical_zoom * 2.0).max(physical_zoom * 0.5);
                docstate.canvas.viewport.snap_to(c, z);
            } else {
                docstate.canvas.viewport.snap_to(lunco_canvas::Pos::new(0.0, 0.0), physical_zoom);
            }
          }
        }
        ui.ctx().request_repaint();
    } else if docstate.projection_task.is_some() {
        ui.ctx().request_repaint();
    }
}

pub(crate) fn trigger_projection_if_needed(
    ui: &mut egui::Ui,
    world: &mut World,
    render_tab_id: Option<crate::model_tabs_types::TabId>,
) {
    let Some(doc_id) = active_doc_from_world(world) else { return; };
    let gen = world.resource::<ModelicaDocumentRegistry>().host(doc_id).map(|h| h.document().generation()).unwrap_or(0);
    
    let current_source = world.resource::<ModelicaDocumentRegistry>().host(doc_id).map(|h| h.document().source_arc()).unwrap_or_else(|| std::sync::Arc::<str>::from(""));

    let needs_project = {
        let state = world.resource::<CanvasDiagramState>();
        let docstate = match render_tab_id { Some(t) => state.get_for_tab(t), None => state.get(Some(doc_id)) };
        // Don't respawn while a projection is already in flight —
        // otherwise every frame cancels and re-spawns the task,
        // it never gets to complete, and the canvas stays blank.
        let task_in_flight = docstate.projection_task.is_some();
        let first_render = !match render_tab_id { Some(t) => state.has_entry_for_tab(t), None => state.has_entry(doc_id) };
        let gen_advanced = gen != docstate.last_seen_gen && gen > docstate.canvas_acked_gen;
        let live_target = render_target(world)
            .filter(|(d, _)| *d == doc_id)
            .and_then(|(_, drilled)| drilled)
            .or_else(|| {
                world
                    .get_resource::<ModelTabs>()
                    .and_then(|t| t.drilled_class_for_doc(doc_id))
            })
            .or_else(|| {
                crate::sim_default::default_simulation_class(world, doc_id)
            });
        let target_changed = live_target != docstate.last_seen_target;
        let ast_stale = world.resource::<ModelicaDocumentRegistry>().host(doc_id).map(|h| h.document().ast_is_stale()).unwrap_or(false);
        if ast_stale { ui.ctx().request_repaint(); }
        // One-shot re-projection requested when MSL became resident — forces a
        // re-project so standard-library icons resolve, independent of gen/target.
        let forced = docstate.force_reproject;

        !task_in_flight && (first_render || target_changed || forced || (!ast_stale && gen_advanced && {
            let new_hash = projection_relevant_source_hash(&*current_source);
            new_hash != docstate.last_seen_source_hash
        }))
    };

    if needs_project {
        spawn_projection_task(world, doc_id, gen, render_tab_id);
    } else {
        let new_hash = projection_relevant_source_hash(&*current_source);
        let mut state = world.resource_mut::<CanvasDiagramState>();
        let docstate = match render_tab_id { Some(t) => state.get_mut_for_tab(t, doc_id), None => state.get_mut(Some(doc_id)) };
        if gen != docstate.last_seen_gen && gen > docstate.canvas_acked_gen && new_hash == docstate.last_seen_source_hash {
             docstate.last_seen_gen = gen;
        }
    }
}

fn spawn_projection_task(world: &mut World, doc_id: lunco_doc::DocumentId, gen: u64, render_tab_id: Option<crate::model_tabs_types::TabId>) {
    let resolved = {
        let registry = world.resource::<ModelicaDocumentRegistry>();
        registry
            .host(doc_id)
            .and_then(|host| host.document().strict_ast().map(|ast| (host.document().source_arc(), ast)))
    };
    let (source, ast_arc) = match resolved {
        Some(v) => v,
        None => {
            // Doc/AST not ready yet (common on a fresh drill-in whose async
            // parse hasn't landed). Without this, the parse→project busy
            // handoff stashed by `drive_drill_in_loads` is never released and
            // the tab spins on "loading" forever, because no projection task
            // is created (so the deadline can't cancel it) and re-entry is
            // gated on gen/target change. Release the handoff so the
            // lifecycle falls through to Empty and re-projects once the AST
            // lands (the parse bumps the document generation → gen_advanced).
            let mut state = world.resource_mut::<CanvasDiagramState>();
            state.complete_projection_handoff(doc_id);
            // Consume the one-shot MSL-ready reprojection request too. If we
            // leave `force_reproject` set, `trigger_projection_if_needed`
            // re-enters every frame (forced=true) and we spin here until the
            // AST lands — wasted work each frame. Safe to drop: when the
            // async parse completes it bumps the generation, and
            // first_render/gen_advanced re-spawns the projection with MSL now
            // resident, so standard-library icons still resolve.
            let docstate = match render_tab_id {
                Some(t) => state.get_mut_for_tab(t, doc_id),
                None => state.get_mut(Some(doc_id)),
            };
            docstate.force_reproject = false;
            return;
        }
    };
    let (max_nodes, max_duration) = world.get_resource::<DiagramProjectionLimits>().map(|l| (l.max_nodes, l.max_duration)).unwrap_or((crate::ui::panels::canvas_projection::DEFAULT_MAX_DIAGRAM_NODES, std::time::Duration::from_secs(60)));
    let target_class = render_target(world)
        .filter(|(d, _)| *d == doc_id)
        .and_then(|(_, drilled)| drilled)
        .or_else(|| {
            world
                .get_resource::<ModelTabs>()
                .and_then(|t| t.drilled_class_for_doc(doc_id))
        })
        .or_else(|| {
            crate::sim_default::default_simulation_class(world, doc_id)
        });
    let layout = world.get_resource::<crate::ui::panels::canvas_projection::DiagramAutoLayoutSettings>().cloned().unwrap_or_default();
    
    let mut state = world.resource_mut::<CanvasDiagramState>();
    let docstate = match render_tab_id { Some(t) => state.get_mut_for_tab(t, doc_id), None => state.get_mut(Some(doc_id)) };
    if docstate.last_seen_target != target_class { docstate.last_seen_gen = 0; }
    
    let bg_handle = docstate.background_diagram.clone();
    let diag = decorations::diagram_annotation_for_target(ast_arc.as_ref(), target_class.as_deref());
    if let Ok(mut guard) = bg_handle.write() { *guard = diag; }
    
    if let Some(t) = docstate.projection_task.as_ref() { if t.gen_at_spawn != gen { t.cancel.store(true, std::sync::atomic::Ordering::Relaxed); } }
    docstate.projection_task = None;

    let spawned_at = web_time::Instant::now();
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cancel_for_task = std::sync::Arc::clone(&cancel);
    let target_for_log = target_class.clone();
    let source_hash = projection_relevant_source_hash(&*source);
    let label = match &target_class { Some(t) => format!("Projecting {t}"), None => "Projecting…".to_string() };

    // Release the `CanvasDiagramState` borrow before grabbing the bus —
    // they are disjoint resources, but Bevy won't let us hold both
    // simultaneously through `World`.
    drop(state);

    let task = {
        let mut bus = world.resource_mut::<lunco_workbench::status_bus::StatusBus>();
        lunco_workbench::tracked_task::spawn_tracked_cancellable(
            &mut bus,
            lunco_workbench::status_bus::BusyScope::Document(doc_id.0),
            "projection",
            label,
            std::sync::Arc::clone(&cancel),
            async move {
                use std::sync::atomic::Ordering;
                if cancel_for_task.load(Ordering::Relaxed) { return Scene::new(); }
                futures_lite::future::yield_now().await;
                let ast_for_recover = std::sync::Arc::clone(&ast_arc);
                let mut diagram = crate::ui::panels::canvas_projection::import_model_to_diagram_from_ast(ast_arc, &*source, max_nodes, target_for_log.as_deref(), &layout).unwrap_or_default();
                futures_lite::future::yield_now().await;
                recover_edges_from_ast(&ast_for_recover, &mut diagram);
                futures_lite::future::yield_now().await;
                let (scene, _) = project_scene(&diagram);
                scene
            },
        )
    };

    // Now that the "projection" entry is on the bus, complete any
    // parse→project handoff: drop the pending handle the driver
    // stashed when it resolved the parse. Bus stays continuously
    // busy for `Document(doc_id)` across the boundary because the
    // new entry was inserted before this drop fires.
    let mut state = world.resource_mut::<CanvasDiagramState>();
    state.complete_projection_handoff(doc_id);
    let docstate = match render_tab_id { Some(t) => state.get_mut_for_tab(t, doc_id), None => state.get_mut(Some(doc_id)) };

    // Consume the one-shot MSL-ready re-projection request (if any) — this
    // spawn satisfies it.
    docstate.force_reproject = false;
    docstate.projection_task = Some(ProjectionTask {
        gen_at_spawn: gen,
        doc_at_spawn: doc_id,
        target_at_spawn: target_class.clone(),
        spawned_at,
        deadline: max_duration,
        cancel,
        task,
        source_hash,
    });
}
