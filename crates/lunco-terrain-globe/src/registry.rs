use bevy::prelude::*;
use std::sync::Arc;

/// Registry of custom terrain maps/heightmaps that can be applied to bodies.
#[derive(Resource, Default, Clone)]
pub struct TerrainMapRegistry {
    pub maps: Arc<Vec<CustomMap>>,
}

/// A custom terrain map definition.
#[derive(Clone)]
pub struct CustomMap {
    pub body_entity: Entity,
    pub height_data: Option<Vec<f32>>,
}
