//! Terrain tile mesh generation and sampling.

use crate::quad_sphere::cube_to_sphere;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_mesh::{Indices, PrimitiveTopology};
use lunco_materials::ATTRIBUTE_GLOBE_DIRECTION;
use lunco_terrain_core::{
    HeightSource, square_boundary_height_at, square_boundary_posting_spacing,
    square_boundary_sample_coordinate,
};

/// The exact local DEM footprint in the body's tangent-plane coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlobeHandoff {
    /// Unit direction from the body centre to the site's tangent point.
    pub dir: DVec3,
    /// Unit east and north axes at the site, in body-fixed coordinates.
    pub east: DVec3,
    pub north: DVec3,
    /// Render radius of the globe shell for the active site datum.
    pub radius_m: f64,
    /// Radius used to project the local square into the globe direction chart.
    pub site_radius_m: f64,
    /// Half side of the DEM square in metres.
    pub half_extent: f64,
    /// Maximum local measured-relief feather width.
    pub collar_m: f64,
    /// Local collar widths in west, east, south, and north directions.
    pub collar_widths: [f64; 4],
    /// Sampling density shared by the collar's outer edge and globe cutout.
    pub edge_segments: usize,
}

/// Return collar mesh bounds in east, east, south-positive-Z order.
pub fn handoff_mesh_bounds(half_extent: f64, collar_widths: [f64; 4]) -> [f64; 4] {
    let [west, east, south, north] = collar_widths;
    [
        -half_extent - west,
        half_extent + east,
        -half_extent - north,
        half_extent + south,
    ]
}

/// Return globe cutout bounds in east, east, north-positive-chart order.
pub fn handoff_cutout_bounds(half_extent: f64, collar_widths: [f64; 4]) -> [f64; 4] {
    let [west, east, south, north] = collar_widths;
    [
        -half_extent - west,
        half_extent + east,
        -half_extent - south,
        half_extent + north,
    ]
}

/// The square removed from globe tiles beneath a local terrain edge stitch.
#[derive(Clone, Copy)]
pub struct GlobeCutout {
    pub dir: DVec3,
    pub east: DVec3,
    pub north: DVec3,
    pub site_radius_m: f64,
    /// Local tangent-chart bounds in east/north coordinates.
    pub bounds: [f64; 4],
    /// Number of deterministic samples along each outer edge.
    pub edge_segments: usize,
}

impl GlobeCutout {
    fn coordinates(self, direction: DVec3) -> Option<[f64; 2]> {
        let denominator = direction.dot(self.dir);
        (denominator > 0.0).then_some([
            direction.dot(self.east) * self.site_radius_m,
            direction.dot(self.north) * self.site_radius_m,
        ])
    }

    fn contains(self, direction: DVec3) -> bool {
        self.coordinates(direction).is_some_and(|[x, z]| {
            x >= self.bounds[0] && x <= self.bounds[1] && z >= self.bounds[2] && z <= self.bounds[3]
        })
    }
}

pub const MIN_GLOBE_CUTOUT_EDGE_SEGMENTS: usize = 32;

/// Build the one local collar mesh that joins a finite tangent-plane surface
/// to its globe. The inner ring follows the measured DEM postings exactly;
/// the outer ring follows the same sampled sphere boundary used to cut globe
/// tiles. Radial interpolation is independent of globe LOD.
pub fn create_rectangular_handoff_collar_mesh(
    handoff: GlobeHandoff,
    source: &dyn HeightSource,
    boundary_grid_resolution: usize,
    outer_edge_segments: usize,
    radial_segments: usize,
) -> Result<Mesh, &'static str> {
    if !handoff.radius_m.is_finite()
        || handoff.radius_m <= 0.0
        || !handoff.site_radius_m.is_finite()
        || handoff.site_radius_m <= 0.0
        || !handoff.half_extent.is_finite()
        || handoff.half_extent <= 0.0
        || handoff
            .collar_widths
            .iter()
            .any(|width| !width.is_finite() || *width <= 0.0)
        || boundary_grid_resolution < 2
        || outer_edge_segments == 0
        || radial_segments == 0
    {
        return Err("collar dimensions or boundary sampling are invalid");
    }

    let outer_bounds = handoff_mesh_bounds(handoff.half_extent, handoff.collar_widths);
    if outer_bounds.iter().any(|bound| !bound.is_finite()) {
        return Err("collar outer bounds are not finite");
    }
    let normal_step_m =
        square_boundary_posting_spacing(handoff.half_extent, boundary_grid_resolution)
            .ok_or("collar boundary posting spacing is invalid")?
            * 0.25;
    let capacity_error = "collar mesh exceeds supported index capacity";
    let outer_resolution = outer_edge_segments.checked_add(1).ok_or(capacity_error)?;
    let ring_count = radial_segments.checked_add(1).ok_or(capacity_error)?;
    // Every ring and both boundary perimeters must fit the mesh index space
    // before allocating the resolution table, on native and wasm32 alike.
    let perimeter_count = |resolution: usize| {
        resolution
            .checked_sub(1)
            .and_then(|edges| edges.checked_mul(4))
    };
    let inner_count = perimeter_count(boundary_grid_resolution).ok_or(capacity_error)?;
    let outer_count = perimeter_count(outer_resolution).ok_or(capacity_error)?;
    let minimum_vertices = inner_count
        .min(outer_count)
        .checked_mul(ring_count)
        .ok_or(capacity_error)?;
    if inner_count > u32::MAX as usize
        || outer_count > u32::MAX as usize
        || minimum_vertices > u32::MAX as usize
    {
        return Err(capacity_error);
    }
    let resolutions = (0..ring_count)
        .map(|ring| {
            collar_ring_resolution(
                boundary_grid_resolution,
                outer_resolution,
                ring,
                radial_segments,
                handoff,
            )
        })
        .collect::<Vec<_>>();
    let ring_vertex_counts = resolutions
        .iter()
        .map(|resolution| perimeter_count(*resolution).ok_or(capacity_error))
        .collect::<Result<Vec<_>, _>>()?;
    let mut vertex_count = 0usize;
    for &count in &ring_vertex_counts {
        vertex_count = vertex_count
            .checked_add(count)
            .ok_or("collar mesh exceeds supported index capacity")?;
    }
    let index_count = ring_vertex_counts
        .windows(2)
        .try_fold(0usize, |sum, pair| {
            pair[0]
                .checked_add(pair[1])
                .and_then(|indices| indices.checked_mul(3))
                .and_then(|indices| sum.checked_add(indices))
        })
        .ok_or("collar mesh exceeds supported index capacity")?;
    if vertex_count > u32::MAX as usize || index_count as u64 > u64::from(u32::MAX) * 3 {
        return Err("collar mesh exceeds supported index capacity");
    }

    let mut positions = Vec::with_capacity(vertex_count);
    let mut normals = Vec::with_capacity(vertex_count);
    let mut directions = Vec::with_capacity(vertex_count);
    let mut uvs = Vec::with_capacity(vertex_count);
    let position_at = |x: f64, z_south: f64| {
        collar_position(&handoff, source, x, z_south, boundary_grid_resolution)
    };

    let mut ring_starts = Vec::with_capacity(ring_count);
    for (radial, &resolution) in resolutions.iter().enumerate() {
        let t = collar_radial_fraction(radial, radial_segments);
        let bounds = handoff_mesh_bounds(
            handoff.half_extent,
            handoff.collar_widths.map(|width| width * t),
        );
        let mut perimeter = rectangle_perimeter_coordinates(bounds, resolution);
        if radial == 0 {
            // Match the DEM's stored posting coordinates, including its f32
            // lattice rounding, before converting at the render boundary.
            for (x, z) in &mut perimeter {
                let snap = |value: f64| {
                    let index = (((value + handoff.half_extent) / (2.0 * handoff.half_extent))
                        * (resolution - 1) as f64)
                        .round() as usize;
                    square_boundary_sample_coordinate(index, resolution, handoff.half_extent)
                        .expect("validated DEM posting lattice")
                };
                *x = snap(*x);
                *z = snap(*z);
            }
        }
        ring_starts.push(positions.len());
        for (x, z_south) in perimeter {
            let z_north = -z_south;
            let position = position_at(x, z_south)?;
            let body_position = handoff.dir * (handoff.radius_m + position.y)
                + handoff.east * x
                + handoff.north * z_north;
            let direction = body_position.normalize_or_zero();
            // The boundary-position helper snaps vertices to the DEM's exact
            // posting line. Use the continuous source for derivatives so a
            // corner does not collapse one gradient axis onto that line.
            let gradient_x = (source.height_at(x + normal_step_m, -z_north)
                - source.height_at(x - normal_step_m, -z_north))
                / (2.0 * normal_step_m);
            let gradient_z = (source.height_at(x, -(z_north + normal_step_m))
                - source.height_at(x, -(z_north - normal_step_m)))
                / (2.0 * normal_step_m);
            // Mesh coordinates use the DEM's east/up/south frame. The owner
            // entity applies that frame's rotation in the body-fixed grid.
            let normal = DVec3::new(-gradient_x, 1.0, gradient_z).normalize_or_zero();
            if !position.is_finite()
                || !body_position.is_finite()
                || !direction.is_finite()
                || direction.length_squared() < 0.9
                || !gradient_x.is_finite()
                || !gradient_z.is_finite()
                || !normal.is_finite()
                || normal.length_squared() < 0.9
            {
                return Err("collar mesh contains non-finite geometry");
            }
            let position = position.as_vec3().to_array();
            let normal = normal.as_vec3().to_array();
            let direction = direction.as_vec3().to_array();
            let uv = [
                ((x + handoff.half_extent) / (2.0 * handoff.half_extent)) as f32,
                ((z_south + handoff.half_extent) / (2.0 * handoff.half_extent)) as f32,
            ];
            if !position
                .iter()
                .chain(&normal)
                .chain(&direction)
                .chain(&uv)
                .all(|value| value.is_finite())
            {
                return Err("collar geometry exceeds the rendering precision range");
            }
            positions.push(position);
            normals.push(normal);
            directions.push(direction);
            uvs.push(uv);
        }
    }

    let mut indices = Vec::with_capacity(index_count);
    for radial in 0..radial_segments {
        connect_square_rings(
            ring_starts[radial],
            ring_vertex_counts[radial],
            ring_starts[radial + 1],
            ring_vertex_counts[radial + 1],
            &mut indices,
        );
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        bevy::asset::RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(ATTRIBUTE_GLOBE_DIRECTION, directions);
    mesh.insert_indices(Indices::U32(indices));
    Ok(mesh)
}

