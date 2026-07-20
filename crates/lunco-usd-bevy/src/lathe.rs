//! Parametric lathes: a `UsdGeomNurbsPatch` that keeps its DEFINITION on the
//! entity and regenerates its `Mesh` when that definition changes.
//!
//! ## The problem this replaces
//!
//! A `NurbsPatch` used to be consumed at load: [`crate::build_usd_nurbs_patch_mesh`]
//! read `points` / `pointWeights` / knots / orders, sampled the surface, produced a
//! `Mesh`, and threw the definition away. Nothing downstream could see what the
//! surface WAS, only what it had been tessellated into. Two consequences:
//!
//! - **Nothing could edit it.** `set(me, "NurbsPatch.points", pts)` fails with
//!   `unknown type 'NurbsPatch'` — the scripting bridge reflects COMPONENTS, and
//!   there was no component. The only workaround was a per-frame rhai actuator
//!   re-lathing the patch every tick, which is both wasted work (a nozzle's shape
//!   does not change while the engine burns) and an error thrown 25×/second that
//!   masked every other scenario error in the log.
//! - **The shape was authored twice.** The engine bell carried 36 literal control
//!   points AND a `LunCoProgram` whose `inputs:` said what the bell was — throat,
//!   exit, length, contour. Those two could and did disagree: the drawn surface's
//!   effective contour exponent was ≈1.3 (a flaring cone) while the model declared
//!   0.55 (a real bell). The model's `r_station_*` outputs existed precisely to
//!   make that disagreement visible, which is a tripwire, not a fix.
//!
//! ## The shape
//!
//! [`NurbsSurface`] is the patch's definition, retained on the spawned entity and
//! change-detectable. [`UsdLathe`] is the *parametric* layer above it: a named
//! profile plus its numbers. The pipeline is two hops, each gated on `Changed`:
//!
//! ```text
//!   UsdLathe  ──Changed──►  NurbsSurface  ──Changed──►  Mesh
//!  (parameters)            (control net)              (triangles)
//! ```
//!
//! Both hops are writable from a script, because both are real reflected
//! components:
//!
//! ```text
//!   set(id, "UsdLathe.profile.exit_radius", 1.6)   // re-lathes, then re-meshes
//!   set(id, "NurbsSurface.points", [[x,y,z], ...]) // re-meshes only
//! ```
//!
//! Neither system runs on an unchanged entity — that is the whole point, and it is
//! enforced by the `Changed<T>` filters rather than by a dirty flag anyone can
//! forget to clear.
//!
//! ## Why the lathe evaluates HERE and not in Modelica
//!
//! `LunCo.Propulsion.BellNozzle` owns the ENGINEERING — expansion ratio, thrust
//! coefficient, Isp, thrust. It does not own the shape; it *consumes* the shape's
//! four numbers. Routing the drawn geometry through the solver would make a static
//! surface depend on a running simulation (and on solver step order at load), for a
//! quantity that is a closed-form function of parameters the USD already carries.
//!
//! So both sides evaluate the SAME LAW from the SAME AUTHORED NUMBERS:
//!
//! ```text
//!   r(s) = throat + (exit - throat) * s^contour ,  s = i / (rings - 1)
//! ```
//!
//! which is verbatim the model's own `r_station_1` / `r_station_2` definition
//! (stations at s = 1/3 and 2/3 — i.e. the 4-ring control net, spelled out). The
//! model's outputs stop being a tripwire for a mismatch that existed and become a
//! live cross-check that agrees by construction.
//!
//! ## What is preserved exactly
//!
//! The u-direction is the classic rational 4-arc circle — 9 control points, √2/2 on
//! the diagonals, doubled interior knots, order 3 — and is generated, not authored,
//! so it cannot drift. That is what keeps the exit rim and the dish edge genuinely
//! ROUND instead of an octagon. Dropping those weights turns a 0.58 m rim into a
//! 0.62 m bulge at the diagonals; see the rationality tests in [`crate::nurbs`].

use bevy::prelude::*;

