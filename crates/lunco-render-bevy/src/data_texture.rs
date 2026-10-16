//! Upload completed texture patches before material preparation and drawing.

use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{
    Extent3d, Origin3d, TexelCopyBufferLayout, TextureFormat, TextureId,
};
use bevy::render::renderer::RenderQueue;
use bevy::render::texture::GpuImage;
use bevy::render::{ExtractSchedule, MainWorld, Render, RenderApp, RenderSystems};
use lunco_render::data_texture::{DataTextureTexels, DataTextureUpdates};
use std::collections::HashMap;

pub(super) fn build(app: &mut App) {
    app.init_resource::<DataTextureUpdates>();
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render
            .init_resource::<DataTextureUpdates>()
            .add_systems(ExtractSchedule, extract_patches)
            .add_systems(
                Render,
                upload_patches.in_set(RenderSystems::PrepareResources),
            );
    }
}

fn extract_patches(mut main: ResMut<MainWorld>, mut pending: ResMut<DataTextureUpdates>) {
    let Some(mut updates) = main.get_resource_mut::<DataTextureUpdates>() else {
        return;
    };
    for id in updates.retired.drain(..) {
        pending.patches.remove(&id);
        pending.uploaded.remove(&id);
    }
    updates.uploaded.clone_from(&pending.uploaded);
    for (_, patch) in updates.patches.drain() {
        pending.submit(patch);
    }
}

fn upload_patches(
    mut pending: ResMut<DataTextureUpdates>,
    images: Res<RenderAssets<GpuImage>>,
    queue: Res<RenderQueue>,
    mut texture_ids: Local<HashMap<AssetId<Image>, TextureId>>,
) {
    let updates = &mut *pending;
    texture_ids.retain(|id, _| updates.uploaded.contains_key(id));
    let uploaded = &mut updates.uploaded;
    updates.patches.retain(|id, patch| {
        let Some(image) = images.get(*id) else {
            return true;
        };
        let descriptor = &image.texture_descriptor;
        if descriptor.size.width != patch.size.x || descriptor.size.height != patch.size.y {
            return true; // The corresponding capacity growth has not been prepared yet.
        }
        let count = (patch.size.x as usize) * (patch.size.y as usize);
        let valid = descriptor.size.depth_or_array_layers == 1
            && match &patch.texels {
                DataTextureTexels::Rgba32Float(texels) => {
                    descriptor.format == TextureFormat::Rgba32Float
                        && texels.keys().next_back().is_none_or(|i| *i < count)
                }
                DataTextureTexels::R8Unorm(bytes) => {
                    descriptor.format == TextureFormat::R8Unorm && bytes.len() == count
                }
            };
        if !valid {
            error!("data texture patch violates its image format or extent");
            return false;
        }
        let patch_bytes = match &patch.texels {
            DataTextureTexels::R8Unorm(bytes) => {
                queue.write_texture(
                    image.texture.as_image_copy(),
                    bytes,
                    TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(patch.size.x),
                        rows_per_image: Some(patch.size.y),
                    },
                    Extent3d {
                        width: patch.size.x,
                        height: patch.size.y,
                        depth_or_array_layers: 1,
                    },
                );
                bytes.len()
            }
            DataTextureTexels::Rgba32Float(texels) => {
                // Each run stays within one row; unchanged texels are never uploaded.
                let mut values = texels.iter().peekable();
                while let Some((&start, &value)) = values.next() {
                    let row = start / patch.size.x as usize;
                    let mut bytes = Vec::from(value.map(f32::to_le_bytes).concat());
                    let mut end = start + 1;
                    while values.peek().is_some_and(|(index, _)| {
                        **index == end && end / patch.size.x as usize == row
                    }) {
                        let (_, value) = values.next().unwrap();
                        bytes.extend(value.iter().flat_map(|v| v.to_le_bytes()));
                        end += 1;
                    }
                    let mut destination = image.texture.as_image_copy();
                    destination.origin = Origin3d {
                        x: (start % patch.size.x as usize) as u32,
                        y: row as u32,
                        z: 0,
                    };
                    queue.write_texture(
                        destination,
                        &bytes,
                        TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(bytes.len() as u32),
                            rows_per_image: Some(1),
                        },
                        Extent3d {
                            width: (end - start) as u32,
                            height: 1,
                            depth_or_array_layers: 1,
                        },
                    );
                }
                texels.len() * 16
            }
        };
        let stats = uploaded.entry(*id).or_default();
        if texture_ids.insert(*id, image.texture.id()) != Some(image.texture.id()) {
            stats.texture_allocations += 1;
        }
        stats.batches += 1;
        stats.sequence = patch.sequence;
        stats.last_patch_bytes = patch_bytes;
        stats.bytes += patch_bytes as u64;
        false
    });
}
