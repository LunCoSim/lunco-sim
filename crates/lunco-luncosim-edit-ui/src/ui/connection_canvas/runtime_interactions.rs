//! Live scenario authorization overlay, independent of authored USD wiring.

use super::{CanvasSourceLease, UsdCanvasSessionState, projection};
use bevy::prelude::*;
use bevy_egui::egui;
use lunco_canvas::NodeId;
use lunco_core::{Command, on_command, register_commands};
use lunco_doc::{Ack, OpId};
use lunco_usd_bevy_scene::UsdPrimPath;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Show admitted scenario accesses separately from authored USD connections.
#[Command(default)]
pub struct SetConnectionRuntimeInteractions {
    /// Exact live Connections view identity.
    pub view_id: u64,
    /// Whether declared accesses are painted over the static graph.
    pub enabled: bool,
}

#[on_command(SetConnectionRuntimeInteractions)]
fn set_runtime_interactions(
    trigger: On<SetConnectionRuntimeInteractions>,
    mut views: ResMut<super::UsdCanvasState>,
) -> Result<Ack, String> {
    let request = trigger.event();
    let state = super::view_files::state_for(&mut views, request.view_id)?;
    if state.doc.is_some() {
        return Err("Scenario access is available only for the live scene".into());
    }
    state.runtime_interactions.enabled = request.enabled;
    Ok(Ack::new(OpId::new()))
}

register_commands!(set_runtime_interactions);

