//! Autopilot behaviour canvas.
//!
//! This is a projector over [`BehaviorSpec`], not a second behaviour format.  The
//! spec is derived from the selected vessel's authored BT.CPP mission, so every
//! sequence, selector, decorator and leaf shown here is exactly what the rover
//! will execute.  The canvas is deliberately read-only: its source is the
//! mission's `info:sourceCode` / `info:sourceAsset`, not canvas layout state.

use std::sync::Arc;

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_autopilot::{AutopilotBehaviorSpec, BehaviorSpec, PatrolWaypoint};
use lunco_canvas::{
    empty_node_data, Canvas, Edge, EdgeVisual, Node, NodeVisual, Port, PortId, PortRef, Pos, Rect,
    Scene, VisualRegistry,
};
use lunco_usd::commands::{ApplyUsdOp, ApplyUsdOps};
use lunco_usd::document::{LayerId, UsdOp};
use lunco_usd_bevy::UsdPrimPath;
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot};

use crate::SelectedEntities;

const NODE_KIND: &str = "autopilot.behavior";
const EDGE_KIND: &str = "autopilot.flow";
const NODE_W: f32 = 190.0;
const NODE_H: f32 = 76.0;
const DEPTH_STEP: f32 = 260.0;
const ROW_STEP: f32 = 120.0;
const MARGIN: f32 = 48.0;

#[derive(Clone, Debug)]
struct BehaviorNodeData {
    detail: String,
    composite: bool,
}

struct BehaviorEdgeVisual;

fn registry() -> VisualRegistry {
    let mut registry = VisualRegistry::new();
    registry.register_node_kind(NODE_KIND, |data| {
        data.downcast_ref::<BehaviorNodeData>()
            .cloned()
            .unwrap_or(BehaviorNodeData {
                detail: String::new(),
                composite: false,
            })
    });
    registry.register_edge_kind(EDGE_KIND, |_| BehaviorEdgeVisual);
    registry
}

impl NodeVisual for BehaviorNodeData {
    fn draw(&self, ctx: &mut lunco_canvas::DrawCtx, node: &Node, selected: bool) {
        let world = ctx
            .viewport
            .world_rect_to_screen(node.rect, ctx.screen_rect);
        let rect = egui::Rect::from_min_max(
            egui::pos2(world.min.x, world.min.y),
            egui::pos2(world.max.x, world.max.y),
        );
        let theme = lunco_theme::active(ctx.ui.ctx());
        let tokens = &theme.tokens;
        let fill = if node.label == "Start" {
            // Entry is a control-flow sentinel, not an authored behaviour node.
            // Give it a clear, stable identity even when selection changes.
            tokens.success
        } else if selected {
            tokens.node_card_selected
        } else if self.composite {
            tokens.node_card_body
        } else {
            tokens.node_card
        };
        let border = if selected {
            tokens.node_border_selected
        } else {
            tokens.node_border
        };
        let painter = ctx.ui.painter();
        painter.rect_filled(rect, 7.0, fill);
        painter.rect_stroke(
            rect,
            7.0,
            egui::Stroke::new(if selected { 2.0 } else { 1.0 }, border),
            egui::StrokeKind::Outside,
        );
        if rect.height() > 20.0 {
            painter.text(
                egui::pos2(rect.center().x, rect.min.y + rect.height() * 0.34),
                egui::Align2::CENTER_CENTER,
                &node.label,
                egui::FontId::proportional((rect.height() * 0.25).clamp(10.0, 16.0)),
                tokens.text,
            );
            if !self.detail.is_empty() && rect.height() > 42.0 {
                painter.text(
                    egui::pos2(rect.center().x, rect.min.y + rect.height() * 0.67),
                    egui::Align2::CENTER_CENTER,
                    &self.detail,
                    egui::FontId::proportional((rect.height() * 0.17).clamp(8.0, 11.0)),
                    tokens.text_subdued,
                );
            }
        }
    }

    fn debug_name(&self) -> &str {
        NODE_KIND
    }
}

