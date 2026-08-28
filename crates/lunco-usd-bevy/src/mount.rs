//! Mount-frame reading — the socket/plug schema behind the Object Builder's
//! retrofit *snap* (`docs/architecture/48-object-builder.md` §3.1).
//!
//! A host body explicitly advertises **sockets** with a relationship; an attached
//! part advertises the **plug** frame it hangs by. Snapping re-derives the part's
//! placement so its plug coincides with the socket — `move the socket, the part
//! and its joint follow` — which is the whole point of declaring mounts instead
//! of hand-authoring a transform and a joint anchor that nothing reconciles.
//!
//! ```usda
//! def Xform "Interfaces" {
//!     def Xform "wheel_fl" (
//!         kind = "subcomponent"
//!     ) {
//!         uniform token   lunco:mount:socket = "wheel"       # what may attach
//!         uniform token   lunco:mount:joint  = "revolute"    # the constraint it implies
//!         uniform token   lunco:mount:axis   = "X"
//!         rel             lunco:mount:part   = </Bogie/Wheel_FL>   # the part it holds
//!         double3 xformOp:translate = (1.2, -0.3, 0.9)      # the socket frame
//!         uniform token[] xformOpOrder = ["xformOp:translate"]
//!     }
//! }
//! ```
//!
//! Detection is by the applied `LunCoMountSocketAPI` ([`read_sockets`]), not by
//! a loose attribute-name convention or by `kind` — `kind` is USD's regulated
//! model taxonomy and `"mount"` is not a valid kind. The plug half likewise must
//! apply `LunCoMountPlugAPI`.
//! and on the part:
//! ```usda
//! uniform token lunco:mount:plug  = "wheel"
//! rel           lunco:mount:frame = </Wheel/Interfaces/hub>   # the plug frame
//! ```
//!
//! This module only *reads* — the frame math ([`resolve_mount_placement`]) and the
//! op-lowering ([`realign_component_ops`]) live in `lunco-usd`, unit-tested with no
//! stage. A socket/plug frame is composed relative to its **body root** (the host or
//! the part), so arbitrary intermediate grouping is handled correctly.
//!
//! [`resolve_mount_placement`]: lunco_usd::attach::resolve_mount_placement
//! [`realign_component_ops`]: lunco_usd::attach::realign_component_ops

use bevy::prelude::Transform;
use openusd::sdf::Path as SdfPath;

use crate::local_transform_at;
use crate::read::UsdRead;

/// A socket explicitly advertised by a host body, carrying `lunco:mount:socket`.
/// What a snap reads to place the part it holds.
#[derive(Debug, Clone)]
pub struct MountSocket {
    /// The explicitly authored socket prim path.
    pub path: String,
    /// The socket leaf name (`wheel_fl`).
    pub name: String,
    /// What plug kind it accepts (`lunco:mount:socket`, e.g. `"wheel"`).
    pub accepts: String,
    /// The required joint kind the socket implies (`lunco:mount:joint`) —
    /// `"fixed"`, `"revolute"`, or `"prismatic"`.
    pub joint: String,
    /// The joint axis token (`lunco:mount:axis`) — `"X"` / `"Y"` / `"Z"`. `None`
    /// for a fixed joint.
    pub axis: Option<String>,
    /// The socket frame, composed into the **host body's** local space.
    pub frame: Transform,
    /// The already-attached part this socket holds (`rel lunco:mount:part`), as an
    /// absolute composed prim path. `None` if the socket names no part — nothing to
    /// snap yet (an **empty** socket).
    pub part: Option<String>,
    /// The component asset this socket is designed to hold (`lunco:mount:asset`, a
    /// raw asset path like `components/wheel.usda`). Drives the **new-attach** flow:
    /// an empty socket offers to reference this asset in and snap its plug to the
    /// socket. `None` when the socket suggests no default part.
    pub asset: Option<String>,
}

/// A rejected socket contract that the editor can show without treating it as
/// an empty socket. The authored USD remains the authority; this is only the
/// derived diagnostic surface for an invalid advertisement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountDiagnostic {
    /// Socket path whose contract could not be read.
    pub path: String,
    /// Human-readable reason for rejection.
    pub message: String,
}

/// Result of reading a host's advertised sockets. Invalid advertisements are
/// returned as diagnostics so callers can distinguish "no socket" from
/// "socket authored but invalid".
#[derive(Debug, Clone, Default)]
pub struct MountReadout {
    /// Valid socket contracts.
    pub sockets: Vec<MountSocket>,
    /// Rejected socket contracts.
    pub diagnostics: Vec<MountDiagnostic>,
}

