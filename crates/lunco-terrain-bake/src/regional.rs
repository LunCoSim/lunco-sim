//! Registered local and regional DEM sampling.
//!
//! A local grid remains exact through its boundary. Outside that boundary,
//! its measured edge derivatives continue into a finite transition to the
//! registered regional grid. This module owns the pure sampling math; terrain
//! lifecycle and all runtime consumers remain the surface owner's responsibility.

use std::sync::Arc;

use lunco_geotiff::{GeoTransform, LocalProjection};
use lunco_obstacle_field::field::HeightGrid;
use lunco_terrain_core::{
    CompositeHeightSource, HeightEnclosure, HeightPlane, HeightSource, Square,
};

struct ContinuedLocal(Arc<HeightGrid>, Arc<HeightGrid>);

/// Preserve the measured edge derivative while bounding its contribution to
/// one local sample spacing. Sub-grid edge slopes must not be extrapolated over
/// the much wider regional blend.
fn continued_distance(distance: f64, spacing: f64) -> f64 {
    distance / (1.0 + distance.abs() / spacing)
}

impl HeightSource for ContinuedLocal {
    fn height_at(&self, x: f64, z: f64) -> f64 {
        let half = f64::from(self.0.half_extent);
        let edge_x = x.clamp(-half, half);
        let edge_z = z.clamp(-half, half);
        let dx = x - edge_x;
        let dz = z - edge_z;
        if dx == 0.0 && dz == 0.0 {
            return self.0.height_at(x, z);
        }
        let continuation = |grid: &HeightGrid| {
            let (height, gradient, mixed) = grid.height_and_derivatives_at(edge_x, edge_z);
            let spacing = f64::from(grid.spacing());
            let dx = continued_distance(dx, spacing);
            let dz = continued_distance(dz, spacing);
            height + gradient[0] * dx + gradient[1] * dz + mixed * dx * dz
        };
        // Preserve the regional grid's observed relief in the transition;
        // continue only the difference between the two registered surfaces.
        self.1.height_at(x, z) + continuation(&self.0) - continuation(&self.1)
    }
}

struct OffsetRegional {
    grid: Arc<HeightGrid>,
    offset_m: f64,
}

impl HeightSource for OffsetRegional {
    fn height_at(&self, x: f64, z: f64) -> f64 {
        self.grid.height_at(x, z) + self.offset_m
    }
}

/// Immutable, co-registered grids and an explicitly chosen handoff width.
pub struct RegisteredRegionalSurface {
    source: CompositeHeightSource<ContinuedLocal, OffsetRegional>,
    content_key: u64,
    local_content_key: u64,
}

fn validate_grid(grid: &HeightGrid, geo: &GeoTransform) -> Result<(), &'static str> {
    if grid.res < 2
        || grid.res.checked_mul(grid.res) != Some(grid.heights.len())
        || !grid.half_extent.is_finite()
        || grid.half_extent <= 0.0
        || !grid.spacing().is_finite()
        || grid.spacing() <= 0.0
        || grid.heights.iter().any(|height| !height.is_finite())
        || !geo.pixel_size_m.is_finite()
        || geo.pixel_size_m <= 0.0
        || !geo.origin_x_m.is_finite()
        || !geo.origin_y_m.is_finite()
    {
        return Err("invalid regional or local height grid");
    }
    // HeightGrid stores geometry as f32; check the authored f64 geometry against
    // that explicit representation rather than accepting a shifted raster.
    let half = f64::from(grid.half_extent);
    let tolerance = (half * f64::from(f32::EPSILON) * 4.0).max(1e-6);
    if (geo.pixel_size_m * (grid.res - 1) as f64 * 0.5 - half).abs() > tolerance
        || (geo.origin_x_m + half).abs() > tolerance
        || (geo.origin_y_m - half).abs() > tolerance
    {
        return Err("height grid is not centered on its declared sample geometry");
    }
    Ok(())
}

