//! Typed uploads for persistent scalar and RGBA floating-point data textures.

use bevy::prelude::*;
use std::collections::{BTreeMap, HashMap};

pub enum DataTextureTexels {
    Rgba32Float(BTreeMap<usize, [f32; 4]>),
    R8Unorm(Vec<u8>),
}

pub struct DataTexturePatch {
    pub image: Handle<Image>,
    pub sequence: u64,
    pub size: UVec2,
    pub texels: DataTextureTexels,
}

#[derive(Clone, Default)]
pub struct DataTextureUploadStats {
    pub batches: u64,
    pub sequence: u64,
    pub bytes: u64,
    pub last_patch_bytes: usize,
    pub texture_allocations: u64,
}

/// Producers submit completed batches; the render adapter drains them during
/// extraction. Coalescing retains the latest value at each dirty address.
#[derive(Resource, Default)]
pub struct DataTextureUpdates {
    pub patches: HashMap<AssetId<Image>, DataTexturePatch>,
    pub retired: Vec<AssetId<Image>>,
    pub uploaded: HashMap<AssetId<Image>, DataTextureUploadStats>,
    next_sequence: u64,
}

impl DataTextureUpdates {
    pub fn reserve_sequence(&mut self) -> u64 {
        self.next_sequence += 1;
        self.next_sequence
    }
    pub fn submit(&mut self, patch: DataTexturePatch) {
        if match &patch.texels {
            DataTextureTexels::Rgba32Float(t) => t.is_empty(),
            DataTextureTexels::R8Unorm(t) => t.is_empty(),
        } {
            return;
        }
        let id = patch.image.id();
        if let Some(previous) = self.patches.get_mut(&id)
            && previous.size == patch.size
            && let (
                DataTextureTexels::Rgba32Float(previous_texels),
                DataTextureTexels::Rgba32Float(texels),
            ) = (&mut previous.texels, &patch.texels)
        {
            previous.sequence = patch.sequence;
            previous_texels.extend(texels);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_texture_patches_coalesce_and_retire() {
        let image = Handle::<Image>::default();
        let mut updates = DataTextureUpdates::default();
        for (sequence, address, value) in [(1, 0, 1.0), (2, 1, 2.0), (3, 0, 3.0)] {
            updates.submit(DataTexturePatch {
                image: image.clone(),
                sequence,
                size: UVec2::new(2, 1),
                texels: DataTextureTexels::Rgba32Float(BTreeMap::from([(address, [value; 4])])),
            });
        }
        let patch = &updates.patches[&image.id()];
        assert_eq!(patch.sequence, 3);
        let DataTextureTexels::Rgba32Float(texels) = &patch.texels else {
            panic!("expected sparse float texels");
        };
        assert_eq!(texels, &BTreeMap::from([(0, [3.0; 4]), (1, [2.0; 4])]));
        for (sequence, bytes) in [(4, vec![0, 255]), (5, vec![64, 128])] {
            updates.submit(DataTexturePatch {
                image: image.clone(),
                sequence,
                size: UVec2::new(2, 1),
                texels: DataTextureTexels::R8Unorm(bytes),
            });
        }
        let patch = &updates.patches[&image.id()];
        assert_eq!(patch.sequence, 5);
        let DataTextureTexels::R8Unorm(bytes) = &patch.texels else {
            panic!("expected scalar texels");
        };
        assert_eq!(bytes, &[64, 128]);
        updates.retire(image.id());
        assert!(updates.patches.is_empty());
        assert_eq!(updates.retired, vec![image.id()]);
    }
}