pub(super) fn init(app: &mut App) {
    register_all_commands(app);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Access {
    Read,
    Write,
}

impl Access {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Read => "declared_read",
            Self::Write => "declared_write",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Interaction {
    pub scenario: String,
    pub target: String,
    pub access: Access,
    pub displayed: Option<(NodeId, NodeId)>,
}

pub(super) struct DisplayedInteraction {
    pub scenario: NodeId,
    pub target: NodeId,
    pub access: Access,
    pub count: usize,
    label: String,
}

#[derive(Default)]
pub(super) struct RuntimeInteractions {
    pub enabled: bool,
    pub revision: Option<u64>,
    source_generation: Option<u64>,
    source_lease: Option<CanvasSourceLease>,
    pub links: Vec<Interaction>,
    pub displayed_links: Vec<DisplayedInteraction>,
    pub issues: Vec<String>,
}

pub(super) fn refresh(
    state: &mut UsdCanvasSessionState,
    owner: Option<&lunco_core_runtime::SimulationBarrierParticipants>,
    paths: &Query<(Entity, &UsdPrimPath)>,
    parents: &Query<&ChildOf>,
    paths_changed: bool,
) {
    let revision = owner.map(|owner| owner.scenario_plan_revision());
    let lease = state.source_lease();
    if !state.built
        || (!paths_changed
            && state.runtime_interactions.revision == revision
            && state.runtime_interactions.source_generation == state.canonical_generation
            && state.runtime_interactions.source_lease == lease)
    {
        return;
    }
    let overlay = &mut state.runtime_interactions;
    overlay.links.clear();
    overlay.displayed_links.clear();
    overlay.issues.clear();
    overlay.revision = revision;
    overlay.source_generation = state.canonical_generation;
    overlay.source_lease = lease;
    let (Some(owner), Some(root), Some(stage)) = (owner, state.source_root, state.stage_id) else {
        return;
    };
    let path_for = |entity| {
        paths.get(entity).ok().and_then(|(_, path)| {
            (path.stage_handle.id() == stage
                && lunco_usd_bevy_scene::is_preview_entity(entity, root, parents))
            .then_some(path.path.as_str())
        })
    };
    for (scenario, plan) in owner.scenario_access_plans() {
        let Some(scenario_path) = path_for(scenario) else {
            continue;
        };
        for (access, entities) in [(Access::Read, plan.reads), (Access::Write, plan.writes)] {
            let targets: BTreeSet<_> = entities
                .iter()
                .chain(plan.modelica.iter())
                .copied()
                .collect();
            for target in targets {
                match path_for(target) {
                    Some(target_path) => overlay.links.push(Interaction {
                        scenario: scenario_path.to_owned(), target: target_path.to_owned(),
                        access, displayed: None,
                    }),
                    None => overlay.issues.push(format!(
                        "{scenario_path}: {} target {target:?} is outside this USD scene or has no live USD path",
                        access.name())),
                }
            }
        }
    }
    overlay.links.sort_unstable();
    overlay.links.dedup();
    overlay.issues.sort_unstable();
    overlay.issues.dedup();
    bind(state);
}

/// Rebind only when projection/grouping changes; rendering does no path search.
pub(super) fn bind(state: &mut UsdCanvasSessionState) {
    let mut displayed = HashMap::new();
    for (id, node) in state.canvas.scene.nodes() {
        if node
            .data
            .downcast_ref::<projection::UsdPrimNodeData>()
            .is_some_and(|data| data.boundary.is_none())
        {
            if let Some(path) = node.origin.as_ref() {
                displayed.insert(path.clone(), *id);
            }
        }
    }
    for group in state.group_plan.iter().filter(|group| group.collapsed) {
        if let Some((id, _)) = state
            .canvas
            .scene
            .nodes()
            .find(|(_, node)| projection::diagram_key(node) == Some(group.key().as_str()))
        {
            for path in &group.members {
                displayed.insert(path.clone(), *id);
            }
        }
    }
    let endpoint = |path: &str| {
        let mut candidate = path;
        loop {
            if let Some(id) = displayed.get(candidate) {
                return Some(*id);
            }
            candidate = candidate.rsplit_once('/')?.0;
        }
    };
    let mut counts = BTreeMap::new();
    for link in &mut state.runtime_interactions.links {
        link.displayed = endpoint(&link.scenario).zip(endpoint(&link.target));
        if let Some((scenario, target)) = link.displayed {
            if scenario != target {
                *counts
                    .entry((scenario, target, link.access))
                    .or_insert(0_usize) += 1;
            }
        }
    }
    state.runtime_interactions.displayed_links = counts
        .into_iter()
        .map(|((scenario, target, access), count)| DisplayedInteraction {
            scenario,
            target,
            access,
            count,
            label: if count > 1 {
                format!(
                    "{count} {}",
                    match access {
                        Access::Read => "reads",
                        Access::Write => "writes",
                    }
                )
            } else {
                String::new()
            },
        })
        .collect();
}

pub(super) fn paint(ui: &mut egui::Ui, state: &UsdCanvasSessionState, rect: egui::Rect) {
    if !state.runtime_interactions.enabled {
        return;
    }
    let theme = lunco_theme::active(ui.ctx());
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let dash = theme.spacing.window_padding.max(1.0);
    let gap = theme.spacing.item_spacing.max(1.0);
    let stroke_width = ui.visuals().widgets.noninteractive.fg_stroke.width;
    let viewport = lunco_canvas::Rect::from_min_max(
        lunco_canvas::Pos::new(rect.min.x, rect.min.y),
        lunco_canvas::Pos::new(rect.max.x, rect.max.y),
    );
    for link in &state.runtime_interactions.displayed_links {
        let (scenario, target) = (link.scenario, link.target);
        let (from, to) = match link.access {
            Access::Read => (target, scenario),
            Access::Write => (scenario, target),
        };
        let (Some(from), Some(to)) = (state.canvas.scene.node(from), state.canvas.scene.node(to))
        else {
            continue;
        };
        let screen = |node: &lunco_canvas::Node| {
            let min = state
                .canvas
                .viewport
                .world_to_screen(node.rect.min, viewport);
            let max = state
                .canvas
                .viewport
                .world_to_screen(node.rect.max, viewport);
            egui::Rect::from_min_max(egui::pos2(min.x, min.y), egui::pos2(max.x, max.y))
        };
        let from = screen(from);
        let to = screen(to);
        if !from.is_finite() || !to.is_finite() {
            continue;
        }
        let delta = to.center() - from.center();
        let length = delta.length();
        if !length.is_finite() || length < 1.0 {
            continue;
        }
        let direction = delta / length;
        let normal = egui::vec2(-direction.y, direction.x);
        let lane = normal * gap * 0.5;
        let start = from.intersects_ray_from_center(direction) + lane;
        let end = to.intersects_ray_from_center(-direction) + lane;
        if (end - start).dot(direction) <= 0.0 {
            continue;
        }
        let clip = rect.intersect(ui.clip_rect());
        let Some((clipped_start, clipped_end)) = lunco_canvas::geometry::clip_segment(
            lunco_canvas::Pos::new(start.x, start.y),
            lunco_canvas::Pos::new(end.x, end.y),
            lunco_canvas::Rect::from_min_max(
                lunco_canvas::Pos::new(clip.min.x, clip.min.y),
                lunco_canvas::Pos::new(clip.max.x, clip.max.y),
            ),
        ) else {
            continue;
        };
        let visible_start = egui::pos2(clipped_start.x, clipped_start.y);
        let visible_end = egui::pos2(clipped_end.x, clipped_end.y);
        let length = (visible_end - visible_start).length();
        if !length.is_finite() || length < 1.0 {
            continue;
        }
        let color = match link.access {
            Access::Read => theme.tokens.port_input,
            Access::Write => theme.tokens.port_output,
        };
        let mut distance = 0.0;
        while distance < length {
            painter.line_segment(
                [
                    visible_start + direction * distance,
                    visible_start + direction * (distance + dash).min(length),
                ],
                egui::Stroke::new(stroke_width, color),
            );
            distance += dash + gap;
        }
        let tip = visible_end - direction * gap;
        painter.line_segment(
            [tip - direction * dash + normal * gap, tip],
            egui::Stroke::new(stroke_width, color),
        );
        painter.line_segment(
            [tip - direction * dash - normal * gap, tip],
            egui::Stroke::new(stroke_width, color),
        );
        if !link.label.is_empty() {
            painter.text(
                visible_start + (visible_end - visible_start) * 0.5 + normal * dash,
                egui::Align2::CENTER_CENTER,
                &link.label,
                egui::FontId::proportional(theme.typography.caption),
                color,
            );
        }
    }
}
