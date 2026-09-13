//! Runtime-owned diagnostic visual leases.
//!
//! Diagnostic geometry is deliberately not authored into USD and is not a
//! second physics or camera model.  This module owns the short-lived command
//! handles and reads the already-projected camera/Avian state for the existing
//! immediate-mode gizmo path.

use avian3d::prelude::{Collider, ColliderOf};
use bevy::camera::{Camera, Projection};
use bevy::prelude::*;
use lunco_api::registry::ApiEntityRegistry;
use lunco_core::{
    on_command, register_commands, Ack, Command, GlobalEntityId, OpId, SceneMountState,
    SceneViewport,
};
use lunco_render::SceneCamera;
use lunco_usd_bevy_scene::UsdSceneRoot;
use serde_json::json;
use std::collections::HashMap;

const MAX_HIERARCHY_WALK: usize = 1024;
const CAMERA_COLOR: Color = Color::srgb(0.2, 0.9, 1.0);
const COLLIDER_COLOR: Color = Color::srgb(1.0, 0.75, 0.15);

/// The diagnostic kinds that share the lease store.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DiagnosticVisualKind {
    Camera,
    Collider,
    PhysicsArrows,
    Joints,
    WheelForces,
    PhysicsMass,
    PhysicsForces,
    PhysicsFrames,
}

impl DiagnosticVisualKind {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "camera" => Some(Self::Camera),
            "collider" => Some(Self::Collider),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Camera => "camera",
            Self::Collider => "collider",
            Self::PhysicsArrows => "physics_arrows",
            Self::Joints => "joints",
            Self::WheelForces => "wheel_forces",
            Self::PhysicsMass => "physics_mass",
            Self::PhysicsForces => "physics_forces",
            Self::PhysicsFrames => "physics_frames",
        }
    }

    fn is_builtin(self) -> bool {
        !matches!(self, Self::Camera | Self::Collider)
    }
}

/// One opaque runtime handle.  The ECS entity is never returned to Rhai or
/// the HTTP API as the handle; the command bridge resolves explicit entity
/// targets from their stable `GlobalEntityId` before this module sees them.
#[derive(Clone, Debug)]
struct DiagnosticVisualLease {
    id: u64,
    target: Option<Entity>,
    kind: DiagnosticVisualKind,
    policy: String,
    generation: u64,
    root: Option<Entity>,
    revision: u64,
}

/// The sole runtime control store for temporary diagnostics.
#[derive(Resource, Debug, Default, Clone)]
pub struct DiagnosticVisualStore {
    leases: HashMap<u64, DiagnosticVisualLease>,
    next_id: u64,
    generation: u64,
    last_error: Option<String>,
}

impl DiagnosticVisualStore {
    fn next_id(&mut self) -> u64 {
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.next_id
    }

    fn set_error(&mut self, message: impl Into<String>) {
        self.last_error = Some(message.into());
    }

    fn acquire(
        &mut self,
        target: Entity,
        kind: DiagnosticVisualKind,
        policy: String,
        root: Entity,
    ) -> (u64, bool) {
        if let Some(existing) = self.leases.values().find(|lease| {
            lease.target == Some(target)
                && lease.kind == kind
                && lease.policy == policy
                && lease.generation == self.generation
                && lease.root == Some(root)
        }) {
            return (existing.id, false);
        }
        let id = self.next_id();
        self.leases.insert(
            id,
            DiagnosticVisualLease {
                id,
                target: Some(target),
                kind,
                policy,
                generation: self.generation,
                root: Some(root),
                revision: 0,
            },
        );
        (id, true)
    }

    fn update(
        &mut self,
        id: u64,
        target: Entity,
        kind: DiagnosticVisualKind,
        policy: String,
        root: Entity,
    ) -> Result<bool, String> {
        let Some(lease) = self.leases.get_mut(&id) else {
            return Err(format!(
                "stale_handle: diagnostic lease {id} does not exist"
            ));
        };
        if lease.generation != self.generation {
            return Err(format!(
                "stale_handle: diagnostic lease {id} belongs to an old mount"
            ));
        }
        if lease.root != Some(root) {
            return Err(format!(
                "cross_twin: diagnostic lease {id} targets another scene mount"
            ));
        }
        let changed = lease.target != Some(target) || lease.kind != kind || lease.policy != policy;
        if changed {
            lease.target = Some(target);
            lease.kind = kind;
            lease.policy = policy;
            lease.revision = lease.revision.wrapping_add(1);
        }
        Ok(changed)
    }