impl EdgeVisual for BehaviorEdgeVisual {
    fn draw(
        &self,
        ctx: &mut lunco_canvas::DrawCtx,
        from: Pos,
        to: Pos,
        _waypoints: &[Pos],
        selected: bool,
    ) {
        let theme = lunco_theme::active(ctx.ui.ctx());
        let color = if selected {
            theme.tokens.node_border_selected
        } else {
            theme.schematic.wire_signal
        };
        let a = egui::pos2(from.x, from.y);
        let b = egui::pos2(to.x, to.y);
        let painter = ctx.ui.painter();
        painter.line_segment(
            [a, b],
            egui::Stroke::new(if selected { 2.5 } else { 1.5 }, color),
        );
        let direction = b - a;
        if direction.length() > 1.0 {
            let d = direction / direction.length();
            let normal = egui::vec2(-d.y, d.x);
            let tip = b - d * 8.0;
            painter.add(egui::Shape::convex_polygon(
                vec![b, tip + normal * 5.0, tip - normal * 5.0],
                color,
                egui::Stroke::NONE,
            ));
        }
    }
}

/// UI-local projection state. Node positions are layout only; mission data stays
/// in the USD-owned BT.CPP source.
#[derive(Resource)]
pub struct AutopilotCanvasState {
    canvas: Canvas,
    selected: Option<Entity>,
    signature: Option<String>,
    built: bool,
    needs_fit: bool,
}

impl Default for AutopilotCanvasState {
    fn default() -> Self {
        let mut canvas = Canvas::new(registry());
        canvas.read_only = true;
        Self {
            canvas,
            selected: None,
            signature: None,
            built: false,
            needs_fit: false,
        }
    }
}

/// Refresh the graph only when the selected vessel or its derived mission changes.
/// This is an O(1) selection lookup; graph layout is never part of the frame loop.
pub fn produce_autopilot_canvas(
    selected: Res<SelectedEntities>,
    specs: Query<&AutopilotBehaviorSpec>,
    mut state: ResMut<AutopilotCanvasState>,
) {
    let vessel = selected.primary();
    let spec = vessel.and_then(|entity| specs.get(entity).ok());
    let signature = spec.and_then(|s| s.to_json().ok());
    if state.selected == vessel && state.signature == signature {
        return;
    }

    state.selected = vessel;
    state.signature = signature;
    state.canvas.selection.clear();
    state.built = false;
    if let Some(spec) = spec {
        state.canvas.scene = build_scene(&spec.0);
        state.built = true;
        state.needs_fit = state.canvas.scene.bounds().is_some();
    } else {
        state.canvas.scene = Scene::new();
    }
}

struct Layout<'a> {
    scene: Scene,
    next_row: f32,
    _marker: std::marker::PhantomData<&'a ()>,
}

impl<'a> Layout<'a> {
    fn node(
        &mut self,
        label: String,
        detail: String,
        composite: bool,
        depth: usize,
    ) -> lunco_canvas::NodeId {
        let id = self.scene.alloc_node_id();
        let row = self.next_row;
        self.next_row += 1.0;
        self.scene.insert_node(Node {
            id,
            rect: Rect::from_min_size(
                Pos::new(MARGIN + depth as f32 * DEPTH_STEP, MARGIN + row * ROW_STEP),
                NODE_W,
                NODE_H,
            ),
            kind: NODE_KIND.into(),
            data: Arc::new(BehaviorNodeData { detail, composite }),
            ports: vec![
                Port {
                    id: PortId::new("parent"),
                    local_offset: Pos::new(0.0, NODE_H * 0.5),
                    kind: "input".into(),
                },
                Port {
                    id: PortId::new("next"),
                    local_offset: Pos::new(NODE_W, NODE_H * 0.5),
                    kind: "output".into(),
                },
            ],
            label,
            origin: Some(String::new()),
            resizable: false,
            visual_rect: None,
        })
    }

