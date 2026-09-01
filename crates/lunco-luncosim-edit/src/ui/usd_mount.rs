//! USD **mount** view-model — the retrofit *snap* surface (doc 48 §3.1).
//!
//! For the selected host body, harvest the sockets it advertises
//! ([`lunco_usd_bevy::mount::read_sockets`]) and, for each socket that names an
//! already-attached part, read the part's plug frame and pre-compute the placement
//! that makes the plug coincide with the socket ([`resolve_mount_placement`]). The
//! Inspector's `mount_section` then renders one row per socket with a **Snap**
//! button that re-authors the part's transform + joint anchor through
//! [`realign_component_ops`] — so "move the socket, the part and its joint follow"
//! without a rebuild.
//!
//! The producer runs on the main thread (the composed stage is `!Send`) and stores
//! only render-ready data; it is never authoritative — the stage is.
//!
//! [`resolve_mount_placement`]: lunco_usd::attach::resolve_mount_placement
//! [`realign_component_ops`]: lunco_usd::attach::realign_component_ops

use std::collections::HashMap;

use bevy::prelude::*;
use lunco_usd::attach::resolve_mount_placement;
use lunco_usd::ui::viewport::{UsdPreviewId, UsdViewportState};
use lunco_usd_bevy::mount::{read_attachment_joint, read_plug, read_sockets, MountDiagnostic};
use lunco_usd_bevy::{CanonicalStages, SdfPath, UsdPrimPath, UsdStageAsset};

/// Position and orientation tolerance for the editor's no-op Snap state.
const MOUNT_ALIGNMENT_TOLERANCE: f32 = 1.0e-3;

/// One socket row, with the snap already resolved when a part is present.
#[derive(Clone)]
pub struct MountItem {
    /// Socket leaf name (`wheel_fl`).
    pub socket: String,
    /// Absolute socket path. The attach command must retain the path, not only
    /// the display name, so it can author the occupancy relationship correctly.
    pub socket_path: String,
    /// What plug kind it accepts.
    pub accepts: String,
    /// Joint kind token (`fixed` / `revolute` / `prismatic`).
    pub joint: String,
    /// Joint axis token, when the joint needs one.
    pub axis: Option<String>,
    /// The part the socket holds (absolute prim path), or `None` — an EMPTY socket
    /// (offer a new-attach instead of a snap).
    pub part_path: Option<String>,
    /// The part's leaf name, for the button label.
    pub part_leaf: Option<String>,
    /// The exact joint prim path recorded by the component, if the attachment
    /// metadata is valid. No path is derived from the component name.
    pub joint_path: Option<String>,
    /// For an EMPTY socket: the component asset it's designed to hold
    /// (`lunco:mount:asset`, raw path). `Some` here + `part_path == None` → the row
    /// offers "⊕ Attach", which references the asset in and snaps its plug to the
    /// socket via `from_mount`. `None` → nothing to attach.
    pub attach_asset: Option<String>,
    /// The socket frame in the host body's local space — needed to compute the
    /// new-attach placement (`from_mount(socket, plug)`) at click time, when the
    /// asset's plug frame is finally read.
    pub socket_frame: Transform,
    /// Resolved placement (host-local) so the plug meets the socket. `None` when no
    /// part / no plug frame — the row is informational only.
    pub placement: Option<[f64; 3]>,
    /// Resolved `rotateXYZ` degrees for the same. `None` alongside `placement`.
    pub rotate_deg: Option<[f64; 3]>,
    /// Whether the part is already essentially at the resolved placement — the
    /// Snap is a no-op (button disabled, "aligned" hint).
    pub aligned: bool,
}

/// Render-ready mount rows for each preview session's selected host. Derived,
/// never authoritative.
#[derive(Clone, Default)]
pub struct UsdMountSessionView {
    pub preview: UsdPreviewId,
    pub doc: lunco_doc::DocumentId,
    pub edit_target: lunco_usd::document::LayerId,
    pub generation: u64,
    pub entity: Option<Entity>,
    pub host_path: String,
    pub items: Vec<MountItem>,
    pub diagnostics: Vec<MountDiagnostic>,
}

/// Session-keyed mount views. Socket facts and resolved frame data belong to
/// the composed stage of their preview lease, not to whichever lease was most
/// recently focused.
#[derive(Resource, Default)]
pub struct UsdMountView {
    sessions: HashMap<UsdPreviewId, UsdMountSessionView>,
}

impl UsdMountView {
    pub(crate) fn focused(&self, viewport: &UsdViewportState) -> Option<&UsdMountSessionView> {
        viewport
            .focused_preview_id()
            .and_then(|preview| self.sessions.get(&preview))
    }
}

/// Leaf name after the last `/`.
fn leaf(path: &str) -> String {
    path.rsplit('/').next().unwrap_or_default().to_string()
}