    fn release(&mut self, id: u64) -> Result<(), String> {
        let Some(lease) = self.leases.get(&id) else {
            return Err(format!(
                "stale_handle: diagnostic lease {id} does not exist"
            ));
        };
        if lease.generation != self.generation {
            return Err(format!(
                "stale_handle: diagnostic lease {id} belongs to an old mount"
            ));
        }
        self.leases.remove(&id);
        Ok(())
    }

    /// Toggle one of the legacy editor diagnostics through the same store.
    /// These leases are scoped to the current mounted scene and are cleared at
    /// `SceneTeardown`; no old command/resource registry remains.
    pub fn set_builtin(&mut self, kind: DiagnosticVisualKind, enabled: bool, root: Option<Entity>) {
        debug_assert!(kind.is_builtin());
        let existing = self
            .leases
            .iter()
            .find(|(_, lease)| lease.target.is_none() && lease.kind == kind)
            .map(|(id, _)| *id);
        match (enabled, existing) {
            (true, None) => {
                let id = self.next_id();
                self.leases.insert(
                    id,
                    DiagnosticVisualLease {
                        id,
                        target: None,
                        kind,
                        policy: "scene".to_string(),
                        generation: self.generation,
                        root,
                        revision: 0,
                    },
                );
            }
            (false, Some(id)) => {
                self.leases.remove(&id);
            }
            _ => {}
        }
    }

