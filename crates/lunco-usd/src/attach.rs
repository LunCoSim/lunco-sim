//! Attach a component asset to a host body as a jointed child — the op-lowering
//! behind "build from parts" (`docs/architecture/45-object-builder.md` §3.1).
//!
//! ## The duplication this removes
//!
//! Authoring an assembly by hand encodes each part's placement *twice*, in two
//! places nothing reconciles: the part's own `xformOp:translate`, and — elsewhere
//! in the file — the joint's `physics:localPos0`. `rocker_bogie.usda` pays this
//! for ten joints. Move a wheel and you must edit both, correctly, or the visual
//! and the constraint disagree.
//!
//! [`attach_component_ops`] derives the joint anchor from the placement instead of
//! asking for it a second time: with a joint located at the part's origin,
//! `localPos0` is the placement in the host frame and `localPos1` is the origin —
//! exactly the convention every shipped joint already follows
//! (`localPos1 = (0,0,0)` throughout `rocker_bogie.usda`). One number, one edit.
//!
//! ## What this is NOT (yet)
//!
//! v1 places by translation only. Rotated mounts and socket/plug frame matching
//! (`lunco:mount:*`) are the layer above this; they compute a *placement* and then
//! call the same lowering. Keeping the geometry here trivial and derivable is
//! deliberate — a wrong frame conversion is a physics bug you can only see with the
//! renderer running, so this function commits to nothing it can't derive exactly.
//!
//! The lowering is a **pure function** returning `Vec<UsdOp>`; the command in
//! `commands.rs` just applies them through the registry, so each op journals and
//! inverts on its own. That keeps the geometry unit-testable with no world, no
//! composition, and no I/O.

use crate::document::{LayerId, UsdOp};
use bevy::prelude::Reflect;

/// The joint that fixes the attached part to its host.
#[derive(Debug, Clone, PartialEq, Eq, Default, Reflect, serde::Serialize, serde::Deserialize)]
pub enum AttachJoint {
    /// Rigidly fixed — the part moves exactly with the host.
    #[default]
    Fixed,
    /// A hinge about `axis` (`"X"` | `"Y"` | `"Z"`), e.g. a wheel or a knuckle.
    Revolute { axis: Axis },
    /// A slider along `axis`, e.g. a suspension travel or a linear actuator.
    Prismatic { axis: Axis },
}

/// A principal axis in the host body's local frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Reflect, serde::Serialize, serde::Deserialize)]
pub enum Axis {
    #[default]
    X,
    Y,
    Z,
}

impl Axis {
    fn token(self) -> &'static str {
        match self {
            Axis::X => "X",
            Axis::Y => "Y",
            Axis::Z => "Z",
        }
    }
}

/// Everything needed to attach one component to one host body.
#[derive(Debug, Clone, PartialEq, Reflect, serde::Serialize, serde::Deserialize)]
pub struct AttachSpec {
    /// Layer the edits land in (base or runtime).
    pub edit_target: LayerId,
    /// Absolute path of the **host body** prim the part hangs off — a prim that
    /// is (or will be) a `PhysicsRigidBodyAPI` body. The part becomes its child,
    /// which is how nested jointed bodies are authored (`RockerL` under
    /// `RockerBogie`); each still becomes its own Avian body because it carries
    /// its own rigid-body opinion — the compound-piece rule is for colliders
    /// *without* a body, not for a nested body.
    pub host_path: String,
    /// Leaf name of the new child prim (e.g. `Wheel_FL`).
    pub name: String,
    /// The component asset path — the **raw** path, no `@…@` delimiters (those are
    /// USDA syntax, not part of the path), exactly like [`UsdOp::AddPrim`]'s
    /// `reference` field: e.g. `components/mobility/wheel.usda` or
    /// `lunco://components/mobility/wheel.usda`. The asset's `defaultPrim` is the
    /// part, so no in-asset prim path is needed when the asset declares one.
    pub asset: String,
    /// Where the part sits in the host body's local frame. Also the derived joint
    /// anchor — authored once, here.
    pub placement: [f64; 3],
    /// The part's orientation in the host frame, as `xformOp:rotateXYZ` Euler
    /// degrees. `[0,0,0]` (the common case, and [`AttachSpec::new`]'s default)
    /// authors no rotation. Set by [`AttachSpec::from_mount`] so a plug frame
    /// aligns to a socket frame.
    pub rotate_deg: [f64; 3],
    /// The joint fixing the part to the host.
    pub joint: AttachJoint,
}