/// View-model producer: resolve each advertised socket's snap for the selected host.
pub fn produce_usd_mount_view(
    selected: Option<Res<lunco_scene_commands::SelectedEntities>>,
    q: Query<&UsdPrimPath>,
    q_parents: Query<&ChildOf>,
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<CanonicalStages>,
    viewport: Option<Res<UsdViewportState>>,
    mut view: ResMut<UsdMountView>,
) {
    let Some(viewport) = viewport else {
        view.sessions.clear();
        return;
    };
    let open: std::collections::HashSet<_> = viewport.sessions().map(|s| s.id()).collect();
    view.sessions.retain(|preview, _| open.contains(preview));

    for session in viewport.sessions() {
        let session_view =
            view.sessions
                .entry(session.id())
                .or_insert_with(|| UsdMountSessionView {
                    preview: session.id(),
                    doc: session.doc(),
                    edit_target: session.edit_target().clone(),
                    generation: 0,
                    entity: None,
                    host_path: String::new(),
                    items: Vec::new(),
                    diagnostics: Vec::new(),
                });
        session_view.preview = session.id();
        session_view.doc = session.doc();
        session_view.edit_target = session.edit_target().clone();
        session_view.generation = session.projected_generation();
        session_view.entity = None;
        session_view.host_path.clear();
        session_view.items.clear();
        session_view.diagnostics.clear();

        let Some(entity) = crate::ui::selected_entity_in_preview(
            session,
            selected.as_deref(),
            None,
            &q,
            &q_parents,
        ) else {
            continue;
        };
        let Ok(prim) = q.get(entity) else {
            continue;
        };
        session_view.entity = Some(entity);

        let stage_id = prim.stage_handle.id();
        if canonical.get(stage_id).is_none() {
            if let Some(recipe) = stages
                .get(&prim.stage_handle)
                .and_then(|a| a.recipe.clone())
            {
                canonical.get_or_build(stage_id, &recipe);
            }
        }
        let Some(cs) = canonical.get(stage_id) else {
            continue;
        };
        let stage_view = cs.view();
        session_view.host_path = prim.path.clone();

        let sockets = read_sockets(&stage_view, &prim.path);
        session_view.diagnostics = sockets.diagnostics;
        for socket in sockets.sockets {
            let (mut placement, mut rotate_deg, mut aligned) = (None, None, false);
            let part_leaf = socket.part.as_deref().map(leaf);
            let joint_path = socket
                .part
                .as_deref()
                .and_then(|part| read_attachment_joint(&stage_view, part));

            if let Some(part) = socket.part.as_deref() {
                if let Some(plug) =
                    read_plug(&stage_view, part).filter(|plug| plug.kind == socket.accepts)
                {
                    match resolve_mount_placement(socket.frame, plug.frame) {
                        Ok((t, r)) => {
                            // Compare the authored local transform with the
                            // resolved placement; no runtime transform is an
                            // authoring source.
                            if let Ok(pp) = SdfPath::new(part) {
                                aligned =
                                    match lunco_usd_bevy::local_transform_at(&stage_view, &pp, 0.0)
                                    {
                                        Ok(Some(transform)) => {
                                            let translation_error = (transform.translation
                                                - Vec3::new(t[0] as f32, t[1] as f32, t[2] as f32))
                                            .length();
                                            let expected_rotation =
                                                lunco_usd_bevy::euler_xyz_deg_to_quat(Vec3::new(
                                                    r[0] as f32,
                                                    r[1] as f32,
                                                    r[2] as f32,
                                                ));
                                            let rotation_error =
                                                transform.rotation.angle_between(expected_rotation);
                                            translation_error < MOUNT_ALIGNMENT_TOLERANCE
                                                && rotation_error < MOUNT_ALIGNMENT_TOLERANCE
                                        }
                                        Ok(None) => false,
                                        Err(error) => {
                                            bevy::log::warn!(
                                                "mount alignment rejected for {}: {}",
                                                pp.as_str(),
                                                error
                                            );
                                            false
                                        }
                                    };
                            }
                            placement = Some(t);
                            rotate_deg = Some(r);
                        }
                        Err(error) => {
                            bevy::log::warn!("mount alignment rejected for {}: {}", part, error);
                        }
                    }
                }
            }

            let attach_asset = if socket.part.is_none() {
                socket.asset.clone()
            } else {
                None
            };

            session_view.items.push(MountItem {
                socket: socket.name,
                socket_path: socket.path,
                accepts: socket.accepts,
                joint: socket.joint,
                axis: socket.axis,
                part_path: socket.part,
                part_leaf,
                joint_path,
                attach_asset,
                socket_frame: socket.frame,
                placement,
                rotate_deg,
                aligned,
            });
        }
    }
}
