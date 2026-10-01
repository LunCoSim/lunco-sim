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

/// Presentation admission boundary for producers of terrain annotations.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SurfaceAnnotationSet {
    Prepare,
    Publish,
}

/// One sparse annotation source, attached to its disposable scene entity.
#[derive(Component, Clone)]
pub struct SurfaceCurveAnnotation {
    pub terrain: Entity,
    pub revision: u64,
    /// Independent segments, oldest first. Disconnected strokes never acquire a joining leg.
    pub segments: Vec<[DVec2; 2]>,
    /// Continuously sampled presentation history. Its oldest segments may be
    /// retired to respect GPU budgets; snapshot sources always remain complete.
    pub streaming: bool,
    pub width_m: f64,
    pub color: LinearRgba,
}

impl SurfaceCurveAnnotation {
    /// Distance to the nearest centreline segment, in terrain-local metres.
    pub fn distance(&self, point: DVec2) -> f64 {
        self.segments
            .iter()
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
    /// Maximum contiguous source legs per bounded simplification block.
    pub stream_chunk_segments: usize,
    /// Worker admission limit, between one and two inclusive.
    pub max_active_builds: usize,
}

impl Default for SurfaceAnnotationSettings {
    fn default() -> Self {
        Self {
            grid_resolution: 32,
            max_cell_segments: 64,
            max_segments: 4096,
            stream_chunk_segments: 64,
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
    source_revisions: HashMap<Entity, Vec<(Entity, u64, bool)>>,
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
    stream_chunk: usize,
) -> Result<Option<Vec<u8>>, String> {
    let _span = info_span!("surface_annotation_index_build_worker").entered();
    if !(1..=64).contains(&grid)
        || max_cell == 0
        || max_cell > 256
        || max_segments > 4096
        || !(1..=64).contains(&stream_chunk)
    {
        return Err("invalid surface annotation preparation bounds".into());
    }
    // Snapshot sources are admitted first. Streaming sources are interleaved
    // newest-first so one long lane cannot evict the live heads of other lanes.
    let mut candidates = Vec::new();
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
        for pair in &annotation.segments {
            if !pair.iter().all(|p| p.is_finite()) || pair[0].distance_squared(pair[1]) <= 1e-18 {
                return Err("surface annotation needs finite, distinct segment endpoints".into());
            }
        }
        if !annotation.streaming {
            for pair in &annotation.segments {
                if candidates.len() == max_segments {
                    return Err("surface annotation segment budget exceeded".into());
                }
                candidates.push((
                    pair[0],
                    pair[1],
                    annotation.width_m * 0.5,
                    annotation.color,
                    false,
                ));
            }
        }
    }
    let streams: Vec<_> = annotations
        .iter()
        .filter(|a| a.streaming)
        .map(|a| {
            (
                a,
                simplify_stream_segments(&a.segments, a.width_m * 0.5 * 0.01, stream_chunk),
            )
        })
        .collect();
    'history: for age in 0..streams
        .iter()
        .map(|(_, segments)| segments.len())
        .max()
        .unwrap_or(0)
    {
        for (annotation, segments) in &streams {
            if let Some(index) = segments.len().checked_sub(age + 1) {
                if candidates.len() == max_segments {
                    break 'history;
                }
                let pair = segments[index];
                candidates.push((
                    pair[0],
                    pair[1],
                    annotation.width_m * 0.5,
                    annotation.color,
                    true,
                ));
            }
        }
    }
    let mut segments = Vec::new();
    if candidates.is_empty() {
        return Ok(None);
    }
    let mut min = DVec2::splat(f64::INFINITY);
    let mut max = DVec2::splat(f64::NEG_INFINITY);
    for (a, b, radius, _, _) in &candidates {
        min = min.min(a.min(*b) - DVec2::splat(*radius));
        max = max.max(a.max(*b) + DVec2::splat(*radius));
    }
    let cell_size = (max - min) / grid as f64;
    let mut cells = vec![Vec::new(); grid * grid];
    for (a, b, radius, color, streaming) in candidates {
        let mut touched = Vec::new();
        let lower = ((a.min(b) - DVec2::splat(radius) - min) / cell_size).floor();
        let upper = ((a.max(b) + DVec2::splat(radius) - min) / cell_size).floor();
        for z in (lower.y.max(0.0) as usize)..=(upper.y as usize).min(grid - 1) {
            for x in (lower.x.max(0.0) as usize)..=(upper.x as usize).min(grid - 1) {
                // Exact segment/expanded-cell intersection avoids filling the
                // whole bounding rectangle of a long diagonal with candidates.
                let lo = min + DVec2::new(x as f64, z as f64) * cell_size - DVec2::splat(radius);
                let hi = lo + cell_size + DVec2::splat(2.0 * radius);
                if !segment_intersects_box(a, b, lo, hi) {
                    continue;
                }
                touched.push(z * grid + x);
            }
        }
        if touched.iter().any(|&cell| cells[cell].len() == max_cell) {
            if streaming {
                break;
            }
            return Err("surface annotation cell budget exceeded".into());
        }
        let index = segments.len();
        segments.push((a, b, radius, color));
        for cell in touched {
            cells[cell].push(index);
        }
    }
    if segments.is_empty() {
        return Ok(None);
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

/// Bounded Ramer–Douglas–Peucker reduction in the worker. Chunking caps the
/// quadratic worst case at 64 legs per block; shared endpoints and gaps survive.
/// Its centreline error uses the same one-percent-of-radius precision contract
/// as GPU coordinate admission, rather than introducing terrain-height samples.
fn simplify_stream_segments(
    source: &[[DVec2; 2]],
    tolerance: f64,
    chunk: usize,
) -> Vec<[DVec2; 2]> {
    fn append(points: &[DVec2], tolerance: f64, out: &mut Vec<[DVec2; 2]>) {
        if points.len() < 2 {
            return;
        }
        if points[0] == points[points.len() - 1] {
            out.extend(points.windows(2).map(|pair| [pair[0], pair[1]]));
            return;
        }
        let mut retained = vec![false; points.len()];
        retained[0] = true;
        retained[points.len() - 1] = true;
        let mut pending = vec![(0, points.len() - 1)];
        while let Some((start, end)) = pending.pop() {
            let delta = points[end] - points[start];
            let mut farthest = None;
            let mut maximum = tolerance * tolerance;
            for index in start + 1..end {
                let t = if delta.length_squared() == 0.0 {
                    0.0
                } else {
                    ((points[index] - points[start]).dot(delta) / delta.length_squared())
                        .clamp(0.0, 1.0)
                };
                let distance = (points[index] - points[start] - t * delta).length_squared();
                if distance > maximum {
                    maximum = distance;
                    farthest = Some(index);
                }
            }
            if let Some(index) = farthest {
                retained[index] = true;
                pending.push((start, index));
                pending.push((index, end));
            }
        }
        let points: Vec<_> = points
            .iter()
            .zip(retained)
            .filter_map(|(&p, keep)| keep.then_some(p))
            .collect();
        out.extend(points.windows(2).map(|pair| [pair[0], pair[1]]));
    }
    let mut result = Vec::new();
    let mut points = Vec::with_capacity(chunk + 1);
    for pair in source {
        if points.last().is_some_and(|last| *last != pair[0]) || points.len() == chunk + 1 {
            append(&points, tolerance, &mut result);
            points.clear();
        }
        if points.is_empty() {
            points.push(pair[0]);
        }
        points.push(pair[1]);
    }
    append(&points, tolerance, &mut result);
    result
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
    state
        .source_revisions
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
            let current_sources: Vec<_> = sources
                .iter()
                .map(|(entity, source)| (*entity, source.revision, source.streaming))
                .collect();
            let continuous_only = !settings.is_changed()
                && !changed_interfaces.contains(terrain)
                && state
                    .source_revisions
                    .get(&terrain)
                    .is_some_and(|previous| {
                        previous.len() == current_sources.len()
                            && previous.iter().zip(&current_sources).all(|(old, new)| {
                                old.0 == new.0 && old.2 == new.2 && (old.1 == new.1 || new.2)
                            })
                    });
            state.source_revisions.insert(terrain, current_sources);
            let revision = if continuous_only {
                state.revisions[&terrain]
            } else {
                state.next_revision += 1;
                let revision = state.next_revision;
                state.revisions.insert(terrain, revision);
                state.published.remove(&terrain);
                revision
            };
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
        let (grid, max_cell, max_segments, stream_chunk) = (
            settings.grid_resolution,
            settings.max_cell_segments,
            settings.max_segments,
            settings.stream_chunk_segments,
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
                    result: image_bytes(&values, grid, max_cell, max_segments, stream_chunk),
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
                // Continuous publication keeps one asset identity. Replacing
                // the handle every frame would restart terrain material/image
                // readiness faster than the renderer can upload the texture.
                let previous = state.published.get(&terrain).and_then(|p| p.image.clone());
                let handle = if let Some(handle) = previous.filter(|h| images.contains(h.id())) {
                    *images.get_mut(&handle).unwrap() = image;
                    handle
                } else {
                    images.add(image)
                };
                (Some(handle), None)
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
            segments: vec![[DVec2::ZERO, DVec2::splat(10_000.0)]],
            streaming: false,
            width_m: 0.12,
            color: LinearRgba::WHITE,
        };
        let bytes = image_bytes(&[curve.clone()], 32, 64, 4096, 64)
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
            std::hint::black_box(
                image_bytes(std::slice::from_ref(&curve), 32, 64, 4096, 64).unwrap(),
            );
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
            segments: vec![[DVec2::ZERO, DVec2::X]],
            streaming: false,
            width_m: 0.12,
            color: LinearRgba::WHITE,
        };
        assert!(
            image_bytes(&[curve.clone(), curve.clone()], 1, 1, 4096, 64)
                .unwrap_err()
                .contains("cell budget")
        );
        assert!(
            image_bytes(std::slice::from_ref(&curve), 32, 64, 0, 64)
                .unwrap_err()
                .contains("segment budget")
        );
        let mut invalid = curve;
        invalid.segments[0][1] = DVec2::ZERO;
        assert!(image_bytes(&[invalid], 32, 64, 4096, 64).is_err());
    }
    #[test]
    fn streaming_history_retires_old_legs_without_evicting_snapshot_or_live_heads() {
        let route = SurfaceCurveAnnotation {
            terrain: Entity::PLACEHOLDER,
            revision: 1,
            streaming: false,
            segments: vec![[DVec2::ZERO, DVec2::X]],
            width_m: 0.12,
            color: LinearRgba::WHITE,
        };
        let stream = |x: f64| SurfaceCurveAnnotation {
            terrain: Entity::PLACEHOLDER,
            revision: 1,
            streaming: true,
            segments: (0..100)
                .map(|i| [DVec2::new(x, i as f64), DVec2::new(x, i as f64 + 0.5)])
                .collect(),
            width_m: 0.28,
            color: LinearRgba::WHITE,
        };
        let bytes = image_bytes(&[route, stream(2.0), stream(3.0)], 1, 3, 3, 64)
            .unwrap()
            .unwrap();
        let values: Vec<_> = bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        assert_eq!(values[7], 3.0);
        let records = values[6] as usize * 4;
        assert_eq!(&values[records..records + 4], &[0.0, 0.0, 1.0, 0.0]);
        assert_eq!(&values[records + 12..records + 16], &[2.0, 99.0, 2.0, 99.5]);
        assert_eq!(&values[records + 24..records + 28], &[3.0, 99.0, 3.0, 99.5]);
    }

    #[test]
    fn streaming_reduction_preserves_turns_gaps_and_bounds_large_history_work() {
        let bend = vec![
            [DVec2::ZERO, DVec2::X],
            [DVec2::X, DVec2::ONE],
            [DVec2::splat(4.0), DVec2::splat(5.0)],
        ];
        assert_eq!(simplify_stream_segments(&bend, 0.0015, 64), bend);
        let streams: Vec<_> = (0..8)
            .map(|wheel| SurfaceCurveAnnotation {
                terrain: Entity::PLACEHOLDER,
                revision: 1,
                streaming: true,
                segments: (0..1024)
                    .map(|i| {
                        [
                            DVec2::new(wheel as f64 * 0.6, i as f64 * 0.5),
                            DVec2::new(wheel as f64 * 0.6, (i + 1) as f64 * 0.5),
                        ]
                    })
                    .collect(),
                width_m: 0.3,
                color: LinearRgba::WHITE,
            })
            .collect();
        let reduced = simplify_stream_segments(&streams[0].segments, 0.0015, 64);
        assert_eq!(reduced.len(), 16);
        assert_eq!(reduced[0][0], streams[0].segments[0][0]);
        assert_eq!(reduced[15][1], streams[0].segments[1023][1]);
        let start = std::time::Instant::now();
        let mut bytes = None;
        for _ in 0..100 {
            bytes = image_bytes(&streams, 32, 64, 4096, 64).unwrap();
        }
        let bytes = bytes.unwrap();
        let count = f32::from_le_bytes(bytes[28..32].try_into().unwrap());
        assert_eq!(count, 128.0);
        eprintln!(
            "eight 1024-leg histories: {:.3} us/build over 100 preparations; {} GPU segments; {} image bytes",
            start.elapsed().as_secs_f64() * 10_000.0,
            count,
            bytes.len()
        );
    }
}
