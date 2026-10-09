//! Typed partial uploads for persistent RGBA32Float data textures.

use bevy::prelude::*;
use std::collections::{BTreeMap, HashMap};

pub struct FloatTexturePatch {
    pub image: Handle<Image>,
    pub sequence: u64,
    pub size: UVec2,
    pub texels: BTreeMap<usize, [f32; 4]>,
}

#[derive(Clone, Default)]
pub struct FloatTextureUploadStats {
    pub batches: u64,
    pub sequence: u64,
    pub bytes: u64,
    pub last_patch_bytes: usize,
}

/// Producers submit completed batches; the render adapter drains them during
/// extraction. Coalescing retains the latest value at each dirty address.
#[derive(Resource, Default)]
pub struct FloatTextureUpdates {
    pub patches: HashMap<AssetId<Image>, FloatTexturePatch>,
    pub retired: Vec<AssetId<Image>>,
    pub uploaded: HashMap<AssetId<Image>, FloatTextureUploadStats>,
    next_sequence: u64,
}

impl FloatTextureUpdates {
    pub fn reserve_sequence(&mut self) -> u64 {
        self.next_sequence += 1;
        self.next_sequence
    }
    pub fn submit(&mut self, patch: FloatTexturePatch) {
        if patch.texels.is_empty() {
            return;
        }
        let id = patch.image.id();
        if let Some(previous) = self.patches.get_mut(&id)
            && previous.size == patch.size
        {
            previous.sequence = patch.sequence;
            previous.texels.extend(patch.texels);
        } else {
            self.patches.insert(id, patch);
        }
    }

    pub fn retire(&mut self, id: AssetId<Image>) {
        self.patches.remove(&id);
        self.uploaded.remove(&id);
        self.retired.push(id);
    }
}