    fn edge(&mut self, parent: lunco_canvas::NodeId, child: lunco_canvas::NodeId) {
        let id = self.scene.alloc_edge_id();
        self.scene.insert_edge(Edge {
            id,
            from: PortRef {
                node: parent,
                port: PortId::new("next"),
            },
            to: PortRef {
                node: child,
                port: PortId::new("parent"),
            },
            kind: EDGE_KIND.into(),
            data: empty_node_data(),
            origin: None,
            waypoints: Vec::new(),
            waypoints_authored: false,
        });
    }

    fn visit(&mut self, spec: &BehaviorSpec, path: &str, depth: usize) -> lunco_canvas::NodeId {
        let (label, detail, composite) = node_text(spec);
        let id = self.node(label, detail, composite, depth);
        self.scene.node_mut(id).expect("inserted node").origin = Some(path.to_string());
        match spec {
            BehaviorSpec::Sequence { children } | BehaviorSpec::ReactiveSequence { children } => {
                // A sequence is a control-flow CHAIN, not a fan-out: each step
                // begins only after its predecessor succeeds. Rendering the
                // edges that way keeps the diagram truthful and makes a route
                // read left-to-right as a program.
                let mut previous = id;
                for (index, child) in children.iter().enumerate() {
                    let child_id = self.visit(
                        child,
                        &format!("{path}/children/{index}"),
                        depth + index + 1,
                    );
                    self.edge(previous, child_id);
                    previous = child_id;
                }
            }
            BehaviorSpec::Selector { children }
            | BehaviorSpec::Parallel { children, .. }
            | BehaviorSpec::ReactiveSelector { children } => {
                for (index, child) in children.iter().enumerate() {
                    let child_id =
                        self.visit(child, &format!("{path}/children/{index}"), depth + 1);
                    self.edge(id, child_id);
                }
            }
            BehaviorSpec::Forever { child }
            | BehaviorSpec::Repeat { child, .. }
            | BehaviorSpec::Invert { child }
            | BehaviorSpec::ForceSuccess { child }
            | BehaviorSpec::ForceFailure { child }
            | BehaviorSpec::Retry { child, .. }
            | BehaviorSpec::Timeout { child, .. }
            | BehaviorSpec::Cooldown { child, .. } => {
                let child_id = self.visit(child, &format!("{path}/child"), depth + 1);
                self.edge(id, child_id);
            }
            BehaviorSpec::Patrol { waypoints, .. } => {
                for (index, waypoint) in waypoints.iter().enumerate() {
                    let child_id = self.waypoint(index, waypoint, depth + 1);
                    self.edge(id, child_id);
                }
            }
            _ => {}
        }
        id
    }

    fn waypoint(
        &mut self,
        index: usize,
        waypoint: &PatrolWaypoint,
        depth: usize,
    ) -> lunco_canvas::NodeId {
        let mut detail = format!(
            "[{:.1}, {:.1}, {:.1}]",
            waypoint.pos[0], waypoint.pos[1], waypoint.pos[2]
        );
        if let Some(dwell) = waypoint.dwell {
            detail.push_str(&format!(" · wait {dwell:.1}s"));
        }
        if !waypoint.on_arrival.is_empty() {
            detail.push_str(&format!(" · {} tool(s)", waypoint.on_arrival.len()));
        }
        self.node(format!("Drive to {}", index + 1), detail, false, depth)
    }
}

fn build_scene(spec: &BehaviorSpec) -> Scene {
    let mut layout = Layout {
        scene: Scene::new(),
        next_row: 0.0,
        _marker: std::marker::PhantomData,
    };
    // Every program has one explicit entry point. It is a canvas-only node
    // (empty origin, so it cannot be edited/deleted) followed by the authored
    // root node. This makes the execution direction unambiguous even for a
    // root selector or decorator.
    let start = layout.node("Start".into(), "program entry".into(), true, 0);
    let root = layout.visit(spec, "", 1);
    layout.edge(start, root);
    layout.scene
}

