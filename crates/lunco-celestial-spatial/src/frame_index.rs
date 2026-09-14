//! Runtime association between semantic celestial frames and BigSpace grids.
//!
//! [`lunco_celestial::ReferenceFrame`] describes what a coordinate means. This
//! module owns the scene-specific lookup that associates that meaning with one
//! concrete BigSpace grid. Keeping the lookup here prevents the analytical
//! celestial package from depending on a storage representation.

use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use big_space::prelude::Grid;
use std::collections::{HashMap, HashSet};

use lunco_celestial::ReferenceFrame;

/// Event-maintained map from semantic frames to their one concrete BigSpace
/// grid. Consumers request a frame; they never search hierarchy markers or
/// choose a parent entity themselves.
///
/// Duplicate declarations are deliberately unresolved. Picking the first one
/// would make camera, networking, and trajectory placement depend on archetype
/// order.
#[derive(Resource, Debug, Default)]
pub struct ReferenceFrameIndex {
    grids: HashMap<ReferenceFrame, Entity>,
    ambiguous: HashSet<ReferenceFrame>,
    /// Changes whenever a frame/grid declaration changes. Consumers use this
    /// as an event revision instead of rescanning the hierarchy each frame.
    pub revision: u64,
}

impl ReferenceFrameIndex {
    /// The unique grid for `frame`, or `None` when absent or duplicated.
    pub fn resolve(&self, frame: ReferenceFrame) -> Option<Entity> {
        if self.ambiguous.contains(&frame) {
            None
        } else {
            self.grids.get(&frame).copied()
        }
    }
}

/// Convert a pose between named semantic frames without exposing concrete
/// BigSpace grids to the caller.
pub fn transform_pose_between_reference_frames<F: bevy::ecs::query::QueryFilter>(
    position: DVec3,
    rotation: DQuat,
    source: ReferenceFrame,
    target: ReferenceFrame,
    index: &ReferenceFrameIndex,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&big_space::prelude::CellCoord>, &Transform), F>,
) -> Option<(DVec3, DQuat)> {
    let source_grid = index.resolve(source)?;
    let target_grid = index.resolve(target)?;
    lunco_spatial::coords::transform_pose_between_grids(
        position,
        rotation,
        source_grid,
        target_grid,
        q_parents,
        q_grids,
        q_spatial,
    )
}

/// Rebuild the tiny frame index only when frame/grid structure changes.
pub fn update_reference_frame_index(
    mut index: ResMut<ReferenceFrameIndex>,
    changed: Query<
        (),
        Or<(
            (
                With<Grid>,
                Or<(Added<ReferenceFrame>, Changed<ReferenceFrame>)>,
            ),
            (With<ReferenceFrame>, Added<Grid>),
        )>,
    >,
    all: Query<(Entity, &ReferenceFrame), With<Grid>>,
    mut removed_frames: RemovedComponents<ReferenceFrame>,
    mut removed_grids: RemovedComponents<Grid>,
) {
    let removed_any = removed_frames.read().count() > 0 || removed_grids.read().count() > 0;
    if changed.is_empty() && !removed_any {
        return;
    }

    index.grids.clear();
    index.ambiguous.clear();
    for (entity, frame) in &all {
        if index.grids.insert(*frame, entity).is_some() {
            index.ambiguous.insert(*frame);
        }
    }
    index.revision = index.revision.wrapping_add(1);
    let ReferenceFrameIndex {
        grids, ambiguous, ..
    } = &mut *index;
    grids.retain(|frame, _| !ambiguous.contains(frame));
    for frame in ambiguous.iter() {
        error!(
            "[celestial-spatial] duplicate {:?} grids; semantic frame is unresolved",
            frame
        );
    }
}
