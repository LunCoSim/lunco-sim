//! Sparse vector annotations evaluated on the terrain's own fragments.
//!
//! Curves stay in terrain-local f64 coordinates. A bounded background job builds
//! a spatial index, not a height-fitting mesh. GPU lookup is independent of LOD,
//! geomorph, edits to elevation, and camera movement. The source entity owns the
//! annotation; removal retires its contribution before another image is admitted.

use bevy::math::DVec2;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};
use lunco_materials::{ShaderLook, ShaderLookSourceInterface, TextureLayer};
use std::collections::{BTreeMap, HashMap};
use wgpu_types::{Extent3d, TextureDimension, TextureFormat};

use crate::{DemHeightField, LodTileOf};

/// One sparse annotation source, attached to its disposable scene entity.
#[derive(Component, Clone)]
pub struct SurfaceCurveAnnotation {
    pub terrain: Entity,
    pub revision: u64,
    pub points: Vec<DVec2>,
    pub width_m: f64,
    pub color: LinearRgba,
}

impl SurfaceCurveAnnotation {
    /// Distance to the nearest centreline segment, in terrain-local metres.
    pub fn distance(&self, point: DVec2) -> f64 {
        self.points
            .windows(2)
            .map(|pair| {
                let delta = pair[1] - pair[0];
                let t = ((point - pair[0]).dot(delta) / delta.length_squared()).clamp(0.0, 1.0);
                (point - pair[0] - t * delta).length()
            })
            .fold(f64::INFINITY, f64::min)
    }
}

/// Bounded preparation and fragment work. Width/colour remain source policy.
#[derive(Resource)]
pub struct SurfaceAnnotationSettings {
    pub grid_resolution: usize,
    pub max_cell_segments: usize,
    pub max_segments: usize,
    /// Worker admission limit, between one and two inclusive.
    pub max_active_builds: usize,
}

impl Default for SurfaceAnnotationSettings {
    fn default() -> Self {
        Self {
            grid_resolution: 32,
            max_cell_segments: 64,
            max_segments: 4096,
            max_active_builds: 2,
        }
    }
}

#[derive(Clone)]
pub struct PublishedSurfaceAnnotations {
    pub sources: Vec<(Entity, u64)>,
    pub image: Option<Handle<Image>>,
    pub error: Option<String>,
}

/// Publication/readiness owner, also consumed by curve-view inspection.
#[derive(Resource, Default)]
pub struct SurfaceAnnotationImages {
    pub published: HashMap<Entity, PublishedSurfaceAnnotations>,
    revisions: HashMap<Entity, u64>,
    queued: BTreeMap<Entity, (u64, Vec<(Entity, SurfaceCurveAnnotation)>)>,
    tasks: HashMap<Entity, Task<AnnotationBuild>>,
    next_revision: u64,
}

struct AnnotationBuild {
    revision: u64,
    sources: Vec<(Entity, u64)>,
    result: Result<Option<Vec<u8>>, String>,
}

// An explicit data ABI shared with terrain_surface.wgsl. Width is fixed so
// a neutral 1x1 optional binding is distinguishable without another uniform.
const IMAGE_WIDTH: usize = 256;