    pub fn builtin_enabled(&self, kind: DiagnosticVisualKind) -> bool {
        self.leases.values().any(|lease| {
            lease.target.is_none() && lease.kind == kind && lease.generation == self.generation
        })
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn reset_for_teardown(&mut self) {
        self.leases.clear();
        self.generation = self.generation.wrapping_add(1);
        self.last_error = None;
    }

    fn json(&self, entities: &ApiEntityRegistry) -> serde_json::Value {
        let mut leases: Vec<_> = self
            .leases
            .values()
            .map(|lease| {
                json!({
                    "lease_id": lease.id,
                    "target": lease.target.and_then(|entity| entities.api_id_for(entity).map(|id| id.get())),
                    "kind": lease.kind.as_str(),
                    "policy": lease.policy,
                    "generation": lease.generation,
                    "root": lease.root.and_then(|entity| entities.api_id_for(entity).map(|id| id.get())),
                    "revision": lease.revision,
                })
            })
            .collect();
        leases.sort_by_key(|lease| lease["lease_id"].as_u64().unwrap_or_default());
        json!({
            "generation": self.generation,
            "count": leases.len(),
            "leases": leases,
            "last_error": self.last_error,
        })
    }
}

/// Acquire one explicit camera or collider diagnostic.
#[Command]
pub struct AcquireDiagnosticVisual {
    /// Stable entity address.  The API bridge accepts the entity's `api_id`.
    pub target: Entity,
    /// `camera` or `collider`.
    pub kind: String,
    /// Presentation policy.  The initial implementation accepts `default`.
    #[serde(default)]
    #[reflect(default)]
    pub policy: String,
}

/// Replace the target/kind/policy of one lease without creating a second one.
#[Command]
pub struct UpdateDiagnosticVisual {
    /// Opaque handle returned by `AcquireDiagnosticVisual`.
    pub lease: u64,
    /// Optional replacement target.
    #[serde(default)]
    #[reflect(default)]
    pub target: Option<GlobalEntityId>,
    /// Optional replacement kind.
    #[serde(default)]
    #[reflect(default)]
    pub kind: Option<String>,
    /// Optional replacement policy.
    #[serde(default)]
    #[reflect(default)]
    pub policy: Option<String>,
}

/// Release one opaque diagnostic lease.
#[Command]
pub struct ReleaseDiagnosticVisual {
    /// Opaque handle returned by `AcquireDiagnosticVisual`.
    pub lease: u64,
}

fn ack(id: u64, changed: bool, kind: DiagnosticVisualKind, target: u64) -> Ack {
    Ack::with_data(
        OpId::new(),
        json!({
            "lease_id": id,
            "changed": changed,
            "kind": kind.as_str(),
            "target": target,
        }),
    )
}

fn scene_root(
    entity: Entity,
    q_parent: &Query<&ChildOf>,
    q_roots: &Query<(), With<UsdSceneRoot>>,
) -> Option<Entity> {
    let mut current = entity;
    for _ in 0..MAX_HIERARCHY_WALK {
        if q_roots.contains(current) {
            return Some(current);
        }
        let Ok(parent) = q_parent.get(current) else {
            return None;
        };
        current = parent.parent();
    }
    None
}

fn validate_target(
    target: Entity,
    kind: DiagnosticVisualKind,
    policy: &str,
    mount: &SceneMountState,
    viewport: &SceneViewport,
    q_parent: &Query<&ChildOf>,
    q_roots: &Query<(), With<UsdSceneRoot>>,
    q_camera: &Query<(&SceneCamera, Option<&Projection>, Option<&Camera>)>,
    q_collider: &Query<(Entity, Option<&ColliderOf>), With<Collider>>,
) -> Result<Entity, String> {
    if !policy.is_empty() && policy != "default" {
        return Err(format!("unsupported_policy: {policy}"));
    }
    let Some(root) = scene_root(target, q_parent, q_roots) else {
        return Err("invalid_target: target is not inside a USD scene mount".to_string());
    };
    if !mount.contains_root(root) {
        return Err("stale_target: target belongs to an invalidated scene mount".to_string());
    }
    match kind {
        DiagnosticVisualKind::Camera => {
            let Ok((_intent, projection, render_camera)) = q_camera.get(target) else {
                return Err("invalid_target: target is not a SceneCamera".to_string());
            };
            if projection.is_none() || render_camera.is_none() || !viewport.visible {
                return Err(
                    "render_unavailable: camera projection or visible viewport is missing"
                        .to_string(),
                );
            }
        }
        DiagnosticVisualKind::Collider => {
            let has_collider = q_collider.iter().any(|(entity, owner)| {
                entity == target || owner.is_some_and(|owner| owner.body == target)
            });
            if !has_collider {
                return Err("invalid_target: target has no runtime Avian collider".to_string());
            }
        }
        _ => {
            return Err("unsupported_kind: only camera and collider leases are public".to_string());
        }
    }
    Ok(root)
}

#[on_command(AcquireDiagnosticVisual)]
fn on_acquire_diagnostic_visual(
    trigger: On<AcquireDiagnosticVisual>,
    mut store: ResMut<DiagnosticVisualStore>,
    mount: Res<SceneMountState>,
    viewport: Res<SceneViewport>,
    q_parent: Query<&ChildOf>,
    q_roots: Query<(), With<UsdSceneRoot>>,
    q_camera: Query<(&SceneCamera, Option<&Projection>, Option<&Camera>)>,
    q_collider: Query<(Entity, Option<&ColliderOf>), With<Collider>>,
    entities: Res<ApiEntityRegistry>,
) -> Result<Ack, String> {
    let cmd = trigger.event();
    if cmd.target == Entity::PLACEHOLDER {
        return Err("invalid_target: target is required".to_string());
    }
    let kind = DiagnosticVisualKind::parse(&cmd.kind)
        .ok_or_else(|| format!("unsupported_kind: {}", cmd.kind))?;
    let root = validate_target(
        cmd.target,
        kind,
        &cmd.policy,
        &mount,
        &viewport,
        &q_parent,
        &q_roots,
        &q_camera,
        &q_collider,
    )?;
    let policy = if cmd.policy.is_empty() {
        "default".to_string()
    } else {
        cmd.policy.clone()
    };
    let target_id = entities
        .api_id_for(cmd.target)
        .ok_or_else(|| "identity_unavailable: target has no stable API id".to_string())?
        .get();
    let (id, changed) = store.acquire(cmd.target, kind, policy, root);
    Ok(ack(id, changed, kind, target_id))
}

#[on_command(UpdateDiagnosticVisual)]
fn on_update_diagnostic_visual(
    trigger: On<UpdateDiagnosticVisual>,
    mut store: ResMut<DiagnosticVisualStore>,
    mount: Res<SceneMountState>,
    viewport: Res<SceneViewport>,
    q_parent: Query<&ChildOf>,
    q_roots: Query<(), With<UsdSceneRoot>>,
    q_camera: Query<(&SceneCamera, Option<&Projection>, Option<&Camera>)>,
    q_collider: Query<(Entity, Option<&ColliderOf>), With<Collider>>,
    entities: Res<ApiEntityRegistry>,
) -> Result<Ack, String> {
    let cmd = trigger.event();
    let Some(existing) = store.leases.get(&cmd.lease).cloned() else {
        return Err(format!(
            "stale_handle: diagnostic lease {} does not exist",
            cmd.lease
        ));
    };
    let Some(old_target) = existing.target else {
        return Err("invalid_handle: built-in diagnostics cannot be updated".to_string());
    };
    let target = match cmd.target {
        Some(target) => entities
            .resolve(&target)
            .ok_or_else(|| "invalid_target: target API id is not live".to_string())?,
        None => old_target,
    };
    let kind = match cmd.kind.as_deref() {
        Some(value) => DiagnosticVisualKind::parse(value)
            .ok_or_else(|| format!("unsupported_kind: {value}"))?,
        None => existing.kind,
    };
    let policy = cmd
        .policy
        .clone()
        .unwrap_or_else(|| existing.policy.clone());
    let root = validate_target(
        target,
        kind,
        &policy,
        &mount,
        &viewport,
        &q_parent,
        &q_roots,
        &q_camera,
        &q_collider,
    )?;
    let target_id = entities
        .api_id_for(target)
        .ok_or_else(|| "identity_unavailable: target has no stable API id".to_string())?
        .get();
    let changed = store.update(cmd.lease, target, kind, policy, root)?;
    Ok(ack(cmd.lease, changed, kind, target_id))
}

#[on_command(ReleaseDiagnosticVisual)]
fn on_release_diagnostic_visual(
    trigger: On<ReleaseDiagnosticVisual>,
    mut store: ResMut<DiagnosticVisualStore>,
) -> Result<Ack, String> {
    let cmd = trigger.event();
    store.release(cmd.lease)?;
    Ok(Ack::new(OpId::new()))
}

register_commands!(
    on_acquire_diagnostic_visual,
    on_update_diagnostic_visual,
    on_release_diagnostic_visual,
);

/// Clear all leases before scene entities are deferred-despawned.
pub fn reset_diagnostic_visuals(mut store: ResMut<DiagnosticVisualStore>) {
    store.reset_for_teardown();
}

/// Revoke explicit leases whose target or mount disappeared.  This runs every
/// frame but performs only bounded hash-map work for the active lease count.
pub fn revoke_invalid_leases(
    mut store: ResMut<DiagnosticVisualStore>,
    q_entities: Query<Entity>,
    q_parent: Query<&ChildOf>,
    q_roots: Query<(), With<UsdSceneRoot>>,
    mount: Res<SceneMountState>,
) {
    let stale: Vec<_> = store
        .leases
        .values()
        .filter_map(|lease| {
            let Some(target) = lease.target else {
                if let Some(root) = lease.root {
                    return (!mount.contains_root(root)).then_some(lease.id);
                }
                return None;
            };
            let invalid = q_entities.get(target).is_err()
                || scene_root(target, &q_parent, &q_roots) != lease.root
                || lease.generation != store.generation;
            invalid.then_some(lease.id)
        })
        .collect();
    if let Some(id) = stale.first() {
        store.set_error(format!("stale_handle: diagnostic lease {id} was revoked"));
    }
    for id in stale {
        store.leases.remove(&id);
    }
}

/// Read-only API state for authored scenarios and runtime acceptance checks.
pub struct DiagnosticVisualsQueryProvider;

impl lunco_api::ApiQueryProvider for DiagnosticVisualsQueryProvider {
    fn name(&self) -> &'static str {
        "DiagnosticVisuals"
    }