impl Default for AttachSpec {
    // `Reflect`/`#[Command(default)]` need a Default. Like `UsdOp::default`, this
    // is the never-dispatched identity placeholder — real callers always fill it.
    fn default() -> Self {
        Self {
            edit_target: LayerId::root(),
            host_path: String::new(),
            name: String::new(),
            asset: String::new(),
            placement: [0.0; 3],
            rotate_deg: [0.0; 3],
            joint: AttachJoint::Fixed,
        }
    }
}

impl AttachSpec {
    /// Attach `asset` as `name` under `host_path`, placed by translation only
    /// (no rotation) with joint `joint`. The common case — a wheel, a sensor, a
    /// tank that sits axis-aligned where you put it.
    pub fn new(
        edit_target: LayerId,
        host_path: impl Into<String>,
        name: impl Into<String>,
        asset: impl Into<String>,
        placement: [f64; 3],
        joint: AttachJoint,
    ) -> Self {
        Self {
            edit_target,
            host_path: host_path.into(),
            name: name.into(),
            asset: asset.into(),
            placement,
            rotate_deg: [0.0; 3],
            joint,
        }
    }

    /// Attach `asset` by matching a **plug frame on the part** to a **socket frame
    /// on the host** — the mount abstraction (doc 45 §3.1). Both frames are given
    /// as local [`Transform`]s (read off `xformOp:*` on the socket/plug prims by the
    /// caller); this computes the part placement + rotation so the plug coincides
    /// with the socket, via [`resolve_mount_placement`]. The joint anchor still
    /// derives from the placement, so a bogie reconfiguration is "move the socket".
    ///
    /// Frame *reading* stays with the caller (it needs the composed stage); this
    /// keeps the frame math a pure, unit-tested function of two transforms — a wrong
    /// frame conversion is a physics bug you can only see with the renderer running,
    /// so it is isolated here where it can be checked against hand-computed matrices.
    pub fn from_mount(
        edit_target: LayerId,
        host_path: impl Into<String>,
        name: impl Into<String>,
        asset: impl Into<String>,
        joint: AttachJoint,
        socket: bevy::prelude::Transform,
        plug: bevy::prelude::Transform,
    ) -> Self {
        let (placement, rotate_deg) = resolve_mount_placement(socket, plug);
        Self {
            edit_target,
            host_path: host_path.into(),
            name: name.into(),
            asset: asset.into(),
            placement,
            rotate_deg,
            joint,
        }
    }

    fn child_path(&self) -> String {
        format!("{}/{}", self.host_path.trim_end_matches('/'), self.name)
    }

    fn joint_path(&self) -> String {
        format!("{}/{}_Joint", self.host_path.trim_end_matches('/'), self.name)
    }

    fn joint_type_name(&self) -> &'static str {
        match self.joint {
            AttachJoint::Fixed => "PhysicsFixedJoint",
            AttachJoint::Revolute { .. } => "PhysicsRevoluteJoint",
            AttachJoint::Prismatic { .. } => "PhysicsPrismaticJoint",
        }
    }
}

fn vec3_literal(v: [f64; 3]) -> String {
    format!("({}, {}, {})", v[0], v[1], v[2])
}

