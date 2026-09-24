//! Render-free USD geometry projection into Bevy mesh assets.
//!
//! This crate owns the visual mesh boundary for USD built-in primitives,
//! native meshes, BasisCurves/NurbsCurves, and NurbsPatch surfaces. It owns
//! mesh tessellation and quality invalidation systems, while scene traversal,
//! hierarchy, material intent, and async projection orchestration remain in
//! `lunco-usd-bevy`. No UI or renderer backend belongs here.

use bevy::prelude::*;
use openusd::schemas::geom::tokens as gtok;
use openusd::sdf::Path as SdfPath;

use lunco_usd_bevy_lathe as lathe;
use lunco_usd_bevy_scene::{
    ShapeDims, UsdPrimPath, UsdStageRevision, read_usd_mesh_points, read_usd_mesh_topology,
};
use lunco_usd_bevy_stage::{
    UsdRead, UsdStageAsset, canonical::CanonicalStages, read, stage_convention,
};

/// Explicit, Graphics-independent tessellation inputs for an authored NURBS
/// collision proxy. Counts are parameter-grid subdivisions, not render quality
/// levels, and are persisted with the generated proxy so it can be reproduced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NurbsCollisionTessellation {
    pub u_subdivisions: usize,
    pub v_subdivisions: usize,
    pub trim_curve_samples: usize,
    pub trim_grid_subdivisions: usize,
}

impl NurbsCollisionTessellation {
    /// Stable physical-cook defaults based only on the source control net.
    pub fn for_surface(surface: &lathe::NurbsSurface) -> Self {
        let u_count = surface.u_count as usize;
        let v_count = surface.v_count as usize;
        Self {
            u_subdivisions: u_count.saturating_mul(8).clamp(16, 256),
            v_subdivisions: v_count.saturating_mul(8).clamp(16, 256),
            trim_curve_samples: 48,
            trim_grid_subdivisions: u_count.max(v_count).saturating_mul(8).clamp(32, 256),
        }
    }

    /// Reject unreasonable or empty authored cook settings before allocation.
    pub fn is_valid(self) -> bool {
        (1..=512).contains(&self.u_subdivisions)
            && (1..=512).contains(&self.v_subdivisions)
            && (2..=4096).contains(&self.trim_curve_samples)
            && (2..=512).contains(&self.trim_grid_subdivisions)
    }
}

/// The reproducible triangle geometry generated from a USD NURBS patch.
#[derive(Debug, Clone, PartialEq)]
pub struct NurbsCollisionMesh {
    /// Canonical metres and Y-up, in the source prim's local frame.
    pub points: Vec<[f32; 3]>,
    /// Triangle topology in USD's standard `faceVertexCounts` representation.
    pub face_vertex_counts: Vec<i32>,
    /// Triangle indices in USD's standard `faceVertexIndices` representation.
    pub face_vertex_indices: Vec<i32>,
    /// Stable fingerprint of the generated canonical points and topology.
    pub geometry_fingerprint: u64,
}

/// Dimensions are decoded by `lunco-usd-bevy-scene`, the shared owner used by
/// both the visual mesh and physics collider paths.
/// Rendering-only provenance for a USD built-in primitive mesh. The dimensions
/// remain owned by [`ShapeDims`] so a quality change can rebuild the mesh without
/// reopening the USD stage or duplicating the dimension reader.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct UsdPrimitiveMesh(pub ShapeDims);

/// Rendering-only marker for a USD curve mesh. The authored curve remains
/// addressable through [`UsdPrimPath`], so a Graphics quality change can rebuild
/// the mesh from the composed stage without duplicating USD geometry data in ECS.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsdCurveMesh;

/// Build one USD primitive's visual mesh from its resolved dimensions and the
/// current Graphics quality profile. USD has no attribute for these tessellation
/// counts; they are viewer policy, unlike the shape dimensions.
pub fn build_primitive_mesh(
    shape: ShapeDims,
    quality: lunco_render::RenderQualityProfile,
) -> Option<Mesh> {
    if quality.primitive_sphere_longitudes < 3
        || quality.primitive_sphere_latitudes < 2
        || quality.primitive_radial_segments < 3
        || quality.primitive_capsule_longitudes < 3
        || quality.primitive_capsule_latitudes < 2
    {
        return None;
    }

    match shape {
        ShapeDims::Cube { size } => Some(Cuboid::new(size as f32, size as f32, size as f32).into()),
        ShapeDims::Sphere { radius } => Some(Sphere::new(radius as f32).mesh().uv(
            quality.primitive_sphere_longitudes,
            quality.primitive_sphere_latitudes,
        )),
        ShapeDims::Cylinder { radius, height, .. } => Some(
            Cylinder::new(radius as f32, height as f32)
                .mesh()
                .resolution(quality.primitive_radial_segments)
                .into(),
        ),
        ShapeDims::Cone { radius, height, .. } => Some(
            Cone::new(radius as f32, height as f32)
                .mesh()
                .resolution(quality.primitive_radial_segments)
                .into(),
        ),
        ShapeDims::Capsule { radius, height, .. } => Some(
            Capsule3d::new(radius as f32, (height / 2.0) as f32)
                .mesh()
                .latitudes(quality.primitive_capsule_latitudes)
                .longitudes(quality.primitive_capsule_longitudes)
                .into(),
        ),
        ShapeDims::Plane {
            width,
            length,
            axis,
        } => {
            // The generated plane is local XZ with a +Y normal. UsdGeomPlane
            // defines X-axis width along Z and length along Y, so account for
            // that axis-specific dimension mapping before the shared transform
            // rotates the normal onto the authored axis.
            let (mesh_width, mesh_length) = axis.plane_local_dimensions(width, length);
            Some(
                Plane3d::default()
                    .mesh()
                    .size(mesh_width as f32, mesh_length as f32)
                    .into(),
            )
        }
    }
}