fn collar_ring_resolution(
    inner_resolution: usize,
    outer_resolution: usize,
    ring: usize,
    radial_segments: usize,
    handoff: GlobeHandoff,
) -> usize {
    let radial = collar_radial_fraction(ring, radial_segments);
    let posting = square_boundary_posting_spacing(handoff.half_extent, inner_resolution)
        .expect("validated boundary lattice");
    let min_width = handoff
        .collar_widths
        .into_iter()
        .fold(f64::INFINITY, f64::min);
    // Keep native connectivity through the edge-slope continuation. Decimating
    // a thinner strip makes nonlinear perimeter relief form steep sliver faces.
    let native_fraction = (posting / min_width).min(1.0);
    let t = if radial >= 1.0 {
        1.0
    } else if native_fraction >= 1.0 {
        0.0
    } else {
        ((radial - native_fraction) / (1.0 - native_fraction)).max(0.0)
    };
    if inner_resolution >= outer_resolution {
        // Preserve the measured perimeter once. Exterior detail falls with
        // distance and converges to the independently sampled sphere boundary.
        let density_ratio = (inner_resolution - 1) as f64 / (outer_resolution - 1) as f64;
        outer_resolution
            + (((inner_resolution - outer_resolution) as f64 * (1.0 - t).powi(2))
                / (1.0 + t * density_ratio))
                .ceil() as usize
    } else {
        inner_resolution
            + ((outer_resolution - inner_resolution) as f64 * t.powi(2)).ceil() as usize
    }
}

fn collar_radial_fraction(ring: usize, radial_segments: usize) -> f64 {
    let linear = ring as f64 / radial_segments as f64;
    linear * linear
}

fn rectangle_perimeter_coordinates(bounds: [f64; 4], resolution: usize) -> Vec<(f64, f64)> {
    let [min_x, max_x, min_z, max_z] = bounds;
    let mut perimeter = Vec::with_capacity(4 * (resolution - 1));
    let sample = |index, min, max| min + (max - min) * index as f64 / (resolution - 1) as f64;
    for index in 0..resolution {
        perimeter.push((sample(index, min_x, max_x), min_z));
    }
    for index in 1..resolution {
        perimeter.push((max_x, sample(index, min_z, max_z)));
    }
    for index in (0..resolution - 1).rev() {
        perimeter.push((sample(index, min_x, max_x), max_z));
    }
    for index in (1..resolution - 1).rev() {
        perimeter.push((min_x, sample(index, min_z, max_z)));
    }
    perimeter
}

fn connect_square_rings(
    inner_start: usize,
    inner_count: usize,
    outer_start: usize,
    outer_count: usize,
    indices: &mut Vec<u32>,
) {
    let mut inner = 0usize;
    let mut outer = 0usize;
    while inner < inner_count || outer < outer_count {
        let a = (inner_start + inner % inner_count) as u32;
        let c = (outer_start + outer % outer_count) as u32;
        if inner == inner_count {
            let d = (outer_start + (outer + 1) % outer_count) as u32;
            indices.extend_from_slice(&[a, d, c]);
            outer += 1;
            continue;
        }
        if outer == outer_count {
            let b = (inner_start + (inner + 1) % inner_count) as u32;
            indices.extend_from_slice(&[a, b, c]);
            inner += 1;
            continue;
        }
        let next_inner = (inner + 1) as u128 * outer_count as u128;
        let next_outer = (outer + 1) as u128 * inner_count as u128;
        match next_inner.cmp(&next_outer) {
            std::cmp::Ordering::Less => {
                let b = (inner_start + (inner + 1) % inner_count) as u32;
                indices.extend_from_slice(&[a, b, c]);
                inner += 1;
            }
            std::cmp::Ordering::Greater => {
                let d = (outer_start + (outer + 1) % outer_count) as u32;
                indices.extend_from_slice(&[a, d, c]);
                outer += 1;
            }
            std::cmp::Ordering::Equal => {
                let b = (inner_start + (inner + 1) % inner_count) as u32;
                let d = (outer_start + (outer + 1) % outer_count) as u32;
                indices.extend_from_slice(&[a, b, c, b, d, c]);
                inner += 1;
                outer += 1;
            }
        }
    }
}