fn image_bytes(
    annotations: &[SurfaceCurveAnnotation],
    grid: usize,
    max_cell: usize,
    max_segments: usize,
) -> Result<Option<Vec<u8>>, String> {
    let _span = info_span!("surface_annotation_index_build_worker").entered();
    if !(1..=64).contains(&grid) || max_cell == 0 || max_cell > 256 || max_segments > 4096 {
        return Err("invalid surface annotation preparation bounds".into());
    }
    let mut segments = Vec::new();
    for annotation in annotations {
        if !annotation.width_m.is_finite()
            || annotation.width_m <= 0.0
            || !annotation
                .color
                .to_f32_array()
                .iter()
                .all(|v| v.is_finite() && *v >= 0.0)
            || annotation.color.alpha > 1.0
        {
            return Err("surface annotation width and colour must be finite".into());
        }
        for pair in annotation.points.windows(2) {
            if !pair.iter().all(|p| p.is_finite()) || pair[0].distance_squared(pair[1]) <= 1e-18 {
                return Err("surface annotation needs finite, distinct consecutive points".into());
            }
            if segments.len() == max_segments {
                return Err("surface annotation segment budget exceeded".into());
            }
            segments.push((pair[0], pair[1], annotation.width_m * 0.5, annotation.color));
        }
    }
    if segments.is_empty() {
        return Ok(None);
    }
    let mut min = DVec2::splat(f64::INFINITY);
    let mut max = DVec2::splat(f64::NEG_INFINITY);
    for (a, b, radius, _) in &segments {
        min = min.min(a.min(*b) - DVec2::splat(*radius));
        max = max.max(a.max(*b) + DVec2::splat(*radius));
    }
    let cell_size = (max - min) / grid as f64;
    let mut cells = vec![Vec::new(); grid * grid];
    for (index, (a, b, radius, _)) in segments.iter().enumerate() {
        let lower = ((a.min(*b) - DVec2::splat(*radius) - min) / cell_size).floor();
        let upper = ((a.max(*b) + DVec2::splat(*radius) - min) / cell_size).floor();
        for z in (lower.y.max(0.0) as usize)..=(upper.y as usize).min(grid - 1) {
            for x in (lower.x.max(0.0) as usize)..=(upper.x as usize).min(grid - 1) {
                // Exact segment/expanded-cell intersection avoids filling the
                // whole bounding rectangle of a long diagonal with candidates.
                let lo = min + DVec2::new(x as f64, z as f64) * cell_size - DVec2::splat(*radius);
                let hi = lo + cell_size + DVec2::splat(2.0 * *radius);
                if !segment_intersects_box(*a, *b, lo, hi) {
                    continue;
                }
                let cell = &mut cells[z * grid + x];
                if cell.len() == max_cell {
                    return Err("surface annotation cell budget exceeded".into());
                }
                cell.push(index);
            }
        }
    }
    let references = 2 + cells.len();
    let records = references + cells.iter().map(Vec::len).sum::<usize>();
    let mut texels = vec![[0.0f32; 4]; records + segments.len() * 3];
    texels[0] = [min.x as f32, min.y as f32, max.x as f32, max.y as f32];
    texels[1] = [
        grid as f32,
        references as f32,
        records as f32,
        segments.len() as f32,
    ];
    let mut next = references;
    for (index, cell) in cells.iter().enumerate() {
        texels[2 + index] = [next as f32, cell.len() as f32, 0.0, 0.0];
        for &segment in cell {
            texels[next][0] = segment as f32;
            next += 1;
        }
    }
    for (index, (a, b, radius, color)) in segments.iter().enumerate() {
        let base = records + index * 3;
        texels[base] = [a.x as f32, a.y as f32, b.x as f32, b.y as f32];
        texels[base + 1] = [*radius as f32, 0.0, 0.0, 0.0];
        texels[base + 2] = color.to_f32_array();
        let encoded = texels[base];
        // This is the explicit GPU narrowing boundary. Reject a width or
        // coordinate whose quantization can erase/move the stroke visibly.
        for (value, narrowed) in [a.x, a.y, b.x, b.y].into_iter().zip(encoded) {
            if !narrowed.is_finite() || (value - f64::from(narrowed)).abs() > *radius * 0.01 {
                return Err("surface annotation coordinates exceed render precision".into());
            }
        }
        if !texels[base + 1][0].is_finite()
            || texels[base + 1][0] <= 0.0
            || (encoded[0] == encoded[2] && encoded[1] == encoded[3])
        {
            return Err("surface annotation segment exceeds render precision".into());
        }
    }
    if texels[0].iter().any(|v| !v.is_finite())
        || texels[0][0] >= texels[0][2]
        || texels[0][1] >= texels[0][3]
    {
        return Err("surface annotation bounds exceed render precision".into());
    }
    texels.resize(texels.len().div_ceil(IMAGE_WIDTH) * IMAGE_WIDTH, [0.0; 4]);
    Ok(Some(
        texels
            .iter()
            .flat_map(|p| p.iter().flat_map(|v| v.to_le_bytes()))
            .collect(),
    ))
}

fn segment_intersects_box(a: DVec2, b: DVec2, min: DVec2, max: DVec2) -> bool {
    let delta = b - a;
    let (mut enter, mut leave) = (0.0f64, 1.0f64);
    for axis in 0..2 {
        if delta[axis].abs() < 1e-18 {
            if a[axis] < min[axis] || a[axis] > max[axis] {
                return false;
            }
        } else {
            let t0 = (min[axis] - a[axis]) / delta[axis];
            let t1 = (max[axis] - a[axis]) / delta[axis];
            enter = enter.max(t0.min(t1));
            leave = leave.min(t0.max(t1));
        }
    }
    enter <= leave
}

