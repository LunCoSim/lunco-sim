//! Attach a component asset to a host body as a jointed child — the op-lowering
//! behind "build from parts" (`docs/architecture/48-object-builder.md` §3.1).
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
//! Socket/plug frame matching (`lunco:mount:*`) computes a placement and calls
//! the same lowering. The lowering also persists the selected socket's
//! `lunco:mount:part` relationship and the child's exact generated-joint
//! relationship, so the composed stage retains the mating identity instead of
//! having to infer it from transforms or names.
//!
//! The lowering is a **pure function** returning `Vec<UsdOp>`; the command in
//! `commands.rs` applies the complete sequence through one journal change set.
//! That keeps the geometry unit-testable with no world, no composition, and no
//! I/O while keeping attach undo atomic.

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
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Reflect, serde::Serialize, serde::Deserialize,
)]
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
    /// The host socket consumed by this attach, when the attach came from a
    /// socket/plug match. Direct component attachments leave this unset.
    /// When present, the lowering authors `lunco:mount:part` on this socket
    /// in the same change set as the child and joint.
    pub socket_path: Option<String>,
    /// Absolute path of the **host body** prim the part hangs off — a prim that
    /// is (or will be) a `PhysicsRigidBodyAPI` body. The part becomes its child,
    /// which is how nested jointed bodies are authored (`RockerL` under
    /// `RockerBogie`); each still becomes its own Avian body because it carries
    /// its own rigid-body opinion — the compound-piece rule is for colliders
    /// *without* a body, not for a nested body.
    pub host_path: String,
    /// Leaf name of the new child prim (e.g. `Wheel_FL`).
    pub name: String,
    /// Explicit leaf name of the generated joint prim. This is an identity
    /// supplied by the authoring policy; no reader may derive it from `name`.
    pub joint_name: String,
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

/// Exact authored identity needed to remove one attached component.
///
/// The component path, generated-joint path, and optional occupied socket are
/// supplied by the authoring policy (normally Rhai after a composed query). The
/// lowering only turns that explicit plan into one compound USD edit; it never
/// discovers or derives a related prim by name.
#[derive(Debug, Clone, PartialEq, Eq, Reflect, serde::Serialize, serde::Deserialize)]
pub struct DetachSpec {
    /// Layer receiving the complete detach edit.
    pub edit_target: LayerId,
    /// Root of the component subtree to remove.
    pub component_path: String,
    /// Exact joint prim generated for this component.
    pub joint_path: String,
    /// Occupied socket to clear, when this component was socket-attached.
    pub socket_path: Option<String>,
}

impl Default for DetachSpec {
    fn default() -> Self {
        Self {
            edit_target: LayerId::root(),
            component_path: String::new(),
            joint_path: String::new(),
            socket_path: None,
        }
    }
}

/// Lower an explicit detach plan to one ordered USD change set.
pub fn detach_component_ops(spec: &DetachSpec) -> Vec<UsdOp> {
    let edit_target = spec.edit_target.clone();
    let mut ops = Vec::new();
    if let Some(socket_path) = &spec.socket_path {
        ops.push(UsdOp::SetRelationship {
            edit_target: edit_target.clone(),
            path: socket_path.clone(),
            name: "lunco:mount:part".into(),
            targets: Vec::new(),
        });
    }
    ops.push(UsdOp::RemovePrim {
        edit_target: edit_target.clone(),
        path: spec.joint_path.clone(),
    });
    ops.push(UsdOp::RemovePrim {
        edit_target,
        path: spec.component_path.clone(),
    });
    ops
}