/// Generate a mesh for one camera-selected QuadSphere tile.
pub fn create_quadsphere_tile_mesh(
    _body_ent: Entity,
    face: u8,
    level: u32,
    i: i32,
    j: i32,
    radius: f64,
    res: u32,
    tile_center: DVec3,
    cutout: Option<GlobeCutout>,
) -> Mesh {
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::new();
    let mut directions = Vec::new();
    let cutout_samples = cutout.map(|cutout| {
        (
            boundary_sample_coordinates(cutout.bounds[0], cutout.bounds[1], cutout.edge_segments),
            boundary_sample_coordinates(cutout.bounds[2], cutout.bounds[3], cutout.edge_segments),
        )
    });
    let tiles_at_level = 1 << level;
    let step = 2.0 / tiles_at_level as f64;
    let start_u = -1.0 + (i as f64) * step;
    let start_v = -1.0 + (j as f64) * step;

    for y in 0..=res {
        for x in 0..=res {
            let u = start_u + (x as f64 / res as f64) * step;
            let v = start_v + (y as f64 / res as f64) * step;
            let pos_sphere = cube_to_sphere(face, u, v);
            let (position, normal) = radial_sphere_vertex(pos_sphere, radius, tile_center);
            positions.push(position);
            normals.push(normal);
            directions.push(pos_sphere);
        }
    }

    for y in 0..res {
        for x in 0..res {
            let i0 = y * (res + 1) + x;
            let i1 = i0 + 1;
            let i2 = (y + 1) * (res + 1) + x;
            let i3 = i2 + 1;

            // CCW for sides, CW for Top/Bottom
            if face == 2 || face == 3 {
                indices.push(i0);
                indices.push(i2);
                indices.push(i1);
                indices.push(i1);
                indices.push(i2);
                indices.push(i3);
            } else {
                indices.push(i0);
                indices.push(i1);
                indices.push(i2);
                indices.push(i1);
                indices.push(i3);
                indices.push(i2);
            }
        }
    }

    if let Some(cutout) = cutout.filter(|cutout| cutout_intersects_tile(*cutout, &directions)) {
        let original_indices = std::mem::take(&mut indices);
        let original_directions = std::mem::take(&mut directions);
        let mut clipped_positions = Vec::new();
        let mut clipped_normals = Vec::new();
        let mut clipped_directions = Vec::new();
        let mut clipped_indices = Vec::new();
        for tri in original_indices.chunks_exact(3) {
            let dirs = [
                original_directions[tri[0] as usize],
                original_directions[tri[1] as usize],
                original_directions[tri[2] as usize],
            ];
            if dirs.iter().all(|&d| cutout.contains(d)) {
                continue;
            }

            let polygons = (0..5u8)
                .map(|region| {
                    let polygon = clip_triangle_to_region(&dirs, &cutout, region);
                    let split = subdivide_square_boundary_at_samples(
                        &polygon,
                        &dirs,
                        &cutout,
                        cutout_samples
                            .as_ref()
                            .map_or((&[][..], &[][..]), |(x, z)| (x.as_slice(), z.as_slice())),
                    );
                    let needs_interior_fan = split.len() > polygon.len();
                    (split, needs_interior_fan)
                })
                .collect::<Vec<_>>();
            for (polygon, interior_fan) in polygons {
                for triangle in triangulate_clipped_polygon(&polygon, interior_fan) {
                    let first = clipped_positions.len() as u32;
                    for v in triangle {
                        let dir = clipped_direction(v, &dirs, &cutout);
                        let (position, normal) = radial_sphere_vertex(dir, radius, tile_center);
                        clipped_positions.push(position);
                        clipped_normals.push(normal);
                        clipped_directions.push(dir);
                    }
                    clipped_indices.extend_from_slice(&[first, first + 1, first + 2]);
                }
            }
        }
        positions = clipped_positions;
        normals = clipped_normals;
        directions = clipped_directions;
        indices = clipped_indices;
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        bevy::asset::RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    // Author the spherical parameter itself, not an approximation of its
    // equirectangular projection. The fragment shader derives UV from this
    // direction, so texture identity cannot change with quadtree level or the
    // diagonal chosen to triangulate a coarse quad.
    mesh.insert_attribute(
        ATTRIBUTE_GLOBE_DIRECTION,
        directions
            .into_iter()
            .map(|direction| direction.as_vec3().to_array())
            .collect::<Vec<_>>(),
    );
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

#[derive(Clone, Copy)]
struct ClipVertex {
    bary: [f64; 3],
    chart: Option<[f64; 2]>,
}

fn radial_sphere_vertex(direction: DVec3, radius: f64, tile_center: DVec3) -> ([f32; 3], [f32; 3]) {
    let position = direction * radius;
    (
        (position - tile_center).as_vec3().to_array(),
        direction.as_vec3().to_array(),
    )
}

/// Evaluate the shared datum-aligned sphere in the local tangent chart. The
/// exterior collar converges to this same analytic shell.
fn collar_position(
    handoff: &GlobeHandoff,
    source: &dyn HeightSource,
    x: f64,
    z_south: f64,
    boundary_grid_resolution: usize,
) -> Result<DVec3, &'static str> {
    let height = collar_height(handoff, source, x, -z_south, boundary_grid_resolution)?;
    Ok(DVec3::new(x, height, z_south))
}

fn collar_height(
    handoff: &GlobeHandoff,
    source: &dyn HeightSource,
    x: f64,
    z_north: f64,
    boundary_grid_resolution: usize,
) -> Result<f64, &'static str> {
    if boundary_grid_resolution >= 2
        && x.abs() <= handoff.half_extent
        && z_north.abs() <= handoff.half_extent
    {
        square_boundary_height_at(
            source,
            x,
            -z_north,
            handoff.half_extent,
            boundary_grid_resolution,
        )
        .ok_or("collar inner vertex is outside the DEM posting perimeter")
    } else {
        Ok(source.height_at(x, -z_north))
    }
}

fn interpolate_dir(dirs: &[DVec3; 3], bary: [f64; 3]) -> DVec3 {
    (dirs[0] * bary[0] + dirs[1] * bary[1] + dirs[2] * bary[2]).normalize()
}

fn interpolate_chart(start: Option<[f64; 2]>, end: Option<[f64; 2]>, t: f64) -> Option<[f64; 2]> {
    start
        .zip(end)
        .map(|(start, end)| std::array::from_fn(|axis| start[axis] + (end[axis] - start[axis]) * t))
}