impl RegisteredRegionalSurface {
    /// Admit two centered grids in the same declared body frame and projection.
    /// `regional_offset_m` explicitly converts the regional vertical datum into
    /// the local grid's datum; the owner must obtain it from source provenance.
    pub fn new(
        local: Arc<HeightGrid>,
        local_geo: GeoTransform,
        regional: Arc<HeightGrid>,
        regional_geo: GeoTransform,
        blend_m: f64,
        regional_offset_m: f64,
    ) -> Result<Self, &'static str> {
        validate_grid(&local, &local_geo)?;
        validate_grid(&regional, &regional_geo)?;
        if local_geo.frame.is_none()
            || local_geo.frame != regional_geo.frame
            || local_geo.projection.is_none()
            || local_geo.projection != regional_geo.projection
            || !local_geo.body_radius_m.is_finite()
            || local_geo.body_radius_m <= 0.0
            || !regional_geo.body_radius_m.is_finite()
            || (local_geo.body_radius_m - regional_geo.body_radius_m).abs() > 1e-6
            || !local_geo.center_lat_deg.is_finite()
            || !regional_geo.center_lat_deg.is_finite()
            || local_geo.center_lat_deg.abs() > 90.0
            || regional_geo.center_lat_deg.abs() > 90.0
            || (local_geo.projection == Some(LocalProjection::Equirectangular)
                && local_geo.center_lat_deg.abs() == 90.0)
            || !local_geo.center_lon_deg.is_finite()
            || !regional_geo.center_lon_deg.is_finite()
            || (local_geo.center_lat_deg - regional_geo.center_lat_deg).abs() > 1e-10
            || ((local_geo.center_lon_deg - regional_geo.center_lon_deg + 180.0).rem_euclid(360.0)
                - 180.0)
                .abs()
                > 1e-10
        {
            return Err("local and regional DEM registration does not match");
        }
        if !blend_m.is_finite()
            || blend_m <= 0.0
            || !regional_offset_m.is_finite()
            || f64::from(local.half_extent) + blend_m >= f64::from(regional.half_extent)
        {
            return Err("regional DEM does not cover the requested transition");
        }
        // The same transition width is supplied to the GPU material. Reject a
        // value that would become zero or non-finite at that explicit boundary.
        let render_blend_m = blend_m as f32;
        if !render_blend_m.is_finite() || render_blend_m <= 0.0 {
            return Err("regional transition width is outside rendering range");
        }
        let render_uv_scale =
            (f64::from(local.half_extent) / f64::from(regional.half_extent)) as f32;
        if !render_uv_scale.is_finite() || render_uv_scale <= 0.0 {
            return Err("regional raster scale is outside rendering range");
        }
        let regional_half = f64::from(regional.half_extent);
        match local_geo.projection {
            Some(LocalProjection::Equirectangular)
                if local_geo.center_lat_deg.abs()
                    + (regional_half / local_geo.body_radius_m).to_degrees()
                    >= 90.0 =>
            {
                return Err("regional DEM crosses the equirectangular pole");
            }
            Some(LocalProjection::Orthographic)
                if regional_half * std::f64::consts::SQRT_2 >= local_geo.body_radius_m =>
            {
                return Err("regional DEM exceeds the orthographic hemisphere");
            }
            _ => {}
        }
        Ok(Self::compose(local, regional, blend_m, regional_offset_m))
    }

    fn compose(
        local: Arc<HeightGrid>,
        regional: Arc<HeightGrid>,
        blend_m: f64,
        regional_offset_m: f64,
    ) -> Self {
        let local_half = f64::from(local.half_extent);
        let mut key = lunco_hash::Fnv1a::new();
        key.write_u64(0x5245_4749_4f4e_414c);
        key.write_u64(3);
        let local_content_key = crate::grid_content_key(&local);
        key.write_u64(local_content_key);
        key.write_u64(crate::grid_content_key(&regional));
        key.write_u64(blend_m.to_bits());
        key.write_u64(regional_offset_m.to_bits());
        Self {
            source: CompositeHeightSource::new(
                ContinuedLocal(local, regional.clone()),
                OffsetRegional {
                    grid: regional,
                    offset_m: regional_offset_m,
                },
                Square {
                    center: [0.0, 0.0],
                    half: local_half,
                },
                blend_m,
            ),
            content_key: key.finish(),
            local_content_key,
        }
    }

    /// Identity of the samples and handoff parameters for derived terrain caches.
    pub fn content_key(&self) -> u64 {
        self.content_key
    }

    /// Identity of the unchanged local grid, independent of the regional tier.
    pub fn local_content_key(&self) -> u64 {
        self.local_content_key
    }

    /// The original local grid; its samples are not resampled by this surface.
    pub fn local_grid(&self) -> &Arc<HeightGrid> {
        &self.source.site.0
    }

    /// The grid that determines the outer footprint and regional traversal cells.
    pub fn regional_grid(&self) -> &Arc<HeightGrid> {
        &self.source.globe.grid
    }

    /// Width of the registered transition beyond the retained local grid.
    pub fn blend_width_m(&self) -> f64 {
        self.source.blend_m
    }

    /// Replace local samples after a terrain edit without changing registration.
    pub fn with_local_grid(&self, local: Arc<HeightGrid>) -> Result<Self, &'static str> {
        if local.res != self.source.site.0.res
            || local.half_extent.to_bits() != self.source.site.0.half_extent.to_bits()
            || local.res.checked_mul(local.res) != Some(local.heights.len())
            || local.heights.iter().any(|height| !height.is_finite())
        {
            return Err("edited local DEM no longer matches its registered geometry");
        }
        Ok(Self::compose(
            local,
            self.source.globe.grid.clone(),
            self.source.blend_m,
            self.source.globe.offset_m,
        ))
    }

    /// Conservative height enclosure over an in-domain rectangle.
    ///
    /// The enclosure includes bicubic interpolation, measured edge continuation,
    /// datum conversion and the complete blend. It is independent of visual LOD.
    /// Each examined grid cell consumes the caller's budget; exhaustion is an
    /// error, not an unverified clear-sky result.
    pub fn height_bounds(
        &self,
        rect: [f64; 4],
        remaining_cells: &mut usize,
    ) -> Result<[f64; 2], &'static str> {
        Ok(self
            .height_enclosure(rect, HeightPlane::horizontal(0.0), remaining_cells)?
            .height)
    }

    /// Residual enclosure against an affine reference. The unchanged raster
    /// interiors retain their Bézier slope correlation; the blended transition
    /// encloses the same registered composition before subtracting the plane.
    pub fn height_enclosure(
        &self,
        rect: [f64; 4],
        plane: HeightPlane,
        remaining_cells: &mut usize,
    ) -> Result<HeightEnclosure, &'static str> {
        let half = f64::from(self.source.globe.grid.half_extent);
        if !plane.is_finite()
            || rect.iter().any(|v| !v.is_finite())
            || rect[0] > rect[2]
            || rect[1] > rect[3]
            || rect[0] < -half
            || rect[1] < -half
            || rect[2] > half
            || rect[3] > half
        {
            return Err("bounds outside the regional DEM footprint");
        }
        let local = &self.source.site.0;
        let local_half = f64::from(local.half_extent);
        let edge = [
            rect[0].clamp(-local_half, local_half),
            rect[1].clamp(-local_half, local_half),
            rect[2].clamp(-local_half, local_half),
            rect[3].clamp(-local_half, local_half),
        ];
        let dx = [rect[0] - edge[0], rect[2] - edge[2]];
        let dz = [rect[1] - edge[1], rect[3] - edge[3]];
        let nearest = |range: [f64; 2]| {
            if range[0] > 0.0 {
                range[0]
            } else if range[1] < 0.0 {
                -range[1]
            } else {
                0.0
            }
        };
        let furthest = |range: [f64; 2]| range[0].abs().max(range[1].abs());
        let min_distance = nearest(dx).hypot(nearest(dz));
        let max_distance = furthest(dx).hypot(furthest(dz));
        let regional_grid = &self.source.globe.grid;
        if min_distance >= self.source.blend_m {
            let sampled = grid_bounds(regional_grid, rect, false, plane, remaining_cells)?;
            return checked_enclosure(
                sampled
                    .model(plane)
                    .add_delta([self.source.globe.offset_m; 2]),
                rect,
            );
        }
        if max_distance == 0.0 {
            let local_bounds = grid_bounds(local, edge, false, plane, remaining_cells)?;
            return checked_enclosure(local_bounds.model(plane), rect);
        }
        let zero = HeightPlane::horizontal(0.0);
        let local_bounds =
            grid_bounds(local, edge, true, zero, remaining_cells)?.scalar_and_derivatives;
        let regional = add(
            grid_bounds(regional_grid, rect, false, zero, remaining_cells)?.scalar_and_derivatives
                [0],
            [self.source.globe.offset_m; 2],
        );
        let regional_edge =
            grid_bounds(regional_grid, edge, true, zero, remaining_cells)?.scalar_and_derivatives;
        let continued = |bounds: [[f64; 2]; 4], spacing: f64| {
            // The continuation is monotone in each signed displacement.
            let ex = dx.map(|distance| continued_distance(distance, spacing));
            let ez = dz.map(|distance| continued_distance(distance, spacing));
            add(
                add(
                    add(bounds[0], multiply(bounds[1], ex)),
                    multiply(bounds[2], ez),
                ),
                multiply(bounds[3], multiply(ex, ez)),
            )
        };
        let local_continuation = continued(local_bounds, f64::from(local.spacing()));
        let regional_continuation = continued(regional_edge, f64::from(regional_grid.spacing()));
        let difference = [
            local_continuation[0] - regional_continuation[1] - self.source.globe.offset_m,
            local_continuation[1] - regional_continuation[0] - self.source.globe.offset_m,
        ];
        let smooth = |distance: f64| {
            let t = (distance / self.source.blend_m).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        let weights = [1.0 - smooth(max_distance), 1.0 - smooth(min_distance)];
        let weighted = multiply(weights, difference);
        let absolute = finite_bounds(
            add(regional, weighted),
            magnitude(local_continuation)
                .max(magnitude(regional))
                .max(magnitude(difference))
                .max(magnitude(weighted)),
        )?;
        let reference = plane.height_bounds(rect);
        let residual = finite_bounds(
            [absolute[0] - reference[1], absolute[1] - reference[0]],
            magnitude(absolute).max(magnitude(reference)),
        )?;
        checked_enclosure(
            HeightEnclosure {
                height: absolute,
                plane,
                residual,
            },
            rect,
        )
    }

    /// Sample the measured local interior and the regional transition.
    /// Points outside the regional footprint have no admitted height.
    pub fn height_at(&self, x: f64, z: f64) -> Result<f64, &'static str> {
        let regional_half = f64::from(self.source.globe.grid.half_extent);
        if !x.is_finite() || !z.is_finite() || x.abs() > regional_half || z.abs() > regional_half {
            return Err("sample outside the regional DEM footprint");
        }
        let result = self.source.height_at(x, z);
        if result.is_finite() {
            Ok(result)
        } else {
            Err("regional DEM interpolation produced a non-finite height")
        }
    }
}

