//! Upload completed texture patches before material preparation and drawing.

use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{Extent3d, Origin3d, TexelCopyBufferLayout, TextureFormat};
use bevy::render::renderer::RenderQueue;
use bevy::render::texture::GpuImage;
use bevy::render::{ExtractSchedule, MainWorld, Render, RenderApp, RenderSystems};
use lunco_materials::float_texture::FloatTextureUpdates;

pub(super) fn build(app: &mut App) {
    app.init_resource::<FloatTextureUpdates>();
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render
            .init_resource::<FloatTextureUpdates>()
            .add_systems(ExtractSchedule, extract_patches)
            .add_systems(
                Render,
                upload_patches.in_set(RenderSystems::PrepareResources),
            );
    }
}

fn extract_patches(mut main: ResMut<MainWorld>, mut pending: ResMut<FloatTextureUpdates>) {
    let Some(mut updates) = main.get_resource_mut::<FloatTextureUpdates>() else {
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
    mut pending: ResMut<FloatTextureUpdates>,
    images: Res<RenderAssets<GpuImage>>,
    queue: Res<RenderQueue>,
) {
    let updates = &mut *pending;
    let uploaded = &mut updates.uploaded;
    updates.patches.retain(|id, patch| {
        let Some(image) = images.get(*id) else {
            return true;
        };
        let descriptor = &image.texture_descriptor;
        if descriptor.size.width != patch.size.x || descriptor.size.height != patch.size.y {
            return true; // The corresponding capacity growth has not been prepared yet.
        }
        if descriptor.format != TextureFormat::Rgba32Float
            || descriptor.size.depth_or_array_layers != 1
            || patch
                .texels
                .keys()
                .next_back()
                .is_some_and(|i| *i >= (patch.size.x * patch.size.y) as usize)
        {
            error!("float texture patch violates its image format or extent");
            return false;
        }
        // Each run stays within one row; unchanged texels are never uploaded.
        let mut values = patch.texels.iter().peekable();
        while let Some((&start, &value)) = values.next() {
            let row = start / patch.size.x as usize;
            let mut bytes = Vec::from(value.map(f32::to_le_bytes).concat());
            let mut end = start + 1;
            while values
                .peek()
                .is_some_and(|(index, _)| **index == end && end / patch.size.x as usize == row)
            {
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
        let stats = uploaded.entry(*id).or_default();
        stats.batches += 1;
        stats.sequence = patch.sequence;
        stats.last_patch_bytes = patch.texels.len() * 16;
        stats.bytes += stats.last_patch_bytes as u64;
        false
    });
}