fn node_text(spec: &BehaviorSpec) -> (String, String, bool) {
    match spec {
        BehaviorSpec::Sequence { children } => (
            "Sequence".into(),
            format!("{} step(s)", children.len()),
            true,
        ),
        BehaviorSpec::Selector { children } => (
            "Selector".into(),
            format!("{} option(s)", children.len()),
            true,
        ),
        BehaviorSpec::Parallel { require, children } => (
            "Parallel".into(),
            format!("{require:?} · {} branch(es)", children.len()),
            true,
        ),
        BehaviorSpec::Forever { .. } => ("Forever".into(), "repeat child".into(), true),
        BehaviorSpec::Repeat { times, .. } => ("Repeat".into(), format!("{times} time(s)"), true),
        BehaviorSpec::DriveTo {
            target,
            speed,
            radius,
        } => (
            "Drive to".into(),
            format!(
                "[{:.1}, {:.1}, {:.1}] · {speed:.1} / r{radius:.1}",
                target[0], target[1], target[2]
            ),
            false,
        ),
        BehaviorSpec::Patrol {
            waypoints,
            speed,
            radius,
            dwell,
        } => (
            "Patrol".into(),
            format!(
                "{} waypoint(s) · {speed:.1} / r{radius:.1} · wait {dwell:.1}s",
                waypoints.len()
            ),
            true,
        ),
        BehaviorSpec::Arrived { target, radius } => (
            "Arrived?".into(),
            format!(
                "[{:.1}, {:.1}, {:.1}] · r{radius:.1}",
                target[0], target[1], target[2]
            ),
            false,
        ),
        BehaviorSpec::Wait { seconds } => ("Wait".into(), format!("{seconds:.1}s"), false),
        BehaviorSpec::Cruise { throttle, steer } => (
            "Cruise".into(),
            format!("throttle {throttle:+.1} · steer {steer:+.1}"),
            false,
        ),
        BehaviorSpec::Brake => ("Brake".into(), "success".into(), false),
        BehaviorSpec::Face { target, tolerance } => (
            "Face".into(),
            format!(
                "[{:.1}, {:.1}, {:.1}] · ±{tolerance:.1}°",
                target[0], target[1], target[2]
            ),
            false,
        ),
        BehaviorSpec::Succeed => ("Succeed".into(), String::new(), false),
        BehaviorSpec::Fail => ("Fail".into(), String::new(), false),
        BehaviorSpec::ReactiveSequence { children } => (
            "Reactive sequence".into(),
            format!("{} step(s)", children.len()),
            true,
        ),
        BehaviorSpec::ReactiveSelector { children } => (
            "Reactive selector".into(),
            format!("{} option(s)", children.len()),
            true,
        ),
        BehaviorSpec::Invert { .. } => ("Invert".into(), "swap success / failure".into(), true),
        BehaviorSpec::ForceSuccess { .. } => ("Force success".into(), String::new(), true),
        BehaviorSpec::ForceFailure { .. } => ("Force failure".into(), String::new(), true),
        BehaviorSpec::Retry { times, .. } => ("Retry".into(), format!("{times} attempt(s)"), true),
        BehaviorSpec::Timeout { seconds, .. } => ("Timeout".into(), format!("{seconds:.1}s"), true),
        BehaviorSpec::Follow {
            target,
            speed,
            radius,
        } => (
            "Follow".into(),
            format!("#{target} · {speed:.1} / r{radius:.1}"),
            false,
        ),
        BehaviorSpec::Intercept {
            target,
            speed,
            radius,
            lead,
        } => (
            "Intercept".into(),
            format!("#{target} · {speed:.1} / r{radius:.1} · lead {lead:.1}s"),
            false,
        ),
        BehaviorSpec::ObstacleAhead { distance, cone } => (
            "Obstacle ahead?".into(),
            format!("{distance:.1}m · {cone:.0}°"),
            false,
        ),
        BehaviorSpec::Facing { target, tolerance } => (
            "Facing?".into(),
            format!(
                "[{:.1}, {:.1}, {:.1}] · ±{tolerance:.1}°",
                target[0], target[1], target[2]
            ),
            false,
        ),
        BehaviorSpec::Hold => ("Hold".into(), "braked · running".into(), false),
        BehaviorSpec::Cooldown { seconds, .. } => {
            ("Cooldown".into(), format!("{seconds:.1}s"), true)
        }
        BehaviorSpec::PathBlocked { distance } => {
            ("Path blocked?".into(), format!("{distance:.1}m"), false)
        }
        BehaviorSpec::SteerClear { speed } => {
            ("Steer clear".into(), format!("speed {speed:.1}"), false)
        }
        BehaviorSpec::RunTool { tool, args } => (
            "Run tool".into(),
            if args.is_empty() {
                tool.clone()
            } else {
                format!("{tool}({args})")
            },
            false,
        ),
    }
}

