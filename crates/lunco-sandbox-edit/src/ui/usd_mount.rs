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

use bevy::prelude::*;
use lunco_usd::attach::resolve_mount_placement;
use lunco_usd_bevy::mount::{read_plug_frame, read_sockets};
use lunco_usd_bevy::{CanonicalStages, SdfPath, UsdPrimPath, UsdStageAsset};

/// One socket row, with the snap already resolved when a part is present.
#[derive(Clone)]
pub struct MountItem {
    /// Socket leaf name (`wheel_fl`).
    pub socket: String,
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
    /// The joint prim path (`<part>_Joint`, the attach convention), authored on snap.
    pub joint_path: String,
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

/// Render-ready mount rows for the selected host. Derived, never authoritative.
#[derive(Resource, Default)]
pub struct UsdMountView {
    pub entity: Option<Entity>,
    pub host_path: String,
    pub items: Vec<MountItem>,
}

/// Leaf name after the last `/`.
fn leaf(path: &str) -> String {
    path.rsplit('/').next().unwrap_or_default().to_string()
}

/// The attach-convention joint path for a part: `<host>/<leaf>_Joint`.
fn joint_path_for(part: &str) -> String {
    let host = part.rsplit_once('/').map(|(h, _)| h).unwrap_or("");
    format!("{host}/{}_Joint", leaf(part))
}

/// View-model producer: resolve each advertised socket's snap for the selected host.
pub fn produce_usd_mount_view(
    selected: Option<Res<crate::SelectedEntities>>,
    q: Query<&UsdPrimPath>,
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<CanonicalStages>,
    mut view: ResMut<UsdMountView>,
) {
    let entity = selected.as_deref().and_then(|s| s.primary());
    view.entity = entity;
    view.host_path.clear();
    view.items.clear();

    let Some(entity) = entity else {
        return;
    };
    let Ok(prim) = q.get(entity) else {
        return;
    };
    let stage_id = prim.stage_handle.id();
    if canonical.get(stage_id).is_none() {
        if let Some(recipe) = stages.get(&prim.stage_handle).and_then(|a| a.recipe.clone()) {
            canonical.get_or_build(stage_id, &recipe);
        }
    }
    let Some(cs) = canonical.get(stage_id) else {
        return;
    };
    let stage_view = cs.view();
    view.host_path = prim.path.clone();

    for socket in read_sockets(&stage_view, &prim.path) {
        let (mut placement, mut rotate_deg, mut aligned) = (None, None, false);
        let part_leaf = socket.part.as_deref().map(leaf);
        let joint_path = socket
            .part
            .as_deref()
            .map(joint_path_for)
            .unwrap_or_default();

        if let Some(part) = socket.part.as_deref() {
            if let Some(plug) = read_plug_frame(&stage_view, part) {
                let (t, r) = resolve_mount_placement(socket.frame, plug);
                // Already there? Compare against the part's authored local transform.
                if let Ok(pp) = SdfPath::new(part) {
                    let cur = lunco_usd_bevy::local_transform_at(&stage_view, &pp, 0.0)
                        .unwrap_or_default();
                    let dt = (cur.translation
                        - Vec3::new(t[0] as f32, t[1] as f32, t[2] as f32))
                    .length();
                    aligned = dt < 1.0e-3;
                }
                placement = Some(t);
                rotate_deg = Some(r);
            }
        }

        // An empty socket (no attached part) that names a default asset offers a
        // new-attach; a socket already holding a part does not.
        let attach_asset = if socket.part.is_none() { socket.asset.clone() } else { None };

        view.items.push(MountItem {
            socket: socket.name,
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
