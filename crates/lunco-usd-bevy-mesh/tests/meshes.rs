//! Low-level USD-to-Bevy mesh projection tests.

use bevy::prelude::Mesh;
use lunco_usd_bevy_core::canonical::CanonicalStage;
use lunco_usd_bevy_mesh::{
    build_primitive_mesh, build_usd_curve_mesh, build_usd_mesh, build_usd_nurbs_patch_mesh,
    read_nurbs_patch_surface,
};
use lunco_usd_bevy_scene::ShapeDims;
use openusd::sdf::Path as SdfPath;

mod curve_mesh_quality_tests {
    use super::*;

    fn stage(source: &str) -> CanonicalStage {
        CanonicalStage::from_recipe(&lunco_usd_core::StageRecipe::from_source(
            "curve.usda",
            source,
        ))
        .expect("build curve stage")
    }

    #[test]
    fn curve_tube_density_follows_graphics_quality() {
        let stage = stage(
            r#"#usda 1.0
(
    metersPerUnit = 1
)
def BasisCurves "Tube"
{
    uniform token type = "cubic"
    uniform token basis = "catmullRom"
    int[] curveVertexCounts = [4]
    point3f[] points = [(0, 0, 0), (1, 0, 1), (2, 0, -1), (3, 0, 0)]
    float[] widths = [0.2]
}
"#,
        );
        let reader = stage.view();
        let path = SdfPath::new("/Tube").unwrap();
        let low = build_usd_curve_mesh(
            &reader,
            &path,
            lunco_render::RenderingQuality::Low.profile(),
        )
        .expect("low curve mesh");
        let high = build_usd_curve_mesh(
            &reader,
            &path,
            lunco_render::RenderingQuality::High.profile(),
        )
        .expect("high curve mesh");
        assert!(high.count_vertices() > low.count_vertices());
    }

    #[test]
    fn malformed_curve_structure_is_rejected_instead_of_guessed() {
        let stage = stage(
            r#"#usda 1.0
def NurbsCurves "MissingKnots"
{
    int[] curveVertexCounts = [3]
    int[] order = [3]
    point3f[] points = [(0, 0, 0), (1, 0, 1), (2, 0, 0)]
    float[] widths = [0.2]
}
def BasisCurves "WrongBasis"
{
    uniform token type = "cubic"
    uniform token basis = "bspline"
    int[] curveVertexCounts = [4]
    point3f[] points = [(0, 0, 0), (1, 0, 1), (2, 0, -1), (3, 0, 0)]
    float[] widths = [0.2]
}
"#,
        );
        let reader = stage.view();
        assert!(build_usd_curve_mesh(
            &reader,
            &SdfPath::new("/MissingKnots").unwrap(),
            lunco_render::RenderingQuality::Balanced.profile(),
        )
        .is_none());
        assert!(build_usd_curve_mesh(
            &reader,
            &SdfPath::new("/WrongBasis").unwrap(),
            lunco_render::RenderingQuality::Balanced.profile(),
        )
        .is_none());
    }

    #[test]
    fn rover_nurbs_antenna_geometry_is_tessellated_as_a_tube() {
        // These are the authored structures used by the Summer Space School
        // rover: a four-control-point order-four NurbsCurves prim with an
        // explicit diameter.  Keep this as a renderer-path regression rather
        // than replacing the curve with a special antenna mesh.
        let stage = stage(
            r#"#usda 1.0
def NurbsCurves "MagnetometerBoom"
{
    int[] curveVertexCounts = [4]
    int[] order = [4]
    double[] knots = [0, 0, 0, 0, 1, 1, 1, 1]
    point3f[] points = [(0, 0.20, -0.82), (0, 0.19, -1.34), (0, 0.20, -1.80), (0, 0.21, -2.16)]
    float[] widths = [0.025]
}
def NurbsCurves "FeedArm"
{
    int[] curveVertexCounts = [4]
    int[] order = [4]
    double[] knots = [0, 0, 0, 0, 1, 1, 1, 1]
    point3f[] points = [(0.56, 0.25, 0), (0.44, 0.44, 0), (0.16, 0.47, 0), (0, 0.38, 0)]
    float[] widths = [0.03]
}
"#,
        );
        let reader = stage.view();
        for path in ["/MagnetometerBoom", "/FeedArm"] {
            let mesh = build_usd_curve_mesh(
                &reader,
                &SdfPath::new(path).unwrap(),
                lunco_render::RenderingQuality::Balanced.profile(),
            )
            .unwrap_or_else(|| panic!("authored rover curve {path} must produce a tube"));
            assert!(mesh.count_vertices() > 0);
            assert!(mesh.indices().is_some(), "tube must have triangle indices");
        }
    }
}

mod primitive_mesh_quality_tests {
    use super::*;