    fn execute(&self, world: &World, _params: &serde_json::Value) -> lunco_api::ApiResponse {
        let Some(store) = world.get_resource::<DiagnosticVisualStore>() else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "DiagnosticVisuals: store is unavailable",
            );
        };
        let Some(entities) = world.get_resource::<ApiEntityRegistry>() else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "DiagnosticVisuals: entity registry is unavailable",
            );
        };
        lunco_api::ApiResponse::ok(store.json(entities))
    }
}

fn camera_corners(projection: &Projection) -> Option<[Vec3; 8]> {
    let (near, far) = match projection {
        Projection::Perspective(projection) => (projection.near, projection.far),
        Projection::Orthographic(projection) => (projection.near, projection.far),
        Projection::Custom(_) => return None,
    };
    Some(
        projection
            .get_frustum_corners(near, far)
            .map(|corner| corner.into()),
    )
}

fn draw_camera(gizmos: &mut Gizmos, transform: &GlobalTransform, projection: &Projection) -> bool {
    let Some(corners) = camera_corners(projection) else {
        return false;
    };
    let corners = corners.map(|corner| transform.transform_point(corner));
    let origin = transform.translation();
    gizmos.line(
        origin,
        transform.transform_point(Vec3::new(0.0, 0.0, -0.5)),
        CAMERA_COLOR,
    );
    gizmos.line(
        origin,
        transform.transform_point(Vec3::X * 0.35),
        Color::srgb(1.0, 0.2, 0.2),
    );
    gizmos.line(
        origin,
        transform.transform_point(Vec3::Y * 0.35),
        Color::srgb(0.2, 1.0, 0.2),
    );
    gizmos.line(
        origin,
        transform.transform_point(Vec3::Z * 0.35),
        Color::srgb(0.2, 0.4, 1.0),
    );
    for start in [0, 4] {
        for i in start..start + 4 {
            gizmos.line(
                corners[i],
                corners[start + (i - start + 1) % 4],
                CAMERA_COLOR,
            );
        }
    }
    for i in 0..4 {
        gizmos.line(corners[i], corners[i + 4], CAMERA_COLOR);
    }
    true
}