/// The parent path of `path` as a string, or `None` at the pseudo-root.
fn parent_str(path: &SdfPath) -> Option<String> {
    let s = path.as_str();
    let idx = s.rfind('/')?;
    if idx == 0 {
        None // parent is the abs-root "/"
    } else {
        Some(s[..idx].to_string())
    }
}

/// Compose `a ∘ b` (apply `b` first, then `a`) via their matrices — the
/// [`Transform`] product bevy has no operator for.
fn compose(a: Transform, b: Transform) -> Transform {
    Transform::from_matrix(a.to_matrix() * b.to_matrix())
}

/// Read a local frame while distinguishing USD's identity-for-unauthored
/// transform from a malformed authored transform.
fn local_mount_transform(reader: &crate::StageView<'_>, path: &SdfPath) -> Option<Transform> {
    match local_transform_at(reader, path, 0.0) {
        Ok(Some(transform)) => Some(transform),
        Ok(None) => Some(Transform::IDENTITY),
        Err(error) => {
            bevy::log::warn!(
                "mount frame {} has malformed authored transform ({error}); mount rejected",
                path.as_str(),
            );
            None
        }
    }
}

/// The local frame of `mount_prim` expressed in `body_root`'s space — the product
/// of local transforms from `body_root`'s child down to `mount_prim`, so every
/// explicitly authored intermediate grouping is folded in, but `body_root`'s own
/// placement is **not** (we want a body-local frame). An unauthored xform reads
/// as USD's identity; a malformed authored xform rejects the frame.
pub fn frame_in_body(
    reader: &crate::StageView<'_>,
    body_root: &str,
    mount_prim: &SdfPath,
) -> Option<Transform> {
    let body_root = body_root.trim_end_matches('/');
    let body_root_path = SdfPath::new(body_root).ok()?;
    if !crate::is_descendant_or_self(mount_prim, body_root) {
        bevy::log::warn!(
            "mount frame {} is outside body root {}; mount rejected",
            mount_prim.as_str(),
            body_root
        );
        return None;
    }
    if mount_prim == &body_root_path {
        return Some(Transform::IDENTITY);
    }
    let mut acc = local_mount_transform(reader, mount_prim)?;
    let mut cur = mount_prim.clone();
    // Walk up, prepending each ancestor's local transform, until the next step
    // would be `body_root` (whose transform we exclude) or the tree runs out.
    while let Some(parent) = parent_str(&cur) {
        if parent == body_root {
            break;
        }
        let Ok(parent_path) = SdfPath::new(&parent) else {
            return None;
        };
        let parent_local = local_mount_transform(reader, &parent_path)?;
        acc = compose(parent_local, acc);
        cur = parent_path;
    }
    Some(acc)
}