/// Rebuild built-in USD primitive meshes when the user changes Graphics quality.
/// Dimensions are retained in [`UsdPrimitiveMesh`], so this is change-driven and
/// does not repeat USD traversal or touch physics colliders.
pub fn retessellate_primitive_meshes_on_quality_change(
    mut meshes: ResMut<Assets<Mesh>>,
    q: Query<(&UsdPrimitiveMesh, &Mesh3d, Option<&Name>)>,
    quality: Res<lunco_render::RenderingQualitySettings>,
) {
    if !quality.is_changed() {
        return;
    }
    let profile = match quality.validated_profile() {
        Ok(profile) => profile,
        Err(reason) => {
            warn!(
                "[usd-bevy-mesh] invalid Graphics primitive quality; retaining current meshes: {reason}"
            );
            return;
        }
    };
    for (primitive, handle, name) in &q {
        let Some(mesh) = build_primitive_mesh(primitive.0, profile) else {
            warn!(
                "[usd-bevy-mesh] {} primitive mesh quality is invalid; retaining the previous mesh",
                name.map(|n| n.as_str()).unwrap_or("<unnamed>")
            );
            continue;
        };
        let Some(mut slot) = meshes.get_mut(&handle.0) else {
            continue;
        };
        *slot = mesh;
    }
}

/// Rebuild USD curve meshes when authored USD geometry or Graphics tessellation
/// changes. The live-stage revision is the generic invalidation signal for
/// authored curve points, topology, and widths; no route or waypoint knowledge
/// belongs in this renderer-owned path. Invalid settings leave the existing mesh
/// in place and are reported; no lower quality profile is selected implicitly.
pub fn refresh_curve_meshes_on_stage_or_quality_change(
    mut meshes: ResMut<Assets<Mesh>>,
    q: Query<(&UsdPrimPath, &Mesh3d, Option<&Name>), With<UsdCurveMesh>>,
    quality: Res<lunco_render::RenderingQualitySettings>,
    stage_revision: Res<UsdStageRevision>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
) {
    if !quality.is_changed() && !stage_revision.is_changed() {
        return;
    }
    let profile = match quality.validated_profile() {
        Ok(profile) => profile,
        Err(reason) => {
            warn!(
                "[usd-bevy-mesh] invalid Graphics curve quality; retaining current meshes: {reason}"
            );
            return;
        }
    };
    for (prim_path, handle, name) in &q {
        let Some(stage_asset) = stages.get(&prim_path.stage_handle) else {
            continue;
        };
        let (reader, _generation) = canonical.reader_for(prim_path.stage_handle.id(), stage_asset);
        let Ok(path) = SdfPath::new(&prim_path.path) else {
            continue;
        };
        let Some(mesh) = build_usd_curve_mesh(&reader, &path, profile) else {
            warn!(
                "[usd-bevy-mesh] {} curve quality is invalid or its authored curve cannot be tessellated; retaining the previous mesh",
                name.map(|n| n.as_str()).unwrap_or("<unnamed>")
            );
            continue;
        };
        let Some(mut slot) = meshes.get_mut(&handle.0) else {
            continue;
        };
        *slot = mesh;
    }
}

/// A `Mesh` prim's normals, rotated into the canonical frame (`n' = Q·n`) — a
/// direction, so never scaled. `primvars:normals` wins over the typed `normals`
/// attribute (UsdGeomPointBased gives the primvar precedence); the returned name
/// says which was read so the caller can resolve its interpolation/indices.
/// `None` when unauthored (the caller then computes flat normals).
fn read_mesh_normals(
    reader: &impl UsdRead,
    path: &SdfPath,
) -> Option<(Vec<[f32; 3]>, &'static str)> {
    // `points3` for the same reason as `points` above — `normal3d[]` is legal, and a
    // strict read of it means "unauthored", which here silently swaps authored
    // shading normals for computed flat ones (a faceted look, not an error).
    let (normals, attr) = {
        let pv = reader.points3(path, "primvars:normals");
        if pv.is_empty() {
            (reader.points3(path, "normals"), "normals")
        } else {
            (pv, "primvars:normals")
        }
    };
    if normals.is_empty() {
        return None;
    }
    let conv = stage_convention(reader).ok()?;
    if conv.is_identity() {
        return Some((normals, attr));
    }
    Some((
        normals
            .into_iter()
            .map(|n| conv.dir(Vec3::from_array(n)).to_array())
            .collect(),
        attr,
    ))
}

