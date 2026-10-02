//! Source gestures plan typed authoring; the USD journal owns admission and results.
use super::{UsdCanvasSessionState, UsdCanvasState, view_files::state_for};
use bevy::prelude::*;
use bevy_egui::egui;
use lunco_core::{
    ActiveCommandId, Command, CommandOutcome, CommandResults, on_command, register_commands,
};
use lunco_doc::diagram_view::{DiagramPosition, DiagramViewOp};
use lunco_doc::{Document, Mutation, OpId};
use lunco_hooks::{
    HookValue, RuntimeClock, RuntimeCycle, RuntimeExecutionContext, RuntimePhase, RuntimeRoute,
};
use lunco_usd_document::document::UsdOp;
use lunco_workbench_core::{PanelCtx, source::SourceDragPayload};

const DROP_HOOK: &str = "diagram.drop.plan";
lunco_hooks::declare_hook! {
    id: DROP_HOOK,
    owner: "lunco-luncosim-edit-ui",
    description: "Choose source attachment kind, USD parent and unique prim name from immutable diagram facts.",
    signature: [facts: Map],
    output: Map,
    deterministic: true,
    required: false,
    installable: true,
}

/// Add a referenced USD asset or source-backed program through the bound USD document.
#[Command(default)]
pub struct DropConnectionAsset {
    pub view_id: u64,
    pub source: String,
    /// Optional Models-palette contract; browser source drops have no inferred ports.
    pub program: Option<lunco_usd_core::program::ProgramAttachSpec>,
    pub target_path: Option<String>,
    pub x: f64,
    pub y: f64,
}