/// Every socket a `host` body advertises through `lunco:mount:sockets`. The
/// relationship is the topology authority; no child/group name is special.
pub fn read_sockets(reader: &crate::StageView<'_>, host: &str) -> MountReadout {
    let Ok(host_path) = SdfPath::new(host.trim_end_matches('/')) else {
        return MountReadout {
            sockets: Vec::new(),
            diagnostics: vec![MountDiagnostic {
                path: host.to_string(),
                message: "host path is not a valid USD prim path".to_string(),
            }],
        };
    };
    if !reader.has_api_schema(&host_path, "LunCoMountHostAPI") {
        return MountReadout::default();
    }
    let mut out = Vec::new();
    let mut diagnostics = Vec::new();
    for child in reader.rel_targets(&host_path, "lunco:mount:sockets") {
        if child.is_property_path() || !crate::is_descendant_or_self(&child, host) {
            reject_socket(
                &mut diagnostics,
                host,
                child.as_str(),
                "advertised target is not a descendant prim of the host",
            );
            continue;
        }
        if !reader.has_api_schema(&child, "LunCoMountSocketAPI") {
            reject_socket(
                &mut diagnostics,
                host,
                child.as_str(),
                "advertised target has no LunCoMountSocketAPI",
            );
            continue;
        }
        let Some(accepts) = reader
            .text(&child, "lunco:mount:socket")
            .filter(|value| !value.is_empty())
        else {
            reject_socket(
                &mut diagnostics,
                host,
                child.as_str(),
                "socket has no accepted plug kind",
            );
            continue;
        };
        let Some(joint) = reader
            .text(&child, "lunco:mount:joint")
            .filter(|value| !value.is_empty())
        else {
            reject_socket(
                &mut diagnostics,
                host,
                child.as_str(),
                "socket has no lunco:mount:joint",
            );
            continue;
        };
        let authored_axis = reader.text(&child, "lunco:mount:axis");
        let axis = match joint.as_str() {
            "fixed" => {
                if authored_axis
                    .as_deref()
                    .is_some_and(|value| !value.is_empty())
                {
                    reject_socket(
                        &mut diagnostics,
                        host,
                        child.as_str(),
                        "fixed socket must not author an axis",
                    );
                    continue;
                }
                None
            }
            "revolute" | "prismatic" => match authored_axis.as_deref() {
                Some(axis @ ("X" | "Y" | "Z")) => Some(axis.to_string()),
                _ => {
                    reject_socket(
                        &mut diagnostics,
                        host,
                        child.as_str(),
                        format!("{joint} socket needs axis X, Y, or Z"),
                    );
                    continue;
                }
            },
            _ => {
                reject_socket(
                    &mut diagnostics,
                    host,
                    child.as_str(),
                    format!("unsupported joint token {joint:?}"),
                );
                continue;
            }
        };
        let Some(frame) = frame_in_body(reader, host, &child) else {
            reject_socket(
                &mut diagnostics,
                host,
                child.as_str(),
                "socket frame has an invalid transform",
            );
            continue;
        };
        let part_targets = reader.rel_targets(&child, "lunco:mount:part");
        if part_targets.len() > 1 {
            reject_socket(
                &mut diagnostics,
                host,
                child.as_str(),
                "socket has multiple lunco:mount:part targets",
            );
            continue;
        }
        let part = match part_targets.into_iter().next() {
            Some(path) => {
                if path.is_property_path() || !crate::is_descendant_or_self(&path, host) {
                    reject_socket(
                        &mut diagnostics,
                        host,
                        child.as_str(),
                        if path.is_property_path() {
                            "part target must be a prim, not a property".to_string()
                        } else {
                            format!("part target {} is outside host", path.as_str())
                        },
                    );
                    continue;
                }
                Some(path.as_str().to_string())
            }
            None => None,
        };
        // `lunco:mount:asset` names a USD FILE, so it is an `asset` — the resolver
        // and the reference-closure walk only see the ones typed as such.
        let asset = reader.asset(&child, "lunco:mount:asset");
        let name = child
            .as_str()
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string();
        out.push(MountSocket {
            path: child.as_str().to_string(),
            name,
            accepts,
            joint,
            axis,
            frame,
            part,
            asset,
        });
    }
    MountReadout {
        sockets: out,
        diagnostics,
    }
}

fn reject_socket(
    diagnostics: &mut Vec<MountDiagnostic>,
    host: &str,
    path: &str,
    message: impl Into<String>,
) {
    let message = message.into();
    bevy::log::warn!(
        "mount host {} advertises invalid socket {}: {}; socket rejected",
        host,
        path,
        message
    );
    diagnostics.push(MountDiagnostic {
        path: path.to_string(),
        message,
    });
}

/// Read the exact joint recorded by an attached component. The relationship is
/// the only supported identity path; callers must not reconstruct a joint name
/// from the component leaf.
pub fn read_attachment_joint(reader: &crate::StageView<'_>, part: &str) -> Option<String> {
    let part_path = SdfPath::new(part).ok()?;
    if !reader.has_api_schema(&part_path, "LunCoMountAttachmentAPI") {
        return None;
    }
    let targets = reader.rel_targets(&part_path, "lunco:mount:attachmentJoint");
    if targets.len() != 1 || targets[0].is_property_path() {
        bevy::log::warn!(
            "attached component {} must have exactly one prim-valued attachment joint",
            part
        );
        return None;
    }
    Some(targets[0].as_str().to_string())
}

/// The plug advertised by an attached `part`, including its kind and frame in the
/// **part's** local space. The part must apply `LunCoMountPlugAPI`; loose
/// `lunco:mount:*` attributes are not a mount contract.
#[derive(Debug, Clone)]
pub struct MountPlug {
    /// Plug kind matched against a socket's accepted kind.
    pub kind: String,
    /// Plug frame composed into the part root's local space.
    pub frame: Transform,
}