/// √2/2 — the weight on a rational circle's diagonal control points. A quarter
/// circle is *exactly* a rational quadratic with this middle weight and is not
/// representable polynomially at all.
const DIAG_W: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// The knot vector of the 4-arc rational circle: doubled interior knots, so each
/// quarter is its own C0-joined conic segment.
const CIRCLE_U_KNOTS: [f64; 12] = [
    0.0, 0.0, 0.0, 0.25, 0.25, 0.5, 0.5, 0.75, 0.75, 1.0, 1.0, 1.0,
];

/// Control points per ring of the 4-arc rational circle (first == last, closing it).
const CIRCLE_U_COUNT: u32 = 9;

/// A `UsdGeomNurbsPatch`'s definition, retained on the entity that renders it.
///
/// This is the data [`crate::build_usd_nurbs_patch_mesh`] used to consume and
/// discard. Keeping it means the surface can be inspected, edited and regenerated
/// without re-reading USD — and, because it is a reflected `Component`, the
/// existing scripting bridge reaches it with no new verb:
/// `set(id, "NurbsSurface.points", [[x, y, z], ...])`.
///
/// `points` is **row-major over v**: `v_count` rings of `u_count` points, which is
/// USD's own `uVertexCount` × `vVertexCount` layout and what [`crate::nurbs`]
/// expects. `weights` follows the same order, or is empty for the polynomial case.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component)]
pub struct NurbsSurface {
    /// Control net, row-major over v (`v_count` rings × `u_count` points).
    pub points: Vec<[f32; 3]>,
    /// One weight per control point, same order; empty ⇒ polynomial.
    pub weights: Vec<f64>,
    pub u_count: u32,
    pub v_count: u32,
    /// USD `order` = degree + 1.
    pub u_order: u32,
    pub v_order: u32,
    pub u_knots: Vec<f64>,
    pub v_knots: Vec<f64>,
    /// USD `orientation == "leftHanded"`: the net is wound the other way, so
    /// normals AND winding must both be flipped. See [`Self::mesh`].
    pub left_handed: bool,
}

impl NurbsSurface {
    /// Tessellate into a `Mesh`. `None` when the definition is malformed —
    /// [`crate::nurbs::sample_nurbs_patch`] has already warned which guard fired.
    ///
    /// This is the UNTRIMMED build. Trimmed patches (`trimCurve:*`) keep their
    /// original load-time path in [`crate::build_usd_nurbs_patch_mesh`] and are not
    /// given a `NurbsSurface`, so they are never regenerated: a trim loop lives in
    /// the patch's parameter space and re-deriving it from a changed control net is
    /// a different problem than this one. Better to not offer the capability than to
    /// offer it wrong.
    pub fn mesh(&self) -> Option<Mesh> {
        use bevy::asset::RenderAssetUsages;
        use bevy_mesh::PrimitiveTopology;

        let (u_count, v_count) = (self.u_count as usize, self.v_count as usize);
        let u_steps = (u_count * 6).clamp(8, 128);
        let v_steps = (v_count * 6).clamp(8, 128);

        let grid = crate::nurbs::sample_nurbs_patch(
            &self.points,
            &self.weights,
            u_count,
            v_count,
            self.u_order as usize,
            self.v_order as usize,
            &self.u_knots,
            &self.v_knots,
            u_steps,
            v_steps,
        );
        if grid.is_empty() {
            return None;
        }

        let cols = u_steps + 1;
        let mut positions = Vec::with_capacity(grid.len());
        let mut normals = Vec::with_capacity(grid.len());
        let mut uvs = Vec::with_capacity(grid.len());
        for s in &grid {
            positions.push(s.position);
            normals.push(s.normal);
            uvs.push(s.uv);
        }

        let mut indices = Vec::with_capacity(u_steps * v_steps * 6);
        for r in 0..v_steps {
            for c in 0..u_steps {
                let (a, b, d, e) = (
                    (r * cols + c) as u32,
                    (r * cols + c + 1) as u32,
                    ((r + 1) * cols + c) as u32,
                    ((r + 1) * cols + c + 1) as u32,
                );
                // Winding must agree with the vertex normals, which come from
                // `sample_nurbs_patch_at` as dP/du x dP/dv. `[a, e, d]` / `[a, b, e]`
                // gives each face that same handedness; the opposite pairing renders
                // black under `doubleSided` because the rasteriser draws the back
                // face and negates an already-correct normal. That is the HAB-1 dome.
                indices.extend_from_slice(&[a, e, d]);
                indices.extend_from_slice(&[a, b, e]);
            }
        }
        flip_if_left_handed(self.left_handed, &mut normals, &mut indices);

        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        mesh.insert_indices(bevy_mesh::Indices::U32(indices));
        Some(mesh)
    }
}