#[on_command(DropConnectionAsset)]
fn drop_asset(
    trigger: On<DropConnectionAsset>,
    mut views: ResMut<UsdCanvasState>,
    active: Res<ActiveCommandId>,
    mut results: ResMut<CommandResults>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    let id = active.get().unwrap_or_else(|| OpId::new().0);
    let planned = (|| -> Result<_, String> {
        let state = state_for(&mut views, cmd.view_id)?;
        let doc = state
            .doc
            .ok_or("Choose Edit connections before adding source assets.")?;
        let edit_target = state
            .edit_target
            .clone()
            .ok_or("No writable USD edit target")?;
        if !cmd.x.is_finite()
            || !cmd.y.is_finite()
            || cmd.x.abs() > f64::from(f32::MAX)
            || cmd.y.abs() > f64::from(f32::MAX)
        {
            return Err("Drop position exceeds the canvas rendering range".into());
        }
        if cmd.source.trim().is_empty() {
            return Err("Drop source is empty".into());
        }
        let target = cmd.target_path.as_deref().unwrap_or("");
        if !target.is_empty() && !state.source_nodes.iter().any(|node| node.path == target) {
            return Err("Drop target is absent from the composed USD source".into());
        }
        let facts = HookValue::map([
            ("source", HookValue::str(&cmd.source)),
            ("scope", HookValue::str(&state.diagram_root)),
            ("target", HookValue::str(target)),
            (
                "paths",
                HookValue::Array(
                    state
                        .source_nodes
                        .iter()
                        .map(|node| HookValue::str(&node.path))
                        .collect(),
                ),
            ),
        ]);
        let context = RuntimeExecutionContext {
            route: Some(RuntimeRoute::application(RuntimeCycle::Visualization)),
            phase: RuntimePhase::Preparation,
            clock: RuntimeClock::None,
            time_seconds: None,
            delta_seconds: None,
            sequence: None,
            producer: None,
        };
        let plan = lunco_hooks::invoke_with_context(DROP_HOOK, &[facts], context)
            .ok_or("Diagram drop policy is unavailable")?
            .map_err(|error| error.to_string())?;
        if let Some(error) = plan.get("error").and_then(HookValue::as_str) {
            return Err(error.into());
        }
        let parent = plan
            .get("parent")
            .and_then(HookValue::as_str)
            .ok_or("Drop plan lacks parent")?;
        let name = plan
            .get("name")
            .and_then(HookValue::as_str)
            .ok_or("Drop plan lacks name")?;
        if parent != state.diagram_root && parent != target {
            return Err("Drop policy selected an unrelated parent".into());
        }
        let path = format!("{}/{}", parent.trim_end_matches('/'), name);
        if state.source_nodes.iter().any(|node| node.path == path) {
            return Err("Drop policy selected an existing prim".into());
        }
        let ops = match plan.get("kind").and_then(HookValue::as_str) {
            Some("reference") => vec![UsdOp::AddPrim {
                edit_target,
                parent_path: parent.into(),
                name: name.into(),
                type_name: None,
                reference: Some(cmd.source.clone()),
                reference_prim_path: None,
            }],
            Some("program") if !target.is_empty() && parent == target => {
                let mut spec =
                    cmd.program
                        .clone()
                        .unwrap_or(lunco_usd_core::program::ProgramAttachSpec {
                            edit_target,
                            host_path: parent.into(),
                            name: name.into(),
                            source_asset: cmd.source.clone(),
                            inputs: Vec::new(),
                            outputs: Vec::new(),
                            realtime_safe: false,
                        });
                if spec.source_asset != cmd.source {
                    return Err("Program contract does not match the dropped source".into());
                }
                spec.edit_target = state
                    .edit_target
                    .clone()
                    .ok_or("No writable USD edit target")?;
                spec.host_path = parent.into();
                spec.name = name.into();
                for input in &mut spec.inputs {
                    if let Some(connection) = &mut input.connection {
                        *connection = connection.replace("{host}", parent);
                    }
                }
                for output in &mut spec.outputs {
                    for connection in &mut output.connections {
                        *connection = connection.replace("{host}", parent);
                    }
                }
                lunco_usd_core::program::program_attach_ops(&spec)?
            }
            _ => return Err("Drop policy returned an invalid attachment kind or target".into()),
        };
        state.last_error = None;
        Ok((
            lunco_usd_core::commands::ApplyUsdOps {
                doc_id: doc,
                parent_gen: Some(state.generation),
                label: "Drop source into USD diagram".into(),
                ops,
            },
            path,
            state.selected_view.clone(),
        ))
    })();
    let (apply, path, view) = match planned {
        Ok(plan) => plan,
        Err(error) => {
            warn!("[connection-drop] {error}");
            if let Ok(state) = state_for(&mut views, cmd.view_id) {
                state.last_error = Some(error.clone());
            }
            results.record(id, Err(error));
            return;
        }
    };
    results.insert(id, CommandOutcome::Pending);
    let mut forwarded = active.clone();
    if forwarded.get().is_none() {
        forwarded.set(Some(id));
    }
    let view_id = cmd.view_id;
    let position = DiagramPosition { x: cmd.x, y: cmd.y };
    let doc = apply.doc_id;
    commands.queue(move |world: &mut World| {
        // The deferred USD owner captures this command identity before applying its journal batch.
        let previous = world.remove_resource::<ActiveCommandId>();
        world.insert_resource(forwarded);
        world.trigger(apply);
        world.remove_resource::<ActiveCommandId>();
        if let Some(previous) = previous {
            world.insert_resource(previous);
        }
        world.commands().queue(move |world: &mut World| {
            if !matches!(
                world.resource::<CommandResults>().get(id),
                Some(CommandOutcome::Succeeded(_))
            ) {
                return;
            }
            let mut views = world.resource_mut::<UsdCanvasState>();
            let Ok(state) = state_for(&mut views, view_id) else {
                return;
            };
            if state.doc != Some(doc) {
                return;
            }
            let Some(host) = state.view_document.as_mut() else {
                return;
            };
            if let Err(error) = host.apply(Mutation::local(DiagramViewOp::SetPosition {
                view,
                path,
                position: Some(position),
            })) {
                warn!("[connection-drop] layout placement rejected: {error}");
                state.last_error = Some(error.to_string());
            }
        });
    });
}
register_commands!(drop_asset);
pub(super) fn init(app: &mut App) {
    register_all_commands(app);
}