pub fn read_plug(reader: &crate::StageView<'_>, part: &str) -> Option<MountPlug> {
    let part_path = SdfPath::new(part).ok()?;
    if !reader.has_api_schema(&part_path, "LunCoMountPlugAPI") {
        return None;
    }
    let kind = reader
        .text(&part_path, "lunco:mount:plug")
        .filter(|value| !value.is_empty())?;
    let plug = reader.rel_target(&part_path, "lunco:mount:frame")?;
    let plug_path = SdfPath::new(&plug).ok()?;
    Some(MountPlug {
        kind,
        frame: frame_in_body(reader, part, &plug_path)?,
    })
}

/// The plug frame of a component **asset that is not yet on the live stage** — the
/// piece the *new-attach* flow needs (unlike a retrofit, the plug lives inside the
/// asset file, not in the composed scene). Composes the asset's full closure
/// off-thread-safe via [`compose_file_to_stage`](crate::compose_file_to_stage)
/// (resolving its references, anchored at the file's own directory), then reads the
/// plug off its `defaultPrim` — the part every `AttachSpec` references in.
///
/// `asset_path` is a **filesystem** path (resolve an asset-relative path against the
/// asset root first). Returns the [`MountPlug`] in the part's local space, or
/// `None` if the asset has no `defaultPrim` or the default part advertises no plug.
/// Native-only: composition does file I/O.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_asset_plug(asset_path: &std::path::Path) -> Option<MountPlug> {
    let stage = crate::compose_file_to_stage(asset_path).ok()?;
    let cs = crate::CanonicalStage::from_stage(stage, asset_path.to_string_lossy().to_string());
    let view = cs.view();
    let default_prim = view.default_prim()?;
    read_plug(&view, &format!("/{default_prim}"))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod mount_reader_tests {
    //! Exercises the socket/plug reader against a **real composed stage** — the
    //! read half of the retrofit snap that unit-testing `resolve_mount_placement`
    //! (in `lunco-usd`, over bare transforms) can't reach: that the frames read
    //! *body-local* through an arbitrary intermediate group, and that the mount
    //! metadata + `part` relationship compose. A wrong frame here is the physics
    //! bug the design deferred the UI for; this pins it deterministically.

    use super::{read_plug, read_sockets};
    use crate::canonical::{CanonicalStage, StageRecipe};

    // Base at (5,6,5); a socket 2.5 up under an arbitrary Interfaces group
    // naming a child Arm; Arm (off at +2 X) carries a plug frame under
    // Arm/Interfaces/hub offset (0.1,0.2,0.3).
    const SCENE: &str = r#"#usda 1.0
(
    defaultPrim = "World"
    metersPerUnit = 1
)
def Xform "World"
{
    def Cube "Base" (
        prepend apiSchemas = ["LunCoMountHostAPI"]
    )
    {
        rel lunco:mount:sockets = [</World/Base/Interfaces/arm>]
        double3 xformOp:translate = (5, 6, 5)
        uniform token[] xformOpOrder = ["xformOp:translate"]
        def Xform "Interfaces"
        {
            def Xform "arm" (
                prepend apiSchemas = ["LunCoMountSocketAPI"]
            )
            {
                uniform token lunco:mount:socket = "arm"
                uniform token lunco:mount:joint = "revolute"
                uniform token lunco:mount:axis = "Z"
                rel lunco:mount:part = </World/Base/Arm>
                double3 xformOp:translate = (0, 2.5, 0)
                uniform token[] xformOpOrder = ["xformOp:translate"]
            }
        }
        def Cube "Arm" (
            prepend apiSchemas = ["LunCoMountPlugAPI"]
        )
        {
            double3 xformOp:translate = (2, 0, 0)
            uniform token[] xformOpOrder = ["xformOp:translate"]
            uniform token lunco:mount:plug = "arm"
            rel lunco:mount:frame = </World/Base/Arm/Interfaces/hub>
            def Xform "Interfaces"
            {
                def Xform "hub"
                {
                    double3 xformOp:translate = (0.1, 0.2, 0.3)
                    uniform token[] xformOpOrder = ["xformOp:translate"]
                }
            }
        }
    }
}
"#;

    fn close(a: bevy::prelude::Vec3, b: [f32; 3]) -> bool {
        (a.x - b[0]).abs() < 1e-4 && (a.y - b[1]).abs() < 1e-4 && (a.z - b[2]).abs() < 1e-4
    }

    #[test]
    fn reads_socket_frame_metadata_and_part_body_local() {
        let cs = CanonicalStage::from_recipe(&StageRecipe::from_source("scene.usda", SCENE))
            .expect("stage builds");
        let view = cs.view();

        let sockets = read_sockets(&view, "/World/Base");
        assert_eq!(sockets.sockets.len(), 1, "one explicitly advertised socket");
        assert!(sockets.diagnostics.is_empty());
        let s = &sockets.sockets[0];
        assert_eq!(s.name, "arm");
        assert_eq!(s.accepts, "arm");
        assert_eq!(s.joint, "revolute");
        assert_eq!(s.axis.as_deref(), Some("Z"));
        assert_eq!(s.part.as_deref(), Some("/World/Base/Arm"));
        // Frame is BODY-LOCAL: the socket sits 2.5 up from Base's origin, NOT at
        // world (5, 8.5, 5) — Base's own (5,6,5) placement is excluded.
        assert!(
            close(s.frame.translation, [0.0, 2.5, 0.0]),
            "socket frame {:?}",
            s.frame.translation
        );
    }

    #[test]
    fn reads_plug_frame_part_local_through_explicit_frame_path() {
        let cs = CanonicalStage::from_recipe(&StageRecipe::from_source("scene.usda", SCENE))
            .expect("stage builds");
        let view = cs.view();

        let plug = read_plug(&view, "/World/Base/Arm").expect("Arm advertises a plug");
        assert_eq!(plug.kind, "arm");
        // Plug is PART-LOCAL: the hub offset (0.1,0.2,0.3), NOT folded with Arm's
        // own (2,0,0) placement — a plug frame is expressed in the part's space.
        assert!(
            close(plug.frame.translation, [0.1, 0.2, 0.3]),
            "plug frame {:?}",
            plug.frame.translation
        );
    }

    #[test]
    fn no_sockets_when_host_declares_none() {
        let cs = CanonicalStage::from_recipe(&StageRecipe::from_source("scene.usda", SCENE))
            .expect("stage builds");
        let view = cs.view();
        // The Arm has a plug but is not a mount host — read_sockets is empty.
        assert!(read_sockets(&view, "/World/Base/Arm").sockets.is_empty());
    }

    #[test]
    fn loose_mount_attributes_are_not_a_schema_contract() {
        let source = SCENE
            .replace("prepend apiSchemas = [\"LunCoMountSocketAPI\"]", "")
            .replace("prepend apiSchemas = [\"LunCoMountPlugAPI\"]", "");
        let cs = CanonicalStage::from_recipe(&StageRecipe::from_source("scene.usda", &source))
            .expect("stage builds");
        let view = cs.view();

        let result = read_sockets(&view, "/World/Base");
        assert!(result.sockets.is_empty());
        assert_eq!(result.diagnostics.len(), 1);
        assert!(read_plug(&view, "/World/Base/Arm").is_none());
    }

    #[test]
    fn invalid_joint_metadata_is_rejected_instead_of_becoming_fixed_or_x() {
        let source = SCENE
            .replace(
                "uniform token lunco:mount:joint = \"revolute\"",
                "uniform token lunco:mount:joint = \"hinge\"",
            )
            .replace(
                "uniform token lunco:mount:axis = \"Z\"",
                "uniform token lunco:mount:axis = \"\"",
            );
        let cs = CanonicalStage::from_recipe(&StageRecipe::from_source("scene.usda", &source))
            .expect("stage builds");
        let view = cs.view();

        let result = read_sockets(&view, "/World/Base");
        assert!(result.sockets.is_empty());
        assert_eq!(result.diagnostics.len(), 1);
    }

    #[test]
    fn reads_plug_frame_off_a_not_yet_loaded_asset_file() {
        // The new-attach path: `read_asset_plug` composes a component asset
        // straight off disk (its plug lives in the file, not the live scene) and
        // reads the plug frame off its `defaultPrim`. Validates against the shipped
        // demo component, whose hub sits 0.4 m above the part origin.
        let asset = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/components/mounting/demo_probe.usda");
        let plug =
            super::read_asset_plug(&asset).expect("demo_probe.usda composes and advertises a plug");
        assert_eq!(plug.kind, "probe");
        assert!(
            close(plug.frame.translation, [0.0, 0.4, 0.0]),
            "asset plug frame {:?}",
            plug.frame.translation
        );
    }
}