/// Build a mesh from a `UsdGeomBasisCurves` or `UsdGeomNurbsCurves` prim.
///
/// An unoriented curve with `widths` is a swept tube whose widths are diameters.
/// An oriented curve with standard `normals` is a flat ribbon whose widths are
/// strip widths. Both paths share the same centerline evaluation and are kept
/// here as one generic USD geometry reader.
///
/// Batches are honoured: `curveVertexCounts` partitions `points` into several
/// curves on one prim, and each is swept and merged into a single mesh so the
/// prim keeps its 1:1 entity mapping.
///
/// `widths` interpolation follows USD: one value is `constant`, otherwise it is
/// per-vertex. Absent `widths` means an infinitely thin curve, which has no
/// surface — returns `None` rather than inventing a radius, so a curve authored
/// as a pure path (a camera rail) does not silently become a visible pipe.
pub fn build_usd_curve_mesh(
    reader: &impl UsdRead,
    path: &SdfPath,
    quality: lunco_render::RenderQualityProfile,
) -> Option<Mesh> {
    use bevy::math::DVec3;
    use lunco_usd_geometry::curve::{CurveBasis, eval_curve};
    use lunco_usd_geometry::curve_sweep::sweep_tube;
    use lunco_usd_geometry::ribbon::{RibbonPoint, build_ribbon_mesh};

    // Canonical-frame points — same conversion the mesh path takes.
    let points = read_usd_mesh_points(reader, path)?;
    if points.is_empty() {
        return None;
    }
    if points
        .iter()
        .any(|point| point.iter().any(|value| !value.is_finite()))
    {
        error!(
            "[usd-bevy-mesh] {} has non-finite authored curve control points",
            path.as_str()
        );
        return None;
    }
    // No `widths` ⇒ no surface. Deliberately not defaulted: see the doc above.
    let widths = match read::read_curve_real_array(reader, path, gtok::A_WIDTHS) {
        Ok(Some(widths)) if !widths.is_empty() => widths,
        Ok(Some(_)) | Ok(None) => return None,
        Err(_) => {
            error!(
                "[usd-bevy-mesh] {} has authored curve widths with an unsupported value type",
                path.as_str()
            );
            return None;
        }
    };

    // USD `widths` means a ribbon width when the curve is oriented by its
    // standard `normals` attribute, and a tube diameter otherwise. Keep that
    // distinction at the generic USD geometry boundary so authored route,
    // camera, and annotation curves all receive the same interpretation.
    let oriented_normals = read_mesh_normals(reader, path).map(|(normals, _)| {
        normals
            .into_iter()
            .map(Vec3::from_array)
            .collect::<Vec<_>>()
    });
    if let Some(normals) = &oriented_normals {
        if normals.len() != points.len() || normals.iter().any(|normal| !normal.is_finite()) {
            error!(
                "[usd-bevy-mesh] {} has curve normals that do not match its point topology",
                path.as_str()
            );
            return None;
        }
    }

    // Radii are a LENGTH, so they scale with `metersPerUnit` — `conv.length`,
    // not `conv.point`. (`read_usd_mesh_points` already converted the centerline.)
    let conv = stage_convention(reader).ok()?;
    let radii: Vec<f32> = widths
        .iter()
        .map(|w| conv.length(*w / 2.0) as f32)
        .collect();
    if radii.iter().any(|r| !r.is_finite() || *r <= 0.0) {
        error!(
            "[usd-bevy-mesh] {} has curve widths that are not finite and positive",
            path.as_str()
        );
        return None;
    }

    // `NurbsCurves` carries its own basis: per-curve `order`, a concatenated
    // `knots` array, and optional rational `pointWeights`. `BasisCurves` carries a
    // `type`/`basis` token pair instead. Both are swept identically once each
    // curve is reduced to a centerline — the only difference is how that
    // centerline is produced.
    let is_nurbs = reader.type_name(path).as_deref() == Some(gtok::T_NURBS_CURVES);
    let counts = match read::read_curve_int_array(
        reader,
        path,
        openusd::schemas::geom::tokens::A_CURVE_VERTEX_COUNTS,
    ) {
        Ok(Some(counts)) if !counts.is_empty() => counts,
        Ok(Some(_)) | Ok(None) => {
            error!(
                "[usd-bevy-mesh] {} has no usable authored curveVertexCounts; USD requires this topology field",
                path.as_str()
            );
            return None;
        }
        Err(_) => {
            error!(
                "[usd-bevy-mesh] {} has authored curveVertexCounts with an unsupported value type",
                path.as_str()
            );
            return None;
        }
    };
    let total_control_points: usize = counts
        .iter()
        .filter_map(|count| usize::try_from(*count).ok())
        .sum();
    if total_control_points != points.len() || counts.iter().any(|count| *count < 2) {
        error!(
            "[usd-bevy-mesh] {} has curveVertexCounts inconsistent with its points",
            path.as_str()
        );
        return None;
    }
    if widths.len() != 1 && widths.len() != counts.len() && widths.len() != points.len() {
        error!(
            "[usd-bevy-mesh] {} has {} widths for {} curves and {} points; expected constant, uniform, or vertex widths",
            path.as_str(),
            widths.len(),
            counts.len(),
            points.len()
        );
        return None;
    }

    let (basis, periodic, orders, all_knots, point_weights) = if is_nurbs {
        let orders = match read::read_curve_int_array(
            reader,
            path,
            openusd::schemas::geom::tokens::A_ORDER,
        ) {
            Ok(Some(orders)) if !orders.is_empty() => orders,
            Ok(Some(_)) | Ok(None) => {
                error!(
                    "[usd-bevy-mesh] {} has no usable authored NurbsCurves order",
                    path.as_str()
                );
                return None;
            }
            Err(_) => {
                error!(
                    "[usd-bevy-mesh] {} has authored NurbsCurves order with an unsupported value type",
                    path.as_str()
                );
                return None;
            }
        };
        if orders.len() != 1 && orders.len() != counts.len() {
            error!(
                "[usd-bevy-mesh] {} has {} NurbsCurves orders for {} curves",
                path.as_str(),
                orders.len(),
                counts.len()
            );
            return None;
        }
        let all_knots = match read::read_curve_real_array(
            reader,
            path,
            openusd::schemas::geom::tokens::A_KNOTS,
        ) {
            Ok(Some(knots)) if !knots.is_empty() => knots,
            Ok(Some(_)) | Ok(None) | Err(_) => {
                error!(
                    "[usd-bevy-mesh] {} has no usable authored NurbsCurves knots",
                    path.as_str()
                );
                return None;
            }
        };
        let point_weights = match read::read_curve_real_array(
            reader,
            path,
            openusd::schemas::geom::tokens::A_POINT_WEIGHTS,
        ) {
            Ok(Some(weights)) if weights.len() == points.len() => weights,
            Ok(Some(_)) => {
                error!(
                    "[usd-bevy-mesh] {} has pointWeights whose length does not match points",
                    path.as_str()
                );
                return None;
            }
            Ok(None) => Vec::new(),
            Err(_) => {
                error!(
                    "[usd-bevy-mesh] {} has authored pointWeights with an unsupported value type",
                    path.as_str()
                );
                return None;
            }
        };
        (CurveBasis::Linear, false, orders, all_knots, point_weights)
    } else {
        let ty = match read::read_curve_token(
            reader,
            path,
            openusd::schemas::geom::tokens::A_TYPE,
            "cubic",
            &["linear", "cubic"],
        ) {
            Ok(token) => token,
            Err(_) => return None,
        };
        let basis = if ty == "linear" {
            CurveBasis::Linear
        } else {
            match read::read_curve_token(
                reader,
                path,
                openusd::schemas::geom::tokens::A_BASIS,
                "bezier",
                &["bezier", "catmullRom"],
            ) {
                Ok(token) if token == "bezier" => CurveBasis::Bezier,
                Ok(_) => CurveBasis::CatmullRom,
                Err(_) => return None,
            }
        };
        let wrap = match read::read_curve_token(
            reader,
            path,
            openusd::schemas::geom::tokens::A_WRAP,
            "nonperiodic",
            &["nonperiodic", "periodic", "pinned"],
        ) {
            Ok(wrap) => wrap,
            Err(_) => return None,
        };
        (
            basis,
            wrap == "periodic",
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
    };

    if is_nurbs {
        let expected_knots = counts
            .iter()
            .enumerate()
            .try_fold(0usize, |total, (index, count)| {
                let count = usize::try_from(*count).ok()?;
                let order = orders
                    .get(index)
                    .or_else(|| orders.first())
                    .and_then(|order| usize::try_from(*order).ok())?;
                total.checked_add(count.checked_add(order)?)
            });
        if expected_knots != Some(all_knots.len()) {
            error!(
                "[usd-bevy-mesh] {} has {} NurbsCurves knots, expected {:?}",
                path.as_str(),
                all_knots.len(),
                expected_knots
            );
            return None;
        }
    }

    if quality.curve_samples_per_segment == 0
        || (oriented_normals.is_none() && quality.curve_radial_segments < 3)
    {
        return None;
    }

    let mut merged: Option<Mesh> = None;
    let mut cursor = 0usize;
    // `knots` is one flat array for the whole batch: curve `i` owns
    // `vertexCount_i + order_i` of them, consumed in order. Tracked separately
    // from the point cursor because the strides differ.
    let mut knot_cursor = 0usize;
    let curve_count = counts.len();
    for (ci, c) in counts.into_iter().enumerate() {
        let Ok(n) = usize::try_from(c) else {
            error!(
                "[usd-bevy-mesh] {} has a negative curveVertexCounts entry",
                path.as_str()
            );
            return None;
        };
        if n < 2 || n > points.len().saturating_sub(cursor) {
            error!(
                "[usd-bevy-mesh] {} has curve topology outside its points",
                path.as_str()
            );
            return None;
        }
        // Captured before `cursor` advances — `pointWeights` is indexed by control
        // point, so it slices with the same offset the points did.
        let cursor_start = cursor;
        let cvs: Vec<Vec3> = points[cursor..cursor + n]
            .iter()
            .map(|p| Vec3::from_array(*p))
            .collect();
        let seg_radii: Vec<f32> = if radii.len() == 1 {
            vec![radii[0]]
        } else if radii.len() == curve_count {
            vec![radii[ci]]
        } else {
            radii
                .iter()
                .skip(cursor)
                .take(n)
                .copied()
                .collect::<Vec<_>>()
        };
        let seg_normals = oriented_normals.as_ref().map(|normals| {
            normals[cursor..cursor + n]
                .iter()
                .copied()
                .collect::<Vec<_>>()
        });
        cursor += n;

        let centerline: Vec<Vec3> = if is_nurbs {
            // Per-curve order; a single authored value applies to the whole batch.
            let Some(order) = orders
                .get(ci)
                .or_else(|| orders.first())
                .and_then(|order| usize::try_from(*order).ok())
                .filter(|order| *order >= 2 && *order <= n)
            else {
                error!(
                    "[usd-bevy-mesh] {} has an invalid NurbsCurves order",
                    path.as_str()
                );
                return None;
            };
            let need = n + order;
            let knot_end = knot_cursor.checked_add(need)?;
            if knot_end > all_knots.len() {
                error!(
                    "[usd-bevy-mesh] {} has insufficient NurbsCurves knots",
                    path.as_str()
                );
                return None;
            }
            let knots = all_knots[knot_cursor..knot_end].to_vec();
            knot_cursor = knot_end;
            let w: Vec<f64> = if point_weights.is_empty() {
                Vec::new()
            } else {
                point_weights[cursor_start..cursor_start + n].to_vec()
            };
            let steps = (n.saturating_sub(1)).max(1) * quality.curve_samples_per_segment;
            let pts: Vec<[f32; 3]> = cvs.iter().map(|p| p.to_array()).collect();
            let sampled =
                lunco_usd_geometry::nurbs::sample_nurbs_curve(&pts, &w, order, &knots, steps);
            if sampled.is_empty() {
                error!(
                    "[usd-bevy-mesh] {} has a NurbsCurves segment that cannot be evaluated",
                    path.as_str()
                );
                return None;
            }
            sampled.into_iter().map(Vec3::from_array).collect()
        } else if basis == CurveBasis::Linear {
            cvs.clone()
        } else {
            let steps = (n.saturating_sub(1)).max(1) * quality.curve_samples_per_segment;
            let Some(samples) = (0..=steps)
                .map(|i| eval_curve(&cvs, basis, periodic, i as f32 / steps as f32))
                .collect::<Option<Vec<_>>>()
            else {
                error!(
                    "[usd-bevy-mesh] {} has a BasisCurves segment that cannot be evaluated",
                    path.as_str()
                );
                return None;
            };
            samples
        };
        // Resampling changes the point count, so per-vertex radii must be
        // resampled with it or a tapered tube would snap back to its control-point
        // radii. Constant width (len 1) passes straight through.
        let seg_radii = if seg_radii.len() <= 1 || centerline.len() == cvs.len() {
            seg_radii
        } else {
            let last = cvs.len() - 1;
            (0..centerline.len())
                .map(|i| {
                    let t = i as f32 / (centerline.len() - 1).max(1) as f32 * last as f32;
                    let (a, f) = (t.floor() as usize, t.fract());
                    let b = (a + 1).min(last);
                    seg_radii[a] * (1.0 - f) + seg_radii[b] * f
                })
                .collect()
        };

        let segment_mesh = if let Some(seg_normals) = seg_normals {
            let seg_normals = if centerline.len() == cvs.len() {
                seg_normals
            } else {
                let last = cvs.len() - 1;
                (0..centerline.len())
                    .map(|i| {
                        let t = i as f32 / (centerline.len() - 1).max(1) as f32 * last as f32;
                        let (a, f) = (t.floor() as usize, t.fract());
                        let b = (a + 1).min(last);
                        seg_normals[a].lerp(seg_normals[b], f).normalize_or_zero()
                    })
                    .collect()
            };
            let points = centerline
                .iter()
                .zip(seg_normals)
                .map(|(position, normal)| RibbonPoint {
                    position: position.as_dvec3(),
                    normal: normal.as_dvec3(),
                })
                .collect::<Vec<_>>();
            build_ribbon_mesh(&points, DVec3::ZERO, &seg_radii, 0.0, periodic)?
        } else {
            sweep_tube(
                &centerline,
                &seg_radii,
                quality.curve_radial_segments,
                periodic,
            )?
        };
        merged = Some(match merged {
            None => segment_mesh,
            Some(mut acc) => {
                acc.merge(&segment_mesh).ok()?;
                acc
            }
        });
    }
    if is_nurbs && knot_cursor != all_knots.len() {
        return None;
    }
    merged
}

/// Build a mesh from a `UsdGeomNurbsPatch` prim.
///
/// A patch is a tensor-product rational surface: a `uVertexCount × vVertexCount`
/// control net with a knot vector and order per direction. It is how USD spells
/// every surface of revolution — which for HAB-1 is **80.4% of the habitat's
/// vertices** (261 lathe objects plus the ellipsoidal dome), and the only way to
/// express a *partial* revolution at all, since `Cylinder`/`Sphere`/`Cone` are
/// complete revolutions with no sweep-angle parameter.
///
/// Normals are analytic (`uder × vder`), not face-averaged — exact at the poles
/// and seams where averaging creases, which is precisely the dome apex.
///
/// **`trimCurve:*` IS honoured** — see [`lunco_usd_geometry::trim`]. A trimmed patch gets an
/// irregular triangulation of its surviving domain instead of a lattice, which is
/// what puts a genuine arched doorway in a wall.
///
/// A malformed authored trim definition refuses the patch. Rendering the
/// untrimmed surface would add geometry the USD scene explicitly removed.
///
/// (This paragraph previously said trimming was unimplemented and silently
/// ignored. It was stale, and it cost a debugging session: the claim was taken at
/// face value while the code underneath was working, so a missing surface was
/// blamed on trim support that in fact existed. A doc comment that describes a
/// capability the code no longer lacks is worse than no comment.)
pub fn has_authored_nurbs_trim(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    path: &SdfPath,
) -> bool {
    [
        "trimCurve:counts",
        "trimCurve:orders",
        "trimCurve:vertexCounts",
        "trimCurve:knots",
        "trimCurve:points",
        "trimCurve:ranges",
    ]
    .into_iter()
    .any(|attr| reader.has_authored_attribute(path, attr))
}

/// Read a `NurbsPatch` prim's definition — either GENERATED from a
/// `lunco:lathe:*` profile, or read from the authored control arrays.
///
/// This is the single place the two spellings of "what surface is this" meet, and
/// they are mutually exclusive by design: a prim that declares a lathe profile does
/// not author `points`, because the whole point of the parametric form is that the
/// control net is derived. Authoring both is the duplication that let the engine
/// bell's drawn contour (effective exponent ≈1.3) drift away from the contour its
/// own Modelica model declared (0.55) with nothing to catch it.
pub fn read_nurbs_patch_surface(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    path: &SdfPath,
) -> Option<(lathe::NurbsSurface, Option<lathe::UsdLathe>)> {
    // Applying the parametric API is the ownership decision: its profile is the
    // only source of the surface. An empty/unknown profile is therefore an
    // invalid parametric definition, not permission to resurrect a competing
    // authored control net. Falling through here used to make a profile typo
    // render stale or unrelated `points` data and violated the schema's explicit
    // "unknown = no surface" contract.
    if reader.has_api_schema(path, "LunCoLatheAPI") {
        let l = lathe::read_lathe(reader, path)?;
        return Some((l.surface()?, Some(l)));
    }

    let points = read_usd_mesh_points(reader, path)?;
    let u_count = lathe::read_required_nurbs_int(reader, path, gtok::A_U_VERTEX_COUNT)?;
    let v_count = lathe::read_required_nurbs_int(reader, path, gtok::A_V_VERTEX_COUNT)?;
    let u_order = lathe::read_required_nurbs_int(reader, path, gtok::A_U_ORDER)?;
    let v_order = lathe::read_required_nurbs_int(reader, path, gtok::A_V_ORDER)?;
    if u_count < u_order || v_count < v_order {
        error!(
            "[usd-bevy-mesh] {} has NurbsPatch order/count mismatch: u {u_count}/{u_order}, v {v_count}/{v_order}",
            path.as_str()
        );
        return None;
    }
    let u_knots = match read::read_curve_real_array(reader, path, gtok::A_U_KNOTS) {
        Ok(Some(knots)) if knots.len() == u_count + u_order => knots,
        Ok(Some(_)) | Ok(None) | Err(_) => {
            error!(
                "[usd-bevy-mesh] {} has no usable authored uKnots for its NurbsPatch",
                path.as_str()
            );
            return None;
        }
    };
    let v_knots = match read::read_curve_real_array(reader, path, gtok::A_V_KNOTS) {
        Ok(Some(knots)) if knots.len() == v_count + v_order => knots,
        Ok(Some(_)) | Ok(None) | Err(_) => {
            error!(
                "[usd-bevy-mesh] {} has no usable authored vKnots for its NurbsPatch",
                path.as_str()
            );
            return None;
        }
    };
    let weights = match read::read_curve_real_array(reader, path, gtok::A_POINT_WEIGHTS) {
        Ok(Some(weights)) if weights.len() == points.len() => weights,
        Ok(Some(_)) => {
            error!(
                "[usd-bevy-mesh] {} has pointWeights whose length does not match its NurbsPatch points",
                path.as_str()
            );
            return None;
        }
        Ok(None) => Vec::new(),
        Err(_) => {
            error!(
                "[usd-bevy-mesh] {} has pointWeights with an unsupported value type",
                path.as_str()
            );
            return None;
        }
    };
    let orientation = match read::read_curve_token(
        reader,
        path,
        "orientation",
        "rightHanded",
        &["rightHanded", "leftHanded"],
    ) {
        Ok(orientation) => orientation == "leftHanded",
        Err(_) => return None,
    };

    Some((
        lathe::NurbsSurface {
            points,
            weights,
            u_count: u_count as u32,
            v_count: v_count as u32,
            u_order: u_order as u32,
            v_order: v_order as u32,
            u_knots,
            v_knots,
            left_handed: orientation,
        },
        None,
    ))
}

/// Build a `NurbsPatch`'s mesh AND the definition to retain alongside it.
///
/// The returned [`lathe::NurbsSurface`] is `None` for a TRIMMED patch. That is
/// deliberate: a trim loop lives in the patch's own `(u, v)` parameter space, and
/// re-deriving the trimmed triangulation from an edited control net is a different
/// problem from retessellating an untrimmed one. Withholding the component means a
/// trimmed patch is simply not live-editable, rather than editable-but-wrong — the
/// trim would silently stop matching the surface it cuts.
pub fn build_usd_nurbs_patch_mesh(
    reader: &impl UsdRead,
    path: &SdfPath,
    quality: lunco_render::RenderQualityProfile,
) -> Option<(Mesh, Option<(lathe::NurbsSurface, Option<lathe::UsdLathe>)>)> {
    let (surface, lathe_params) = read_nurbs_patch_surface(reader, path)?;
    let tessellation = NurbsCollisionTessellation {
        u_subdivisions: quality.nurbs_surface_subdivisions(surface.u_count as usize),
        v_subdivisions: quality.nurbs_surface_subdivisions(surface.v_count as usize),
        trim_curve_samples: quality.nurbs_trim_curve_samples,
        trim_grid_subdivisions: quality
            .nurbs_trim_subdivisions(surface.u_count.max(surface.v_count) as usize),
    };
    build_usd_nurbs_patch_mesh_with_tessellation(reader, path, surface, lathe_params, tessellation)
}

/// Derive collision geometry from a standard USD NURBS patch or `LunCoLatheAPI`
/// patch with explicit, non-render tessellation settings.
pub fn build_nurbs_collision_mesh_from_usd(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    path: &SdfPath,
    tessellation: NurbsCollisionTessellation,
) -> Option<NurbsCollisionMesh> {
    if !tessellation.is_valid() {
        return None;
    }
    let (surface, lathe_params) = read_nurbs_patch_surface(reader, path)?;
    let (mesh, _) = build_usd_nurbs_patch_mesh_with_tessellation(
        reader,
        path,
        surface,
        lathe_params,
        tessellation,
    )?;
    let bevy_mesh::VertexAttributeValues::Float32x3(points) =
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)?
    else {
        return None;
    };
    let indices: Vec<u32> = match mesh.indices()? {
        bevy_mesh::Indices::U32(indices) => indices.clone(),
        bevy_mesh::Indices::U16(indices) => indices.iter().map(|index| u32::from(*index)).collect(),
    };
    if points.is_empty()
        || indices.is_empty()
        || indices.len() % 3 != 0
        || points.iter().flatten().any(|value| !value.is_finite())
        || indices.iter().any(|index| *index as usize >= points.len())
    {
        return None;
    }
    let face_vertex_indices: Vec<i32> = indices
        .into_iter()
        .map(i32::try_from)
        .collect::<Result<_, _>>()
        .ok()?;
    let face_vertex_counts = vec![3; face_vertex_indices.len() / 3];
    let geometry_fingerprint = nurbs_collision_geometry_fingerprint(points, &face_vertex_indices);
    Some(NurbsCollisionMesh {
        points: points.clone(),
        face_vertex_counts,
        face_vertex_indices,
        geometry_fingerprint,
    })
}

