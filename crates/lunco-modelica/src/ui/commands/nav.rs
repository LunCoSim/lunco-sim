//! UI navigation and view control: Focus, ViewMode, Zoom, Fit, and Pan.

use bevy::prelude::*;
use lunco_doc::DocumentId;
use lunco_core::{Command, on_command};

// ─── Command Structs ─────────────────────────────────────────────────────────

#[Command(default)]
pub struct AutoArrangeDiagram {
    pub doc: DocumentId,
}

#[Command(default)]
pub struct FocusDocumentByName {
    pub pattern: String,
}

#[Command(default)]
pub struct SetViewMode {
    pub doc: DocumentId,
    pub mode: String,
}

#[Command(default)]
pub struct SetZoom {
    pub doc: DocumentId,
    pub zoom: f32,
}

#[Command(default)]
pub struct FitCanvas {
    pub doc: DocumentId,
}

#[Command(default)]
pub struct FocusComponent {
    pub doc: DocumentId,
    pub name: String,
    pub padding: f32,
}

#[Command(default)]
pub struct PanCanvas {
    pub doc: DocumentId,
    pub x: f32,
    pub y: f32,
}

// ─── Observers ───────────────────────────────────────────────────────────────

#[on_command(FocusDocumentByName)]
pub fn on_focus_document_by_name(
    trigger: On<FocusDocumentByName>,
    mut commands: Commands,
) {
    let pattern = trigger.event().pattern.clone();
    if pattern.is_empty() {
        return;
    }
    commands.queue(move |world: &mut World| {
        let hit = {
            let ws = world.resource::<lunco_workspace::WorkspaceResource>();
            let needle = pattern.to_lowercase();
            ws.documents()
                .iter()
                .find(|d| d.title.to_lowercase().contains(&needle))
                .map(|d| d.id)
        };
        let Some(doc) = hit else {
            bevy::log::info!(
                "[FocusDocumentByName] no tab matches '{}'",
                pattern
            );
            return;
        };
        let tab_id = world
            .resource_mut::<crate::model_tabs::ModelTabs>()
            .ensure_for(doc, None);
        world.commands().trigger(lunco_workbench::OpenTab {
            kind: crate::model_tabs_types::MODEL_VIEW_KIND,
            instance: tab_id,
        });
    });
}

#[on_command(SetViewMode)]
pub fn on_set_view_mode(trigger: On<SetViewMode>, mut commands: Commands) {
    let raw = trigger.event().doc;
    let mode_str = trigger.event().mode.clone();
    commands.queue(move |world: &mut World| {
        let Some(doc) = (if raw.is_unassigned() {
            super::resolve_active_doc(world)
        } else {
            Some(raw)
        }) else {
            return;
        };
        use crate::model_tabs::ModelTabs; use crate::model_tabs_types::ModelViewMode;
        let new_mode = match mode_str.to_lowercase().as_str() {
            "text" | "source" => ModelViewMode::Text,
            "diagram" | "canvas" => ModelViewMode::Canvas,
            "icon" => ModelViewMode::Icon,
            "docs" | "documentation" => ModelViewMode::Docs,
            other => {
                bevy::log::warn!(
                    "[SetViewMode] unknown mode '{other}' — expected text|diagram|icon|docs"
                );
                return;
            }
        };
        if let Some(mut tabs) = world.get_resource_mut::<ModelTabs>() {
            if let Some(tab_id) = tabs.any_for_doc(doc) {
                if let Some(state) = tabs.get_mut(tab_id) {
                    state.view_mode = new_mode;
                }
            }
        }
    });
}

#[on_command(SetZoom)]
pub fn on_set_zoom(trigger: On<SetZoom>, mut commands: Commands) {
    let raw = trigger.event().doc;
    let zoom = trigger.event().zoom;
    commands.queue(move |world: &mut World| {
        let doc = if raw.is_unassigned() {
            super::resolve_active_doc(world)
        } else {
            Some(raw)
        };
        use crate::ui::panels::canvas_diagram::CanvasDiagramState;
        let Some(mut state) = world.get_resource_mut::<CanvasDiagramState>() else {
            return;
        };
        let docstate = state.get_mut(doc);
        if zoom <= 0.0 {
            if let Some(bounds) = docstate.canvas.scene.bounds() {
                let sr = super::approx_screen_rect();
                let (c, z) = docstate.canvas.viewport.fit_values(bounds, sr, 40.0);
                docstate.canvas.viewport.set_target(c, z);
            }
        } else {
            let vp = &mut docstate.canvas.viewport;
            let c = vp.center;
            vp.set_target(c, zoom);
        }
    });
}

#[on_command(FocusComponent)]
pub fn on_focus_component(trigger: On<FocusComponent>, mut commands: Commands) {
    let raw = trigger.event().doc;
    let name = trigger.event().name.clone();
    let padding = if trigger.event().padding > 0.0 { trigger.event().padding } else { 0.5 };
    commands.queue(move |world: &mut World| {
        let doc = if raw.is_unassigned() {
            super::resolve_active_doc(world)
        } else {
            Some(raw)
        };
        use crate::ui::panels::canvas_diagram::CanvasDiagramState;
        let Some(mut state) = world.get_resource_mut::<CanvasDiagramState>() else {
            return;
        };
        let docstate = state.get_mut(doc);
        let target = docstate
            .canvas
            .scene
            .nodes()
            .find(|(_, n)| n.label == name)
            .map(|(_, n)| n.rect);
        let Some(rect) = target else {
            bevy::log::warn!("[FocusComponent] no node named `{}` on canvas", name);
            return;
        };
        let sr = super::approx_screen_rect();
        let viewport_dim = sr.width().min(sr.height());
        let world_dim = rect.width().max(rect.height()).max(1e-3);
        let zoom = (viewport_dim * padding) / world_dim;
        let centre = lunco_canvas::Pos::new(
            (rect.min.x + rect.max.x) * 0.5,
            (rect.min.y + rect.max.y) * 0.5,
        );
        docstate.canvas.viewport.set_target(centre, zoom);
    });
}

#[on_command(FitCanvas)]
pub fn on_fit_canvas(trigger: On<FitCanvas>, mut commands: Commands) {
    let raw = trigger.event().doc;
    commands.queue(move |world: &mut World| {
        let doc = if raw.is_unassigned() {
            super::resolve_active_doc(world)
        } else {
            Some(raw)
        };
        use crate::ui::panels::canvas_diagram::CanvasDiagramState;
        let Some(mut state) = world.get_resource_mut::<CanvasDiagramState>() else {
            return;
        };
        state.get_mut(doc).pending_fit = true;
    });
}

#[on_command(PanCanvas)]
pub fn on_pan_canvas(trigger: On<PanCanvas>, mut commands: Commands) {
    let ev = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        let doc = if ev.doc.is_unassigned() {
            super::resolve_active_doc(world)
        } else {
            Some(ev.doc)
        };
        use crate::ui::panels::canvas_diagram::CanvasDiagramState;
        let Some(mut state) = world.get_resource_mut::<CanvasDiagramState>() else {
            return;
        };
        let docstate = state.get_mut(doc);
        let z = docstate.canvas.viewport.zoom;
        docstate.canvas.viewport.set_target(lunco_canvas::Pos::new(ev.x, ev.y), z);
    });
}