fn clipped_direction(vertex: ClipVertex, dirs: &[DVec3; 3], cutout: &GlobeCutout) -> DVec3 {
    match vertex.chart {
        Some([x, z]) => {
            let east = x / cutout.site_radius_m;
            let north = z / cutout.site_radius_m;
            cutout.dir * (1.0 - east * east - north * north).max(0.0).sqrt()
                + cutout.east * east
                + cutout.north * north
        }
        None => interpolate_dir(dirs, vertex.bary),
    }
}

fn boundary_sample_coordinates(min: f64, max: f64, segments: usize) -> Vec<f64> {
    (0..=segments)
        .map(|index| min + (max - min) * index as f64 / segments as f64)
        .collect()
}

fn subdivide_square_boundary_at_samples(
    polygon: &[ClipVertex],
    dirs: &[DVec3; 3],
    handoff: &GlobeCutout,
    samples: (&[f64], &[f64]),
) -> Vec<ClipVertex> {
    if polygon.len() < 2 {
        return polygon.to_vec();
    }

    let bounds = handoff.bounds;
    let tolerance = bounds
        .iter()
        .map(|bound| bound.abs())
        .fold(1.0_f64, f64::max)
        * 1.0e-9;
    let mut subdivided = Vec::with_capacity(polygon.len());
    for (index, start) in polygon.iter().copied().enumerate() {
        let end = polygon[(index + 1) % polygon.len()];
        subdivided.push(start);
        let Some([start_x, start_z]) = handoff.coordinates(clipped_direction(start, dirs, handoff))
        else {
            continue;
        };
        let Some([end_x, end_z]) = handoff.coordinates(clipped_direction(end, dirs, handoff))
        else {
            continue;
        };
        let on_x_side = ((start_x - bounds[0]).abs() <= tolerance
            && (end_x - bounds[0]).abs() <= tolerance)
            || ((start_x - bounds[1]).abs() <= tolerance && (end_x - bounds[1]).abs() <= tolerance);
        let on_z_side = ((start_z - bounds[2]).abs() <= tolerance
            && (end_z - bounds[2]).abs() <= tolerance)
            || ((start_z - bounds[3]).abs() <= tolerance && (end_z - bounds[3]).abs() <= tolerance);
        let (start_value, end_value, a, b, edge_samples) = if on_x_side {
            (start_z, end_z, 0.0, 1.0, samples.1)
        } else if on_z_side {
            (start_x, end_x, 1.0, 0.0, samples.0)
        } else {
            continue;
        };

        let min_value = start_value.min(end_value);
        let max_value = start_value.max(end_value);
        let segment_tolerance = tolerance;
        let first_sample =
            edge_samples.partition_point(|target| *target <= min_value + segment_tolerance);
        let end_sample =
            edge_samples.partition_point(|target| *target < max_value - segment_tolerance);
        let add_sample = |target: f64, subdivided: &mut Vec<ClipVertex>| {
            // Interpolate in the shared orthographic chart, preserving the
            // fixed side coordinate as well as the requested edge sample.
            let c = -target;
            subdivided.push(intersect_chart_boundary(
                start, end, dirs, handoff, 0, a, b, c,
            ));
        };
        if first_sample >= end_sample {
            continue;
        }
        if end_value > start_value {
            for &target in &edge_samples[first_sample..end_sample] {
                add_sample(target, &mut subdivided);
            }
        } else {
            for &target in edge_samples[first_sample..end_sample].iter().rev() {
                add_sample(target, &mut subdivided);
            }
        }
    }
    subdivided
}

fn clip_triangle_to_half_planes(
    dirs: &[DVec3; 3],
    handoff: &GlobeCutout,
    region: u8,
    cuts: &[(f64, f64, f64)],
) -> Vec<ClipVertex> {
    let mut polygon = vec![
        ClipVertex {
            bary: [1.0, 0.0, 0.0],
            chart: None,
        },
        ClipVertex {
            bary: [0.0, 1.0, 0.0],
            chart: None,
        },
        ClipVertex {
            bary: [0.0, 0.0, 1.0],
            chart: None,
        },
    ];
    if dirs
        .iter()
        .all(|direction| direction.dot(handoff.dir) > 0.0)
    {
        for (vertex, direction) in polygon.iter_mut().zip(dirs) {
            vertex.chart = handoff.coordinates(*direction);
        }
    }
    for &(a, b, c) in cuts {
        if polygon.is_empty() {
            break;
        }
        let mut next = Vec::with_capacity(polygon.len() + 1);
        let Some(mut start) = polygon.last().copied() else {
            break;
        };
        let mut fs = region_value(start, dirs, handoff, region, a, b, c);
        for &end in &polygon {
            let fe = region_value(end, dirs, handoff, region, a, b, c);
            let start_inside = fs <= 0.0;
            let end_inside = fe <= 0.0;
            if start_inside != end_inside {
                next.push(intersect_chart_boundary(
                    start, end, dirs, handoff, region, a, b, c,
                ));
            }
            if end_inside {
                next.push(end);
            }
            start = end;
            fs = fe;
        }
        polygon = next;
    }
    polygon
}

fn intersect_chart_boundary(
    start: ClipVertex,
    end: ClipVertex,
    dirs: &[DVec3; 3],
    handoff: &GlobeCutout,
    region: u8,
    a: f64,
    b: f64,
    c: f64,
) -> ClipVertex {
    if start.chart.is_some() && end.chart.is_some() && (a != 0.0 || b != 0.0) {
        let fs = region_value(start, dirs, handoff, region, a, b, c);
        let fe = region_value(end, dirs, handoff, region, a, b, c);
        let t = fs / (fs - fe);
        return ClipVertex {
            bary: std::array::from_fn(|axis| {
                start.bary[axis] + (end.bary[axis] - start.bary[axis]) * t
            }),
            chart: interpolate_chart(start.chart, end.chart, t),
        };
    }
    let start_inside = region_value(start, dirs, handoff, region, a, b, c) <= 0.0;
    let mut low = 0.0;
    let mut high = 1.0;
    for _ in 0..52 {
        let t = (low + high) * 0.5;
        let midpoint = ClipVertex {
            bary: std::array::from_fn(|component| {
                start.bary[component] + (end.bary[component] - start.bary[component]) * t
            }),
            chart: interpolate_chart(start.chart, end.chart, t),
        };
        let midpoint_inside = region_value(midpoint, dirs, handoff, region, a, b, c) <= 0.0;
        if midpoint_inside == start_inside {
            low = t;
        } else {
            high = t;
        }
    }
    let t = (low + high) * 0.5;
    ClipVertex {
        bary: std::array::from_fn(|component| {
            start.bary[component] + (end.bary[component] - start.bary[component]) * t
        }),
        chart: interpolate_chart(start.chart, end.chart, t),
    }
}

fn triangulate_clipped_polygon(polygon: &[ClipVertex], interior_fan: bool) -> Vec<[ClipVertex; 3]> {
    if polygon.len() < 3 {
        return Vec::new();
    }
    if interior_fan {
        if let Some(center) = polygon_centroid(polygon) {
            return (0..polygon.len())
                .map(|index| [center, polygon[index], polygon[(index + 1) % polygon.len()]])
                .collect();
        }
    }
    (1..polygon.len() - 1)
        .map(|index| [polygon[0], polygon[index], polygon[index + 1]])
        .collect()
}