/// Stable FNV-1a fingerprint for one canonical NURBS proxy mesh.
pub fn nurbs_collision_geometry_fingerprint(points: &[[f32; 3]], indices: &[i32]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    let mut feed = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    };
    feed(&(points.len() as u64).to_le_bytes());
    for point in points {
        for coordinate in point {
            feed(&coordinate.to_bits().to_le_bytes());
        }
    }
    feed(&(indices.len() as u64).to_le_bytes());
    for index in indices {
        feed(&index.to_le_bytes());
    }
    hash & i64::MAX as u64
}

fn build_usd_nurbs_patch_mesh_with_tessellation(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    path: &SdfPath,
    surface: lathe::NurbsSurface,
    lathe_params: Option<lathe::UsdLathe>,
    tessellation: NurbsCollisionTessellation,
) -> Option<(Mesh, Option<(lathe::NurbsSurface, Option<lathe::UsdLathe>)>)> {
    use bevy::asset::RenderAssetUsages;
    use bevy_mesh::PrimitiveTopology;

    let points = surface.points.clone();
    let weights = surface.weights.clone();
    let u_count = surface.u_count as usize;
    let v_count = surface.v_count as usize;
    let u_order = surface.u_order as usize;
    let v_order = surface.v_order as usize;
    let u_knots = surface.u_knots.clone();
    let v_knots = surface.v_knots.clone();

    // ── Trim curves ─────────────────────────────────────────────────────────
    // `trimCurve:*` IS applied — see `lunco_usd_geometry::trim`. A trimmed patch gets an
    // irregular triangulation of its surviving domain instead of a lattice.
    //
    // Two things that used to block this are handled there rather than guessed:
    // USD never states the keep/discard winding rule, so classification is
    // even-odd with the domain rectangle as an implicit outer loop
    // (orientation-independent); and the geometry crate handles constraint
    // crossings without panicking, so
    // loops are inserted with `add_constraint_and_split` rather than gated with
    // `can_add_constraint` — gating would silently drop part of a loop and leave
    // the hole with a missing side.
    let trim_authored = has_authored_nurbs_trim(reader, path);
    let trim_loops = if !trim_authored {
        None
    } else {
        let counts = match read::read_curve_int_array(reader, path, "trimCurve:counts") {
            Ok(Some(counts)) if !counts.is_empty() => counts,
            _ => {
                error!(
                    "[usd-bevy-mesh] {} has malformed trimCurve:counts; refusing the patch",
                    path.as_str()
                );
                return None;
            }
        };
        let orders = match read::read_curve_int_array(reader, path, "trimCurve:orders") {
            Ok(Some(orders)) => orders,
            _ => {
                error!(
                    "[usd-bevy-mesh] {} has malformed trimCurve:orders; refusing the patch",
                    path.as_str()
                );
                return None;
            }
        };
        let vertex_counts = match read::read_curve_int_array(reader, path, "trimCurve:vertexCounts")
        {
            Ok(Some(vertex_counts)) => vertex_counts,
            _ => {
                error!(
                    "[usd-bevy-mesh] {} has malformed trimCurve:vertexCounts; refusing the patch",
                    path.as_str()
                );
                return None;
            }
        };
        let tknots = match read::read_curve_real_array(reader, path, "trimCurve:knots") {
            Ok(Some(tknots)) if !tknots.is_empty() => tknots,
            _ => {
                error!(
                    "[usd-bevy-mesh] {} has malformed trimCurve:knots; refusing the patch",
                    path.as_str()
                );
                return None;
            }
        };
        let tpoints = reader.points3(path, "trimCurve:points");
        if tpoints.is_empty() {
            error!(
                "[usd-bevy-mesh] {} has malformed trimCurve:points; refusing the patch",
                path.as_str()
            );
            return None;
        }
        let ranges = match read::read_double2_array_strict(reader, path, "trimCurve:ranges") {
            Ok(Some(ranges)) => ranges,
            Ok(None) => Vec::new(),
            Err(_) => {
                error!(
                    "[usd-bevy-mesh] {} has malformed trimCurve:ranges; refusing the patch",
                    path.as_str()
                );
                return None;
            }
        };

        let u_span = [u_knots[u_order - 1], u_knots[u_count]];
        let v_span = [v_knots[v_order - 1], v_knots[v_count]];
        let loops = lunco_usd_geometry::trim::assemble_loops(
            &counts,
            &orders,
            &vertex_counts,
            &tknots,
            &ranges,
            &tpoints,
            u_span,
            v_span,
            tessellation.trim_curve_samples,
        );
        if loops.is_empty() {
            error!(
                "[usd-bevy-mesh] {} has authored trimCurve data but no usable loop",
                path.as_str()
            );
            return None;
        }
        Some(loops)
    };

    if let Some(loops) = trim_loops {
        let grid = tessellation.trim_grid_subdivisions;
        bevy::log::info!(
            "[usd-bevy-mesh] {} trimming: {} loop(s), grid {}",
            path.as_str(),
            loops.loops.len(),
            grid
        );
        let Some(domain) = lunco_usd_geometry::trim::triangulate_trimmed(&loops, grid) else {
            error!(
                "[usd-bevy-mesh] {} authored trim could not be triangulated; refusing the patch",
                path.as_str()
            );
            return None;
        };
        bevy::log::info!(
            "[usd-bevy-mesh] {} trimmed domain: {} verts, {} tris",
            path.as_str(),
            domain.uvs.len(),
            domain.indices.len() / 3
        );
        let samples = lunco_usd_geometry::nurbs::sample_nurbs_patch_at(
            &points,
            &weights,
            u_count,
            v_count,
            u_order,
            v_order,
            &u_knots,
            &v_knots,
            &domain.uvs,
        );
        if samples.is_empty() {
            error!(
                "[usd-bevy-mesh] {} authored trim produced no surface samples; refusing the patch",
                path.as_str()
            );
            return None;
        }
        let mut positions = Vec::with_capacity(samples.len());
        let mut normals = Vec::with_capacity(samples.len());
        let mut uvs = Vec::with_capacity(samples.len());
        for s in &samples {
            positions.push(s.position);
            normals.push(s.normal);
            uvs.push(s.uv);
        }
        let mut indices = domain.indices;
        lathe::flip_if_left_handed(surface.left_handed, &mut normals, &mut indices);
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        mesh.insert_indices(bevy_mesh::Indices::U32(indices));
        // No `NurbsSurface` for a trimmed patch — see the fn doc.
        return Some((mesh, None));
    }

    // The untrimmed build now lives on `NurbsSurface` itself, because it is
    // EXACTLY the operation the regeneration system has to perform when a parameter
    // changes. Keeping a second copy here would be two tessellators that can
    // disagree — the same trap `lunco_usd_geometry::nurbs`' module doc describes for evaluators.
    let Some(mesh) =
        surface.mesh_with_subdivisions(tessellation.u_subdivisions, tessellation.v_subdivisions)
    else {
        // `sample_nurbs_patch_at` has already warned WHICH guard fired; this
        // adds the prim path, which it has no way to know.
        bevy::log::warn!(
            "[usd-bevy-mesh] {} untrimmed patch produced no samples — no mesh",
            path.as_str()
        );
        return None;
    };
    // Parity with the trimmed branch above, which logs its vert/tri counts. The
    // untrimmed branch used to be completely SILENT, so a patch that reached
    // here and built correctly was indistinguishable in the log from one whose
    // prim was never traversed at all. Telling those two apart is exactly what
    // you need when a surface is missing from the render, and not being able to
    // is what made the HAB-1 dome expensive to diagnose.
    bevy::log::info!(
        "[usd-bevy-mesh] {} untrimmed patch: {}x{} net{}, {} verts",
        path.as_str(),
        surface.u_count,
        surface.v_count,
        match &lathe_params {
            Some(l) => format!(" (lathed, {:?})", l.profile),
            None => String::new(),
        },
        mesh.count_vertices()
    );
    Some((mesh, Some((surface, lathe_params))))
}