#[derive(Clone, Copy)]
enum EditAction {
    Append(NodeTemplate),
    ReplaceBrake,
}

#[derive(Clone, Copy)]
enum NodeTemplate {
    DriveTo,
    Brake,
    Wait,
    Sequence,
    Selector,
    Parallel,
    Retry,
    Invert,
}

impl NodeTemplate {
    fn label(self) -> &'static str {
        match self {
            Self::DriveTo => "Drive to",
            Self::Brake => "Brake",
            Self::Wait => "Wait",
            Self::Sequence => "Sequence",
            Self::Selector => "Selector",
            Self::Parallel => "Parallel",
            Self::Retry => "Retry",
            Self::Invert => "Invert",
        }
    }

    fn json(self) -> serde_json::Value {
        match self {
            Self::DriveTo => serde_json::json!({"kind":"drive_to", "target":[0.0, 0.0, 0.0]}),
            Self::Brake => serde_json::json!({"kind":"brake"}),
            Self::Wait => serde_json::json!({"kind":"wait", "seconds":1.0}),
            Self::Sequence => serde_json::json!({"kind":"sequence", "children":[]}),
            Self::Selector => serde_json::json!({"kind":"selector", "children":[]}),
            Self::Parallel => {
                serde_json::json!({"kind":"parallel", "require":"All", "children":[]})
            }
            Self::Retry => serde_json::json!({"kind":"retry", "times":3, "child":{"kind":"brake"}}),
            Self::Invert => serde_json::json!({"kind":"invert", "child":{"kind":"brake"}}),
        }
    }
}

fn edit_spec_json(source: &str, path: &str, action: EditAction) -> Result<String, String> {
    let mut root: serde_json::Value = serde_json::from_str(source).map_err(|e| e.to_string())?;
    let node = root
        .pointer_mut(path)
        .ok_or_else(|| "selected node is no longer in this mission".to_string())?;
    match action {
        EditAction::ReplaceBrake => *node = NodeTemplate::Brake.json(),
        EditAction::Append(template) => {
            let children = node
                .get_mut("children")
                .and_then(serde_json::Value::as_array_mut)
                .ok_or_else(|| {
                    "select a sequence, selector, parallel, or reactive branch to append a child"
                        .to_string()
                })?;
            children.push(template.json());
        }
    }
    serde_json::to_string(&root).map_err(|e| e.to_string())
}

fn apply_editor_action(
    ctx: &mut PanelCtx,
    vessel: Option<Entity>,
    source: Option<&str>,
    selected_path: Option<&str>,
    action: EditAction,
) {
    let (Some(vessel), Some(source), Some(path)) = (vessel, source, selected_path) else {
        return;
    };
    match edit_spec_json(source, path, action) {
        Ok(json) => write_mission_xml(ctx, vessel, json),
        Err(err) => bevy::log::warn!("[autopilot-canvas] {err}"),
    }
}

#[derive(Clone, Copy)]
struct DriveToFields {
    target: [f64; 3],
    speed: f64,
    radius: f64,
}

fn drive_to_fields(source: &str, path: &str) -> Option<DriveToFields> {
    let root: serde_json::Value = serde_json::from_str(source).ok()?;
    let node = root.pointer(path)?;
    (node.get("kind")?.as_str()? == "drive_to").then_some(DriveToFields {
        target: node
            .get("target")?
            .as_array()?
            .iter()
            .map(serde_json::Value::as_f64)
            .collect::<Option<Vec<_>>>()?
            .try_into()
            .ok()?,
        speed: node
            .get("speed")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.5),
        radius: node
            .get("radius")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(2.0),
    })
}