/// Reverse normals AND winding together for a `leftHanded` net.
///
/// A NURBS normal is `dP/du × dP/dv`, so which way it faces is a property of how
/// the control net was WOUND, not of the shape. Flipping normals alone is not
/// enough: back-face culling keys off winding, so a flipped normal on unflipped
/// triangles renders lit-but-invisible from the side you want. Both move together
/// or neither does.
pub fn flip_if_left_handed(left_handed: bool, normals: &mut [[f32; 3]], indices: &mut [u32]) {
    if !left_handed {
        return;
    }
    for n in normals.iter_mut() {
        n[0] = -n[0];
        n[1] = -n[1];
        n[2] = -n[2];
    }
    for tri in indices.chunks_exact_mut(3) {
        tri.swap(1, 2);
    }
}

/// A surface of revolution's profile: which curve is swept, and its numbers.
///
/// A profile is a function `s ∈ [0, 1] → (radius, height)` walked from station 0 to
/// station 1. Named rather than free-form because a name is what makes the
/// parameters mean something: `contour` is only "the bell exponent" inside `Bell`.
#[derive(Reflect, Clone, Debug, PartialEq)]
pub enum LatheProfile {
    /// Rocket engine bell, throat → exit, flaring DOWN (−Y).
    ///
    /// `r(s) = throat_radius + (exit_radius − throat_radius) · s^contour`,
    /// `y(s) = −length · s`.
    ///
    /// `contour = 1` is a straight cone; below 1 the flare is fast off the throat
    /// and eases toward the exit — the family Rao's method produces. The exponent is
    /// AUTHORED, not derived: a true Rao contour solves a method-of-characteristics
    /// problem needing chamber conditions the vehicle does not carry. This is the
    /// same law, verbatim, as `LunCo.Propulsion.BellNozzle`'s `r_station_*`.
    Bell {
        throat_radius: f32,
        exit_radius: f32,
        length: f32,
        contour: f32,
    },
    /// Parabolic dish reflector, apex → rim, opening UP (+Y).
    ///
    /// `r(s) = apex_radius + (rim_radius − apex_radius) · s`, `y(s) = r² / (4f)`.
    ///
    /// `apex_radius` is small but non-zero on purpose: a ring collapsed exactly to
    /// the axis is a degenerate row whose surface normal is a zero cross product,
    /// and a NaN normal renders as a black hole at the vertex of the dish.
    Paraboloid {
        apex_radius: f32,
        rim_radius: f32,
        focal_length: f32,
    },
}

impl LatheProfile {
    /// `(radius, height)` at normalised station `s ∈ [0, 1]`.
    pub fn at(&self, s: f32) -> (f32, f32) {
        match *self {
            LatheProfile::Bell {
                throat_radius,
                exit_radius,
                length,
                contour,
            } => {
                // `powf` on a clamped, non-negative `s` — a negative base with a
                // fractional exponent is NaN, and NaN control points make the whole
                // patch vanish with no mesh and no obvious cause.
                let t = s.clamp(0.0, 1.0).powf(contour.max(1e-6));
                (throat_radius + (exit_radius - throat_radius) * t, -length * s)
            }
            LatheProfile::Paraboloid {
                apex_radius,
                rim_radius,
                focal_length,
            } => {
                let r = apex_radius + (rim_radius - apex_radius) * s.clamp(0.0, 1.0);
                (r, r * r / (4.0 * focal_length.max(1e-6)))
            }
        }
    }
}

