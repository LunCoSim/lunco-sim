//! Mount-frame reading — the socket/plug schema behind the Object Builder's
//! retrofit *snap* (`docs/architecture/48-object-builder.md` §3.1).
//!
//! A host body advertises **sockets** under a `Mounts` group; an attached part
//! advertises the **plug** frame it hangs by. Snapping re-derives the part's
//! placement so its plug coincides with the socket — `move the socket, the part
//! and its joint follow` — which is the whole point of declaring mounts instead
//! of hand-authoring a transform and a joint anchor that nothing reconciles.
//!
//! ```usda
//! def Xform "Mounts" {
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
//! Detection is by the authored `lunco:mount:socket` attribute ([`read_sockets`]),
//! NOT by `kind` — `kind` is USD's regulated model taxonomy and `"mount"` is not a
//! valid kind, so the socket discriminator is a LunCo attribute.
//! and on the part:
//! ```usda
//! uniform token lunco:mount:plug  = "wheel"
//! rel           lunco:mount:frame = </Wheel/Mounts/hub>   # the plug frame
//! ```
//!
//! This module only *reads* — the frame math ([`resolve_mount_placement`]) and the
//! op-lowering ([`realign_component_ops`]) live in `lunco-usd`, unit-tested with no
//! stage. A socket/plug frame is composed relative to its **body root** (the host or
//! the part), so a non-identity `Mounts` group is handled correctly.
//!
//! [`resolve_mount_placement`]: lunco_usd::attach::resolve_mount_placement
//! [`realign_component_ops`]: lunco_usd::attach::realign_component_ops

use bevy::prelude::Transform;
use openusd::sdf::Path as SdfPath;

use crate::read::UsdRead;
use crate::local_transform_at;