fn add(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

fn multiply(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    let values = [a[0] * b[0], a[0] * b[1], a[1] * b[0], a[1] * b[1]];
    [
        values.into_iter().fold(f64::INFINITY, f64::min),
        values.into_iter().fold(f64::NEG_INFINITY, f64::max),
    ]
}

fn magnitude(bounds: [f64; 2]) -> f64 {
    bounds[0].abs().max(bounds[1].abs())
}

fn finite_bounds(bounds: [f64; 2], scale: f64) -> Result<[f64; 2], &'static str> {
    if !bounds[0].is_finite() || !bounds[1].is_finite() || bounds[0] > bounds[1] {
        return Err("invalid regional DEM height enclosure");
    }
    let pad = 256.0 * f64::EPSILON * magnitude(bounds).max(scale).max(1.0);
    let padded = [bounds[0] - pad, bounds[1] + pad];
    if padded[0].is_finite() && padded[1].is_finite() {
        Ok(padded)
    } else {
        Err("regional DEM height enclosure exceeds finite range")
    }
}

/// Union the cell-wise interpolation and derivative enclosures over a rectangle.
struct GridBounds {
    scalar_and_derivatives: [[f64; 2]; 4],
    residual: [f64; 2],
}
impl GridBounds {
    fn model(self, plane: HeightPlane) -> HeightEnclosure {
        HeightEnclosure {
            height: self.scalar_and_derivatives[0],
            plane,
            residual: self.residual,
        }
    }
}
fn checked_enclosure(
    model: HeightEnclosure,
    rect: [f64; 4],
) -> Result<HeightEnclosure, &'static str> {
    if model.is_valid(rect) {
        Ok(model)
    } else {
        Err("invalid registered terrain enclosure")
    }
}

