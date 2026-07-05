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

    /// **All** composed connection sources of attribute `name` on `prim` — the
    /// full `connectionPaths` list (fan-in), as path strings, in list order.
    /// [`rel_target`](Self::rel_target) returns only the *first* target; the
    /// co-sim wiring derivation needs *every* source on an `inputs:` attr (a
    /// fan-in sink sums multiple producers). Empty when the attribute carries no
    /// authored connections.
    fn connections(&self, prim: &SdfPath, name: &str) -> Vec<String>;

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

    /// Whether a prim exists at `prim` in the composed scene — the existence
    /// test the incremental structural reconcile uses to tell a spawn (present in
    /// the stage, no live entity) from a remove (absent, but a live entity
    /// survives). On the live stage this is `Prim::is_valid`; on the flatten it
    /// is a spec lookup.
    fn has_prim(&self, prim: &SdfPath) -> bool;

    /// The stage's `defaultPrim` bare name (no leading slash), or `None` when the
    /// stage declares none. The empty-path scene-root sentinel resolves through
    /// this to the concrete subtree the reference/scene mounts.
    fn default_prim(&self) -> Option<String>;

    /// Whether attribute `name` on `prim` actually carries authored
    /// `timeSamples` (not merely a `default`) — the per-channel test the
    /// [`UsdAnimated`](crate::UsdAnimated) tagging uses so only genuinely
    /// animated prims are sampled per frame.
    fn has_time_samples(&self, prim: &SdfPath, name: &str) -> bool;

    /// The stage's `timeCodesPerSecond` — seconds × this = time code (USD maps a
    /// time code `t` to `t / tcps` seconds). On `StageView` this is the composed
    /// stage metadata; on the flatten it is the pseudo-root `timeCodesPerSecond`
    /// field. Defaults to 24.0 (USD spec) when unauthored; callers apply their
    /// own non-positive guard.
    fn time_codes_per_second(&self) -> f64;

    /// The authored `timeSamples` time codes of attribute `name` on `prim`,
    /// ascending. Empty when the attribute carries no `timeSamples`. Feeds the
    /// animated-clip span ([`time_sample_span`](Self::time_sample_span)) and the
    /// camera-track key enumeration.
    fn time_sample_times(&self, prim: &SdfPath, name: &str) -> Vec<f64>;

    /// The authored `timeSamples` span `(first, last)` of attribute `name` on
    /// `prim` — the min/max sample time codes. Provided from
    /// [`time_sample_times`](Self::time_sample_times) (samples are stored
    /// ascending). `None` when the attribute is unsampled.
    fn time_sample_span(&self, prim: &SdfPath, name: &str) -> Option<(f64, f64)> {
        let ts = self.time_sample_times(prim, name);
        Some((*ts.first()?, *ts.last()?))
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

    fn connections(&self, prim: &SdfPath, name: &str) -> Vec<String> {
        // `Attribute::connections()` returns the composed list-op resolved to a
        // flat `Vec<Path>` — exactly the fan-in set the derivation needs.
        self.stage()
            .prim(prim.clone())
            .attribute(name)
            .connections()
            .map(|cs| cs.into_iter().map(|p| p.to_string()).collect())
            .unwrap_or_default()
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

    fn has_prim(&self, prim: &SdfPath) -> bool {
        self.stage().prim(prim.clone()).is_valid().unwrap_or(false)
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

    fn time_codes_per_second(&self) -> f64 {
        self.stage().time_codes_per_second()
    }

    fn time_sample_times(&self, prim: &SdfPath, name: &str) -> Vec<f64> {
        self.stage()
            .prim(prim.clone())
            .attribute(name)
            .time_samples()
            .ok()
            .flatten()
            .map(|s| s.iter().map(|(t, _)| *t).collect())
            .unwrap_or_default()
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

    fn connections(&self, prim: &SdfPath, name: &str) -> Vec<String> {
        // The flattened reader stores the authored `connectionPaths` list-op;
        // collect every item (explicit + the composed list-op edit fields) so a
        // fan-in sink surfaces all its producers.
        let Ok(attr) = prim.append_property(name) else {
            return Vec::new();
        };
        match self.field(&attr, "connectionPaths") {
            Some(Value::PathListOp(op)) => op
                .explicit_items
                .iter()
                .chain(op.prepended_items.iter())
                .chain(op.appended_items.iter())
                .chain(op.added_items.iter())
                .map(|p| p.to_string())
                .collect(),
            _ => Vec::new(),
        }
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

    fn has_prim(&self, prim: &SdfPath) -> bool {
        self.spec(prim).is_some()
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

    fn time_codes_per_second(&self) -> f64 {
        // Pseudo-root `timeCodesPerSecond` (flattened onto abs-root); default 24.0
        // (USD spec) when unauthored. The non-positive guard lives in the free
        // `stage_time_codes_per_second` wrapper, shared with the live path.
        self.field_as::<f64>(&SdfPath::abs_root(), "timeCodesPerSecond")
            .unwrap_or(24.0)
    }

    fn time_sample_times(&self, prim: &SdfPath, name: &str) -> Vec<f64> {
        prim.append_property(name)
            .ok()
            .and_then(|ap| self.field(&ap, "timeSamples"))
            .and_then(|v| match v {
                Value::TimeSamples(s) => Some(s.iter().map(|(t, _)| *t).collect()),
                _ => None,
            })
            .unwrap_or_default()
    }
}
