//! Typed uploads for persistent scalar, RGBA float and mipped RGBA8 textures.

use bevy::prelude::*;
use std::collections::{BTreeMap, HashMap};

pub enum DataTextureTexels {
    Rgba32Float(BTreeMap<usize, [f32; 4]>),
    R8Unorm(Vec<u8>),
    /// Ordered rectangular updates of an RGBA8 texture mip chain.
    Rgba8MipRegions(Vec<Rgba8MipRegion>),
}

pub struct Rgba8MipRegion {
    pub level: u32,
    pub origin: UVec2,
    pub size: UVec2,
    pub bytes: Vec<u8>,
}

impl Rgba8MipRegion {
    pub fn valid(&self, base_size: UVec2, mip_count: u32) -> bool {
        if self.level >= mip_count || self.level >= 32 || self.size.x == 0 || self.size.y == 0 {
            return false;
        }
        let extent = UVec2::new(
            (base_size.x >> self.level).max(1),
            (base_size.y >> self.level).max(1),
        );
        self.origin
            .x
            .checked_add(self.size.x)
            .is_some_and(|end| end <= extent.x)
            && self
                .origin
                .y
                .checked_add(self.size.y)
                .is_some_and(|end| end <= extent.y)
            && (self.size.x as usize)
                .checked_mul(self.size.y as usize)
                .and_then(|n| n.checked_mul(4))
                == Some(self.bytes.len())
    }
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
    pub fn submit(&mut self, mut patch: DataTexturePatch) {
        if match &patch.texels {
            DataTextureTexels::Rgba32Float(t) => t.is_empty(),
            DataTextureTexels::R8Unorm(t) => t.is_empty(),
            DataTextureTexels::Rgba8MipRegions(t) => t.is_empty(),
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
        } else if let Some(previous) = self.patches.get_mut(&id)
            && previous.size == patch.size
            && let (
                DataTextureTexels::Rgba8MipRegions(regions),
                DataTextureTexels::Rgba8MipRegions(incoming),
            ) = (&mut previous.texels, &mut patch.texels)
        {
            previous.sequence = patch.sequence;
            // Ordered writes preserve later overlapping updates even when the
            // render world has not drained the preceding batch yet.
            for region in incoming.drain(..) {
                // A later write to the same rectangle fully supersedes its
                // predecessor. Toroidal tile updates remain bounded even when
                // extraction takes longer than one application frame.
                regions.retain(|old| {
                    old.level != region.level
                        || old.origin != region.origin
                        || old.size != region.size
                });
                regions.push(region);
            }
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
    fn mip_rectangles_validate_and_supersede_exact_slots() {
        let image = Handle::<Image>::default();
        let size = UVec2::splat(16);
        let region = |value| Rgba8MipRegion {
            level: 2,
            origin: UVec2::new(2, 0),
            size: UVec2::splat(2),
            bytes: vec![value; 16],
        };
        let mut invalid = region(0);
        invalid.origin.x = 3;
        assert!(!invalid.valid(size, 4));
        assert!(!region(0).valid(size, 2));
        assert!(region(0).valid(size, 4));
        let mut updates = DataTextureUpdates::default();
        for sequence in 1..=3 {
            updates.submit(DataTexturePatch {
                image: image.clone(),
                sequence,
                size,
                texels: DataTextureTexels::Rgba8MipRegions(vec![region(sequence as u8)]),
            });
        }
        let patch = &updates.patches[&image.id()];
        assert_eq!(patch.sequence, 3);
        let DataTextureTexels::Rgba8MipRegions(regions) = &patch.texels else {
            panic!("expected mip rectangles")
        };
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].bytes, vec![3; 16]);
    }

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