fn grid_bounds(
    grid: &HeightGrid,
    rect: [f64; 4],
    derivatives: bool,
    plane: HeightPlane,
    remaining_cells: &mut usize,
) -> Result<GridBounds, &'static str> {
    let half = f64::from(grid.half_extent);
    let spacing = f64::from(grid.spacing());
    let cell =
        |coordinate: f64| (((coordinate + half) / spacing).floor() as usize).min(grid.res - 2);
    let mut out = [[f64::INFINITY, f64::NEG_INFINITY]; 4];
    let mut residual = [f64::INFINITY, f64::NEG_INFINITY];
    for z in cell(rect[1])..=cell(rect[3]) {
        for x in cell(rect[0])..=cell(rect[2]) {
            if *remaining_cells == 0 {
                return Err("regional DEM cell enclosure budget exceeded");
            }
            *remaining_cells -= 1;
            let x0 = -half + x as f64 * spacing;
            let z0 = -half + z as f64 * spacing;
            let x1 = if x == grid.res - 2 {
                half
            } else {
                x0 + spacing
            };
            let z1 = if z == grid.res - 2 {
                half
            } else {
                z0 + spacing
            };
            let sub = [
                rect[0].max(x0),
                rect[1].max(z0),
                rect[2].min(x1),
                rect[3].min(z1),
            ];
            let model = grid.cell_height_enclosure([x, z], sub, plane);
            out[0] = [
                out[0][0].min(model.height[0]),
                out[0][1].max(model.height[1]),
            ];
            residual = [
                residual[0].min(model.residual[0]),
                residual[1].max(model.residual[1]),
            ];
            if derivatives {
                for (index, bounds) in grid
                    .cell_derivative_bounds([x, z], sub)
                    .into_iter()
                    .enumerate()
                {
                    out[index + 1] = [
                        out[index + 1][0].min(bounds[0]),
                        out[index + 1][1].max(bounds[1]),
                    ];
                }
            }
        }
    }
    if !derivatives {
        out[1..].fill([0.0, 0.0]);
    }
    if out
        .iter()
        .flatten()
        .chain(residual.iter())
        .any(|value| !value.is_finite())
    {
        return Err("non-finite DEM cell enclosure");
    }
    Ok(GridBounds {
        scalar_and_derivatives: out,
        residual,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_geotiff::LunarFrame;

    fn grid(res: usize, half: f32, f: impl Fn(f64, f64) -> f64) -> Arc<HeightGrid> {
        let mut grid = HeightGrid::new_flat(res, half);
        let spacing = f64::from(grid.spacing());
        for z in 0..res {
            for x in 0..res {
                grid.heights[z * res + x] = f(
                    -f64::from(half) + x as f64 * spacing,
                    -f64::from(half) + z as f64 * spacing,
                );
            }
        }
        Arc::new(grid)
    }

    fn geo(half: f64, res: usize) -> GeoTransform {
        GeoTransform::centred_square(half * 2.0, res, 1_737_400.0, -84.0, 29.0)
            .with_frame(LunarFrame::MoonMe)
    }

    fn surface() -> RegisteredRegionalSurface {
        RegisteredRegionalSurface::new(
            grid(17, 8.0, |x, z| 4.0 + 0.4 * x - 0.2 * z + 0.02 * x * z),
            geo(8.0, 17),
            grid(33, 32.0, |x, z| 30.0 + 0.1 * x + 0.05 * z),
            geo(32.0, 33),
            6.0,
            -3.0,
        )
        .unwrap()
    }

    #[test]
    fn affine_residual_encloses_registered_local_regional_and_blend() {
        let surface = RegisteredRegionalSurface::new(
            grid(17, 8.0, |x, z| 4.0 + 0.4 * x - 0.2 * z),
            geo(8.0, 17),
            grid(33, 32.0, |x, z| 10.0 + 0.1 * x + 0.05 * z),
            geo(32.0, 33),
            6.0,
            -3.0,
        )
        .unwrap();
        for (rect, plane, tight) in [
            (
                [-0.7, -0.5, 0.6, 0.7],
                HeightPlane {
                    origin: [0.0; 2],
                    height: 4.0,
                    gradient: [0.4, -0.2],
                },
                true,
            ),
            (
                [18.0, -0.5, 20.0, 0.7],
                HeightPlane {
                    origin: [0.0; 2],
                    height: 7.0,
                    gradient: [0.1, 0.05],
                },
                true,
            ),
            (
                [8.2, -0.5, 10.0, 0.7],
                HeightPlane {
                    origin: [0.0; 2],
                    height: 4.0,
                    gradient: [0.4, -0.2],
                },
                false,
            ),
        ] {
            let mut budget = 4096;
            let model = surface.height_enclosure(rect, plane, &mut budget).unwrap();
            assert!(model.is_valid(rect));
            let bounds = model.residual;
            if tight {
                assert!(bounds[1] - bounds[0] < 1.0e-8, "{bounds:?}");
            }
            for z in 0..=16 {
                for x in 0..=16 {
                    let wx = rect[0] + (rect[2] - rect[0]) * x as f64 / 16.0;
                    let wz = rect[1] + (rect[3] - rect[1]) * z as f64 / 16.0;
                    let height = surface.height_at(wx, wz).unwrap();
                    assert!(height >= model.height[0] && height <= model.height[1]);
                    let residual = height - plane.height_at(wx, wz);
                    assert!(
                        residual >= bounds[0] && residual <= bounds[1],
                        "{residual} outside {bounds:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn registered_transition_preserves_interior_and_edge_derivatives() {
        let surface = surface();
        for (x, z) in [(0.0, 0.0), (8.0, -3.0), (-8.0, 8.0), (3.7, -2.1)] {
            assert_eq!(
                surface.height_at(x, z).unwrap(),
                surface.source.site.0.height_at(x, z)
            );
        }
        assert!((surface.height_at(20.0, -3.0).unwrap() - 28.85).abs() < 1e-10);
        for (x, z, axis) in [(8.0, 2.0, 0), (14.0, 2.0, 0), (10.0, 8.0, 1)] {
            let eps = 1e-4;
            let sample = |offset| {
                if axis == 0 {
                    surface.height_at(x + offset, z).unwrap()
                } else {
                    surface.height_at(x, z + offset).unwrap()
                }
            };
            let middle = sample(0.0);
            let left = (middle - sample(-eps)) / eps;
            let right = (sample(eps) - middle) / eps;
            assert!(
                (left - right).abs() < 0.001,
                "x={x} z={z}: {left} != {right}"
            );
        }
        assert!(surface.height_at(32.01, 0.0).is_err());
        assert!(surface.height_at(f64::NAN, 0.0).is_err());
    }

    #[test]
    fn registration_and_coverage_are_required() {
        let local = grid(17, 8.0, |_, _| 0.0);
        let regional = grid(33, 32.0, |_, _| 0.0);
        let build = |regional_geo, blend| {
            RegisteredRegionalSurface::new(
                local.clone(),
                geo(8.0, 17),
                regional.clone(),
                regional_geo,
                blend,
                0.0,
            )
        };
        let mut shifted = geo(32.0, 33);
        shifted.center_lon_deg += 0.01;
        assert!(build(shifted, 6.0).is_err());
        let mut wrong_frame = geo(32.0, 33);
        wrong_frame.frame = None;
        assert!(build(wrong_frame, 6.0).is_err());
        let mut shifted_origin = geo(32.0, 33);
        shifted_origin.origin_x_m += 1.0;
        assert!(build(shifted_origin, 6.0).is_err());
        let mut wrong_radius = geo(32.0, 33);
        wrong_radius.body_radius_m += 1.0;
        assert!(build(wrong_radius, 6.0).is_err());
        let mut wrong_projection = geo(32.0, 33);
        wrong_projection.projection = Some(LocalProjection::Orthographic);
        assert!(build(wrong_projection, 6.0).is_err());
        let mut polar_local = geo(8.0, 17);
        let mut polar_regional = geo(32.0, 33);
        polar_local.center_lat_deg = 89.9999;
        polar_regional.center_lat_deg = 89.9999;
        assert!(
            RegisteredRegionalSurface::new(
                local.clone(),
                polar_local,
                regional.clone(),
                polar_regional,
                6.0,
                0.0,
            )
            .is_err()
        );
        assert!(build(geo(32.0, 33), 24.0).is_err());
        assert!(build(geo(32.0, 33), f64::NAN).is_err());
        assert_ne!(
            build(geo(32.0, 33), 6.0).unwrap().content_key(),
            build(geo(32.0, 33), 7.0).unwrap().content_key()
        );
        let shifted_datum = RegisteredRegionalSurface::new(
            local.clone(),
            geo(8.0, 17),
            regional.clone(),
            geo(32.0, 33),
            6.0,
            1.0,
        )
        .unwrap();
        assert_ne!(
            build(geo(32.0, 33), 6.0).unwrap().content_key(),
            shifted_datum.content_key()
        );
        let original = surface();
        let mut edited = (*original.local_grid().as_ref()).clone();
        edited.heights[8 * edited.res + 8] += 1.0;
        let updated = original.with_local_grid(Arc::new(edited)).unwrap();
        assert_ne!(original.content_key(), updated.content_key());
        assert_eq!(
            updated.height_at(0.0, 0.0).unwrap(),
            original.height_at(0.0, 0.0).unwrap() + 1.0
        );
        assert_eq!(
            updated.height_at(20.0, 0.0).unwrap(),
            original.height_at(20.0, 0.0).unwrap()
        );
        assert!(original.with_local_grid(grid(9, 8.0, |_, _| 0.0)).is_err());
    }

    #[test]
    fn edge_derivative_continuation_is_continuous_and_bounded() {
        let local = grid(17, 8.0, |x, _| x);
        let continued = ContinuedLocal(local.clone(), grid(33, 32.0, |_, _| 0.0));
        let edge = continued.height_at(8.0, 0.0);
        let epsilon = 1e-5;
        let derivative = (continued.height_at(8.0 + epsilon, 0.0) - edge) / epsilon;
        assert!((derivative - 1.0).abs() < 1e-4);
        assert_eq!(edge, local.height_at(8.0, 0.0));
        assert!(continued.height_at(1000.0, 0.0) < edge + f64::from(local.spacing()));
    }

    #[test]
    fn measured_regional_relief_survives_the_transition() {
        let local = grid(17, 8.0, |_, _| 0.0);
        let regional = grid(
            65,
            32.0,
            |x, _| {
                if (10.0..=12.0).contains(&x) { 8.0 } else { 0.0 }
            },
        );
        let surface =
            RegisteredRegionalSurface::new(local, geo(8.0, 17), regional, geo(32.0, 65), 8.0, 0.0)
                .unwrap();
        assert!(surface.height_at(11.0, 0.0).unwrap() > 7.0);
        assert_eq!(surface.height_at(8.0, 0.0).unwrap(), 0.0);
    }

    #[test]
    fn height_enclosures_cover_local_transition_and_regional_samples() {
        let surface = surface();
        for rect in [
            [-2.3, -3.1, 4.6, 5.2],
            [7.8, -1.3, 8.6, 2.7],
            [8.2, 7.9, 10.4, 10.6],
            [-14.3, -2.2, -13.7, 4.4],
            [28.0, 28.0, 32.0, 32.0],
            [8.0, -3.0, 8.0, -3.0],
        ] {
            let bounds = surface.height_bounds(rect, &mut 10_000).unwrap();
            for row in 0..=20 {
                for col in 0..=20 {
                    let x = rect[0] + (rect[2] - rect[0]) * col as f64 / 20.0;
                    let z = rect[1] + (rect[3] - rect[1]) * row as f64 / 20.0;
                    let height = surface.height_at(x, z).unwrap();
                    assert!(
                        height >= bounds[0] && height <= bounds[1],
                        "{height} outside {bounds:?} at ({x}, {z})"
                    );
                }
            }
        }
        assert!(
            surface
                .height_bounds([0.0, 0.0, 33.0, 1.0], &mut 10_000)
                .is_err()
        );
        assert!(
            surface
                .height_bounds([1.0, 0.0, 0.0, 1.0], &mut 10_000)
                .is_err()
        );
        assert!(surface.height_bounds([0.0, 0.0, 1.0, 1.0], &mut 0).is_err());
    }

    #[test]
    fn height_enclosures_cover_uneven_surface_across_handoffs() {
        let surface = RegisteredRegionalSurface::new(
            grid(17, 8.0, |x, z| {
                -1900.0 + (x * 0.8).sin() * 7.0 + (z * 0.6).cos() * 5.0
            }),
            geo(8.0, 17),
            grid(33, 32.0, |x, z| {
                -1902.0 + (x * 0.3).cos() * 12.0 + (z * 0.2).sin() * 9.0
            }),
            geo(32.0, 33),
            6.0,
            0.0,
        )
        .unwrap();
        let mut state = 42u64;
        let mut next = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            (state >> 11) as f64 / ((1u64 << 53) as f64)
        };
        for _ in 0..150 {
            let x = -15.0 + 30.0 * next();
            let z = -15.0 + 30.0 * next();
            let rect = [x, z, x + 0.1 + 3.0 * next(), z + 0.1 + 3.0 * next()];
            let bounds = surface.height_bounds(rect, &mut 10_000).unwrap();
            for row in 0..=8 {
                for col in 0..=8 {
                    let px = rect[0] + (rect[2] - rect[0]) * col as f64 / 8.0;
                    let pz = rect[1] + (rect[3] - rect[1]) * row as f64 / 8.0;
                    let height = surface.height_at(px, pz).unwrap();
                    assert!(
                        height >= bounds[0] && height <= bounds[1],
                        "{height} outside {bounds:?} at ({px}, {pz}) for {rect:?}"
                    );
                }
            }
        }
    }
}
