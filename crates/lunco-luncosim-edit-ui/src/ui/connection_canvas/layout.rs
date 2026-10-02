//! Bounded, revision-fenced presentation policy over immutable canvas facts.
use super::{UsdCanvasSessionState, UsdCanvasState};
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};
use lunco_doc::Document;
use lunco_hooks::{
    HookValue, RuntimeClock, RuntimeCycle, RuntimeExecutionContext, RuntimePhase, RuntimeRoute,
};
use std::collections::{BTreeMap, HashMap};

const LAYOUT_HOOK: &str = "diagram.layout";
lunco_hooks::declare_hook! {
    id: LAYOUT_HOOK,
    owner: "lunco-luncosim-edit-ui",
    description: "Place USD diagram nodes using immutable path, size, causal rank and connection facts.",
    signature: [facts: Map],
    output: ArrayOfMap,
    deterministic: true,
    required: false,
    installable: true,
}

struct Placement {
    x: f64,
    y: f64,
    label: Option<String>,
    accent: Option<super::projection::DiagramAccent>,
}
type Placements = BTreeMap<String, Placement>;
#[derive(Resource, Default)]
pub struct LayoutJobs {
    jobs: HashMap<u64, (u64, Task<Result<Placements, String>>)>,
    hook_generation: u64,
}