/// Lower an [`AttachSpec`] to the primitive [`UsdOp`] sequence that references the
/// part in, places it, and joints it — with the joint anchor **derived** from the
/// placement (`localPos0 = placement`, `localPos1 = origin`).
///
/// The ops are ordered so each is valid when applied in turn: the child prim
/// exists before it is placed; both bodies exist before the joint relates them.
pub fn attach_component_ops(spec: &AttachSpec) -> Vec<UsdOp> {
    let child = spec.child_path();
    let joint = spec.joint_path();
    let et = spec.edit_target.clone();

    let mut ops = vec![
        // 1. Reference the component in as a child of the host body.
        UsdOp::AddPrim {
            edit_target: et.clone(),
            parent_path: spec.host_path.clone(),
            name: spec.name.clone(),
            type_name: None,
            reference: Some(spec.asset.clone()),
        },
        // 2. Place it. This is the ONE authored placement.
        UsdOp::SetTranslate {
            edit_target: et.clone(),
            path: child.clone(),
            value: spec.placement,
        },
        // 3. The joint prim, typed by the requested kind.
        UsdOp::AddPrim {
            edit_target: et.clone(),
            parent_path: spec.host_path.clone(),
            name: format!("{}_Joint", spec.name),
            type_name: Some(spec.joint_type_name().to_string()),
            reference: None,
        },
        // 4/5. Relate the two bodies.
        UsdOp::SetRelationship {
            edit_target: et.clone(),
            path: joint.clone(),
            name: "physics:body0".into(),
            targets: vec![spec.host_path.clone()],
        },
        UsdOp::SetRelationship {
            edit_target: et.clone(),
            path: joint.clone(),
            name: "physics:body1".into(),
            targets: vec![child.clone()],
        },
        // 6. The anchor — DERIVED from the placement, not typed again. `localPos0`
        //    is the part's origin in the host frame (== its translate); `localPos1`
        //    is the part's own origin.
        UsdOp::SetAttribute {
            edit_target: et.clone(),
            path: joint.clone(),
            name: "physics:localPos0".into(),
            type_name: "point3f".into(),
            value: vec3_literal(spec.placement),
        },
        UsdOp::SetAttribute {
            edit_target: et.clone(),
            path: joint.clone(),
            name: "physics:localPos1".into(),
            type_name: "point3f".into(),
            value: vec3_literal([0.0, 0.0, 0.0]),
        },
    ];

    // 2b. Orientation, only when the placement carries one (a mount that rotates
    //     the part). Authored right after the translate so the placement stays one
    //     unit; the joint anchor is a point (`localPos*`), unaffected by rotation.
    if spec.rotate_deg != [0.0, 0.0, 0.0] {
        ops.insert(
            2,
            UsdOp::SetRotate {
                edit_target: et.clone(),
                path: child.clone(),
                value: spec.rotate_deg,
            },
        );
    }

    // 7. The moving axis, for the non-fixed joints.
    let axis = match spec.joint {
        AttachJoint::Fixed => None,
        AttachJoint::Revolute { axis } | AttachJoint::Prismatic { axis } => Some(axis),
    };
    if let Some(axis) = axis {
        ops.push(UsdOp::SetAttribute {
            edit_target: et,
            path: joint,
            name: "physics:axis".into(),
            type_name: "token".into(),
            // A `token` value literal is a QUOTED string in USD — bare `X` fails to
            // parse ("want String"). Author `"X"`, quotes included.
            value: format!("\"{}\"", axis.token()),
        });
    }

    ops
}