/// Find the area centroid in the original triangle's barycentric plane so
/// bounded samples along the cutout edge do not create long fan triangles.
fn polygon_centroid(polygon: &[ClipVertex]) -> Option<ClipVertex> {
    let first = *polygon.first()?;
    let mut total_area = 0.0;
    let mut bary = [0.0; 3];
    let mut chart = [0.0; 2];
    let has_chart = polygon.iter().all(|vertex| vertex.chart.is_some());
    for pair in polygon[1..].windows(2) {
        let a = pair[0];
        let b = pair[1];
        let u = [a.bary[1] - first.bary[1], a.bary[2] - first.bary[2]];
        let v = [b.bary[1] - first.bary[1], b.bary[2] - first.bary[2]];
        let area = u[0] * v[1] - u[1] * v[0];
        total_area += area;
        for axis in 0..3 {
            bary[axis] += area * (first.bary[axis] + a.bary[axis] + b.bary[axis]) / 3.0;
        }
        if has_chart {
            let origin = first.chart.expect("front chart");
            let ac = a.chart.expect("front chart");
            let bc = b.chart.expect("front chart");
            for axis in 0..2 {
                chart[axis] += area * (origin[axis] + ac[axis] + bc[axis]) / 3.0;
            }
        }
    }
    if !total_area.is_finite() || total_area.abs() <= f64::EPSILON {
        return None;
    }
    let bary = bary.map(|value| value / total_area);
    let chart = has_chart.then(|| chart.map(|value| value / total_area));
    bary.iter()
        .all(|value| value.is_finite())
        .then_some(ClipVertex { bary, chart })
}

/// Conservative tile-level rejection for the rectangular cutout.
fn cutout_intersects_tile(cutout: GlobeCutout, directions: &[DVec3]) -> bool {
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_z = f64::INFINITY;
    let mut max_z = f64::NEG_INFINITY;
    for &direction in directions {
        let Some([x, z]) = cutout.coordinates(direction) else {
            return true;
        };
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_z = min_z.min(z);
        max_z = max_z.max(z);
    }
    !(max_x < cutout.bounds[0]
        || min_x > cutout.bounds[1]
        || max_z < cutout.bounds[2]
        || min_z > cutout.bounds[3])
}

fn clip_triangle_to_region(
    dirs: &[DVec3; 3],
    handoff: &GlobeCutout,
    region: u8,
) -> Vec<ClipVertex> {
    let cuts: &[(f64, f64, f64)] = match region {
        0 => &[
            (0.0, 0.0, 0.0), // denominator >= 0
            (1.0, 0.0, -handoff.bounds[0]),
        ], // x <= min_x
        1 => &[
            (0.0, 0.0, 0.0), // denominator >= 0
            (-1.0, 0.0, handoff.bounds[1]),
        ], // x >= max_x
        2 => &[
            (0.0, 0.0, 0.0), // denominator >= 0
            (0.0, -1.0, handoff.bounds[3]),
            (-1.0, 0.0, handoff.bounds[0]),
            (1.0, 0.0, -handoff.bounds[1]),
        ], // z >= max_z, between the two x edges
        3 => &[
            (0.0, 0.0, 0.0), // denominator >= 0
            (0.0, 1.0, -handoff.bounds[2]),
            (-1.0, 0.0, handoff.bounds[0]),
            (1.0, 0.0, -handoff.bounds[1]),
        ],
        4 => &[(0.0, 0.0, 0.0)], // denominator <= 0: the back hemisphere
        _ => &[],
    };
    clip_triangle_to_half_planes(dirs, handoff, region, cuts)
}