/// A `NurbsPatch` generated by revolving a [`LatheProfile`] about +Y.
///
/// This is the component that makes the surface PARAMETRIC: change a number on it
/// and the control net — and then the mesh — follow, once, on the change.
///
/// Authored in USD on the patch prim:
///
/// ```usda
/// def NurbsPatch "Nozzle"
/// {
///     uniform token lunco:lathe:profile = "bell"
///     float lunco:lathe:throatRadius = 0.35
///     float lunco:lathe:exitRadius   = 1.35
///     float lunco:lathe:length       = 1.90
///     float lunco:lathe:contour      = 0.55
/// }
/// ```
///
/// No `points`, no `pointWeights`, no `uKnots`, no `uVertexCount` — those are
/// DERIVED, and authoring them alongside the parameters is the duplication this
/// component exists to delete.
///
/// The two knobs that are NOT derived — how many control rings the profile is
/// sampled at and the degree between them — are read from the STANDARD
/// `UsdGeomNurbsPatch` fields `vVertexCount` and `vOrder`, because that is what
/// those fields already mean. A `lunco:` spelling of them would be a second name
/// for a quantity USD has a first name for.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component)]
pub struct UsdLathe {
    pub profile: LatheProfile,
    /// Control rings along the profile. Each ring is a full rational circle.
    ///
    /// These are CONTROL points, not surface points: with `v_order = 3` the swept
    /// curve interpolates only the first and last ring and is pulled toward the
    /// interior ones. 4 is the authored default because it is exactly the station
    /// set `BellNozzle.mo` defines (s = 0, 1/3, 2/3, 1).
    pub rings: u32,
    /// USD `order` = degree + 1 along the profile. 3 (quadratic) gives a smooth,
    /// continuously-shaded sweep; 2 would put the control points exactly on the
    /// surface at the cost of visible faceting between rings.
    pub v_order: u32,
    /// Mirrors `orientation = "leftHanded"` onto the generated [`NurbsSurface`].
    pub left_handed: bool,
}

impl UsdLathe {
    /// Revolve the profile into a control net.
    ///
    /// The u direction is the exact rational 4-arc circle, generated here rather
    /// than authored so it cannot drift: 9 control points per ring with √2/2 on the
    /// diagonals and doubled interior knots. v walks the profile.
    pub fn surface(&self) -> NurbsSurface {
        let rings = self.rings.max(2);
        let v_order = self.v_order.clamp(2, rings);

        let mut points = Vec::with_capacity((rings * CIRCLE_U_COUNT) as usize);
        let mut weights = Vec::with_capacity((rings * CIRCLE_U_COUNT) as usize);
        for i in 0..rings {
            let s = i as f32 / (rings - 1) as f32;
            let (r, y) = self.profile.at(s);
            // The 4-arc form: on-circle points at 0/90/180/270 with weight 1, corner
            // points between them at (±r, ±r) with weight √2/2. First == last closes
            // the ring exactly rather than nearly.
            points.extend_from_slice(&[
                [r, y, 0.0],
                [r, y, r],
                [0.0, y, r],
                [-r, y, r],
                [-r, y, 0.0],
                [-r, y, -r],
                [0.0, y, -r],
                [r, y, -r],
                [r, y, 0.0],
            ]);
            weights.extend_from_slice(&[
                1.0, DIAG_W, 1.0, DIAG_W, 1.0, DIAG_W, 1.0, DIAG_W, 1.0,
            ]);
        }

        NurbsSurface {
            points,
            weights,
            u_count: CIRCLE_U_COUNT,
            v_count: rings,
            u_order: 3,
            v_order,
            u_knots: CIRCLE_U_KNOTS.to_vec(),
            v_knots: crate::nurbs::default_clamped_knots(rings as usize, v_order as usize),
            left_handed: self.left_handed,
        }
    }
}