    #[test]
    fn primitive_mesh_density_follows_graphics_quality() {
        let shape = ShapeDims::Sphere { radius: 1.0 };
        let low = build_primitive_mesh(shape, lunco_render::RenderingQuality::Low.profile())
            .expect("low-quality sphere mesh");
        let high = build_primitive_mesh(shape, lunco_render::RenderingQuality::High.profile())
            .expect("high-quality sphere mesh");
        assert!(
            high.count_vertices() > low.count_vertices(),
            "primitive mesh quality must control sphere tessellation density"
        );
    }

    #[test]
    fn invalid_primitive_mesh_quality_is_rejected() {
        let mut quality = lunco_render::RenderingQuality::Balanced.profile();
        quality.primitive_radial_segments = 2;
        assert!(build_primitive_mesh(
            ShapeDims::Cylinder {
                radius: 1.0,
                height: 2.0
            },
            quality
        )
        .is_none());
    }
}

mod parametric_surface_tests {
    use super::*;

    #[test]
    fn lathe_api_owns_surface_even_when_profile_is_invalid() {
        let recipe = lunco_usd_core::StageRecipe::from_source(
            "lathe.usda",
            r#"#usda 1.0
def NurbsPatch "Nozzle" (
    prepend apiSchemas = ["LunCoLatheAPI"]
)
{
    uniform token lunco:lathe:profile = "typo"
    point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0), (1, 1, 0)]
    int uVertexCount = 2
    int vVertexCount = 2
    int uOrder = 2
    int vOrder = 2
}
"#,
        );
        let stage = CanonicalStage::from_recipe(&recipe).expect("build stage");
        let path = SdfPath::new("/Nozzle").unwrap();
        assert!(
            read_nurbs_patch_surface(&stage.view(), &path).is_none(),
            "an invalid parametric profile must not fall through to authored points"
        );
    }

    #[test]
    fn lathe_api_rejects_invalid_profile_parameters_without_clamping_them() {
        let recipe = lunco_usd_core::StageRecipe::from_source(
            "lathe.usda",
            r#"#usda 1.0
def NurbsPatch "Nozzle" (
    prepend apiSchemas = ["LunCoLatheAPI"]
)
{
    uniform token lunco:lathe:profile = "paraboloid"
    float lunco:lathe:focalLength = 0
    point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0), (1, 1, 0)]
    int uVertexCount = 2
    int vVertexCount = 2
    int uOrder = 2
    int vOrder = 2
}
"#,
        );
        let stage = CanonicalStage::from_recipe(&recipe).expect("build stage");
        let path = SdfPath::new("/Nozzle").unwrap();
        assert!(
            read_nurbs_patch_surface(&stage.view(), &path).is_none(),
            "an invalid focal length must not be replaced with a tiny denominator"
        );
    }

    #[test]
    fn lathe_api_requires_standard_sampling_fields() {
        let recipe = lunco_usd_core::StageRecipe::from_source(
            "lathe.usda",
            r#"#usda 1.0
def NurbsPatch "Nozzle" (
    prepend apiSchemas = ["LunCoLatheAPI"]
)
{
    uniform token lunco:lathe:profile = "bell"
    float lunco:lathe:throatRadius = 0.35
    float lunco:lathe:exitRadius = 1.35
    float lunco:lathe:length = 1.90
    float lunco:lathe:contour = 0.55
}
"#,
        );
        let stage = CanonicalStage::from_recipe(&recipe).expect("build stage");
        let path = SdfPath::new("/Nozzle").unwrap();
        assert!(
            read_nurbs_patch_surface(&stage.view(), &path).is_none(),
            "a parametric patch must author its standard sampling fields"
        );
    }

    #[test]
    fn authored_patch_requires_standard_sampling_fields() {
        let recipe = lunco_usd_core::StageRecipe::from_source(
            "patch.usda",
            r#"#usda 1.0
def NurbsPatch "Patch"
{
    point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0), (1, 1, 0)]
}
"#,
        );
        let stage = CanonicalStage::from_recipe(&recipe).expect("build stage");
        let path = SdfPath::new("/Patch").unwrap();
        assert!(
            read_nurbs_patch_surface(&stage.view(), &path).is_none(),
            "an authored patch must not receive renderer sampling defaults"
        );
    }

    #[test]
    fn authored_patch_requires_authored_knot_vectors() {
        let recipe = lunco_usd_core::StageRecipe::from_source(
            "patch.usda",
            r#"#usda 1.0
def NurbsPatch "Patch"
{
    point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0), (1, 1, 0)]
    int uVertexCount = 2
    int vVertexCount = 2
    int uOrder = 2
    int vOrder = 2
}
"#,
        );
        let stage = CanonicalStage::from_recipe(&recipe).expect("build stage");
        let path = SdfPath::new("/Patch").unwrap();
        assert!(
            read_nurbs_patch_surface(&stage.view(), &path).is_none(),
            "a patch must not receive guessed clamped knot vectors"
        );
    }

    #[test]
    fn authored_trim_data_cannot_fall_back_to_an_untrimmed_patch() {
        let recipe = lunco_usd_core::StageRecipe::from_source(
            "patch.usda",
            r#"#usda 1.0
def NurbsPatch "Patch"
{
    point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0), (1, 1, 0)]
    int uVertexCount = 2
    int vVertexCount = 2
    int uOrder = 2
    int vOrder = 2
    float[] uKnots = [0, 0, 1, 1]
    float[] vKnots = [0, 0, 1, 1]
    int[] trimCurve:counts = [1]
}
"#,
        );
        let stage = CanonicalStage::from_recipe(&recipe).expect("build stage");
        let path = SdfPath::new("/Patch").unwrap();
        assert!(
            build_usd_nurbs_patch_mesh(
                &stage.view(),
                &path,
                lunco_render::RenderingQuality::Balanced.profile()
            )
            .is_none(),
            "partial authored trim data must refuse the patch instead of restoring its hole"
        );
    }
}

