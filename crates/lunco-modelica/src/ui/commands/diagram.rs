//! Diagram-specific commands: MoveComponent and AddCanvasPlot.

use bevy::prelude::*;
use lunco_core::{on_command, Command};

// ─── Command Structs ─────────────────────────────────────────────────────────

/// Move a component instance in the diagram.
#[Command(default)]
pub struct MoveComponent {
    pub class: String,
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Drop a "Scope" plot onto the active canvas.
#[Command(default)]
pub struct AddCanvasPlot {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub signal: String,
}

// ─── Observers ───────────────────────────────────────────────────────────────

#[on_command(MoveComponent)]
pub fn on_move_component(trigger: On<MoveComponent>, mut commands: Commands) {
    let ev = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        use crate::document::ModelicaOp;
        use crate::pretty::Placement;
        let active_doc = world
            .get_resource::<lunco_workspace::WorkspaceResource>()
            .and_then(|ws| ws.active_document);
        let Some(doc_id) = active_doc else {
            bevy::log::warn!("[MoveComponent] no active document");
            return;
        };
        let class = if ev.class.is_empty() {
            crate::sim_default::drilled_class_for_doc(world, doc_id)
                .or_else(|| crate::state::detected_name_for(world, doc_id))
                .unwrap_or_default()
        } else {
            ev.class.clone()
        };
        if class.is_empty() {
            bevy::log::warn!("[MoveComponent] could not resolve target class for doc");
            return;
        }
        let (width, height) = if ev.width > 0.0 && ev.height > 0.0 {
            (ev.width, ev.height)
        } else {
            use crate::ui::panels::canvas_diagram::CanvasDiagramState;
            world
                .get_resource::<CanvasDiagramState>()
                .and_then(|state| {
                    let docstate = state.get(Some(doc_id));
                    docstate.canvas.scene.nodes().find_map(|(_id, n)| {
                        if n.origin.as_deref() == Some(ev.name.as_str()) {
                            Some((n.rect.width().max(1.0), n.rect.height().max(1.0)))
                        } else {
                            None
                        }
                    })
                })
                .unwrap_or((20.0, 20.0))
        };
        let op = ModelicaOp::SetPlacement {
            class: class,
            name: ev.name.clone(),
            placement: Placement {
                x: ev.x,
                y: ev.y,
                width,
                height,
            },
        };
        crate::ui::panels::canvas_diagram::apply_ops_public(world, doc_id, vec![op]);
    });
}

#[on_command(AddCanvasPlot)]
pub fn on_add_canvas_plot(trigger: On<AddCanvasPlot>, mut commands: Commands) {
    let ev = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        use crate::document::ModelicaOp;
        let Some(doc) = super::resolve_active_doc(world) else {
            bevy::log::warn!("[AddCanvasPlot] no active document");
            return;
        };
        let w = if ev.width > 0.0 { ev.width } else { 120.0 };
        let h = if ev.height > 0.0 { ev.height } else { 90.0 };
        if ev.signal.is_empty() {
            bevy::log::warn!("[AddCanvasPlot] empty signal — skipping (bind one first)");
            return;
        }
        let class = crate::sim_default::drilled_class_for_doc(world, doc)
            .or_else(|| crate::state::detected_name_for(world, doc))
            .unwrap_or_default();
        if class.is_empty() {
            bevy::log::warn!("[AddCanvasPlot] could not resolve target class for doc");
            return;
        }
        let plot = crate::pretty::LunCoPlotNodeSpec {
            x1: ev.x,
            y1: ev.y,
            x2: ev.x + w,
            y2: ev.y + h,
            signal: ev.signal.clone(),
            title: String::new(),
        };
        crate::ui::panels::canvas_diagram::apply_ops_public(
            world,
            doc,
            vec![ModelicaOp::AddPlotNode { class, plot }],
        );
    });
}