/// Read a prim's lathe parameters into a [`UsdLathe`].
///
/// The profile comes from `lunco:lathe:*`; its sampling comes from the standard
/// `UsdGeomNurbsPatch` fields `vVertexCount` and `vOrder`.
///
/// `None` when `lunco:lathe:profile` is absent — that is an ordinary hand-authored
/// patch, read from its `points` array as before. An UNKNOWN profile token warns
/// and also returns `None`: falling back to the (now deleted) point arrays would
/// render nothing at all, so the author needs to hear about the typo.
///
/// `real` throughout, not `scalar::<f64>` — `float lunco:lathe:contour = 0.55` is
/// the natural authoring and a strict `double` read of it is indistinguishable from
/// "unauthored", which would silently substitute a default.
pub fn read_lathe<R: crate::read::UsdRead>(
    reader: &R,
    path: &openusd::sdf::Path,
) -> Option<UsdLathe> {
    let kind = reader.text(path, "lunco:lathe:profile")?;
    let p = |name: &str, default: f32| -> f32 {
        reader.real(path, name).map(|v| v as f32).unwrap_or(default)
    };

    let profile = match kind.as_str() {
        "bell" => LatheProfile::Bell {
            throat_radius: p("lunco:lathe:throatRadius", 0.35),
            exit_radius: p("lunco:lathe:exitRadius", 1.35),
            length: p("lunco:lathe:length", 1.90),
            contour: p("lunco:lathe:contour", 0.55),
        },
        "paraboloid" => LatheProfile::Paraboloid {
            apex_radius: p("lunco:lathe:apexRadius", 0.02),
            rim_radius: p("lunco:lathe:rimRadius", 0.58),
            focal_length: p("lunco:lathe:focalLength", 0.35),
        },
        other => {
            warn!(
                "[usd-bevy] {} declares `lunco:lathe:profile = \"{}\"`, which is not a \
                 known profile (`bell` | `paraboloid`) — no surface generated",
                path.as_str(),
                other
            );
            return None;
        }
    };

    // STANDARD UsdGeomNurbsPatch fields, not `lunco:` ones. How many control rings
    // the profile is sampled at, and the polynomial degree between them, are
    // properties of the PATCH — `vVertexCount` and `vOrder` already mean exactly
    // that, so a vendor namespace has nothing to add and would only give the same
    // quantity two spellings. Only the profile's SHAPE needs `lunco:`: USD has no
    // surface-of-revolution schema at all (the parametric gprims are Sphere / Cube /
    // Cylinder / Cone / Capsule / Plane, and NurbsPatch is a RESULT format — points
    // and knots — not a generator), so there is no standard field to reuse for
    // `profile`, `throatRadius`, `contour` and friends.
    Some(UsdLathe {
        profile,
        rings: reader.scalar::<i32>(path, "vVertexCount").unwrap_or(4).max(2) as u32,
        v_order: reader.scalar::<i32>(path, "vOrder").unwrap_or(3).max(2) as u32,
        left_handed: reader.text(path, "orientation").as_deref() == Some("leftHanded"),
    })
}

/// Re-lathe the control net when a [`UsdLathe`]'s parameters change.
///
/// `Changed<UsdLathe>` — this does NOT run per frame. Bevy's change detection also
/// fires on insert, so the load-time net could come from here; it does not, because
/// the mesh must exist on the spawn frame rather than one frame later, and a patch
/// that pops in a frame late is exactly the kind of glitch that survives into a
/// recorded take.
///
/// The write to `NurbsSurface` is what propagates: it trips that component's own
/// change detection and [`regenerate_patch_meshes`] picks it up in the same frame,
/// given the ordering in the plugin. Guarded by an equality check so re-lathing to
/// an identical net does not cascade into a pointless retessellation.
pub fn relathe_changed(mut q: Query<(&UsdLathe, &mut NurbsSurface), Changed<UsdLathe>>) {
    for (lathe, mut surface) in &mut q {
        let next = lathe.surface();
        if *surface != next {
            *surface = next;
        }
    }
}

