//! Built-in **shader** layer: the surface material IS a layer. Picks which shader the
//! streamed LOD tiles draw with by setting the terrain's [`TerrainShaderMode`].
//! (Per-material parameter authoring can extend this later.)

use std::sync::Arc;

use bevy::prelude::*;

use crate::stream_viz::TerrainShaderMode;

use super::{LayerAttrSource, TerrainLayer};

struct ShaderLayer {
    mode: TerrainShaderMode,
}

impl TerrainLayer for ShaderLayer {
    fn id(&self) -> &'static str {
        "shader"
    }
    fn configure(&self, terrain: Entity, commands: &mut Commands) {
        commands.entity(terrain).insert(self.mode);
    }
}

/// Parse a `lunco:layer = "shader"` prim: `mode` = `lit` (regolith, default) |
/// `plain` (flat grey) | `debug` (per-LOD colours).
pub(super) fn parse_shader_layer(a: &dyn LayerAttrSource) -> Option<Arc<dyn TerrainLayer>> {
    let mode = match a.get_string("mode").as_deref() {
        Some("debug") | Some("debuglod") | Some("debug_lod") => TerrainShaderMode::DebugLod,
        Some("plain") => TerrainShaderMode::Plain,
        _ => TerrainShaderMode::Lit,
    };
    Some(Arc::new(ShaderLayer { mode }))
}
