//! `UsdRead` — the minimal composed-read surface shared by the flattened
//! `sdf::Data` (legacy) and the live [`StageView`](crate::view::StageView)
//! (Ph0′ canonical stage).
//!
//! This is the **decoupling seam** for the Ph0′ migration: a USD extractor
//! written generically over `UsdRead` reads the flattened `sdf::Data` today and
//! the live composed stage after cutover — with no change to its body. Because
//! `sdf::Data`'s impl routes through the exact same `field("default") +
//! TryFrom<Value>` path the extractors used before, migrating a function to
//! `UsdRead` is behaviour-preserving for the current (flattened) path — every
//! existing `&UsdData` call site keeps compiling, since `sdf::Data: UsdRead`.
//!
//! S2b scope: the trait + both impls + the pure-scalar/topology reads
//! (`read_shape_dims`, `read_int_array`). Schema/relationship/mesh/scale reads
//! migrate onto this same seam in later slices.

use openusd::sdf::{Path as SdfPath, Value};

use crate::usd_data::UsdDataExt;
use crate::view::StageView;

/// Composed, default-time reads that both the flattened `sdf::Data` and a live
/// `StageView` can serve. Generic (`<R: UsdRead>`) so extractors are written
/// once and read either source.
pub trait UsdRead {
    /// Composed `typeName` of the prim at `prim` (e.g. `"Cube"`, `"Mesh"`).
    /// Named `type_name` (not `prim_type_name`) to avoid colliding with
    /// [`UsdDataExt::prim_type_name`](crate::usd_data::UsdDataExt) when both
    /// traits are in scope on `sdf::Data`.
    fn type_name(&self, prim: &SdfPath) -> Option<String>;

    /// The default-time composed value of attribute `name` on `prim`, owned.
    fn attr_value(&self, prim: &SdfPath, name: &str) -> Option<Value>;

    /// Typed default-time read of attribute `name` on `prim`, via the SAME
    /// `TryFrom<Value>` conversion the flattened reader uses. Provided.
    fn scalar<T>(&self, prim: &SdfPath, name: &str) -> Option<T>
    where
        T: TryFrom<Value>,
    {
        self.attr_value(prim, name).and_then(|v| v.get::<T>())
    }
}

impl UsdRead for StageView<'_> {
    fn type_name(&self, prim: &SdfPath) -> Option<String> {
        self.stage()
            .prim(prim.clone())
            .type_name()
            .ok()
            .flatten()
            .map(|t| t.to_string())
    }

    fn attr_value(&self, prim: &SdfPath, name: &str) -> Option<Value> {
        self.stage()
            .prim(prim.clone())
            .attribute(name)
            .get::<Value>()
            .ok()
            .flatten()
    }
}

impl UsdRead for openusd::sdf::Data {
    fn type_name(&self, prim: &SdfPath) -> Option<String> {
        UsdDataExt::prim_type_name(self, prim)
    }

    fn attr_value(&self, prim: &SdfPath, name: &str) -> Option<Value> {
        let attr = prim.append_property(name).ok()?;
        self.field(&attr, "default").cloned()
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod read_parity_tests {
    //! S2b seam parity: the two `UsdRead` impls (live `StageView` vs flattened
    //! `sdf::Data`) return identical reads, so the generic `read_shape_dims` /
    //! `read_int_array` produce the same result from either source.

    use super::*;
    use crate::compose::{compose_file_to_stage, flatten_stage};

    const FIXTURE: &str = r#"#usda 1.0

def Xform "Shapes"
{
    def Cube "Box"
    {
        double size = 3
    }

    def Sphere "Ball"
    {
        double radius = 2
    }

    def Mesh "Tri"
    {
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0, 1, 2]
    }
}
"#;

    #[test]
    fn usdread_impls_agree_stage_vs_flatten() {
        let dir = std::env::temp_dir().join("lunco_usdread_parity");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("shapes.usda");
        std::fs::write(&f, FIXTURE).unwrap();

        let stage = compose_file_to_stage(&f).expect("compose stage");
        let flat = flatten_stage(&stage).expect("flatten");
        let view = StageView::new(&stage);

        // typeName agrees across sources.
        for (p, ty) in [("/Shapes/Box", "Cube"), ("/Shapes/Ball", "Sphere"), ("/Shapes/Tri", "Mesh")] {
            let path = SdfPath::new(p).unwrap();
            assert_eq!(
                UsdRead::type_name(&view, &path),
                UsdRead::type_name(&flat, &path),
                "typeName mismatch at {p}"
            );
            assert_eq!(UsdRead::type_name(&flat, &path).as_deref(), Some(ty));
        }

        // Scalar reads (what read_shape_dims consumes) agree.
        let box_p = SdfPath::new("/Shapes/Box").unwrap();
        assert_eq!(view.scalar::<f64>(&box_p, "size"), flat.scalar::<f64>(&box_p, "size"));
        assert_eq!(view.scalar::<f64>(&box_p, "size"), Some(3.0));
        let ball = SdfPath::new("/Shapes/Ball").unwrap();
        assert_eq!(view.scalar::<f64>(&ball, "radius"), flat.scalar::<f64>(&ball, "radius"));
        assert_eq!(view.scalar::<f64>(&ball, "radius"), Some(2.0));

        // Int-array topology reads (what read_int_array serves) agree.
        let tri = SdfPath::new("/Shapes/Tri").unwrap();
        assert_eq!(
            crate::read_int_array(&view, &tri, "faceVertexIndices"),
            crate::read_int_array(&flat, &tri, "faceVertexIndices"),
        );
        assert_eq!(
            crate::read_int_array(&flat, &tri, "faceVertexIndices"),
            Some(vec![0, 1, 2])
        );

        // The generic reads themselves run against BOTH sources.
        assert!(crate::read_shape_dims(&view, &box_p, "Cube").is_some());
        assert!(crate::read_shape_dims(&flat, &box_p, "Cube").is_some());
    }
}