/// A socket advertised by a host body — `<host>/Mounts/<name>` carrying
/// `lunco:mount:socket`. What a snap reads to place the part it holds.
#[derive(Debug, Clone)]
pub struct MountSocket {
    /// The socket prim path (`<host>/Mounts/<name>`).
    pub path: String,
    /// The socket leaf name (`wheel_fl`).
    pub name: String,
    /// What plug kind it accepts (`lunco:mount:socket`, e.g. `"wheel"`).
    pub accepts: String,
    /// The joint kind the socket implies (`lunco:mount:joint`) — `"fixed"`,
    /// `"revolute"`, or `"prismatic"`. Defaults to `"fixed"` when unauthored.
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

/// The `Mounts` group path under a body root (`<body>/Mounts`).
fn mounts_group(body_root: &str) -> String {
    format!("{}/Mounts", body_root.trim_end_matches('/'))
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

/// The local frame of `mount_prim` expressed in `body_root`'s space — the product
/// of local transforms from `body_root`'s child down to `mount_prim`, i.e. every
/// intermediate `Mounts` xform is folded in, but `body_root`'s own placement is
/// **not** (we want a body-local frame). An unauthored xform reads as identity.
pub fn frame_in_body<R: UsdRead>(reader: &R, body_root: &str, mount_prim: &SdfPath) -> Transform {
    let body_root = body_root.trim_end_matches('/');
    let mut acc = local_transform_at(reader, mount_prim, 0.0).unwrap_or(Transform::IDENTITY);
    let mut cur = mount_prim.clone();
    // Walk up, prepending each ancestor's local transform, until the next step
    // would be `body_root` (whose transform we exclude) or the tree runs out.
    while let Some(parent) = parent_str(&cur) {
        if parent == body_root {
            break;
        }
        let Ok(parent_path) = SdfPath::new(&parent) else { break };
        let parent_local = local_transform_at(reader, &parent_path, 0.0).unwrap_or(Transform::IDENTITY);
        acc = compose(parent_local, acc);
        cur = parent_path;
    }
    acc
}

/// Every socket a `host` body advertises under its `Mounts` group. Empty when the
/// host declares none (the common case today) — the caller shows nothing.
pub fn read_sockets<R: UsdRead>(reader: &R, host: &str) -> Vec<MountSocket> {
    let Ok(group) = SdfPath::new(&mounts_group(host)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for child in reader.children(&group) {
        let Some(accepts) = reader.text(&child, "lunco:mount:socket") else {
            continue; // not a socket — skip mount groups that hold other data
        };
        let joint = reader
            .text(&child, "lunco:mount:joint")
            .unwrap_or_else(|| "fixed".to_string());
        // A fixed joint carries no axis; drop any stray authored one.
        let axis = if joint == "fixed" {
            None
        } else {
            reader.text(&child, "lunco:mount:axis")
        };
        let frame = frame_in_body(reader, host, &child);
        let part = reader.rel_target(&child, "lunco:mount:part");
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
    out
}

/// The plug frame of an attached `part`, in the **part's** local space — follows
/// the part's `rel lunco:mount:frame` to the plug prim and composes it back to the
/// part root. `None` when the part advertises no plug frame.
pub fn read_plug_frame<R: UsdRead>(reader: &R, part: &str) -> Option<Transform> {
    let part_path = SdfPath::new(part).ok()?;
    let plug = reader.rel_target(&part_path, "lunco:mount:frame")?;
    let plug_path = SdfPath::new(&plug).ok()?;
    Some(frame_in_body(reader, part, &plug_path))
}

/// The plug frame of a component **asset that is not yet on the live stage** — the
/// piece the *new-attach* flow needs (unlike a retrofit, the plug lives inside the
/// asset file, not in the composed scene). Composes the asset's full closure
/// off-thread-safe via [`compose_file_to_stage`](crate::compose_file_to_stage)
/// (resolving its references, anchored at the file's own directory), then reads the
/// plug frame off its `defaultPrim` — the part every `AttachSpec` references in.
///
/// `asset_path` is a **filesystem** path (resolve an asset-relative path against the
/// asset root first). Returns the plug [`Transform`] in the part's local space, or
/// `None` if the asset has no `defaultPrim` or the default part advertises no plug.
/// Native-only: composition does file I/O.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_asset_plug_frame(asset_path: &std::path::Path) -> Option<Transform> {
    let stage = crate::compose_file_to_stage(asset_path).ok()?;
    let cs = crate::CanonicalStage::from_stage(stage, asset_path.to_string_lossy().to_string());
    let view = cs.view();
    let default_prim = view.default_prim()?;
    read_plug_frame(&view, &format!("/{default_prim}"))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod mount_reader_tests {
    //! Exercises the socket/plug reader against a **real composed stage** — the
    //! read half of the retrofit snap that unit-testing `resolve_mount_placement`
    //! (in `lunco-usd`, over bare transforms) can't reach: that the frames read
    //! *body-local* through an intermediate `Mounts` group, and that the mount
    //! metadata + `part` relationship compose. A wrong frame here is the physics
    //! bug the design deferred the UI for; this pins it deterministically.

    use super::{read_plug_frame, read_sockets};
    use crate::canonical::{CanonicalStage, StageRecipe};

    // Base at (5,6,5); a socket 2.5 up under Base/Mounts naming a child Arm; Arm
    // (off at +2 X) carries a plug frame under Arm/Mounts/hub offset (0.1,0.2,0.3).
    const SCENE: &str = r#"#usda 1.0
(
    defaultPrim = "World"
)
def Xform "World"
{
    def Cube "Base"
    {
        double3 xformOp:translate = (5, 6, 5)
        uniform token[] xformOpOrder = ["xformOp:translate"]
        def Xform "Mounts"
        {
            def Xform "arm"
            {
                uniform token lunco:mount:socket = "arm"
                uniform token lunco:mount:joint = "revolute"
                uniform token lunco:mount:axis = "Z"
                rel lunco:mount:part = </World/Base/Arm>
                double3 xformOp:translate = (0, 2.5, 0)
                uniform token[] xformOpOrder = ["xformOp:translate"]
            }
        }
        def Cube "Arm"
        {
            double3 xformOp:translate = (2, 0, 0)
            uniform token[] xformOpOrder = ["xformOp:translate"]
            uniform token lunco:mount:plug = "arm"
            rel lunco:mount:frame = </World/Base/Arm/Mounts/hub>
            def Xform "Mounts"
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
        assert_eq!(sockets.len(), 1, "one socket under Base/Mounts");
        let s = &sockets[0];
        assert_eq!(s.name, "arm");
        assert_eq!(s.accepts, "arm");
        assert_eq!(s.joint, "revolute");
        assert_eq!(s.axis.as_deref(), Some("Z"));
        assert_eq!(s.part.as_deref(), Some("/World/Base/Arm"));
        // Frame is BODY-LOCAL: the socket sits 2.5 up from Base's origin, NOT at
        // world (5, 8.5, 5) — Base's own (5,6,5) placement is excluded.
        assert!(close(s.frame.translation, [0.0, 2.5, 0.0]), "socket frame {:?}", s.frame.translation);
    }

    #[test]
    fn reads_plug_frame_part_local_through_mounts_group() {
        let cs = CanonicalStage::from_recipe(&StageRecipe::from_source("scene.usda", SCENE))
            .expect("stage builds");
        let view = cs.view();

        let plug = read_plug_frame(&view, "/World/Base/Arm").expect("Arm advertises a plug");
        // Plug is PART-LOCAL: the hub offset (0.1,0.2,0.3), NOT folded with Arm's
        // own (2,0,0) placement — a plug frame is expressed in the part's space.
        assert!(close(plug.translation, [0.1, 0.2, 0.3]), "plug frame {:?}", plug.translation);
    }

    #[test]
    fn no_sockets_when_host_declares_none() {
        let cs = CanonicalStage::from_recipe(&StageRecipe::from_source("scene.usda", SCENE))
            .expect("stage builds");
        let view = cs.view();
        // The Arm has a plug but no `Mounts` sockets — read_sockets is empty.
        assert!(read_sockets(&view, "/World/Base/Arm").is_empty());
    }

    #[test]
    fn reads_plug_frame_off_a_not_yet_loaded_asset_file() {
        // The new-attach path: `read_asset_plug_frame` composes a component asset
        // straight off disk (its plug lives in the file, not the live scene) and
        // reads the plug frame off its `defaultPrim`. Validates against the shipped
        // demo component, whose hub sits 0.4 m above the part origin.
        let asset = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/components/mount_probe.usda");
        let plug = super::read_asset_plug_frame(&asset)
            .expect("mount_probe.usda composes and advertises a plug");
        assert!(
            close(plug.translation, [0.0, 0.4, 0.0]),
            "asset plug frame {:?}",
            plug.translation
        );
    }
}