/// Compute a part's host-local placement (translation + `rotateXYZ` Euler degrees)
/// so its **plug frame** coincides with the host's **socket frame** — the geometry
/// behind [`AttachSpec::from_mount`].
///
/// Both `socket` and `plug` are local frames: `socket` in the host body's space,
/// `plug` in the part's space. Placing the part at transform `P` puts its plug at
/// `P ∘ plug` (in host space); we want that to equal `socket`, so
///
/// ```text
///   P ∘ plug = socket   ⇒   P = socket ∘ plug⁻¹
/// ```
///
/// Returns `P` decomposed into `(translation, rotateXYZ-degrees)`, the two things
/// [`attach_component_ops`] authors. Pure and total — no stage, no I/O — so the
/// frame conversion (a physics bug you'd otherwise only catch in the renderer) is
/// unit-tested against hand-computed matrices.
pub fn resolve_mount_placement(
    socket: bevy::prelude::Transform,
    plug: bevy::prelude::Transform,
) -> ([f64; 3], [f64; 3]) {
    use bevy::math::EulerRot;
    let p = socket.compute_affine() * plug.compute_affine().inverse();
    let (_scale, rot, trans) = p.to_scale_rotation_translation();
    let (rx, ry, rz) = rot.to_euler(EulerRot::XYZ);
    (
        [trans.x as f64, trans.y as f64, trans.z as f64],
        [
            (rx as f64).to_degrees(),
            (ry as f64).to_degrees(),
            (rz as f64).to_degrees(),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::Quat;
    use bevy::prelude::Transform;

    fn wheel_spec(joint: AttachJoint) -> AttachSpec {
        AttachSpec {
            edit_target: LayerId::root(),
            host_path: "/RockerBogie/RockerL".into(),
            name: "Wheel_FL".into(),
            asset: "components/mobility/wheel.usda".into(),
            placement: [-0.25, -0.4, -1.2],
            rotate_deg: [0.0, 0.0, 0.0],
            joint,
        }
    }

    #[test]
    fn derives_joint_anchor_from_placement_not_a_second_number() {
        let ops = attach_component_ops(&wheel_spec(AttachJoint::Revolute { axis: Axis::X }));

        // The placement authored on the child…
        let translate = ops.iter().find_map(|op| match op {
            UsdOp::SetTranslate { path, value, .. } if path == "/RockerBogie/RockerL/Wheel_FL" => {
                Some(*value)
            }
            _ => None,
        });
        assert_eq!(translate, Some([-0.25, -0.4, -1.2]));

        // …is the SAME number the joint's localPos0 carries — derived, not retyped.
        let local_pos0 = ops.iter().find_map(|op| match op {
            UsdOp::SetAttribute { name, value, .. } if name == "physics:localPos0" => {
                Some(value.clone())
            }
            _ => None,
        });
        assert_eq!(local_pos0.as_deref(), Some("(-0.25, -0.4, -1.2)"));

        // localPos1 is the part's origin — the convention every shipped joint uses.
        let local_pos1 = ops.iter().find_map(|op| match op {
            UsdOp::SetAttribute { name, value, .. } if name == "physics:localPos1" => {
                Some(value.clone())
            }
            _ => None,
        });
        assert_eq!(local_pos1.as_deref(), Some("(0, 0, 0)"));
    }

    #[test]
    fn relates_host_and_part_as_the_two_bodies() {
        let ops = attach_component_ops(&wheel_spec(AttachJoint::Revolute { axis: Axis::X }));
        let body0 = ops.iter().find_map(|op| match op {
            UsdOp::SetRelationship { name, targets, .. } if name == "physics:body0" => {
                Some(targets.clone())
            }
            _ => None,
        });
        let body1 = ops.iter().find_map(|op| match op {
            UsdOp::SetRelationship { name, targets, .. } if name == "physics:body1" => {
                Some(targets.clone())
            }
            _ => None,
        });
        assert_eq!(body0.as_deref(), Some(&["/RockerBogie/RockerL".to_string()][..]));
        assert_eq!(body1.as_deref(), Some(&["/RockerBogie/RockerL/Wheel_FL".to_string()][..]));
    }

    #[test]
    fn revolute_authors_axis_fixed_does_not() {
        let rev = attach_component_ops(&wheel_spec(AttachJoint::Revolute { axis: Axis::X }));
        assert!(rev.iter().any(|op| matches!(op,
            // Token value is a QUOTED literal — `"X"`, not bare `X` (see the apply test).
            UsdOp::SetAttribute { name, value, .. } if name == "physics:axis" && value == "\"X\"")));

        let fixed = attach_component_ops(&wheel_spec(AttachJoint::Fixed));
        assert!(!fixed.iter().any(|op| matches!(op,
            UsdOp::SetAttribute { name, .. } if name == "physics:axis")));
        // Fixed still relates both bodies and derives both anchors.
        assert_eq!(
            fixed.iter().filter(|op| matches!(op, UsdOp::SetRelationship { .. })).count(),
            2
        );
    }

    #[test]
    fn joint_type_matches_the_requested_kind() {
        let cases = [
            (AttachJoint::Fixed, "PhysicsFixedJoint"),
            (AttachJoint::Revolute { axis: Axis::Y }, "PhysicsRevoluteJoint"),
            (AttachJoint::Prismatic { axis: Axis::Z }, "PhysicsPrismaticJoint"),
        ];
        for (joint, ty) in cases {
            let ops = attach_component_ops(&wheel_spec(joint));
            assert!(ops.iter().any(|op| matches!(op,
                UsdOp::AddPrim { type_name: Some(t), name, .. }
                    if t == ty && name == "Wheel_FL_Joint")));
        }
    }

    #[test]
    fn child_referenced_before_it_is_placed_and_bodies_exist_before_joint() {
        // Ordering is a correctness property: applying these in sequence must never
        // touch a prim that isn't authored yet.
        let ops = attach_component_ops(&wheel_spec(AttachJoint::Fixed));
        let pos = |pred: fn(&UsdOp) -> bool| ops.iter().position(pred).unwrap();
        let add_child = pos(|op| matches!(op, UsdOp::AddPrim { reference: Some(_), .. }));
        let place = pos(|op| matches!(op, UsdOp::SetTranslate { .. }));
        let add_joint = pos(|op| matches!(op, UsdOp::AddPrim { type_name: Some(_), .. }));
        let relate = pos(|op| matches!(op, UsdOp::SetRelationship { .. }));
        assert!(add_child < place, "child exists before it is placed");
        assert!(add_joint < relate, "joint exists before it relates bodies");
        assert!(add_child < relate, "part exists before the joint targets it");
    }

    // ── Mount-frame math (doc 45 §3.1) ──

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-4)
    }

    #[test]
    fn mount_identity_frames_place_at_origin() {
        let (t, r) = resolve_mount_placement(Transform::IDENTITY, Transform::IDENTITY);
        assert!(close(t, [0.0, 0.0, 0.0]) && close(r, [0.0, 0.0, 0.0]));
    }

    #[test]
    fn mount_socket_translation_becomes_placement() {
        // Socket sits at (1,2,3) on the host, plug at the part origin → the part
        // goes to (1,2,3), no rotation.
        let socket = Transform::from_xyz(1.0, 2.0, 3.0);
        let (t, r) = resolve_mount_placement(socket, Transform::IDENTITY);
        assert!(close(t, [1.0, 2.0, 3.0]), "translation carried through: {t:?}");
        assert!(close(r, [0.0, 0.0, 0.0]), "no rotation: {r:?}");
    }

    #[test]
    fn mount_cancels_plug_offset() {
        // Socket at the host origin; the plug sticks out +Z by 1 from the part
        // origin. To land the plug on the socket, the part origin must go to -Z.
        let plug = Transform::from_xyz(0.0, 0.0, 1.0);
        let (t, _r) = resolve_mount_placement(Transform::IDENTITY, plug);
        assert!(close(t, [0.0, 0.0, -1.0]), "plug offset is cancelled: {t:?}");
    }

    #[test]
    fn mount_socket_rotation_becomes_placement_rotation() {
        // A 45° socket rotation about Z (Z is the last XYZ-Euler axis — no gimbal
        // lock) → the part is rotated 45° about Z, at the origin.
        let socket = Transform::from_rotation(Quat::from_rotation_z(45f32.to_radians()));
        let (t, r) = resolve_mount_placement(socket, Transform::IDENTITY);
        assert!(close(t, [0.0, 0.0, 0.0]), "no translation: {t:?}");
        assert!(close(r, [0.0, 0.0, 45.0]), "rotation carried through: {r:?}");
    }

    #[test]
    fn from_mount_authors_rotation_only_when_present() {
        // A rotating socket → the lowering emits a SetRotate; a plain placement
        // (via `new`) does not (the common axis-aligned case stays translate-only).
        let rotated = AttachSpec::from_mount(
            LayerId::root(),
            "/RockerBogie/RockerL",
            "Wheel_FL",
            "components/mobility/wheel.usda",
            AttachJoint::Fixed,
            Transform::from_rotation(Quat::from_rotation_z(30f32.to_radians())),
            Transform::IDENTITY,
        );
        assert!(
            attach_component_ops(&rotated).iter().any(|op| matches!(op, UsdOp::SetRotate { .. })),
            "a rotated mount authors SetRotate"
        );

        let plain = AttachSpec::new(
            LayerId::root(),
            "/RockerBogie/RockerL",
            "Wheel_FL",
            "components/mobility/wheel.usda",
            [1.0, 0.0, 0.0],
            AttachJoint::Fixed,
        );
        assert!(
            !attach_component_ops(&plain).iter().any(|op| matches!(op, UsdOp::SetRotate { .. })),
            "an axis-aligned placement stays translate-only"
        );
    }
}
