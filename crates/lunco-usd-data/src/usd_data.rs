//! Typed reads over an authored USD layer.
//!
//! [`sdf::Data`] is the `Send`-safe representation used by document authoring,
//! save, journal, network, and authoring-only UI code. It contains the layer's
//! own specs and preserves composition arcs as authored. Runtime projection
//! reads the live composed `lunco_usd_bevy_stage::StageView` through `UsdRead`
//! instead; this extension must not be used as a runtime-stage substitute.

use openusd::sdf::{self, Path, SpecType, Value};
use openusd::tf;

/// Send-safe authored layer data used by document and authoring APIs.
pub type UsdData = sdf::Data;

/// Ergonomic reads over authored [`sdf::Data`].
pub trait UsdDataExt {
    /// A typed view of field `key` on the spec at `path` (clones then extracts
    /// via `TryFrom<Value>`); `None` if absent or the wrong type.
    fn field_as<T: TryFrom<Value>>(&self, path: &Path, key: &str) -> Option<T>;

    /// Immediate prim children of `path` (prim specs whose parent is `path`).
    fn prim_children(&self, path: &Path) -> Vec<Path>;

    /// A prim's authored `typeName` (e.g. `"Xform"`, `"Mesh"`), if any.
    fn prim_type_name(&self, prim: &Path) -> Option<String>;

    /// The `default` value of attribute `name` on prim `prim`, typed as `T`.
    fn prim_attribute_value<T: TryFrom<Value>>(&self, prim: &Path, name: &str) -> Option<T>;

    /// The `default` value of the attribute at `attr_path`, typed as `T`.
    fn attribute_value<T: TryFrom<Value>>(&self, attr_path: &Path) -> Option<T>;
}

impl UsdDataExt for sdf::Data {
    fn field_as<T: TryFrom<Value>>(&self, path: &Path, key: &str) -> Option<T> {
        self.spec(path)
            .and_then(|spec| spec.get(key))
            .cloned()
            .and_then(|v| v.get::<T>())
    }

    fn prim_children(&self, path: &Path) -> Vec<Path> {
        self.iter()
            .filter(|(p, s)| s.ty == SpecType::Prim && p.parent().as_ref() == Some(path))
            .map(|(p, _)| p.clone())
            .collect()
    }

    fn prim_type_name(&self, prim: &Path) -> Option<String> {
        self.field_as::<tf::Token>(prim, "typeName")
            .map(|t| t.to_string())
    }

    fn prim_attribute_value<T: TryFrom<Value>>(&self, prim: &Path, name: &str) -> Option<T> {
        let attr = prim.append_property(name).ok()?;
        self.field_as::<T>(&attr, "default")
    }

    fn attribute_value<T: TryFrom<Value>>(&self, attr_path: &Path) -> Option<T> {
        self.field_as::<T>(attr_path, "default")
    }
}
