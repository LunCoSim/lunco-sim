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

use openusd::sdf::{Path as SdfPath, SpecType, Value};

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

    /// Whether `prim` applies the named API schema (its composed `apiSchemas`) —
    /// the physics extractor's body/collider/terrain detection read
    /// (`PhysicsRigidBodyAPI` / `PhysicsCollisionAPI` / `PhysxTerrainAPI`).
    fn has_api_schema(&self, prim: &SdfPath, schema: &str) -> bool;

    /// First composed target of relationship `name` on `prim`, as a path string
    /// (e.g. a joint's `physics:body0`). Composed = PCP-translated.
    fn rel_target(&self, prim: &SdfPath, name: &str) -> Option<String>;

    /// Immediate composed prim children of `prim`.
    fn children(&self, prim: &SdfPath) -> Vec<SdfPath>;

    /// Every live composed prim path (active, defined, non-abstract), in
    /// traversal order — the set a per-stage scan iterates. On the live stage
    /// this is `Stage::traverse`; on the flatten it is the `Prim`-typed specs.
    fn prim_paths(&self) -> Vec<SdfPath>;

    /// The leaf names of every authored property on `prim` (e.g.
    /// `"primvars:baseColor"`, `"xformOp:translate"`) — the set the shader
    /// authoring pass enumerates to apply arbitrary `primvars:*`. On the live
    /// stage this is `Prim::property_names`; on the flatten it is the child
    /// specs directly under `<prim>.`.
    fn attr_names(&self, prim: &SdfPath) -> Vec<String>;

    /// The composed value of attribute `name` on `prim` at time code `time` —
    /// authored `timeSamples` (interpolated) win, else the `default` opinion.
    /// The transform decoders read at `time = 0.0` for static geometry.
    fn attr_value_at(&self, prim: &SdfPath, name: &str, time: f64) -> Option<Value>;

    /// Typed timeSamples-or-default read — the `_at` sibling of [`scalar`](Self::scalar).
    fn scalar_at<T>(&self, prim: &SdfPath, name: &str, time: f64) -> Option<T>
    where
        T: TryFrom<Value>,
    {
        self.attr_value_at(prim, name, time).and_then(|v| v.get::<T>())
    }

    /// The glTF/binary asset URI resolved for `prim` (`lunco:resolvedAsset`) —
    /// authored, or synthesized from a binary (glTF) reference/payload in the
    /// prim's composition stack. On the flattened reader this reads the attr
    /// `flatten_stage` synthesized; on the live stage it is synthesized here.
    fn resolved_asset(&self, prim: &SdfPath) -> Option<String>;

    /// Whether `prim` is active (`active` metadata; defaults to `true`, matching
    /// USD semantics). The visual extractor skips mesh/child creation for
    /// inactive prims.
    fn is_active(&self, prim: &SdfPath) -> bool;

    /// The stage's `defaultPrim` bare name (no leading slash), or `None` when the
    /// stage declares none. The empty-path scene-root sentinel resolves through
    /// this to the concrete subtree the reference/scene mounts.
    fn default_prim(&self) -> Option<String>;

    /// Whether attribute `name` on `prim` actually carries authored
    /// `timeSamples` (not merely a `default`) — the per-channel test the
    /// [`UsdAnimated`](crate::UsdAnimated) tagging uses so only genuinely
    /// animated prims are sampled per frame.
    fn has_time_samples(&self, prim: &SdfPath, name: &str) -> bool;
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

    fn has_api_schema(&self, prim: &SdfPath, schema: &str) -> bool {
        self.stage()
            .prim(prim.clone())
            .api_schemas()
            .map(|v| v.iter().any(|s| s.as_str() == schema))
            .unwrap_or(false)
    }

    fn rel_target(&self, prim: &SdfPath, name: &str) -> Option<String> {
        let p = self.stage().prim(prim.clone());
        // Relationship targets first (`material:binding`, `physics:body0`)…
        if let Some(t) = p.relationship(name).targets().unwrap_or_default().into_iter().next() {
            return Some(t.to_string());
        }
        // …else an attribute connection (`.connect`, e.g. `outputs:surface`). The
        // flattened reader folds both `targetPaths` and `connectionPaths` into one
        // `read_rel_target`; mirror that here so the bind→shader walk resolves off
        // the live stage.
        p.attribute(name)
            .connections()
            .ok()
            .and_then(|c| c.into_iter().next())
            .map(|t| t.to_string())
    }

    fn children(&self, prim: &SdfPath) -> Vec<SdfPath> {
        self.stage()
            .prim(prim.clone())
            .children()
            .map(|cs| cs.iter().map(|c| c.path().clone()).collect())
            .unwrap_or_default()
    }

    fn prim_paths(&self) -> Vec<SdfPath> {
        // Inherent `StageView::prim_paths` (composed traversal).
        StageView::prim_paths(self)
    }

    fn attr_names(&self, prim: &SdfPath) -> Vec<String> {
        self.stage()
            .prim(prim.clone())
            .property_names()
            .map(|ns| ns.iter().map(|t| t.to_string()).collect())
            .unwrap_or_default()
    }

    fn attr_value_at(&self, prim: &SdfPath, name: &str, time: f64) -> Option<Value> {
        let attr = self.stage().prim(prim.clone()).attribute(name);
        if let Ok(Some(samples)) = attr.time_samples() {
            if let Some(v) =
                openusd::usd::evaluate(&samples, time, openusd::usd::InterpolationType::Linear)
            {
                return Some(v);
            }
        }
        attr.get::<Value>().ok().flatten()
    }

    fn resolved_asset(&self, prim: &SdfPath) -> Option<String> {
        // Authored value wins; else synthesize from a binary arc in the stack.
        if let Some(authored) = self.value_str(prim, "lunco:resolvedAsset") {
            return Some(authored);
        }
        let sites = self.binary_sites()?;
        let stack = self.stage().prim(prim.clone()).prim_stack().ok()?;
        stack.iter().find_map(|site| sites.get(site)).cloned()
    }

    fn is_active(&self, prim: &SdfPath) -> bool {
        self.stage().prim(prim.clone()).is_active().unwrap_or(true)
    }

    fn default_prim(&self) -> Option<String> {
        self.stage()
            .default_prim()
            .map(|t| t.to_string())
            .filter(|s| !s.is_empty())
    }

    fn has_time_samples(&self, prim: &SdfPath, name: &str) -> bool {
        self.stage()
            .prim(prim.clone())
            .attribute(name)
            .time_samples()
            .ok()
            .flatten()
            .is_some_and(|s| !s.is_empty())
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

    fn has_api_schema(&self, prim: &SdfPath, schema: &str) -> bool {
        crate::has_api_schema(self, prim, schema)
    }

    fn rel_target(&self, prim: &SdfPath, name: &str) -> Option<String> {
        crate::read_rel_target(self, prim, name)
    }

    fn children(&self, prim: &SdfPath) -> Vec<SdfPath> {
        UsdDataExt::prim_children(self, prim)
    }

    fn prim_paths(&self) -> Vec<SdfPath> {
        self.iter()
            .filter(|(_, s)| s.ty == SpecType::Prim)
            .map(|(p, _)| p.clone())
            .collect()
    }

    fn attr_names(&self, prim: &SdfPath) -> Vec<String> {
        // A prim's properties are child specs at `<prim>.<name>`; keep the ones
        // directly under this prim (leaf name after the USD `.` separator).
        let prefix = format!("{}.", prim.as_str());
        self.iter()
            .filter_map(|(p, _)| p.as_str().strip_prefix(&prefix).map(str::to_string))
            .collect()
    }

    fn attr_value_at(&self, prim: &SdfPath, name: &str, time: f64) -> Option<Value> {
        crate::value_at(self, prim, name, time)
    }

    fn resolved_asset(&self, prim: &SdfPath) -> Option<String> {
        // The flatten already synthesized `lunco:resolvedAsset` onto the prim.
        match self.attr_value(prim, "lunco:resolvedAsset")? {
            Value::String(s) => Some(s),
            Value::Token(s) => Some(s.to_string()),
            Value::AssetPath(a) => Some(a.as_str().to_string()),
            _ => None,
        }
    }

    fn is_active(&self, prim: &SdfPath) -> bool {
        UsdDataExt::prim_is_active(self, prim)
    }

    fn default_prim(&self) -> Option<String> {
        // `defaultPrim` is authored as `Value::Token`; `as_str` coerces
        // Token/String/AssetPath uniformly (matches `stage_default_prim`).
        let name = self.field(&SdfPath::abs_root(), "defaultPrim")?.as_str()?;
        (!name.is_empty()).then(|| name.to_string())
    }

    fn has_time_samples(&self, prim: &SdfPath, name: &str) -> bool {
        prim.append_property(name)
            .ok()
            .and_then(|ap| self.field(&ap, "timeSamples"))
            .is_some_and(|v| matches!(v, Value::TimeSamples(_)))
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

    def Xform "Body" (
        prepend apiSchemas = ["PhysicsRigidBodyAPI"]
    )
    {
        rel physics:body0 = </Shapes/Box>
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

        // Physics detection reads (has_api_schema / rel_target) agree — the reads
        // the observer flip (S2e-ii) needs.
        let body = SdfPath::new("/Shapes/Body").unwrap();
        assert_eq!(
            UsdRead::has_api_schema(&view, &body, "PhysicsRigidBodyAPI"),
            UsdRead::has_api_schema(&flat, &body, "PhysicsRigidBodyAPI"),
        );
        assert!(UsdRead::has_api_schema(&flat, &body, "PhysicsRigidBodyAPI"));
        assert!(!UsdRead::has_api_schema(&view, &body, "PhysxTerrainAPI"));
        assert_eq!(
            UsdRead::rel_target(&view, &body, "physics:body0"),
            UsdRead::rel_target(&flat, &body, "physics:body0"),
        );
        assert_eq!(
            UsdRead::rel_target(&flat, &body, "physics:body0").as_deref(),
            Some("/Shapes/Box")
        );
    }
}
