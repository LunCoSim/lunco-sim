//! Exact binding edits lower through the shared USD validator and journal.
use super::{UsdCanvasState, projection, view_files::state_for, workspace::full_scene};
use bevy::prelude::*;
use lunco_canvas::{PortRef, Scene, SceneEvent};
use lunco_core::{ActiveCommandId, Command, CommandOutcome, on_command, register_commands};
use lunco_usd_document::document::{LayerId, UsdOp};

/// Exact presentation key and authored property, including boundary terminals.
#[derive(Clone, Debug, PartialEq, Eq, Reflect, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingEndpoint {
    /// Exact node key returned by InspectConnectionDiagram, including boundary roles.
    pub key: String,
    /// Authored USD property name, such as inputs:feedback.
    pub port: String,
}

/// The exact oriented pair returned for an existing diagram connection.
#[derive(Clone, Debug, Reflect, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingPair {
    /// Original connection's source endpoint.
    pub from: BindingEndpoint,
    /// Original connection's target endpoint.
    pub to: BindingEndpoint,
}

impl BindingEndpoint {
    pub(super) fn capture(scene: &Scene, endpoint: &PortRef) -> Result<Self, String> {
        let (node, port) = super::groups::endpoint(scene, endpoint)?;
        Ok(Self {
            key: projection::diagram_key(node)
                .ok_or("Binding endpoint has no view identity")?
                .into(),
            port: port.into(),
        })
    }

    pub(super) fn resolve(&self, scene: &Scene) -> Result<PortRef, String> {
        let (id, node) = scene
            .nodes()
            .find(|(_, node)| projection::diagram_key(node) == Some(self.key.as_str()))
            .ok_or_else(|| format!("Binding card `{}` is absent from this view", self.key))?;
        let port = node
            .ports
            .iter()
            .find(|port| port.id.as_str() == self.port)
            .ok_or_else(|| format!("Binding port `{}` is absent from `{}`", self.port, self.key))?;
        Ok(PortRef {
            node: *id,
            port: port.id.clone(),
        })
    }
}

impl BindingPair {
    pub(super) fn capture(scene: &Scene, from: &PortRef, to: &PortRef) -> Result<Self, String> {
        Ok(Self {
            from: BindingEndpoint::capture(scene, from)?,
            to: BindingEndpoint::capture(scene, to)?,
        })
    }
}

/// Add a binding or replace one exact existing binding in one USD undo transaction.
#[Command]
pub struct ApplyConnectionBinding {
    /// Exact view identity returned by InspectConnectionDiagram.
    pub view_id: u64,
    /// Source generation returned by InspectConnectionDiagram; stale drafts reject.
    pub parent_gen: u64,
    /// First selected endpoint; the USD validator resolves causal direction.
    pub from: BindingEndpoint,
    /// Second selected endpoint.
    pub to: BindingEndpoint,
    /// Existing binding to remove atomically before installing the new endpoints.
    pub previous: Option<BindingPair>,
    /// Explicitly replace every other source on the new sink.
    #[serde(default)]
    pub replace_sources: bool,
}

fn binding_ops(
    scene: &Scene,
    request: &ApplyConnectionBinding,
    target: &LayerId,
) -> Result<Vec<UsdOp>, String> {
    let from = request.from.resolve(scene)?;
    let to = request.to.resolve(scene)?;
    let mut connection = super::connect_op(scene, &from, &to, target)?;
    let mut ops = if let Some(previous) = &request.previous {
        let old_from = previous.from.resolve(scene)?;
        let old_to = previous.to.resolve(scene)?;
        let (id, _) = scene
            .edges()
            .find(|(_, edge)| edge.from == old_from && edge.to == old_to)
            .ok_or("The original binding is no longer present")?;
        let sink = super::edge_sink(scene, *id)
            .ok_or("The original binding has no editable USD property")?;
        super::build_ops(
            scene,
            &Default::default(),
            &[(*id, sink)].into(),
            &[SceneEvent::EdgeDeleted { id: *id }],
            target,
        )?
    } else {
        Vec::new()
    };
    let (source, _) = super::connection_endpoints(scene, &from, &to)?;
    let (source_node, source_port) = super::groups::endpoint(scene, source)?;
    let source = format!(
        "{}.{source_port}",
        source_node
            .origin
            .as_deref()
            .ok_or("Source card has no USD origin")?
    );
    if let UsdOp::SetConnection {
        path,
        name,
        sources,
        ..
    } = &mut connection
    {
        // A source change on the same sink starts from the deletion result,
        // rather than the pre-edit snapshot that still includes the old source.
        for op in &ops {
            if let UsdOp::SetConnection {
                path: old_path,
                name: old_name,
                sources: remaining,
                ..
            } = op
            {
                if old_path == path && old_name == name {
                    sources.clone_from(remaining);
                }
            }
        }
        if request.replace_sources {
            sources.clear();
        }
        if !sources.contains(&source) {
            sources.push(source);
        }
    }
    ops.push(connection);
    Ok(ops)
}

#[on_command(ApplyConnectionBinding)]
fn apply_binding(
    trigger: On<ApplyConnectionBinding>,
    mut views: ResMut<UsdCanvasState>,
    active: Res<ActiveCommandId>,
    pending: Option<Res<lunco_api::executor::PendingApiRequest>>,
    mut commands: Commands,
) -> Result<CommandOutcome, String> {
    let request = trigger.event();
    let state = state_for(&mut views, request.view_id)?;
    let planned: Result<_, String> = (|| {
        let doc_id = state
            .doc
            .ok_or("Choose Edit connections before changing a binding")?;
        if state.generation != request.parent_gen {
            return Err(
                "The USD source changed; inspect the current binding before applying this draft"
                    .into(),
            );
        }
        let target = state
            .edit_target
            .as_ref()
            .ok_or("No writable USD edit target")?;
        Ok(lunco_usd_core::commands::ApplyUsdOps {
            doc_id,
            parent_gen: Some(request.parent_gen),
            label: if request.previous.is_some() {
                "Edit USD binding"
            } else {
                "Connect USD ports"
            }
            .into(),
            ops: binding_ops(full_scene(state), request, target)?,
        })
    })();
    let apply = match planned {
        Ok(apply) => {
            state.last_error = None;
            apply
        }
        Err(error) => {
            state.last_error = Some(error.clone());
            return Err(error);
        }
    };
    let active = active.as_ref().clone();
    let correlation = pending
        .map(|pending| pending.correlation_id)
        .filter(|id| *id != 0);
    commands.queue(move |world: &mut World| {
        lunco_api::executor::trigger_command_with_context(world, apply, active, correlation);
    });
    Ok(CommandOutcome::Pending)
}

register_commands!(apply_binding);
pub(super) fn init(app: &mut App) {
    register_all_commands(app);
}