pub(crate) fn prepare_surface_annotations(
    annotations: Query<(Entity, &SurfaceCurveAnnotation)>,
    changed: Query<(), Changed<SurfaceCurveAnnotation>>,
    mut removed: RemovedComponents<SurfaceCurveAnnotation>,
    terrains: Query<Entity, With<DemHeightField>>,
    mut removed_terrains: RemovedComponents<DemHeightField>,
    interfaces: Query<&ShaderLookSourceInterface>,
    changed_interfaces: Query<(), (Changed<ShaderLookSourceInterface>, With<DemHeightField>)>,
    settings: Res<SurfaceAnnotationSettings>,
    mut state: ResMut<SurfaceAnnotationImages>,
) {
    let removed_sources = removed.read().count() > 0;
    let removed_owners = removed_terrains.read().count() > 0;
    let dirty = !changed.is_empty()
        || removed_sources
        || removed_owners
        || settings.is_changed()
        || !changed_interfaces.is_empty();
    if !dirty
        && !state
            .queued
            .keys()
            .any(|terrain| !state.tasks.contains_key(terrain) && interfaces.contains(*terrain))
    {
        return;
    }
    state
        .published
        .retain(|terrain, _| terrains.contains(*terrain));
    state
        .revisions
        .retain(|terrain, _| terrains.contains(*terrain));
    state
        .queued
        .retain(|terrain, _| terrains.contains(*terrain));
    if dirty {
        let mut grouped: BTreeMap<Entity, Vec<(Entity, SurfaceCurveAnnotation)>> = BTreeMap::new();
        for (entity, annotation) in &annotations {
            if terrains.contains(annotation.terrain) {
                grouped
                    .entry(annotation.terrain)
                    .or_default()
                    .push((entity, annotation.clone()));
            }
        }
        // Include formerly published/queued owners to remove their last curve.
        for terrain in state.revisions.keys() {
            grouped.entry(*terrain).or_default();
        }
        for (terrain, mut sources) in grouped {
            sources.sort_by_key(|(entity, _)| *entity);
            let keys: Vec<_> = sources
                .iter()
                .map(|(entity, source)| (*entity, source.revision))
                .collect();
            if !settings.is_changed()
                && !changed_interfaces.contains(terrain)
                && state
                    .published
                    .get(&terrain)
                    .is_some_and(|p| p.sources == keys)
            {
                continue;
            }
            state.next_revision += 1;
            let revision = state.next_revision;
            state.revisions.insert(terrain, revision);
            state.published.remove(&terrain);
            if !(1..=2).contains(&settings.max_active_builds) {
                state.queued.remove(&terrain);
                let error =
                    "surface annotation worker limit must be between one and two".to_string();
                warn!("[surface-annotations] {error}");
                state.published.insert(
                    terrain,
                    PublishedSurfaceAnnotations {
                        sources: keys,
                        image: None,
                        error: Some(error),
                    },
                );
                continue;
            }
            state.queued.insert(terrain, (revision, sources));
        }
    }
    while state.tasks.len() < settings.max_active_builds {
        let Some(terrain) = state
            .queued
            .keys()
            .find(|t| !state.tasks.contains_key(t) && interfaces.contains(**t))
            .copied()
        else {
            break;
        };
        let Some(interface) = interfaces.get(terrain).ok() else {
            break;
        };
        let (revision, sources) = state.queued.remove(&terrain).unwrap();
        if !interface.source_valid
            || !interface
                .capabilities
                .contains("lunco.surface-annotations.v1")
        {
            let error =
                "terrain shader does not implement lunco.surface-annotations.v1".to_string();
            warn!("[surface-annotations] {error}");
            state.published.insert(
                terrain,
                PublishedSurfaceAnnotations {
                    sources: sources
                        .iter()
                        .map(|(entity, source)| (*entity, source.revision))
                        .collect(),
                    image: None,
                    error: Some(error),
                },
            );
            continue;
        }
        let (grid, max_cell, max_segments) = (
            settings.grid_resolution,
            settings.max_cell_segments,
            settings.max_segments,
        );
        state.tasks.insert(
            terrain,
            AsyncComputeTaskPool::get().spawn(async move {
                let keys = sources
                    .iter()
                    .map(|(entity, source)| (*entity, source.revision))
                    .collect();
                let values: Vec<_> = sources.into_iter().map(|(_, source)| source).collect();
                AnnotationBuild {
                    revision,
                    sources: keys,
                    result: image_bytes(&values, grid, max_cell, max_segments),
                }
            }),
        );
    }
}

