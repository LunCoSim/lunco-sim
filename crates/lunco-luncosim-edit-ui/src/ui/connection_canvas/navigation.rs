//! Policy-driven navigation reuses USD scope and existing source document owners.
use super::{UsdCanvasState, projection::UsdPrimNodeData, view_files::state_for};
use bevy::prelude::*;
use lunco_core::{Command, on_command, register_commands};
use lunco_doc::{Ack, OpId};
use lunco_hooks::{
    HookValue, RuntimeClock, RuntimeCycle, RuntimeExecutionContext, RuntimePhase, RuntimeRoute,
};

const OPEN_HOOK: &str = "diagram.open.plan";
/// Pending framing resolves against the current graph at the rendering boundary.
pub(super) enum FrameTarget {
    System,
    Node(String),
}

/// Frame the complete Connections system, or center a card at its natural scale.
#[Command(default)]
pub struct FrameConnectionDiagram {
    pub view_id: u64,
    /// Exact current view key; omitted fits the complete system.
    pub key: Option<String>,
}

#[on_command(FrameConnectionDiagram)]
fn frame_diagram(
    trigger: On<FrameConnectionDiagram>,
    mut views: ResMut<UsdCanvasState>,
) -> Result<Ack, String> {
    let cmd = trigger.event();
    let state = state_for(&mut views, cmd.view_id)?;
    if let Some(key) = &cmd.key {
        if !state
            .canvas
            .scene
            .nodes()
            .any(|(_, node)| super::projection::diagram_key(node) == Some(key.as_str()))
        {
            let error = "Diagram card is absent from the current view";
            state.last_error = Some(error.into());
            return Err(error.into());
        }
        state.frame_request = Some(FrameTarget::Node(key.clone()));
    } else {
        state.frame_request = Some(FrameTarget::System);
    }
    state.last_error = None;
    Ok(Ack::new(OpId::new()))
}
lunco_hooks::declare_hook! {
    id: OPEN_HOOK,
    owner: "lunco-luncosim-edit-ui",
    description: "Choose USD scope or attached source navigation from immutable diagram facts.",
    signature: [facts: Map], output: Map, deterministic: true, required: false, installable: true,
}

/// Open a diagram card's USD internals or attached source according to authored policy.
#[Command(default)]
pub struct OpenConnectionNode {
    pub view_id: u64,
    /// Stable node key returned by InspectConnectionDiagram.
    pub key: String,
    /// Exact attached program prim to open; omitted opens the card's topology or own model.
    pub program_path: Option<String>,
}

#[on_command(OpenConnectionNode)]
fn open_node(
    trigger: On<OpenConnectionNode>,
    mut views: ResMut<UsdCanvasState>,
    mut commands: Commands,
) -> Result<Ack, String> {
    let cmd = trigger.event();
    let state = state_for(&mut views, cmd.view_id)?;
    let result = (|| -> Result<Ack, String> {
        let node = state
            .canvas
            .scene
            .nodes()
            .find(|(_, node)| super::projection::diagram_key(node) == Some(cmd.key.as_str()))
            .map(|(_, node)| node)
            .ok_or("Diagram card is absent from the current view")?;
        let path = node
            .origin
            .clone()
            .ok_or("Diagram card has no USD origin")?;
        let data = node
            .data
            .downcast_ref::<UsdPrimNodeData>()
            .ok_or("Diagram card has no typed USD facts")?;
        let selected_path = cmd.program_path.as_deref().unwrap_or(&path);
        if cmd.program_path.is_some()
            && !data
                .programs
                .iter()
                .any(|program| program.path == selected_path)
        {
            return Err("Selected program is absent from the diagram card".into());
        }
        let programs: Vec<_> = data
            .programs
            .iter()
            .filter(|program| program.path == selected_path)
            .cloned()
            .collect();
        let has_children = state.source_nodes.iter().any(|node| {
            node.path
                .strip_prefix(&path)
                .is_some_and(|tail| tail.starts_with('/'))
        });
        let facts = HookValue::map([
            (
                "intent",
                HookValue::str(if cmd.program_path.is_some() {
                    "program"
                } else {
                    "node"
                }),
            ),
            ("path", HookValue::str(&path)),
            ("has_children", HookValue::Bool(has_children)),
            (
                "programs",
                HookValue::Array(
                    programs
                        .iter()
                        .map(|program| {
                            HookValue::map([
                                ("source", HookValue::str(&program.source)),
                                ("backend", HookValue::str(&program.backend)),
                                ("valid", HookValue::Bool(program.issue.is_none())),
                            ])
                        })
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
        let plan = lunco_hooks::invoke_with_context(OPEN_HOOK, &[facts], context)
            .ok_or("Diagram navigation policy is unavailable")?
            .map_err(|error| error.to_string())?;
        if let Some(error) = plan.get("error").and_then(HookValue::as_str) {
            return Err(error.into());
        }
        match plan.get("action").and_then(HookValue::as_str) {
            Some("scope") => {
                if path != state.diagram_root {
                    state.navigation_history.push(state.diagram_root.clone());
                    state.diagram_root = path;
                    state.rebuild_view();
                }
            }
            Some(action @ ("modelica" | "source")) => {
                let source = plan
                    .get("source")
                    .and_then(HookValue::as_str)
                    .ok_or("Navigation plan lacks source")?;
                let program = programs
                    .iter()
                    .find(|program| program.source == source && program.issue.is_none())
                    .ok_or("Navigation plan selected an unavailable attached source")?;
                if action == "modelica" {
                    if program.backend != "Modelica" {
                        return Err(
                            "Navigation plan selected a non-Modelica program for its schema editor"
                                .into(),
                        );
                    }
                    commands.trigger(lunco_doc_bevy::OpenFile {
                        path: source.into(),
                    });
                } else {
                    commands.trigger(lunco_workbench_core::source::OpenSourceView {
                        asset_path: source.into(),
                    });
                }
            }
            _ => return Err("Navigation policy returned an invalid action".into()),
        }
        Ok(Ack::new(OpId::new()))
    })();
    match &result {
        Ok(_) => state.last_error = None,
        Err(error) => {
            warn!("[diagram-open] {error}");
            state.last_error = Some(error.clone());
        }
    }
    result
}
register_commands!(open_node, frame_diagram);
pub(super) fn init(app: &mut App) {
    register_all_commands(app);
}