/// Build a Bevy [`Mesh`] from a native USD `Mesh` prim (UsdGeomMesh):
/// `point3f[] points`, `int[] faceVertexCounts`, `int[] faceVertexIndices`,
/// with optional `normal3f[] normals` and `texCoord2f[] primvars:st`.
///
/// Polygons are **fan-triangulated** into an *unindexed* triangle list — one
/// vertex per face-corner — so per-face-varying normals/uvs need no welding
/// and quads/n-gons render directly. Attribute interpolation is inferred by
/// array length: `== points.len()` → per-vertex (indexed by point), `==
/// faceVertexIndices.len()` → per-face-varying (indexed by corner); any other
/// length is ignored. `orientation = "leftHanded"` flips the winding (USD
/// default is right-handed = CCW, which matches Bevy). Missing `normals` are
/// computed flat; missing `primvars:st` get a zeroed UV set so the standard /
/// shader material paths don't choke.
///
/// Returns `None` if the required topology attributes are absent/empty or the
/// indices reference out-of-range points (malformed mesh). Rendering only —
/// native-mesh **colliders** are still the glTF side-channel's job
/// (see `resolver.rs` `TODO(glb-composability)`).
pub fn build_usd_mesh(reader: &impl UsdRead, path: &SdfPath) -> Option<Mesh> {
    use bevy::asset::RenderAssetUsages;
    // `bevy_mesh`, NOT `bevy::render::render_resource` — the latter is a
    // re-export through `bevy_render` (wgpu + naga). `bevy_mesh` depends only on
    // `wgpu-types`, so naming the topology here costs no GPU stack.
    // See docs/architecture/render-decoupling.md.
    use bevy_mesh::PrimitiveTopology;

    // Canonical-frame points/normals (Y-up, metres); identity for our stages.
    // Topology is decoded by the render-free scene contract so physics and
    // visual projection consume the same authored mesh facts.
    let topology = read_usd_mesh_topology(reader, path)?;
    let points = topology.points;
    let counts = topology.face_vertex_counts;
    let indices = topology.face_vertex_indices;

    // Optional vertex attributes. `primvars:st` is THE UV channel — the
    // `primvars:st0` / bare `st` spellings are gone. A UV set is a primvar, so it
    // is namespaced; a bare `st` is not one, and accepting it let a mesh carry UVs
    // in a form no other DCC binds.
    let normals = read_mesh_normals(reader, path).map(|(values, _source)| values);
    // `points2`, NOT `scalar::<Vec<[f32; 2]>>`: Maya and Houdini export
    // `texCoord2d[]`, Blender exports `texCoord2f[]`. A strict `2f` read of a `2d` UV
    // set yields "no UVs", and the documented response to that is a ZEROED UV set —
    // so the mesh samples its texture entirely at (0,0) and renders as one flat
    // colour. That misreads as a material/texture bug, which is the wrong place to
    // look. `None` (rather than empty) keeps the `uvs_per_vertex`/`per_corner` logic
    // below unchanged.
    let uvs = Some(reader.points2(path, "primvars:st")).filter(|v: &Vec<[f32; 2]>| !v.is_empty());

    let n_corners = indices.len();
    let normals_per_vertex = normals.as_ref().is_some_and(|n| n.len() == points.len());
    let normals_per_corner = normals.as_ref().is_some_and(|n| n.len() == n_corners);
    let uvs_per_vertex = uvs.as_ref().is_some_and(|u| u.len() == points.len());
    let uvs_per_corner = uvs.as_ref().is_some_and(|u| u.len() == n_corners);

    let left_handed = reader.text(path, "orientation").as_deref() == Some("leftHanded");

    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(n_corners);
    let mut out_normals: Vec<[f32; 3]> = Vec::new();
    let mut out_uvs: Vec<[f32; 2]> = Vec::new();

    // Walk faces; `base` is the running offset of the face's first corner into
    // the flat `indices` (and per-corner attribute) arrays.
    let mut base = 0usize;
    for &count in &counts {
        let count = count as usize;
        if base + count > n_corners {
            return None; // counts/indices disagree → malformed
        }
        if count >= 3 {
            // Fan: triangle (0, k, k+1) for k in 1..count-1.
            for k in 1..count - 1 {
                let tri = if left_handed {
                    [0, k + 1, k]
                } else {
                    [0, k, k + 1]
                };
                for local in tri {
                    let corner = base + local;
                    let vidx = indices[corner] as usize;
                    if vidx >= points.len() {
                        return None; // index out of range → malformed
                    }
                    positions.push(points[vidx]);
                    if normals_per_vertex {
                        out_normals.push(normals.as_ref().unwrap()[vidx]);
                    } else if normals_per_corner {
                        out_normals.push(normals.as_ref().unwrap()[corner]);
                    }
                    if uvs_per_vertex {
                        out_uvs.push(uvs.as_ref().unwrap()[vidx]);
                    } else if uvs_per_corner {
                        out_uvs.push(uvs.as_ref().unwrap()[corner]);
                    }
                }
            }
        }
        base += count;
    }
    if positions.is_empty() {
        return None;
    }

    let have_normals = out_normals.len() == positions.len();
    let have_uvs = out_uvs.len() == positions.len();

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    if have_normals {
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, out_normals);
    }
    if have_uvs {
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, out_uvs);
    } else {
        // ShaderMaterial / StandardMaterial both expect a UV channel.
        let zero = vec![[0.0f32, 0.0]; mesh.count_vertices()];
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, zero);
    }
    if !have_normals {
        // Unindexed triangle soup → flat per-face normals.
        mesh.compute_flat_normals();
    }
    Some(mesh)
}