pub(super) fn request(state: &mut UsdCanvasSessionState) {
    state.layout_revision = state.layout_revision.wrapping_add(1);
    let nodes = state
        .canvas
        .scene
        .nodes()
        .map(|(_, node)| {
            HookValue::map([
                (
                    "path",
                    HookValue::str(super::projection::diagram_key(node).unwrap_or_default()),
                ),
                (
                    "role",
                    HookValue::str(
                        node.data
                            .downcast_ref::<super::UsdPrimNodeData>()
                            .and_then(|data| data.boundary)
                            .map(|role| role.name())
                            .unwrap_or("prim"),
                    ),
                ),
                ("width", HookValue::Float(f64::from(node.rect.width()))),
                (
                    "programs",
                    HookValue::Array(
                        node.data
                            .downcast_ref::<super::UsdPrimNodeData>()
                            .map(|data| {
                                data.programs
                                    .iter()
                                    .map(|program| HookValue::str(&program.backend))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    ),
                ),
                ("height", HookValue::Float(f64::from(node.rect.height()))),
                (
                    "rank",
                    HookValue::Int(
                        ((node.rect.min.x - super::projection::MARGIN)
                            / super::projection::COL_SPACING)
                            .round() as i64,
                    ),
                ),
            ])
        })
        .collect();
    let edges = state
        .canvas
        .scene
        .edges()
        .filter_map(|(_, edge)| {
            let source = super::projection::diagram_key(state.canvas.scene.node(edge.from.node)?)?;
            let target = super::projection::diagram_key(state.canvas.scene.node(edge.to.node)?)?;
            let kind = edge.data.downcast_ref::<super::UsdWireData>()?.kind;
            Some(HookValue::map([
                ("source", HookValue::str(source)),
                ("target", HookValue::str(target)),
                ("kind", HookValue::str(format!("{kind:?}"))),
            ]))
        })
        .collect();
    state.layout_request = Some(HookValue::map([
        ("nodes", HookValue::Array(nodes)),
        ("edges", HookValue::Array(edges)),
        ("scope", HookValue::str(&state.diagram_root)),
    ]));
}

fn evaluate(facts: HookValue) -> Result<Placements, String> {
    let context = RuntimeExecutionContext {
        route: Some(RuntimeRoute::application(RuntimeCycle::Visualization)),
        phase: RuntimePhase::Preparation,
        clock: RuntimeClock::None,
        time_seconds: None,
        delta_seconds: None,
        sequence: None,
        producer: None,
    };
    let result = lunco_hooks::invoke_with_context(LAYOUT_HOOK, &[facts.clone()], context)
        .ok_or("Diagram layout policy is unavailable")?
        .map_err(|error| error.to_string())?;
    let HookValue::Array(nodes) = facts.get("nodes").ok_or("Layout facts lack nodes")? else {
        return Err("Layout facts nodes must be an array".into());
    };
    let expected: std::collections::BTreeSet<_> = nodes
        .iter()
        .filter_map(|node| node.get("path").and_then(HookValue::as_str))
        .collect();
    let HookValue::Array(entries) = result else {
        return Err("Layout policy must return placements".into());
    };
    let mut placements = BTreeMap::new();
    for entry in entries {
        let path = entry
            .get("path")
            .and_then(HookValue::as_str)
            .ok_or("Placement lacks a USD path")?;
        let x = entry
            .get("x")
            .and_then(HookValue::as_f64)
            .ok_or("Placement lacks x")?;
        let y = entry
            .get("y")
            .and_then(HookValue::as_f64)
            .ok_or("Placement lacks y")?;
        let label = entry
            .get("label")
            .map(|value| {
                let label = value.as_str().ok_or("Placement label must be text")?;
                if label.trim().is_empty() || label.len() > 256 {
                    return Err("Placement label is empty or too long");
                }
                Ok(label.to_string())
            })
            .transpose()?;
        let accent = entry
            .get("accent")
            .map(|value| {
                use super::projection::DiagramAccent;
                match value.as_str() {
                    Some("model") => Ok(DiagramAccent::Model),
                    Some("block") => Ok(DiagramAccent::Block),
                    Some("record") => Ok(DiagramAccent::Record),
                    Some("package") => Ok(DiagramAccent::Package),
                    Some("class") => Ok(DiagramAccent::Class),
                    Some("warning") => Ok(DiagramAccent::Warning),
                    _ => Err("Placement accent is not a schematic theme role"),
                }
            })
            .transpose()?;
        if !expected.contains(path)
            || !x.is_finite()
            || !y.is_finite()
            || x.abs() > f64::from(f32::MAX)
            || y.abs() > f64::from(f32::MAX)
            || placements
                .insert(
                    path.into(),
                    Placement {
                        x,
                        y,
                        label,
                        accent,
                    },
                )
                .is_some()
        {
            return Err("Layout contains an unknown/duplicate path or invalid coordinates".into());
        }
    }
    if placements.len() != expected.len() {
        return Err("Layout policy omitted source nodes".into());
    }
    Ok(placements)
}

pub fn update_layouts(
    mut views: ResMut<UsdCanvasState>,
    mut pending: ResMut<LayoutJobs>,
    mut status: Option<ResMut<lunco_status_core::status_bus::StatusBus>>,
) {
    let views = &mut *views;
    let hook_generation = lunco_hooks::generation();
    let changed_policy = pending.hook_generation != hook_generation;
    pending.hook_generation = hook_generation;
    let mut live = std::collections::HashSet::new();
    for state in std::iter::once(&mut views.scene).chain(views.sessions.values_mut()) {
        let Some(id) = state
            .view_document
            .as_ref()
            .map(|host| host.document().id().raw())
        else {
            continue;
        };
        live.insert(id);
        if changed_policy && state.built {
            state.rebuild_view();
        }
        let completed = pending.jobs.get_mut(&id).and_then(|(revision, task)| {
            future::block_on(future::poll_once(task)).map(|result| (*revision, result))
        });
        if let Some((revision, result)) = completed {
            pending.jobs.remove(&id);
            if state.layout_revision == revision {
                match result {
                    Ok(placements) => {
                        let ids: Vec<_> = state.canvas.scene.nodes().map(|(id, _)| *id).collect();
                        for id in ids {
                            if let Some(node) = state.canvas.scene.node_mut(id) {
                                if let Some(placement) = super::projection::diagram_key(node)
                                    .and_then(|path| placements.get(path))
                                {
                                    // Validated explicit narrowing at the canvas rendering boundary.
                                    node.rect = lunco_canvas::Rect::from_min_size(
                                        lunco_canvas::Pos::new(
                                            placement.x as f32,
                                            placement.y as f32,
                                        ),
                                        node.rect.width(),
                                        node.rect.height(),
                                    );
                                    if let Some(label) = &placement.label {
                                        node.label.clone_from(label);
                                    }
                                    if let Some(accent) = placement.accent {
                                        if let Some(data) =
                                            node.data.downcast_ref::<super::UsdPrimNodeData>()
                                        {
                                            let mut data = data.clone();
                                            data.accent = Some(accent);
                                            node.data = std::sync::Arc::new(data);
                                        }
                                    }
                                }
                            }
                        }
                        state.restore_placements();
                        state.needs_fit = true;
                    }
                    Err(error) => {
                        warn!("[diagram-layout] {error}");
                        state.last_error = Some(error);
                    }
                }
            }
        }
        if !pending.jobs.contains_key(&id) {
            if let Some(facts) = state.layout_request.take() {
                pending.jobs.insert(
                    id,
                    (
                        state.layout_revision,
                        AsyncComputeTaskPool::get().spawn(async move { evaluate(facts) }),
                    ),
                );
            }
        }
    }
    pending.jobs.retain(|id, _| live.contains(id));
    for state in std::iter::once(&mut views.scene).chain(views.sessions.values_mut()) {
        if state.published_layout_revision == state.layout_revision
            && state.published_error == state.last_error
        {
            continue;
        }
        state.published_layout_revision = state.layout_revision;
        state.published_error = state.last_error.clone();
        let mut diagnostic = state.last_error.clone().unwrap_or_default();
        if !state.unresolved_links.is_empty() {
            diagnostic.push_str(&format!(
                "\n{} USD links could not be displayed:\n{}",
                state.unresolved_links.len(),
                state.unresolved_links.join("\n")
            ));
        }
        if diagnostic != state.published_diagnostic {
            if !diagnostic.is_empty() {
                if let Some(status) = status.as_mut() {
                    status.push(
                        "Connections",
                        lunco_status_core::status_bus::StatusLevel::Warn,
                        &diagnostic,
                    );
                }
                warn!("[connections] {diagnostic}");
            }
            state.published_diagnostic = diagnostic;
        }
    }
}
