//! F64 ray occlusion against admitted collision geometry. This is an explicit
//! geometry representation: callers select their participating shapes. Terrain
//! consumers use the continuous height oracle separately from streamed colliders.
use avian3d::parry::{
    math::Pose,
    partitioning::{Bvh, BvhBuildStrategy},
    query::Ray,
    shape::SharedShape,
};
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;

/// One immutable shape and pose in the active physics frame.
#[derive(Clone)]
pub struct CollisionOccluder {
    pub entity: Entity,
    pub position: DVec3,
    pub rotation: DQuat,
    pub shape: SharedShape,
}
impl CollisionOccluder {
    fn pose(&self) -> Pose {
        Pose::from_parts(self.position, self.rotation.into())
    }
}
/// Reusable double-precision acceleration for explicitly selected geometry.
#[derive(Default)]
pub struct CollisionOcclusionIndex {
    shapes: Vec<CollisionOccluder>,
    tree: Bvh,
    revision: u64,
}
impl CollisionOcclusionIndex {
    /// Replace the snapshot only when a participating shape or exact pose changed.
    pub fn update(&mut self, mut shapes: Vec<CollisionOccluder>) -> Result<(), String> {
        shapes.sort_by_key(|s| s.entity);
        if shapes
            .iter()
            .any(|s| !s.position.is_finite() || !s.rotation.is_finite())
        {
            return Err("occluder has an invalid canonical pose".into());
        }
        if self.shapes.len() == shapes.len()
            && self.shapes.iter().zip(&shapes).all(|(a, b)| {
                a.entity == b.entity
                    && a.position == b.position
                    && a.rotation == b.rotation
                    && std::sync::Arc::ptr_eq(&a.shape.0, &b.shape.0)
            })
        {
            return Ok(());
        }
        let bounds: Vec<_> = shapes
            .iter()
            .map(|s| s.shape.compute_aabb(&s.pose()))
            .collect();
        if bounds
            .iter()
            .any(|b| !b.mins.is_finite() || !b.maxs.is_finite())
        {
            return Err("occluder has invalid geometric bounds".into());
        }
        self.tree = Bvh::from_leaves(BvhBuildStrategy::default(), &bounds);
        self.shapes = shapes;
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// Ray/shape math and broad-phase bounds remain f64 at this boundary.
    pub fn occludes(
        &self,
        origin: DVec3,
        direction: DVec3,
        participants: &[Entity],
    ) -> Result<bool, String> {
        if !origin.is_finite()
            || !direction.is_finite()
            || (direction.length_squared() - 1.0).abs() > 1.0e-8
        {
            return Err("occlusion requires a finite unit ray".into());
        }
        let ray = Ray::new(origin, direction);
        Ok(self
            .tree
            .cast_ray(&ray, f64::MAX, |index, max| {
                let s = &self.shapes[index as usize];
                if !participants.contains(&s.entity) {
                    return None;
                }
                s.shape.cast_ray(&s.pose(), &ray, max, true)
            })
            .is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn indexed_occlusion_tracks_thin_moving_shapes_in_f64() {
        let mut index = CollisionOcclusionIndex::default();
        let mut shape = CollisionOccluder {
            entity: Entity::PLACEHOLDER,
            position: DVec3::new(1.0e9 + 1.0, 0.0, 0.0),
            rotation: DQuat::IDENTITY,
            shape: SharedShape::cuboid(0.00001, 1.0, 1.0),
        };
        index.update(vec![shape.clone()]).unwrap();
        assert!(
            index
                .occludes(
                    DVec3::new(1.0e9, 0.0, 0.0),
                    DVec3::X,
                    &[Entity::PLACEHOLDER]
                )
                .unwrap()
        );
        let revision = index.revision();
        index.update(vec![shape.clone()]).unwrap();
        assert_eq!(revision, index.revision());
        shape.position.y = 3.0;
        index.update(vec![shape.clone()]).unwrap();
        assert!(
            !index
                .occludes(
                    DVec3::new(1.0e9, 0.0, 0.0),
                    DVec3::X,
                    &[Entity::PLACEHOLDER]
                )
                .unwrap()
        );
        shape.position.x = f64::NAN;
        assert!(index.update(vec![shape]).is_err());
    }
}