impl Default for AttachSpec {
    // `Reflect`/`#[Command(default)]` need a Default. Like `UsdOp::default`, this
    // is the never-dispatched identity placeholder — real callers always fill it.
    fn default() -> Self {
        Self {
            edit_target: LayerId::root(),
            socket_path: None,
            host_path: String::new(),
            name: String::new(),
            joint_name: String::new(),
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
        joint_name: impl Into<String>,
        asset: impl Into<String>,
        placement: [f64; 3],
        joint: AttachJoint,
    ) -> Self {
        Self {
            edit_target,
            socket_path: None,
            host_path: host_path.into(),
            name: name.into(),
            joint_name: joint_name.into(),
            asset: asset.into(),
            placement,
            rotate_deg: [0.0; 3],
            joint,
        }
    }

    /// Attach `asset` by matching a **plug frame on the part** to a **socket frame
    /// on the host** — the mount abstraction (doc 48 §3.1). Both frames are given
    /// as local [`Transform`]s (read off `xformOp:*` on the socket/plug prims by the
    /// caller); this computes the part placement + rotation so the plug coincides
    /// with the socket, via [`resolve_mount_placement`]. The joint anchor still
    /// derives from the placement, so a bogie reconfiguration is "move the socket".
    ///
    /// Returns an error when the frame composition contains unsupported scale
    /// or non-finite values, because the current USD attach representation is
    /// rigid and cannot author scale or invalid transforms.
    /// Frame *reading* stays with the caller (it needs the composed stage); this
    /// keeps the frame math a pure, unit-tested function of two transforms — a wrong
    /// frame conversion is a physics bug you can only see with the renderer running,
    /// so it is isolated here where it can be checked against hand-computed matrices.
    pub fn from_mount(
        edit_target: LayerId,
        socket_path: impl Into<String>,
        host_path: impl Into<String>,
        name: impl Into<String>,
        joint_name: impl Into<String>,
        asset: impl Into<String>,
        joint: AttachJoint,
        socket: bevy::prelude::Transform,
        plug: bevy::prelude::Transform,
    ) -> Result<Self, String> {
        let (placement, rotate_deg) = resolve_mount_placement(socket, plug)?;
        Ok(Self {
            edit_target,
            socket_path: Some(socket_path.into()),
            host_path: host_path.into(),
            name: name.into(),
            joint_name: joint_name.into(),
            asset: asset.into(),
            placement,
            rotate_deg,
            joint,
        })
    }

    fn child_path(&self) -> String {
        format!("{}/{}", self.host_path.trim_end_matches('/'), self.name)
    }

    fn joint_path(&self) -> String {
        format!(
            "{}/{}",
            self.host_path.trim_end_matches('/'),
            self.joint_name
        )
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
/// placement (`localPos0 = placement`, `localPos1 = origin`). A socket-sourced
/// spec also records the socket occupancy relationship.
///
/// The ops are ordered so each is valid when applied in turn: the child prim
/// exists before it is placed; both bodies exist before the joint relates them.
pub fn attach_component_ops(spec: &AttachSpec) -> Vec<UsdOp> {
    let child = spec.child_path();
    let joint = spec.joint_path();
    let et = spec.edit_target.clone();

    // Keep the ordered plan explicit: the child exists before its metadata and
    // transform, and the joint exists before any relationship targets it.
    let mut ops = vec![UsdOp::AddPrim {
        edit_target: et.clone(),
        parent_path: spec.host_path.clone(),
        name: spec.name.clone(),
        type_name: None,
        reference: Some(spec.asset.clone()),
    }];

    // Persist socket occupancy in the same change set as the child and joint.
    if let Some(socket_path) = &spec.socket_path {
        ops.push(UsdOp::SetRelationship {
            edit_target: et.clone(),
            path: socket_path.clone(),
            name: "lunco:mount:part".into(),
            targets: vec![child.clone()],
        });
    }

    // Detach reads this exact component relation; it never reconstructs a name.
    ops.push(UsdOp::SetApiSchemas {
        edit_target: et.clone(),
        path: child.clone(),
        schemas: vec!["LunCoMountAttachmentAPI".into()],
    });

    // Rotation is optional, but when present it is authored before translation
    // to preserve the existing xform-op order.
    if spec.rotate_deg != [0.0, 0.0, 0.0] {
        ops.push(UsdOp::SetRotate {
            edit_target: et.clone(),
            path: child.clone(),
            value: spec.rotate_deg,
        });
    }

    // This is the ONE authored placement.
    ops.push(UsdOp::SetTranslate {
        edit_target: et.clone(),
        path: child.clone(),
        value: spec.placement,
    });

    // The joint prim, typed by the requested kind.
    ops.push(UsdOp::AddPrim {
        edit_target: et.clone(),
        parent_path: spec.host_path.clone(),
        name: spec.joint_name.clone(),
        type_name: Some(spec.joint_type_name().to_string()),
        reference: None,
    });
    // Relate the two bodies.
    ops.push(UsdOp::SetRelationship {
        edit_target: et.clone(),
        path: joint.clone(),
        name: "physics:body0".into(),
        targets: vec![spec.host_path.clone()],
    });
    ops.push(UsdOp::SetRelationship {
        edit_target: et.clone(),
        path: joint.clone(),
        name: "physics:body1".into(),
        targets: vec![child.clone()],
    });
    // The anchor is derived from the placement, not typed again. `localPos0`
    // is the part's origin in the host frame; `localPos1` is the part's origin.
    ops.push(UsdOp::SetAttribute {
        edit_target: et.clone(),
        path: joint.clone(),
        name: "physics:localPos0".into(),
        type_name: "point3f".into(),
        value: vec3_literal(spec.placement),
    });
    ops.push(UsdOp::SetAttribute {
        edit_target: et.clone(),
        path: joint.clone(),
        name: "physics:localPos1".into(),
        type_name: "point3f".into(),
        value: vec3_literal([0.0, 0.0, 0.0]),
    });

    // The generated joint now exists, so the component can safely point at its
    // exact identity. Keep this relation in the same compound change set as the
    // child, joint, and socket occupancy.
    ops.push(UsdOp::SetRelationship {
        edit_target: et.clone(),
        path: child,
        name: "lunco:mount:attachmentJoint".into(),
        targets: vec![joint.clone()],
    });

    // The moving axis, for the non-fixed joints.
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

/// Re-derive an **already-attached** part's placement + joint anchor from mount
/// frames, without re-referencing it — the *retrofit* half of the mount story
/// (`docs/architecture/48-object-builder.md` §3.1). Where [`attach_component_ops`]
/// adds a new part, this touches only the two things that duplicate today: the
/// part's `xformOp:translate`/`rotateXYZ`, and its joint's `localPos0`. The part
/// prim and its joint already exist on the stage (a wheel under a bogie); "move the
/// socket" then moves both, because the anchor is derived from the same placement,
/// never a second hand-typed number.
///
/// `localPos1` stays the part's origin, the convention every shipped joint uses
/// (`localPos1 = (0,0,0)` throughout `rocker_bogie.usda`). Rotation is authored
/// **unconditionally** — a retrofit must be able to *clear* a stale rotation back
/// to zero, so unlike the attach path it always emits `SetRotate` (even for
/// `[0,0,0]`). Emits no `AddPrim`/`SetRelationship`: the topology is untouched, so
/// this never rebuilds the world (all four ops replay incrementally, §3.3).
///
/// **Apply these four as one transaction** — hand them to
/// [`crate::commands::apply_ops_as_change_set`], which wraps them in a journal
/// change set so the realign is ONE undo unit (H10). Applying them one-by-one
/// journals four independent entries and a single undo peels off one, leaving the
/// part moved but its joint anchor stale (or vice-versa). Same rule as
/// [`attach_component_ops`], whose command handler already routes through it.
pub fn realign_component_ops(
    edit_target: LayerId,
    part_path: impl Into<String>,
    joint_path: impl Into<String>,
    placement: [f64; 3],
    rotate_deg: [f64; 3],
) -> Vec<UsdOp> {
    let part = part_path.into();
    let joint = joint_path.into();
    vec![
        UsdOp::SetTranslate {
            edit_target: edit_target.clone(),
            path: part.clone(),
            value: placement,
        },
        UsdOp::SetRotate {
            edit_target: edit_target.clone(),
            path: part,
            value: rotate_deg,
        },
        UsdOp::SetAttribute {
            edit_target: edit_target.clone(),
            path: joint.clone(),
            name: "physics:localPos0".into(),
            type_name: "point3f".into(),
            value: vec3_literal(placement),
        },
        UsdOp::SetAttribute {
            edit_target,
            path: joint,
            name: "physics:localPos1".into(),
            type_name: "point3f".into(),
            value: vec3_literal([0.0, 0.0, 0.0]),
        },
    ]
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
/// [`attach_component_ops`] authors. Frames whose resulting placement contains
/// non-unit scale or non-finite values are rejected: the current attach
/// representation has no scale operation and must not silently discard invalid
/// state. Pure and total — no stage, no I/O — so the frame conversion is
/// unit-tested against hand-computed matrices.
pub fn resolve_mount_placement(
    socket: bevy::prelude::Transform,
    plug: bevy::prelude::Transform,
) -> Result<([f64; 3], [f64; 3]), String> {
    use bevy::math::EulerRot;
    let p = socket.compute_affine() * plug.compute_affine().inverse();
    let (scale, rot, trans) = p.to_scale_rotation_translation();
    let scale_error = (scale - bevy::prelude::Vec3::ONE).abs().max_element();
    if !scale.x.is_finite() || !scale.y.is_finite() || !scale.z.is_finite() || scale_error > 1.0e-4
    {
        return Err(format!(
            "mount frame placement has unsupported scale {:?}; socket/plug frames must compose to a rigid transform",
            scale
        ));
    }
    if !rot.x.is_finite()
        || !rot.y.is_finite()
        || !rot.z.is_finite()
        || !rot.w.is_finite()
        || !trans.x.is_finite()
        || !trans.y.is_finite()
        || !trans.z.is_finite()
    {
        return Err("mount frame placement is non-finite".into());
    }
    let (rx, ry, rz) = rot.to_euler(EulerRot::XYZ);
    let placement = [trans.x as f64, trans.y as f64, trans.z as f64];
    let rotation = [
        (rx as f64).to_degrees(),
        (ry as f64).to_degrees(),
        (rz as f64).to_degrees(),
    ];
    if placement.iter().any(|value| !value.is_finite())
        || rotation.iter().any(|value| !value.is_finite())
    {
        return Err("mount frame placement is non-finite".into());
    }
    Ok((placement, rotation))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::Quat;
    use bevy::prelude::Transform;

    fn wheel_spec(joint: AttachJoint) -> AttachSpec {
        AttachSpec {
            edit_target: LayerId::root(),
            socket_path: None,
            host_path: "/RockerBogie/RockerL".into(),
            name: "Wheel_FL".into(),
            joint_name: "constraint_47".into(),
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
        assert_eq!(
            body0.as_deref(),
            Some(&["/RockerBogie/RockerL".to_string()][..])
        );
        assert_eq!(
            body1.as_deref(),
            Some(&["/RockerBogie/RockerL/Wheel_FL".to_string()][..])
        );
    }

    #[test]
    fn revolute_authors_axis_fixed_does_not() {
        let rev = attach_component_ops(&wheel_spec(AttachJoint::Revolute { axis: Axis::X }));
        assert!(
            rev.iter().any(|op| matches!(op,
            // Token value is a QUOTED literal — `"X"`, not bare `X` (see the apply test).
            UsdOp::SetAttribute { name, value, .. } if name == "physics:axis" && value == "\"X\""))
        );

        let fixed = attach_component_ops(&wheel_spec(AttachJoint::Fixed));
        assert!(!fixed.iter().any(|op| matches!(op,
            UsdOp::SetAttribute { name, .. } if name == "physics:axis")));
        // Fixed still relates both bodies and derives both anchors.
        assert_eq!(
            fixed
                .iter()
                .filter(|op| matches!(op, UsdOp::SetRelationship { .. }))
                .count(),
            3
        );
    }

    #[test]
    fn joint_type_matches_the_requested_kind() {
        let cases = [
            (AttachJoint::Fixed, "PhysicsFixedJoint"),
            (
                AttachJoint::Revolute { axis: Axis::Y },
                "PhysicsRevoluteJoint",
            ),
            (
                AttachJoint::Prismatic { axis: Axis::Z },
                "PhysicsPrismaticJoint",
            ),
        ];
        for (joint, ty) in cases {
            let ops = attach_component_ops(&wheel_spec(joint));
            assert!(ops.iter().any(|op| matches!(op,
                UsdOp::AddPrim { type_name: Some(t), name, .. }
                    if t == ty && name == "constraint_47")));
        }
    }

    #[test]
    fn child_referenced_before_it_is_placed_and_bodies_exist_before_joint() {
        // Ordering is a correctness property: applying these in sequence must never
        // touch a prim that isn't authored yet.
        let ops = attach_component_ops(&wheel_spec(AttachJoint::Fixed));
        let pos = |pred: fn(&UsdOp) -> bool| ops.iter().position(pred).unwrap();
        let add_child = pos(|op| {
            matches!(
                op,
                UsdOp::AddPrim {
                    reference: Some(_),
                    ..
                }
            )
        });
        let place = pos(|op| matches!(op, UsdOp::SetTranslate { .. }));
        let add_joint = pos(|op| {
            matches!(
                op,
                UsdOp::AddPrim {
                    type_name: Some(_),
                    ..
                }
            )
        });
        let relate = pos(|op| matches!(op, UsdOp::SetRelationship { .. }));
        assert!(add_child < place, "child exists before it is placed");
        assert!(add_joint < relate, "joint exists before it relates bodies");
        assert!(
            add_child < relate,
            "part exists before the joint targets it"
        );
    }

    #[test]
    fn realign_re_authors_placement_and_anchor_without_touching_topology() {
        // The retrofit path: the part and joint already exist, so realign emits ONLY
        // the two duplicated frames — no AddPrim, no SetRelationship (which would
        // rebuild the world). It re-authors translate + rotate on the part and
        // derives localPos0 from the same placement.
        let ops = realign_component_ops(
            LayerId::root(),
            "/RockerBogie/RockerL/Wheel_FL",
            "/RockerBogie/RockerL/constraint_47",
            [0.1, -0.2, 0.3],
            [0.0, 90.0, 0.0],
        );
        assert!(
            !ops.iter()
                .any(|op| matches!(op, UsdOp::AddPrim { .. } | UsdOp::SetRelationship { .. })),
            "retrofit touches no topology"
        );
        // Translate authored on the part…
        assert!(ops.iter().any(|op| matches!(op,
            UsdOp::SetTranslate { path, value, .. }
                if path == "/RockerBogie/RockerL/Wheel_FL" && *value == [0.1, -0.2, 0.3])));
        // …and the SAME number as the joint's localPos0 (derived, not retyped).
        assert!(ops.iter().any(|op| matches!(op,
            UsdOp::SetAttribute { name, value, .. }
                if name == "physics:localPos0" && value == "(0.1, -0.2, 0.3)")));
        assert!(ops.iter().any(|op| matches!(op,
            UsdOp::SetAttribute { name, value, .. }
                if name == "physics:localPos1" && value == "(0, 0, 0)")));
    }

    #[test]
    fn realign_always_authors_rotation_so_it_can_clear_a_stale_one() {
        // Unlike attach (which skips SetRotate for [0,0,0]), realign must be able to
        // reset a part that WAS rotated back to axis-aligned — so it always emits it.
        let ops = realign_component_ops(
            LayerId::root(),
            "/A/Part",
            "/A/constraint_47",
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 0.0],
        );
        assert!(
            ops.iter().any(|op| matches!(op,
                UsdOp::SetRotate { value, .. } if *value == [0.0, 0.0, 0.0])),
            "realign authors rotation unconditionally, even zero"
        );
    }

    // ── Mount-frame math (doc 48 §3.1) ──

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-4)
    }

    #[test]
    fn mount_identity_frames_place_at_origin() {
        let (t, r) = resolve_mount_placement(Transform::IDENTITY, Transform::IDENTITY).unwrap();
        assert!(close(t, [0.0, 0.0, 0.0]) && close(r, [0.0, 0.0, 0.0]));
    }

    #[test]
    fn mount_socket_translation_becomes_placement() {
        // Socket sits at (1,2,3) on the host, plug at the part origin → the part
        // goes to (1,2,3), no rotation.
        let socket = Transform::from_xyz(1.0, 2.0, 3.0);
        let (t, r) = resolve_mount_placement(socket, Transform::IDENTITY).unwrap();
        assert!(
            close(t, [1.0, 2.0, 3.0]),
            "translation carried through: {t:?}"
        );
        assert!(close(r, [0.0, 0.0, 0.0]), "no rotation: {r:?}");
    }

    #[test]
    fn mount_cancels_plug_offset() {
        // Socket at the host origin; the plug sticks out +Z by 1 from the part
        // origin. To land the plug on the socket, the part origin must go to -Z.
        let plug = Transform::from_xyz(0.0, 0.0, 1.0);
        let (t, _r) = resolve_mount_placement(Transform::IDENTITY, plug).unwrap();
        assert!(
            close(t, [0.0, 0.0, -1.0]),
            "plug offset is cancelled: {t:?}"
        );
    }

    #[test]
    fn mount_socket_rotation_becomes_placement_rotation() {
        // A 45° socket rotation about Z (Z is the last XYZ-Euler axis — no gimbal
        // lock) → the part is rotated 45° about Z, at the origin.
        let socket = Transform::from_rotation(Quat::from_rotation_z(45f32.to_radians()));
        let (t, r) = resolve_mount_placement(socket, Transform::IDENTITY).unwrap();
        assert!(close(t, [0.0, 0.0, 0.0]), "no translation: {t:?}");
        assert!(
            close(r, [0.0, 0.0, 45.0]),
            "rotation carried through: {r:?}"
        );
    }

    #[test]
    fn from_mount_authors_rotation_only_when_present() {
        // A rotating socket → the lowering emits a SetRotate; a plain placement
        // (via `new`) does not (the common axis-aligned case stays translate-only).
        let rotated = AttachSpec::from_mount(
            LayerId::root(),
            "/RockerBogie/RockerL/Interfaces/wheel_fl",
            "/RockerBogie/RockerL",
            "Wheel_FL",
            "constraint_47",
            "components/mobility/wheel.usda",
            AttachJoint::Fixed,
            Transform::from_rotation(Quat::from_rotation_z(30f32.to_radians())),
            Transform::IDENTITY,
        )
        .unwrap();
        assert!(
            attach_component_ops(&rotated)
                .iter()
                .any(|op| matches!(op, UsdOp::SetRotate { .. })),
            "a rotated mount authors SetRotate"
        );

        let plain = AttachSpec::new(
            LayerId::root(),
            "/RockerBogie/RockerL",
            "Wheel_FL",
            "constraint_47",
            "components/mobility/wheel.usda",
            [1.0, 0.0, 0.0],
            AttachJoint::Fixed,
        );
        assert!(
            !attach_component_ops(&plain)
                .iter()
                .any(|op| matches!(op, UsdOp::SetRotate { .. })),
            "an axis-aligned placement stays translate-only"
        );
    }

    #[test]
    fn socket_attach_records_occupancy_relationship() {
        let spec = AttachSpec::from_mount(
            LayerId::root(),
            "/Rig/Chassis/Interfaces/wheel_fl",
            "/Rig/Chassis",
            "Wheel",
            "constraint_47",
            "components/mobility/wheel.usda",
            AttachJoint::Fixed,
            Transform::IDENTITY,
            Transform::IDENTITY,
        )
        .unwrap();
        assert!(attach_component_ops(&spec).iter().any(|op| matches!(
            op,
            UsdOp::SetRelationship { path, name, targets, .. }
                if path == "/Rig/Chassis/Interfaces/wheel_fl"
                    && name == "lunco:mount:part"
                    && targets == &["/Rig/Chassis/Wheel".to_string()]
        )));
        assert!(attach_component_ops(&spec).iter().any(|op| matches!(
            op,
            UsdOp::SetRelationship { path, name, targets, .. }
                if path == "/Rig/Chassis/Wheel"
                    && name == "lunco:mount:attachmentJoint"
                    && targets == &["/Rig/Chassis/constraint_47".to_string()]
        )));
    }

    #[test]
    fn detach_lowering_clears_occupancy_before_removing_topology() {
        let ops = detach_component_ops(&DetachSpec {
            edit_target: LayerId::root(),
            component_path: "/Rig/Chassis/Wheel".into(),
            joint_path: "/Rig/Chassis/constraint_47".into(),
            socket_path: Some("/Rig/Chassis/Interfaces/wheel_fl".into()),
        });
        assert!(matches!(
            &ops[0],
            UsdOp::SetRelationship { path, name, targets, .. }
                if path == "/Rig/Chassis/Interfaces/wheel_fl"
                    && name == "lunco:mount:part"
                    && targets.is_empty()
        ));
        assert!(matches!(
            &ops[1],
            UsdOp::RemovePrim { path, .. } if path == "/Rig/Chassis/constraint_47"
        ));
        assert!(matches!(
            &ops[2],
            UsdOp::RemovePrim { path, .. } if path == "/Rig/Chassis/Wheel"
        ));
    }

    #[test]
    fn mount_placement_rejects_non_rigid_result() {
        let error = resolve_mount_placement(
            Transform::from_scale(bevy::prelude::Vec3::new(2.0, 1.0, 1.0)),
            Transform::IDENTITY,
        )
        .expect_err("non-unit placement scale must not be discarded");
        assert!(error.contains("unsupported scale"), "{error}");
    }

    #[test]
    fn mount_placement_rejects_non_finite_result() {
        let error =
            resolve_mount_placement(Transform::from_xyz(f32::NAN, 0.0, 0.0), Transform::IDENTITY)
                .expect_err("non-finite placement must not be authored");
        assert!(error.contains("non-finite"), "{error}");
    }
}