/// Rebuild the `Mesh` when a [`NurbsSurface`] changes.
///
/// `Changed<NurbsSurface>` — the "only on changes" requirement, enforced by the
/// query filter rather than a dirty flag. On an untouched scene this system
/// iterates nothing.
///
/// Writes THROUGH the existing `Handle<Mesh>` rather than adding a new asset and
/// swapping `Mesh3d`: everything already pointing at that handle (the render world's
/// prepared mesh, a collider derived from it) follows the edit, and no orphaned
/// asset accumulates each time a parameter is nudged.
pub fn regenerate_patch_meshes(
    mut meshes: ResMut<Assets<Mesh>>,
    q: Query<(&NurbsSurface, &Mesh3d, Option<&Name>), Changed<NurbsSurface>>,
) {
    for (surface, handle, name) in &q {
        let Some(mesh) = surface.mesh() else {
            warn!(
                "[usd-bevy] {} NurbsSurface changed but produced no samples — mesh left \
                 as it was (check the control net / knot vectors)",
                name.map(|n| n.as_str().to_string()).unwrap_or_default()
            );
            continue;
        };
        let Some(mut slot) = meshes.get_mut(&handle.0) else {
            continue;
        };
        *slot = mesh;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dish the lander actually ships, regenerated from its parameters, must
    /// reproduce the control net that was hand-authored for it. This is the
    /// "identical unless a parameter changed" guarantee, as a number.
    ///
    /// It also pins the AUTHORING CONVENTION: control points sit directly on the
    /// profile at `s = i / (rings - 1)`. The authored dish matches that to 3
    /// decimals, which is how the convention was recovered in the first place.
    #[test]
    fn paraboloid_lathe_reproduces_the_authored_dish_net() {
        let lathe = UsdLathe {
            profile: LatheProfile::Paraboloid {
                apex_radius: 0.02,
                rim_radius: 0.58,
                focal_length: 0.35,
            },
            rings: 4,
            v_order: 3,
            left_handed: false,
        };
        let s = lathe.surface();
        assert_eq!(s.u_count, 9);
        assert_eq!(s.v_count, 4);
        assert_eq!(s.points.len(), 36);

        // The authored rings, (r, y): the numbers that were in the .usda.
        let want = [(0.02, 0.0), (0.20, 0.03), (0.40, 0.11), (0.58, 0.24)];
        for (ring, (wr, wy)) in want.iter().enumerate() {
            let p = s.points[ring * 9]; // the (r, y, 0) point of each ring
            assert!(
                (p[0] - wr).abs() < 0.01,
                "ring {ring} radius {} != authored {wr}",
                p[0]
            );
            assert!(
                (p[1] - wy).abs() < 0.01,
                "ring {ring} height {} != authored {wy}",
                p[1]
            );
        }
    }

    /// The bell's control net must be exactly what `BellNozzle.mo` says it is.
    /// `r_station_1` / `r_station_2` are defined at s = 1/3 and 2/3, so with the
    /// default 4 rings they ARE control points 1 and 2 — the model and the drawn
    /// surface agree by construction, which is what the whole conversion buys.
    #[test]
    fn bell_lathe_matches_the_modelica_station_radii() {
        let (throat, exit, contour) = (0.35f32, 1.35f32, 0.55f32);
        let lathe = UsdLathe {
            profile: LatheProfile::Bell {
                throat_radius: throat,
                exit_radius: exit,
                length: 1.90,
                contour,
            },
            rings: 4,
            v_order: 3,
            left_handed: false,
        };
        let s = lathe.surface();
        for (ring, station) in [(1usize, 1.0f32 / 3.0), (2, 2.0 / 3.0)] {
            // Verbatim `r_station_N = throat + (exit - throat) * (N/3)^contour`.
            let want = throat + (exit - throat) * station.powf(contour);
            let got = s.points[ring * 9][0];
            assert!(
                (got - want).abs() < 1e-5,
                "ring {ring} radius {got} != model station {want}"
            );
        }
        // Throat and exit are interpolated exactly (clamped knots), so the two
        // numbers a nozzle is actually named by are not approximations.
        assert!((s.points[0][0] - throat).abs() < 1e-6, "throat ring");
        assert!((s.points[27][0] - exit).abs() < 1e-6, "exit ring");
    }

    /// THE constraint. The generated u-direction must be an EXACT circle, not a
    /// polygon: every sampled point on a ring sits at the ring's radius, including
    /// at the 45° diagonals where an unweighted net bulges outward by ~7%.
    ///
    /// This is what keeps the exit rim and the dish edge round, and it is the one
    /// property that a "simpler" hand-rolled lathe would quietly lose.
    #[test]
    fn generated_rings_are_exact_circles_not_polygons() {
        let lathe = UsdLathe {
            profile: LatheProfile::Bell {
                throat_radius: 0.35,
                exit_radius: 1.35,
                length: 1.90,
                contour: 0.55,
            },
            rings: 4,
            v_order: 3,
            left_handed: false,
        };
        let s = lathe.surface();
        let grid = crate::nurbs::sample_nurbs_patch(
            &s.points,
            &s.weights,
            s.u_count as usize,
            s.v_count as usize,
            s.u_order as usize,
            s.v_order as usize,
            &s.u_knots,
            &s.v_knots,
            64,
            8,
        );
        assert!(!grid.is_empty(), "generated net must evaluate");

        // The exit plane (v = 1) is interpolated, so its radius is known exactly.
        let mut checked = 0;
        for smp in grid.iter().filter(|g| g.uv[1] > 1.0 - 1e-9) {
            let r = (smp.position[0].powi(2) + smp.position[2].powi(2)).sqrt();
            assert!(
                (r - 1.35).abs() < 1e-4,
                "exit rim radius {r} != 1.35 at u = {} — the rational weights are gone \
                 and the rim is a polygon",
                smp.uv[0]
            );
            checked += 1;
        }
        assert!(checked > 32, "expected a full exit ring, got {checked}");
    }

    /// Changing a parameter must actually move the surface — the test that would
    /// fail if `relathe_changed` were wired to a net that never regenerates.
    #[test]
    fn changing_a_parameter_changes_the_net() {
        let mut lathe = UsdLathe {
            profile: LatheProfile::Bell {
                throat_radius: 0.35,
                exit_radius: 1.35,
                length: 1.90,
                contour: 0.55,
            },
            rings: 4,
            v_order: 3,
            left_handed: false,
        };
        let before = lathe.surface();
        if let LatheProfile::Bell {
            ref mut exit_radius,
            ..
        } = lathe.profile
        {
            *exit_radius = 2.0;
        }
        let after = lathe.surface();
        assert_ne!(before, after, "a changed parameter must change the net");
        assert!((after.points[27][0] - 2.0).abs() < 1e-6, "new exit radius");
        // ...and the untouched end must NOT have moved.
        assert!((after.points[0][0] - 0.35).abs() < 1e-6, "throat is unchanged");
    }

    /// A profile with a wild exponent must not produce NaN control points. A NaN in
    /// the net makes the entire patch vanish with no mesh, which is the worst
    /// failure mode available and the reason `at()` clamps its base.
    #[test]
    fn degenerate_parameters_do_not_produce_nan_points() {
        for profile in [
            LatheProfile::Bell {
                throat_radius: 0.0,
                exit_radius: 0.0,
                length: 0.0,
                contour: 0.0,
            },
            LatheProfile::Paraboloid {
                apex_radius: 0.0,
                rim_radius: 0.0,
                focal_length: 0.0,
            },
        ] {
            let s = UsdLathe {
                profile,
                rings: 4,
                v_order: 3,
                left_handed: false,
            }
            .surface();
            for p in &s.points {
                assert!(p.iter().all(|c| c.is_finite()), "non-finite control point {p:?}");
            }
        }
    }
}