pub(crate) fn publish_surface_annotations(
    mut state: ResMut<SurfaceAnnotationImages>,
    mut images: ResMut<Assets<Image>>,
    mut look_queries: ParamSet<(
        Query<
            (),
            (
                Changed<ShaderLook>,
                Or<(With<DemHeightField>, With<LodTileOf>)>,
            ),
        >,
        Query<
            (Entity, &mut ShaderLook, Option<&LodTileOf>),
            Or<(With<DemHeightField>, With<LodTileOf>)>,
        >,
    )>,
) {
    let looks_changed = !look_queries.p0().is_empty();
    if state.tasks.is_empty() && !state.is_changed() && !looks_changed {
        return;
    }
    let mut completed = Vec::new();
    for (&terrain, task) in &mut state.bypass_change_detection().tasks {
        if let Some(result) = future::block_on(future::poll_once(task)) {
            completed.push((terrain, result));
        }
    }
    for (terrain, build) in completed {
        state.tasks.remove(&terrain);
        if state.revisions.get(&terrain) != Some(&build.revision) {
            continue;
        }
        let (image, error) = match build.result {
            Ok(Some(bytes)) => {
                let height = bytes.len() / (IMAGE_WIDTH * 16);
                let image = Image::new(
                    Extent3d {
                        width: IMAGE_WIDTH as u32,
                        height: height as u32,
                        depth_or_array_layers: 1,
                    },
                    TextureDimension::D2,
                    bytes,
                    TextureFormat::Rgba32Float,
                    bevy::asset::RenderAssetUsages::default(),
                );
                (Some(images.add(image)), None)
            }
            Ok(None) => (None, None),
            Err(error) => {
                warn!("[surface-annotations] {error}");
                (None, Some(error))
            }
        };
        state.published.insert(
            terrain,
            PublishedSurfaceAnnotations {
                sources: build.sources,
                image,
                error,
            },
        );
    }
    if !state.is_changed() && !looks_changed {
        return;
    }
    for (entity, mut look, tile) in &mut look_queries.p1() {
        let terrain = tile.map_or(entity, |tile| tile.0);
        let image = state.published.get(&terrain).and_then(|p| p.image.as_ref());
        if look.textures.get(&TextureLayer::SurfaceAnnotations) == image {
            continue;
        }
        if let Some(image) = image {
            look.textures
                .insert(TextureLayer::SurfaceAnnotations, image.clone());
        } else {
            look.textures.remove(&TextureLayer::SurfaceAnnotations);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_index_preserves_a_long_diagonal_without_height_samples() {
        let curve = SurfaceCurveAnnotation {
            terrain: Entity::PLACEHOLDER,
            revision: 1,
            points: vec![DVec2::ZERO, DVec2::splat(10_000.0)],
            width_m: 0.12,
            color: LinearRgba::WHITE,
        };
        let bytes = image_bytes(&[curve.clone()], 32, 64, 4096)
            .unwrap()
            .unwrap();
        let values: Vec<_> = bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        assert_eq!(values[7], 1.0); // one segment, regardless of route length
        let counts: Vec<_> = (0..1024).map(|i| values[(2 + i) * 4 + 1]).collect();
        assert!(counts.iter().filter(|&&v| v > 0.0).count() < 100);
        let start = std::time::Instant::now();
        for _ in 0..1000 {
            std::hint::black_box(image_bytes(std::slice::from_ref(&curve), 32, 64, 4096).unwrap());
        }
        eprintln!(
            "10 km sparse index: {:.3} us/build over 1000 preparations; {} image bytes",
            start.elapsed().as_secs_f64() * 1000.0,
            bytes.len()
        );
        assert_eq!(curve.distance(DVec2::splat(5000.0)), 0.0);
        assert!(curve.distance(DVec2::new(5000.0, 5001.0)) > 0.06);
    }

    #[test]
    fn invalid_and_overcrowded_annotations_fail_explicitly() {
        let curve = SurfaceCurveAnnotation {
            terrain: Entity::PLACEHOLDER,
            revision: 1,
            points: vec![DVec2::ZERO, DVec2::X],
            width_m: 0.12,
            color: LinearRgba::WHITE,
        };
        assert!(
            image_bytes(&[curve.clone(), curve.clone()], 1, 1, 4096)
                .unwrap_err()
                .contains("cell budget")
        );
        assert!(
            image_bytes(std::slice::from_ref(&curve), 32, 64, 0)
                .unwrap_err()
                .contains("segment budget")
        );
        let mut invalid = curve;
        invalid.points[1] = DVec2::ZERO;
        assert!(image_bytes(&[invalid], 32, 64, 4096).is_err());
    }
}