mod mesh_tests {
    //! Native UsdGeomMesh → Bevy [`Mesh`] decode ([`build_usd_mesh`]).
    use super::*;

    /// Build a real composed stage. The extractors read through `StageView` — the
    /// live, PCP-composed stage — which is the ONLY read path now that the
    /// Runtime reads come from the live canonical stage. Tests read what the app reads.
    fn parse(usda: &str) -> CanonicalStage {
        CanonicalStage::from_recipe(&lunco_usd_core::StageRecipe::from_source("t.usda", usda))
            .expect("build canonical stage")
    }

    /// A single quad fan-triangulates to 2 tris (6 unindexed verts); per-vertex
    /// `primvars:st` carries through and missing normals are computed.
    #[test]
    fn quad_triangulates_with_uvs_and_computed_normals() {
        let __cs = parse(
            "#usda 1.0\n\
             def Mesh \"Quad\"\n{\n\
             point3f[] points = [(0,0,0),(1,0,0),(1,1,0),(0,1,0)]\n\
             int[] faceVertexCounts = [4]\n\
             int[] faceVertexIndices = [0,1,2,3]\n\
             texCoord2f[] primvars:st = [(0,0),(1,0),(1,1),(0,1)]\n}\n",
        );
        let reader = __cs.view();
        let mesh = build_usd_mesh(&reader, &SdfPath::new("/Quad").unwrap()).expect("mesh built");
        assert_eq!(mesh.count_vertices(), 6, "one quad → two triangles");
        assert!(
            mesh.attribute(Mesh::ATTRIBUTE_UV_0).is_some(),
            "st preserved"
        );
        assert!(
            mesh.attribute(Mesh::ATTRIBUTE_NORMAL).is_some(),
            "normals computed"
        );
    }

    /// Two triangles, no optional attrs → 6 verts, a zeroed UV set, flat normals.
    #[test]
    fn bare_triangles_get_default_uvs() {
        let __cs = parse(
            "#usda 1.0\n\
             def Mesh \"Tris\"\n{\n\
             point3f[] points = [(0,0,0),(1,0,0),(0,1,0),(1,1,0)]\n\
             int[] faceVertexCounts = [3,3]\n\
             int[] faceVertexIndices = [0,1,2,1,3,2]\n}\n",
        );
        let reader = __cs.view();
        let mesh = build_usd_mesh(&reader, &SdfPath::new("/Tris").unwrap()).expect("mesh built");
        assert_eq!(mesh.count_vertices(), 6);
        assert!(
            mesh.attribute(Mesh::ATTRIBUTE_UV_0).is_some(),
            "zeroed UVs inserted"
        );
    }

    /// Missing topology attributes → `None` (caller falls back to no mesh).
    #[test]
    fn missing_topology_returns_none() {
        let __cs = parse("#usda 1.0\ndef Mesh \"Empty\"\n{\n}\n");
        let reader = __cs.view();
        assert!(build_usd_mesh(&reader, &SdfPath::new("/Empty").unwrap()).is_none());
    }

    /// An index pointing past the end of `points` is rejected, not panicked on.
    #[test]
    fn out_of_range_index_is_rejected() {
        let __cs = parse(
            "#usda 1.0\n\
             def Mesh \"Bad\"\n{\n\
             point3f[] points = [(0,0,0),(1,0,0),(1,1,0)]\n\
             int[] faceVertexCounts = [3]\n\
             int[] faceVertexIndices = [0,1,9]\n}\n",
        );
        let reader = __cs.view();
        assert!(build_usd_mesh(&reader, &SdfPath::new("/Bad").unwrap()).is_none());
    }
}