fn edit_drive_to_json(source: &str, path: &str, fields: DriveToFields) -> Result<String, String> {
    let mut root: serde_json::Value = serde_json::from_str(source).map_err(|e| e.to_string())?;
    let node = root
        .pointer_mut(path)
        .ok_or_else(|| "selected Drive to node is no longer in this mission".to_string())?;
    if node.get("kind").and_then(serde_json::Value::as_str) != Some("drive_to") {
        return Err("selected node is not Drive to".to_string());
    }
    node["target"] = serde_json::json!(fields.target);
    node["speed"] = serde_json::json!(fields.speed.max(0.0));
    node["radius"] = serde_json::json!(fields.radius.max(0.01));
    serde_json::to_string(&root).map_err(|e| e.to_string())
}

fn apply_drive_to_fields(
    ctx: &mut PanelCtx,
    vessel: Option<Entity>,
    source: Option<&str>,
    path: Option<&str>,
    fields: DriveToFields,
) {
    let (Some(vessel), Some(source), Some(path)) = (vessel, source, path) else {
        return;
    };
    match edit_drive_to_json(source, path, fields) {
        Ok(json) => write_mission_xml(ctx, vessel, json),
        Err(err) => bevy::log::warn!("[autopilot-canvas] {err}"),
    }
}

fn write_mission_xml(ctx: &mut PanelCtx, vessel: Entity, json: String) {
    let xml = match lunco_autopilot::btcpp_xml::value_to_xml(
        &serde_json::from_str::<serde_json::Value>(&json)
            .map_err(|e| e.to_string())
            .unwrap_or_default(),
    ) {
        Ok(xml) => xml,
        Err(err) => {
            bevy::log::warn!("[autopilot-canvas] cannot write mission: {err}");
            return;
        }
    };
    ctx.defer(move |world| {
        let Some(doc) = lunco_scene_commands::doc_resolve::resolve_doc_for_entity(world, vessel)
        else {
            bevy::log::warn!("[autopilot-canvas] selected vessel is not document-backed");
            return;
        };
        let Some(prim) = world.get::<UsdPrimPath>(vessel) else {
            return;
        };
        world.trigger(ApplyUsdOp {
            doc,
            op: UsdOp::SetAttribute {
                edit_target: LayerId::runtime(),
                path: format!("{}/Mission", prim.path),
                name: "info:sourceCode".to_string(),
                type_name: "string".to_string(),
                value: xml,
            },
        });
    });
}

/// Create the standard program child for a vessel that did not ship with one.
/// These are the same USD operations the waypoint tool uses, so the mission is
/// persisted, journaled and immediately projected back into the running rover.
fn create_mission(ctx: &mut PanelCtx, vessel: Entity) {
    let value = serde_json::json!({"kind":"sequence", "children":[]});
    let Ok(xml) = lunco_autopilot::btcpp_xml::value_to_xml(&value) else {
        return;
    };
    ctx.defer(move |world| {
        let Some(doc) = lunco_scene_commands::doc_resolve::resolve_doc_for_entity(world, vessel)
        else {
            bevy::log::warn!("[autopilot-canvas] selected vessel is not document-backed");
            return;
        };
        let Some(prim) = world.get::<UsdPrimPath>(vessel) else {
            return;
        };
        let mission = format!("{}/Mission", prim.path);
        world.trigger(ApplyUsdOps {
            doc,
            label: "Create autopilot program".to_string(),
            ops: vec![
                UsdOp::AddPrim {
                    edit_target: LayerId::runtime(),
                    parent_path: prim.path.clone(),
                    name: "Mission".to_string(),
                    type_name: Some("Scope".to_string()),
                    reference: None,
                },
                UsdOp::SetApiSchemas {
                    edit_target: LayerId::runtime(),
                    path: mission.clone(),
                    schemas: vec!["LunCoProgramAPI".to_string()],
                },
                UsdOp::SetAttribute {
                    edit_target: LayerId::runtime(),
                    path: mission,
                    name: "info:sourceCode".to_string(),
                    type_name: "string".to_string(),
                    value: xml,
                },
            ],
        });
    });
}