fn pose_to_isometry(pose: &avian3d::parry::math::Pose) -> Isometry3d {
    Isometry3d::new(
        Vec3::new(
            pose.translation.x as f32,
            pose.translation.y as f32,
            pose.translation.z as f32,
        ),
        Quat::from_xyzw(
            pose.rotation.x as f32,
            pose.rotation.y as f32,
            pose.rotation.z as f32,
            pose.rotation.w as f32,
        ),
    )
}

fn draw_collider_shape(
    gizmos: &mut Gizmos,
    shape: &avian3d::parry::shape::SharedShape,
    transform: Isometry3d,
) -> bool {
    use avian3d::parry::shape::TypedShape;
    match shape.as_typed_shape() {
        TypedShape::Ball(ball) => {
            gizmos.primitive_3d(&Sphere::new(ball.radius as f32), transform, COLLIDER_COLOR);
            true
        }
        TypedShape::Cuboid(cuboid) => {
            gizmos.primitive_3d(
                &Cuboid {
                    half_size: Vec3::new(
                        cuboid.half_extents.x as f32,
                        cuboid.half_extents.y as f32,
                        cuboid.half_extents.z as f32,
                    ),
                },
                transform,
                COLLIDER_COLOR,
            );
            true
        }
        TypedShape::Capsule(capsule) => {
            let a = Vec3::new(
                capsule.segment.a.x as f32,
                capsule.segment.a.y as f32,
                capsule.segment.a.z as f32,
            );
            let b = Vec3::new(
                capsule.segment.b.x as f32,
                capsule.segment.b.y as f32,
                capsule.segment.b.z as f32,
            );
            let axis = b - a;
            let local = Isometry3d::new(
                (a + b) * 0.5,
                Quat::from_rotation_arc(Vec3::Y, axis.normalize_or_zero()),
            );
            gizmos.primitive_3d(
                &Capsule3d::new(capsule.radius as f32, axis.length()),
                transform * local,
                COLLIDER_COLOR,
            );
            true
        }
        TypedShape::Cylinder(cylinder) => {
            gizmos.primitive_3d(
                &Cylinder::new(cylinder.radius as f32, cylinder.half_height as f32 * 2.0),
                transform,
                COLLIDER_COLOR,
            );
            true
        }
        TypedShape::Cone(cone) => {
            gizmos.primitive_3d(
                &Cone::new(cone.radius as f32, cone.half_height as f32 * 2.0),
                transform,
                COLLIDER_COLOR,
            );
            true
        }
        TypedShape::Compound(compound) => compound.shapes().iter().all(|(pose, child)| {
            draw_collider_shape(gizmos, child, transform * pose_to_isometry(pose))
        }),
        _ => false,
    }
}

