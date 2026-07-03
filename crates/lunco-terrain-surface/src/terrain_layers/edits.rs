//! Built-in **edit** layer — dynamic terrain modifications from tools.
//!
//! A dig, a raised berm, a flattened landing pad — each is a
//! [`HeightModifier`](lunco_terrain_core::HeightModifier) carried as one more layer
//! on the [`TerrainLayerStack`](super::TerrainLayerStack). Appending an edit layer and
//! marking the stack `Changed` triggers the existing off-thread re-stamp, so the edit
//! shows up in the tiles AND the collider with no bespoke rebuild path — the same
//! mechanism the crater/rock layers ride. Edits are non-destructive layers: pop the
//! layer to undo; reorder to change precedence.
//!
//! [`EditLayer`] applies its modifier to every grid sample via
//! `modifier.apply(x, z, current)`, which is why the modifier signature threads the
//! current height — an additive brush *adds* to it, a flatten *pulls it toward* a
//! target. The visual tiles and the heightfield collider both read the one stamped
//! grid, so the edit the rover drives is the edit you see.

use std::sync::Arc;

use bevy::log::info;
use lunco_obstacle_field::field::HeightGrid;
use lunco_terrain_core::{BrushModifier, FlattenModifier, HeightModifier};

use super::TerrainLayer;

/// A single terrain edit: any [`HeightModifier`] stamped into the working grid. The
/// `kind` is the layer's logging tag (`"edit:dig"`, `"edit:flatten"`, …).
pub struct EditLayer {
    kind: &'static str,
    modifier: Arc<dyn HeightModifier>,
}

impl EditLayer {
    /// Wrap any modifier as an edit layer under a static `kind` tag.
    pub fn new(kind: &'static str, modifier: Arc<dyn HeightModifier>) -> Arc<dyn TerrainLayer> {
        Arc::new(EditLayer { kind, modifier })
    }
}

impl TerrainLayer for EditLayer {
    fn id(&self) -> &'static str {
        self.kind
    }

    fn stamp(&self, grid: &mut HeightGrid) {
        // Apply the modifier at every sample: `apply` folds the current height, so
        // additive edits add and replacing edits (flatten) blend from it.
        let res = grid.res;
        let s = grid.spacing();
        let origin = -grid.half_extent;
        for iz in 0..res {
            let z = (origin + iz as f32 * s) as f64;
            for ix in 0..res {
                let x = (origin + ix as f32 * s) as f64;
                let i = iz * res + ix;
                grid.heights[i] = self.modifier.apply(x, z, grid.heights[i]);
            }
        }
        info!("[terrain-layer/{}] stamped edit (±{:.0} m)", self.kind, grid.half_extent);
    }
}

/// A **dig / raise** edit: a smooth radial brush. `amplitude` metres at the centre,
/// falling to zero at `radius`; negative digs, positive raises.
pub fn dig_layer(center: [f64; 2], radius: f64, amplitude: f64) -> Arc<dyn TerrainLayer> {
    EditLayer::new("edit:brush", Arc::new(BrushModifier::new(center, radius, amplitude)))
}

/// A **flatten** edit: level the surface toward `target_y` within `radius`, blending
/// back to the terrain at the edge. The "level a landing pad" tool.
pub fn flatten_layer(center: [f64; 2], radius: f64, target_y: f64) -> Arc<dyn TerrainLayer> {
    EditLayer::new("edit:flatten", Arc::new(FlattenModifier::new(center, radius, target_y)))
}