pub struct AutopilotCanvasPanel;

impl Panel for AutopilotCanvasPanel {
    fn id(&self) -> PanelId {
        PanelId("autopilot_canvas")
    }
    fn title(&self) -> String {
        "Autopilot graph".into()
    }
    fn menu_group(&self) -> lunco_workbench::PanelMenuGroup {
        lunco_workbench::PanelMenuGroup::Tools
    }
    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Center
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        ctx.resource_scope::<AutopilotCanvasState, ()>(|_ctx, state| {
            if state.selected.is_none() {
                ui.centered_and_justified(|ui| {
                    ui.label("Select a vessel to inspect its autopilot program.")
                });
                return;
            }
            if !state.built {
                ui.centered_and_justified(|ui| {
                    ui.vertical(|ui| {
                        ui.label("The selected vessel has no authored autopilot mission.");
                        if ui.button("Create autopilot program").clicked() {
                            if let Some(vessel) = state.selected {
                                create_mission(_ctx, vessel);
                            }
                        }
                    });
                });
                return;
            }
            // `""` is JSON Pointer's valid root. Do not filter it out: the
            // initial root Sequence must be able to receive its first child.
            // Start also has an empty origin, so distinguish the canvas-only
            // sentinel by label before exposing an edit target.
            let selected_path = state
                .canvas
                .selection
                .primary()
                .and_then(|selected| match selected {
                    lunco_canvas::SelectItem::Node(id) => state.canvas.scene.node(id),
                    lunco_canvas::SelectItem::Edge(_) => None,
                })
                .and_then(|node| (node.label != "Start").then(|| node.origin.clone()))
                .flatten();
            if let (Some(source), Some(path)) =
                (state.signature.as_deref(), selected_path.as_deref())
            {
                if let Some(mut fields) = drive_to_fields(source, path) {
                    ui.group(|ui| {
                        ui.label("Drive to target (scene metres)");
                        ui.horizontal(|ui| {
                            let mut changed = false;
                            changed |= ui
                                .add(egui::DragValue::new(&mut fields.target[0]).prefix("X "))
                                .changed();
                            changed |= ui
                                .add(egui::DragValue::new(&mut fields.target[1]).prefix("Y "))
                                .changed();
                            changed |= ui
                                .add(egui::DragValue::new(&mut fields.target[2]).prefix("Z "))
                                .changed();
                            changed |= ui
                                .add(egui::DragValue::new(&mut fields.speed).prefix("speed "))
                                .changed();
                            changed |= ui
                                .add(egui::DragValue::new(&mut fields.radius).prefix("arrival r "))
                                .changed();
                            if changed {
                                apply_drive_to_fields(
                                    _ctx,
                                    state.selected,
                                    state.signature.as_deref(),
                                    selected_path.as_deref(),
                                    fields,
                                );
                            }
                        });
                    });
                }
            }
            ui.horizontal(|ui| {
                if ui.button("Activate autopilot").clicked() {
                    if let (Some(vessel), Some(spec_json)) =
                        (state.selected, state.signature.clone())
                    {
                        _ctx.defer(move |world| {
                            // The autopilot writes the vessel's public control ports.
                            // That is the same boundary Modelica/GNSS wiring consumes;
                            // graph activation never bypasses or replaces co-sim.
                            world.trigger(lunco_autopilot::EngageAutopilot {
                                vessel,
                                index: 0,
                                throttle: 0.0,
                                spec_json,
                            });
                        });
                    }
                }
                ui.separator();
                ui.label("Select a composite, then use Add child or right-click the graph.");
                let enabled = selected_path.is_some();
                ui.menu_button("Add child…", |ui| {
                    for template in [
                        NodeTemplate::DriveTo,
                        NodeTemplate::Brake,
                        NodeTemplate::Wait,
                        NodeTemplate::Sequence,
                        NodeTemplate::Selector,
                        NodeTemplate::Parallel,
                        NodeTemplate::Retry,
                        NodeTemplate::Invert,
                    ] {
                        if ui
                            .add_enabled(enabled, egui::Button::new(template.label()))
                            .clicked()
                        {
                            apply_editor_action(
                                _ctx,
                                state.selected,
                                state.signature.as_deref(),
                                selected_path.as_deref(),
                                EditAction::Append(template),
                            );
                            ui.close();
                        }
                    }
                });
                if ui
                    .add_enabled(enabled, egui::Button::new("Add Drive to"))
                    .clicked()
                {
                    apply_editor_action(
                        _ctx,
                        state.selected,
                        state.signature.as_deref(),
                        selected_path.as_deref(),
                        EditAction::Append(NodeTemplate::DriveTo),
                    );
                }
                if ui
                    .add_enabled(enabled, egui::Button::new("Add Sequence"))
                    .clicked()
                {
                    apply_editor_action(
                        _ctx,
                        state.selected,
                        state.signature.as_deref(),
                        selected_path.as_deref(),
                        EditAction::Append(NodeTemplate::Sequence),
                    );
                }
                if ui
                    .add_enabled(enabled, egui::Button::new("Replace with brake"))
                    .clicked()
                {
                    apply_editor_action(
                        _ctx,
                        state.selected,
                        state.signature.as_deref(),
                        selected_path.as_deref(),
                        EditAction::ReplaceBrake,
                    );
                }
                if !enabled {
                    ui.weak("select a node");
                }
            });
            if state.needs_fit {
                if let Some(bounds) = state.canvas.scene.bounds() {
                    let size = ui.available_size();
                    let rect = Rect::from_min_max(
                        Pos::new(0.0, 0.0),
                        Pos::new(size.x.max(1.0), size.y.max(1.0)),
                    );
                    let (center, zoom) = state.canvas.viewport.fit_values(bounds, rect, 48.0);
                    state.canvas.viewport.snap_to(center, zoom);
                }
                state.needs_fit = false;
            }
            // Canvas layout is a projection, not authored mission state. Keep the
            // generic canvas tool read-only: selection/pan/zoom stay available,
            // while Start (and every source node) cannot be deleted or rewired
            // outside the typed mission operations above.
            state.canvas.read_only = true;
            let (response, _events) = state.canvas.ui(ui);
            let vessel = state.selected;
            let source = state.signature.clone();
            let selected_path = selected_path.clone();
            response.context_menu(|ui| {
                let Some(path) = selected_path.as_deref() else {
                    ui.label("Select a composite node first.");
                    return;
                };
                ui.label("Add child");
                for template in [
                    NodeTemplate::DriveTo,
                    NodeTemplate::Brake,
                    NodeTemplate::Wait,
                    NodeTemplate::Sequence,
                    NodeTemplate::Selector,
                    NodeTemplate::Parallel,
                    NodeTemplate::Retry,
                    NodeTemplate::Invert,
                ] {
                    if ui.button(template.label()).clicked() {
                        apply_editor_action(
                            _ctx,
                            vessel,
                            source.as_deref(),
                            Some(path),
                            EditAction::Append(template),
                        );
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button("Replace selected node with Brake").clicked() {
                    apply_editor_action(
                        _ctx,
                        vessel,
                        source.as_deref(),
                        Some(path),
                        EditAction::ReplaceBrake,
                    );
                    ui.close();
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_every_nested_root_to_nodes_and_edges() {
        let spec = BehaviorSpec::Sequence {
            children: vec![
                BehaviorSpec::ReactiveSelector {
                    children: vec![BehaviorSpec::Brake, BehaviorSpec::SteerClear { speed: 0.4 }],
                },
                BehaviorSpec::Retry {
                    times: 2,
                    child: Box::new(BehaviorSpec::DriveTo {
                        target: [1.0, 2.0, 3.0],
                        speed: 0.5,
                        radius: 2.0,
                    }),
                },
            ],
        };
        let scene = build_scene(&spec);
        // Start + the six authored nodes; its edge enters the authored root.
        assert_eq!(scene.node_count(), 7);
        assert_eq!(scene.edge_count(), 6);
    }
}