fn draw_collider(
    gizmos: &mut Gizmos,
    target: Entity,
    q_colliders: &Query<(Entity, &Collider, Option<&ColliderOf>, &GlobalTransform)>,
) -> bool {
    let mut found = false;
    let mut supported = true;
    for (entity, collider, owner, transform) in q_colliders.iter() {
        if entity != target && owner.is_none_or(|owner| owner.body != target) {
            continue;
        }
        found = true;
        supported &= draw_collider_shape(
            gizmos,
            collider.shape_scaled(),
            Isometry3d::new(transform.translation(), transform.rotation()),
        );
    }
    found && supported
}

/// Draw explicit camera/collider leases through Bevy's existing Gizmos pass.
pub fn draw_diagnostic_visuals(
    mut gizmos: Gizmos,
    store: Res<DiagnosticVisualStore>,
    viewport: Res<SceneViewport>,
    q_camera: Query<(&GlobalTransform, &Projection, Option<&Camera>), With<SceneCamera>>,
    q_colliders: Query<(Entity, &Collider, Option<&ColliderOf>, &GlobalTransform)>,
) {
    if !viewport.visible {
        return;
    }
    for lease in store.leases.values() {
        let Some(target) = lease.target else {
            continue;
        };
        match lease.kind {
            DiagnosticVisualKind::Camera => {
                let Ok((transform, projection, render_camera)) = q_camera.get(target) else {
                    continue;
                };
                if render_camera.is_none() {
                    continue;
                }
                let _ = draw_camera(&mut gizmos, transform, projection);
            }
            DiagnosticVisualKind::Collider => {
                let _ = draw_collider(&mut gizmos, target, &q_colliders);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_is_idempotent_for_same_target_kind_and_policy() {
        let mut store = DiagnosticVisualStore::default();
        let target = Entity::from_raw_u32(1).unwrap();
        let root = Entity::from_raw_u32(2).unwrap();
        let first = store.acquire(target, DiagnosticVisualKind::Camera, "default".into(), root);
        let second = store.acquire(target, DiagnosticVisualKind::Camera, "default".into(), root);
        assert_eq!(first.0, second.0);
        assert!(first.1);
        assert!(!second.1);
        assert_eq!(store.leases.len(), 1);
    }

    #[test]
    fn teardown_makes_old_handles_stale() {
        let mut store = DiagnosticVisualStore::default();
        let target = Entity::from_raw_u32(1).unwrap();
        let root = Entity::from_raw_u32(2).unwrap();
        let (id, _) = store.acquire(
            target,
            DiagnosticVisualKind::Collider,
            "default".into(),
            root,
        );
        store.reset_for_teardown();
        assert!(store.release(id).is_err());
        assert_eq!(store.generation(), 1);
    }

    #[test]
    fn update_only_increments_revision_when_spec_changes() {
        let mut store = DiagnosticVisualStore::default();
        let target = Entity::from_raw_u32(1).unwrap();
        let replacement = Entity::from_raw_u32(3).unwrap();
        let root = Entity::from_raw_u32(2).unwrap();
        let (id, _) = store.acquire(target, DiagnosticVisualKind::Camera, "default".into(), root);
        assert!(!store
            .update(
                id,
                target,
                DiagnosticVisualKind::Camera,
                "default".into(),
                root
            )
            .unwrap());
        assert!(store
            .update(
                id,
                replacement,
                DiagnosticVisualKind::Camera,
                "default".into(),
                root
            )
            .unwrap());
        assert_eq!(store.leases.get(&id).unwrap().revision, 1);
    }
}