pub(super) fn drop_ui(
    ui: &mut egui::Ui,
    response: &egui::Response,
    state: &mut UsdCanvasSessionState,
    ctx: &mut PanelCtx,
) {
    if response.contains_pointer() && ui.input(|input| input.raw.dropped_files.len() > 1) {
        let error = "Drop one file at a time into Connections";
        warn!("[connection-drop] {error}");
        state.last_error = Some(error.into());
        return;
    }
    let hovered = response.dnd_hover_payload::<SourceDragPayload>();
    if hovered.is_some()
        || response
            .dnd_hover_payload::<lunco_usd_core::program::ProgramAttachSpec>()
            .is_some()
    {
        let target = ui
            .input(|input| input.pointer.latest_pos())
            .zip(state.canvas_rect)
            .and_then(|(pointer, rect)| {
                let position = state
                    .canvas
                    .viewport
                    .screen_to_world(lunco_canvas::Pos::new(pointer.x, pointer.y), rect);
                state
                    .canvas
                    .scene
                    .hit_node(position, 0.0)
                    .and_then(|(id, _)| state.canvas.scene.node(id))
            });
        if let (Some(node), Some(rect)) = (target, state.canvas_rect) {
            let bounds = state.canvas.viewport.world_rect_to_screen(node.rect, rect);
            let target_rect = egui::Rect::from_min_max(
                egui::pos2(bounds.min.x, bounds.min.y),
                egui::pos2(bounds.max.x, bounds.max.y),
            );
            ui.painter().rect_stroke(
                target_rect,
                ui.visuals().widgets.active.corner_radius,
                egui::Stroke::new(
                    2.0,
                    lunco_theme::active(ui.ctx()).tokens.node_border_selected,
                ),
                egui::StrokeKind::Outside,
            );
        }
        response.clone().on_hover_text(if state.doc.is_some() {
            format!("Target: {}\nDrop USD to add a reference; drop Modelica or Rhai onto its owning prim.", target.and_then(|node| node.origin.as_deref()).unwrap_or(&state.diagram_root))
        } else {
            "Scene browsing: choose Edit connections before adding assets.".into()
        });
    }
    // Egui consumes the payload before attempting its downcast: inspect its type first.
    let program = if response
        .dnd_hover_payload::<lunco_usd_core::program::ProgramAttachSpec>()
        .is_some()
    {
        response
            .dnd_release_payload::<lunco_usd_core::program::ProgramAttachSpec>()
            .map(|payload| (*payload).clone())
    } else {
        None
    };
    let source = program
        .as_ref()
        .map(|spec| spec.source_asset.clone())
        .or_else(|| {
            if hovered.is_some() {
                response
                    .dnd_release_payload::<SourceDragPayload>()
                    .map(|payload| payload.source.clone())
            } else {
                None
            }
        })
        .or_else(|| {
            if response.contains_pointer() {
                ui.input(|input| {
                    input
                        .raw
                        .dropped_files
                        .first()
                        .and_then(|file| file.path.as_ref())
                        .map(|path| path.to_string_lossy().replace('\\', "/"))
                })
            } else {
                None
            }
        });
    let Some(source) = source else {
        return;
    };
    let Some(pointer) = ui.input(|input| input.pointer.latest_pos()) else {
        return;
    };
    let Some(rect) = state.canvas_rect else {
        return;
    };
    let position = state
        .canvas
        .viewport
        .screen_to_world(lunco_canvas::Pos::new(pointer.x, pointer.y), rect);
    if state
        .canvas
        .scene
        .hit_node(position, 0.0)
        .and_then(|(id, _)| state.canvas.scene.node(id))
        .and_then(|node| node.data.downcast_ref::<super::UsdPrimNodeData>())
        .is_some_and(|data| data.group_id.is_some())
    {
        state.last_error = Some("Expand the group and drop onto an exact USD prim".into());
        return;
    }
    let target_path = state
        .canvas
        .scene
        .hit_node(position, 0.0)
        .and_then(|(id, _)| state.canvas.scene.node(id))
        .and_then(|node| node.origin.clone());
    let Some(host) = state.view_document.as_ref() else {
        return;
    };
    ctx.trigger(DropConnectionAsset {
        view_id: host.document().id().raw(),
        source,
        program,
        target_path,
        x: f64::from(position.x),
        y: f64::from(position.y),
    });
}