fn region_value(
    vertex: ClipVertex,
    dirs: &[DVec3; 3],
    handoff: &GlobeCutout,
    region: u8,
    a: f64,
    b: f64,
    c: f64,
) -> f64 {
    let raw = dirs[0] * vertex.bary[0] + dirs[1] * vertex.bary[1] + dirs[2] * vertex.bary[2];
    if region == 4 {
        return raw.dot(handoff.dir);
    }
    if a == 0.0 && b == 0.0 && c == 0.0 {
        return -raw.dot(handoff.dir);
    }
    let [x, z] = vertex.chart.unwrap_or_else(|| {
        let direction = raw.normalize_or_zero();
        [
            direction.dot(handoff.east) * handoff.site_radius_m,
            direction.dot(handoff.north) * handoff.site_radius_m,
        ]
    });
    a * x + b * z + c
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::DMat3;
    use bevy::mesh::VertexAttributeValues;
    use lunco_materials::{ATTRIBUTE_MORPH_EDGE, ATTRIBUTE_MORPH_NORMAL, ATTRIBUTE_MORPH_TARGET};

    const RADIUS: f64 = 100_000.0;
    const HALF_EXTENT: f64 = 10_000.0;
    const COLLAR: f64 = 20_000.0;

    #[derive(Clone, Copy)]
    struct EdgeToSphere;

    impl HeightSource for EdgeToSphere {
        fn height_at(&self, x: f64, z_south: f64) -> f64 {
            (RADIUS * RADIUS - x * x - z_south * z_south).sqrt() - RADIUS
        }
    }

    #[derive(Clone, Copy)]
    struct SlopedEdgeToSphere;

    impl HeightSource for SlopedEdgeToSphere {
        fn height_at(&self, x: f64, z_south: f64) -> f64 {
            (RADIUS * RADIUS - x * x - z_south * z_south).sqrt() - RADIUS
                + 0.15 * x
                + 0.08 * z_south
        }
    }

    fn handoff() -> GlobeHandoff {
        GlobeHandoff {
            dir: DVec3::X,
            east: DVec3::Y,
            north: DVec3::Z,
            radius_m: RADIUS,
            site_radius_m: RADIUS,
            half_extent: HALF_EXTENT,
            collar_m: COLLAR,
            collar_widths: [COLLAR; 4],
            edge_segments: MIN_GLOBE_CUTOUT_EDGE_SEGMENTS,
        }
    }

    #[test]
    fn asymmetric_collar_widths_map_to_matching_cutout_sides() {
        let widths = [10.0, 20.0, 30.0, 40.0];
        let mesh = handoff_mesh_bounds(100.0, widths);
        let cutout = handoff_cutout_bounds(100.0, widths);
        assert_eq!(mesh, [-110.0, 120.0, -140.0, 130.0]);
        assert_eq!(cutout, [-110.0, 120.0, -130.0, 140.0]);
        assert_eq!([cutout[2], cutout[3]], [-mesh[3], -mesh[2]]);
    }

    fn tangent_to_body(handoff: GlobeHandoff) -> DMat3 {
        DMat3::from_cols(handoff.east, handoff.dir, -handoff.north)
    }

    fn body_position(handoff: GlobeHandoff, local: DVec3) -> DVec3 {
        handoff.dir * handoff.radius_m + tangent_to_body(handoff) * local
    }

    fn chart_direction(cutout: GlobeCutout, x: f64, z: f64) -> DVec3 {
        let radial = (1.0 - (x * x + z * z) / cutout.site_radius_m.powi(2)).sqrt();
        cutout.dir * radial
            + cutout.east * (x / cutout.site_radius_m)
            + cutout.north * (z / cutout.site_radius_m)
    }

    fn cutout_for(center: DVec3, half_extent: f64) -> GlobeCutout {
        let helper = if center.y.abs() < 0.9 {
            DVec3::Y
        } else {
            DVec3::X
        };
        let east = helper.cross(center).normalize();
        let north = center.cross(east).normalize();
        GlobeCutout {
            dir: center,
            east,
            north,
            site_radius_m: RADIUS,
            bounds: [-half_extent, half_extent, -half_extent, half_extent],
            edge_segments: MIN_GLOBE_CUTOUT_EDGE_SEGMENTS,
        }
    }

    fn point_in_polygon(point: [f64; 2], polygon: &[ClipVertex]) -> bool {
        if polygon.len() < 3 {
            return false;
        }
        let signed_area: f64 = polygon
            .iter()
            .enumerate()
            .map(|(index, vertex)| {
                let next = polygon[(index + 1) % polygon.len()].bary;
                vertex.bary[1] * next[2] - next[1] * vertex.bary[2]
            })
            .sum();
        let orientation = signed_area.signum();
        polygon.iter().enumerate().all(|(index, vertex)| {
            let next = polygon[(index + 1) % polygon.len()].bary;
            let a = [vertex.bary[1], vertex.bary[2]];
            let b = [next[1], next[2]];
            orientation * ((b[0] - a[0]) * (point[1] - a[1]) - (b[1] - a[1]) * (point[0] - a[0]))
                >= -1.0e-12
        })
    }

    #[test]
    fn one_collar_mesh_matches_dem_edge_and_radial_globe_boundary() {
        let handoff = handoff();
        let source = EdgeToSphere;
        let resolution = 5;
        let radial_segments = 4;
        let ring_resolutions = (0..=radial_segments)
            .map(|ring| {
                collar_ring_resolution(
                    resolution,
                    handoff.edge_segments + 1,
                    ring,
                    radial_segments,
                    handoff,
                )
            })
            .collect::<Vec<_>>();
        let mesh = create_rectangular_handoff_collar_mesh(
            handoff,
            &source,
            resolution,
            handoff.edge_segments,
            radial_segments,
        )
        .expect("valid finite crop collar");
        let VertexAttributeValues::Float32x3(positions) = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .expect("collar positions")
        else {
            panic!("collar positions have an unexpected format");
        };
        let VertexAttributeValues::Float32x3(normals) = mesh
            .attribute(Mesh::ATTRIBUTE_NORMAL)
            .expect("collar normals")
        else {
            panic!("collar normals have an unexpected format");
        };
        let VertexAttributeValues::Float32x3(directions) = mesh
            .attribute(ATTRIBUTE_GLOBE_DIRECTION)
            .expect("collar directions")
        else {
            panic!("collar directions have an unexpected format");
        };
        let VertexAttributeValues::Float32x2(uvs) =
            mesh.attribute(Mesh::ATTRIBUTE_UV_0).expect("collar UVs")
        else {
            panic!("collar UVs have an unexpected format");
        };
        assert!(mesh.attribute(ATTRIBUTE_MORPH_TARGET).is_none());
        assert!(mesh.attribute(ATTRIBUTE_MORPH_NORMAL).is_none());
        assert!(mesh.attribute(ATTRIBUTE_MORPH_EDGE).is_none());

        let inner_corner = body_position(handoff, DVec3::from_array(positions[0].map(f64::from)));
        let inner_height = source.height_at(-HALF_EXTENT, -HALF_EXTENT);
        let expected_inner = handoff.dir * (RADIUS + inner_height) - handoff.east * HALF_EXTENT
            + handoff.north * HALF_EXTENT;
        assert!((inner_corner - expected_inner).length() < 2.0e-3);

        let outer_start = ring_resolutions[..radial_segments]
            .iter()
            .map(|ring_resolution| 4 * (ring_resolution - 1))
            .sum::<usize>();
        let outer_count = 4 * handoff.edge_segments;
        let outer_bounds = [
            -HALF_EXTENT - COLLAR,
            HALF_EXTENT + COLLAR,
            -HALF_EXTENT - COLLAR,
            HALF_EXTENT + COLLAR,
        ];
        let cutout = GlobeCutout {
            dir: handoff.dir,
            east: handoff.east,
            north: handoff.north,
            site_radius_m: handoff.site_radius_m,
            bounds: outer_bounds,
            edge_segments: handoff.edge_segments,
        };
        let expected_perimeter =
            rectangle_perimeter_coordinates(outer_bounds, handoff.edge_segments + 1);
        assert_eq!(expected_perimeter.len(), outer_count);
        for (index, &(x, z_south)) in expected_perimeter.iter().enumerate() {
            let position = body_position(
                handoff,
                DVec3::from_array(positions[outer_start + index].map(f64::from)),
            );
            assert!((position.length() - RADIUS).abs() < 2.0e-3);
            let expected = chart_direction(cutout, x, -z_south) * RADIUS;
            assert!(
                (position - expected).length() < 2.0e-3,
                "outer collar/cutout mismatch at {index}: position={position:?}, expected={expected:?}"
            );
            let normal = DVec3::from_array(normals[outer_start + index].map(f64::from));
            assert!(
                (tangent_to_body(handoff) * normal).dot(position.normalize()) > 0.98,
                "outer collar normal mismatch at {index}: normal={normal:?}, position={position:?}"
            );
        }

        let outer_mid_index = outer_start + handoff.edge_segments / 2;
        let outer_mid = body_position(
            handoff,
            DVec3::from_array(positions[outer_mid_index].map(f64::from)),
        );
        let outer_mid_direction = DVec3::from_array(directions[outer_mid_index].map(f64::from));
        let expected_globe = chart_direction(cutout, 0.0, HALF_EXTENT + COLLAR) * RADIUS;
        assert!((outer_mid - expected_globe).length() < 2.0e-3);
        assert!(outer_mid_direction.distance(expected_globe.normalize()) < 1.0e-6);
        assert_eq!(uvs[0], [0.0, 0.0]);
        assert!(uvs[outer_start][0] < 0.0 && uvs[outer_start][1] < 0.0);

        let indices = mesh.indices().expect("collar indices");
        let Indices::U32(indices) = indices else {
            panic!("collar mesh must use 32-bit indices");
        };
        for triangle in indices.chunks_exact(3) {
            let point = |index: u32| {
                body_position(
                    handoff,
                    DVec3::from_array(positions[index as usize].map(f64::from)),
                )
            };
            let a = point(triangle[0]);
            let b = point(triangle[1]);
            let c = point(triangle[2]);
            assert!((b - a).cross(c - a).dot(handoff.dir) > 0.0);
        }
    }

    #[test]
    fn collar_inner_corner_normal_preserves_both_measured_edge_slopes() {
        let handoff = handoff();
        let resolution = 5;
        let mesh =
            create_rectangular_handoff_collar_mesh(handoff, &SlopedEdgeToSphere, resolution, 32, 4)
                .expect("valid finite crop collar");
        let VertexAttributeValues::Float32x3(normals) = mesh
            .attribute(Mesh::ATTRIBUTE_NORMAL)
            .expect("collar normals")
        else {
            panic!("collar normals have an unexpected format");
        };

        let x = -HALF_EXTENT;
        let z_north = HALF_EXTENT;
        let radial = (RADIUS * RADIUS - x * x - z_north * z_north).sqrt();
        let gradient_x = -x / radial + 0.15;
        let gradient_z_north = -z_north / radial - 0.08;
        let expected = DVec3::new(-gradient_x, 1.0, gradient_z_north).normalize();
        let actual = DVec3::from_array(normals[0].map(f64::from));
        assert!(
            actual.dot(expected) > 0.999,
            "inner corner normal lost a measured edge slope: actual={actual:?}, expected={expected:?}"
        );
    }

    #[test]
    fn collar_rings_concentrate_samples_at_the_measured_edge() {
        assert!((collar_radial_fraction(1, 16) - 1.0 / 256.0).abs() < f64::EPSILON);
        assert_eq!(collar_radial_fraction(16, 16), 1.0);
        let near_edge = collar_ring_resolution(512, 33, 1, 128, handoff());
        let outer = collar_ring_resolution(512, 33, 16, 16, handoff());
        assert!(near_edge == 512, "near-edge resolution was {near_edge}");
        assert_eq!(outer, 33);
    }

    #[test]
    fn collar_mesh_rejects_invalid_dimensions_and_non_finite_source_geometry() {
        let handoff = handoff();
        assert!(create_rectangular_handoff_collar_mesh(handoff, &EdgeToSphere, 1, 32, 4).is_err());
        for (inner, outer, radial) in [
            (5, usize::MAX, 4),
            (5, 32, usize::MAX),
            (usize::MAX, 32, 4),
            (5, 32, u32::MAX as usize),
        ] {
            assert_eq!(
                create_rectangular_handoff_collar_mesh(
                    handoff,
                    &EdgeToSphere,
                    inner,
                    outer,
                    radial
                )
                .unwrap_err(),
                "collar mesh exceeds supported index capacity"
            );
        }
        let mut invalid = handoff;
        invalid.collar_widths[2] = f64::NAN;
        assert!(create_rectangular_handoff_collar_mesh(invalid, &EdgeToSphere, 5, 32, 4).is_err());
    }

    #[test]
    fn globe_cutout_uses_fixed_edge_sampling_independent_of_dem_resolution() {
        let edge_segments = MIN_GLOBE_CUTOUT_EDGE_SEGMENTS;
        let samples =
            boundary_sample_coordinates(-HALF_EXTENT - COLLAR, HALF_EXTENT + COLLAR, edge_segments);
        assert_eq!(samples.len(), edge_segments + 1);
        assert_eq!(samples[0], -(HALF_EXTENT + COLLAR));
        assert_eq!(samples[samples.len() - 1], HALF_EXTENT + COLLAR);
        assert!(
            (samples[1] - samples[0] - 2.0 * (HALF_EXTENT + COLLAR) / edge_segments as f64).abs()
                < 1.0e-9
        );
    }

    #[test]
    fn rectangular_cutout_partitions_the_globe_triangles_without_gaps_or_overlap() {
        let cutout = GlobeCutout {
            dir: DVec3::X,
            east: DVec3::Y,
            north: DVec3::Z,
            site_radius_m: 100.0,
            bounds: [-10.0, 10.0, -12.0, 20.0],
            edge_segments: MIN_GLOBE_CUTOUT_EDGE_SEGMENTS,
        };
        let direction_at = |x: f64, z: f64| chart_direction(cutout, x, z);
        let dirs = [
            direction_at(-25.0, -20.0),
            direction_at(25.0, -20.0),
            direction_at(0.0, 25.0),
        ];
        let samples = (
            boundary_sample_coordinates(cutout.bounds[0], cutout.bounds[1], cutout.edge_segments),
            boundary_sample_coordinates(cutout.bounds[2], cutout.bounds[3], cutout.edge_segments),
        );
        let polygons = (0..5u8)
            .map(|region| {
                let polygon = clip_triangle_to_region(&dirs, &cutout, region);
                let split = subdivide_square_boundary_at_samples(
                    &polygon,
                    &dirs,
                    &cutout,
                    (&samples.0, &samples.1),
                );
                (split.clone(), split.len() > polygon.len())
            })
            .filter(|(polygon, _)| polygon.len() >= 3)
            .collect::<Vec<_>>();

        for first in 1..40 {
            for second in 1..40 - first {
                let bary = [
                    first as f64 / 40.0,
                    second as f64 / 40.0,
                    1.0 - (first + second) as f64 / 40.0,
                ];
                let [x, z] = std::array::from_fn(|axis| {
                    dirs.iter()
                        .zip(bary)
                        .map(|(direction, weight)| {
                            cutout.coordinates(*direction).expect("front-side sample")[axis]
                                * weight
                        })
                        .sum::<f64>()
                });
                if (x - cutout.bounds[0]).abs() < 0.1
                    || (x - cutout.bounds[1]).abs() < 0.1
                    || (z - cutout.bounds[2]).abs() < 0.1
                    || (z - cutout.bounds[3]).abs() < 0.1
                {
                    continue;
                }
                let count = polygons
                    .iter()
                    .filter(|(polygon, _)| point_in_polygon([bary[1], bary[2]], polygon))
                    .count();
                let inside_cutout = x > cutout.bounds[0]
                    && x < cutout.bounds[1]
                    && z > cutout.bounds[2]
                    && z < cutout.bounds[3];
                assert_eq!(
                    count,
                    usize::from(!inside_cutout),
                    "cutout sample ({x}, {z})"
                );
            }
        }
    }

    #[test]
    fn cutout_tile_mesh_remains_radial_and_outward_wound() {
        let center = cube_to_sphere(0, 0.0, 0.0);
        let cutout = cutout_for(center, 20_000.0);
        let mesh = create_quadsphere_tile_mesh(
            Entity::PLACEHOLDER,
            0,
            0,
            0,
            0,
            RADIUS,
            8,
            center * RADIUS,
            Some(cutout),
        );
        let VertexAttributeValues::Float32x3(positions) = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .expect("tile positions")
        else {
            panic!("tile positions have an unexpected format");
        };
        let VertexAttributeValues::Float32x3(normals) = mesh
            .attribute(Mesh::ATTRIBUTE_NORMAL)
            .expect("tile normals")
        else {
            panic!("tile normals have an unexpected format");
        };
        let Indices::U32(indices) = mesh.indices().expect("tile indices") else {
            panic!("tile mesh must use 32-bit indices");
        };
        assert!(!indices.is_empty());
        for triangle in indices.chunks_exact(3) {
            let a =
                DVec3::from_array(positions[triangle[0] as usize].map(f64::from)) + center * RADIUS;
            let b =
                DVec3::from_array(positions[triangle[1] as usize].map(f64::from)) + center * RADIUS;
            let c =
                DVec3::from_array(positions[triangle[2] as usize].map(f64::from)) + center * RADIUS;
            let geometric = (b - a).cross(c - a);
            if geometric.length_squared() < 1.0e-8 {
                continue;
            }
            let supplied = DVec3::from_array(normals[triangle[0] as usize].map(f64::from))
                + DVec3::from_array(normals[triangle[1] as usize].map(f64::from))
                + DVec3::from_array(normals[triangle[2] as usize].map(f64::from));
            assert!(
                geometric.dot(supplied) > 0.0,
                "inward clipped triangle: a={a:?}, b={b:?}, c={c:?}, dot={}",
                geometric.dot(supplied)
            );
            assert!((a.length() - RADIUS).abs() < 0.05);
        }
    }

    #[test]
    fn cutout_covering_a_selected_tile_removes_its_globe_triangles() {
        let face = 0;
        let level = 3;
        let i = 3;
        let j = 3;
        let step = 2.0 / f64::from(1 << level);
        let start_u = -1.0 + f64::from(i) * step;
        let start_v = -1.0 + f64::from(j) * step;
        let center = cube_to_sphere(face, start_u + step * 0.5, start_v + step * 0.5);
        let cutout = cutout_for(center, RADIUS * step * 0.75);
        let mesh = create_quadsphere_tile_mesh(
            Entity::PLACEHOLDER,
            face,
            level,
            i,
            j,
            RADIUS,
            8,
            center * RADIUS,
            Some(cutout),
        );
        assert!(mesh.indices().is_some_and(Indices::is_empty));
    }

    #[test]
    fn globe_mesh_authors_body_fixed_direction_instead_of_projected_uv() {
        let radius = 100.0;
        let tile_center = cube_to_sphere(0, 0.0, 0.0) * radius;
        let mesh = create_quadsphere_tile_mesh(
            Entity::PLACEHOLDER,
            0,
            0,
            0,
            0,
            radius,
            4,
            tile_center,
            None,
        );
        let directions = mesh
            .attribute(ATTRIBUTE_GLOBE_DIRECTION)
            .expect("body-fixed globe direction attribute");
        assert!(matches!(directions, VertexAttributeValues::Float32x3(_)));
        let VertexAttributeValues::Float32x3(positions) = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .expect("globe positions")
        else {
            panic!("globe positions have an unexpected format");
        };
        let VertexAttributeValues::Float32x3(directions) = directions else {
            unreachable!();
        };
        for (position, direction) in positions.iter().zip(directions) {
            let position = DVec3::from_array(position.map(f64::from)) + tile_center;
            let direction = DVec3::from_array(direction.map(f64::from));
            assert!(position.normalize().distance(direction) < 1.0e-6);
        }
    }
    #[test]
    fn asymmetric_collar_preserves_finite_dem_edge_and_reduces_exterior_density() {
        let mut geometry = handoff();
        geometry.collar_widths = [11_000.0, 20_000.0, 15_000.0, 18_000.0];
        let source = EdgeToSphere;
        // Extensions of a finite DEM edge must evaluate the exterior sphere,
        // rather than clamping to the measured corner height.
        let z = HALF_EXTENT + geometry.collar_widths[2];
        let height = collar_height(&geometry, &source, HALF_EXTENT, -z, 129).unwrap();
        assert_eq!(height, source.height_at(HALF_EXTENT, z));
        let radial_segments = 32;
        let inner_resolution = 2049;
        let outer_resolution = geometry.edge_segments + 1;
        let counts = (0..=radial_segments)
            .map(|ring| {
                4 * (collar_ring_resolution(
                    inner_resolution,
                    outer_resolution,
                    ring,
                    radial_segments,
                    geometry,
                ) - 1)
            })
            .collect::<Vec<_>>();
        assert_eq!(counts[0], 4 * (inner_resolution - 1));
        assert_eq!(*counts.last().unwrap(), 4 * geometry.edge_segments);
        assert!(counts.windows(2).all(|pair| pair[1] <= pair[0]));
        assert!(counts.iter().sum::<usize>() < counts[0] * (radial_segments + 1) / 4);
        let mesh = create_rectangular_handoff_collar_mesh(
            geometry,
            &source,
            129,
            geometry.edge_segments,
            16,
        )
        .unwrap();
        let VertexAttributeValues::Float32x3(positions) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap()
        else {
            panic!("collar position layout");
        };
        let outer_count = 4 * geometry.edge_segments;
        let cutout = GlobeCutout {
            dir: geometry.dir,
            east: geometry.east,
            north: geometry.north,
            site_radius_m: geometry.site_radius_m,
            bounds: handoff_cutout_bounds(geometry.half_extent, geometry.collar_widths),
            edge_segments: geometry.edge_segments,
        };
        let perimeter = rectangle_perimeter_coordinates(
            handoff_mesh_bounds(geometry.half_extent, geometry.collar_widths),
            outer_resolution,
        );
        for (vertex, &(x, z)) in positions[positions.len() - outer_count..]
            .iter()
            .zip(&perimeter)
        {
            let actual = body_position(geometry, DVec3::from_array(vertex.map(f64::from)));
            let expected = chart_direction(cutout, x, -z) * geometry.radius_m;
            assert!(actual.distance(expected) < 0.005);
        }
        geometry.collar_widths[0] = f64::NAN;
        assert!(create_rectangular_handoff_collar_mesh(geometry, &source, 129, 32, 16).is_err());
    }

    #[test]
    fn subdivided_cutout_edges_keep_both_rectangle_coordinates() {
        let cutout = GlobeCutout {
            dir: DVec3::X,
            east: DVec3::Y,
            north: DVec3::Z,
            site_radius_m: 100.0,
            bounds: [-10.0, 10.0, -12.0, 20.0],
            edge_segments: 32,
        };
        let dirs = [
            chart_direction(cutout, -25.0, -20.0),
            chart_direction(cutout, 25.0, -20.0),
            chart_direction(cutout, 0.0, 25.0),
        ];
        let samples = (
            boundary_sample_coordinates(-10.0, 10.0, 32),
            boundary_sample_coordinates(-12.0, 20.0, 32),
        );
        let mut checked = 0;
        for region in 0..4 {
            let polygon = clip_triangle_to_region(&dirs, &cutout, region);
            let split = subdivide_square_boundary_at_samples(
                &polygon,
                &dirs,
                &cutout,
                (&samples.0, &samples.1),
            );
            for vertex in &split {
                let [x, z] = vertex.chart.expect("front chart");
                let [actual_x, actual_z] = cutout
                    .coordinates(clipped_direction(*vertex, &dirs, &cutout))
                    .unwrap();
                assert!((actual_x - x).abs() < 1e-10 && (actual_z - z).abs() < 1e-10);
                if x == -10.0 || x == 10.0 || z == -12.0 || z == 20.0 {
                    checked += 1;
                }
            }
        }
        assert!(checked > 32, "no densely sampled cutout boundary exercised");
    }
}
