//! Typed reads over an authored USD layer.
//!
//! [`sdf::Data`] is the `Send`-safe representation used by document authoring,
//! save, journal, network, and authoring-only UI code. It contains the layer's
//! own specs and preserves composition arcs as authored. Runtime projection
//! reads the live composed [`crate::StageView`] through [`crate::UsdRead`]
//! instead; this extension must not be used as a runtime-stage substitute.

use openusd::sdf::{self, Path, SpecType, Value};
use openusd::tf;

/// Send-safe authored layer data used by document and authoring APIs.
pub type UsdData = sdf::Data;

/// Ergonomic reads over authored [`sdf::Data`].
pub trait UsdDataExt {
    /// The raw value of field `key` on the spec at `path`, if present. (A
    /// missing spec or field is simply `None`.)
    fn field(&self, path: &Path, key: &str) -> Option<&Value>;

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

    /// Whether the prim is active, including the USD default of true.
    fn prim_is_active(&self, prim: &Path) -> bool;
}

/// Whether a prim in an authored layer applies the named API schema.
///
/// This deliberately reads the layer's own `apiSchemas` opinion. Runtime
/// queries over a composed stage use the composed-stage reader instead;
/// authoring and validation code must not silently substitute one read surface
/// for the other.
pub fn has_authored_api_schema(reader: &sdf::Data, path: &Path, schema_name: &str) -> bool {
    let Some(value) = reader.field(path, "apiSchemas") else {
        return false;
    };
    match value {
        Value::Token(value) => value.as_str() == schema_name,
        Value::String(value) => value == schema_name,
        Value::TokenVec(values) => values.iter().any(|value| value.as_str() == schema_name),
        Value::TokenListOp(op) => op
            .explicit_items
            .iter()
            .chain(op.prepended_items.iter())
            .chain(op.appended_items.iter())
            .chain(op.added_items.iter())
            .any(|value| value.as_str() == schema_name),
        _ => false,
    }
}

impl UsdDataExt for sdf::Data {
    fn field(&self, path: &Path, key: &str) -> Option<&Value> {
        self.spec(path).and_then(|s| s.get(key))
    }

    fn field_as<T: TryFrom<Value>>(&self, path: &Path, key: &str) -> Option<T> {
        self.field(path, key).cloned().and_then(|v| v.get::<T>())
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

    fn prim_is_active(&self, prim: &Path) -> bool {
        self.field_as::<bool>(prim, "active").unwrap_or(true)
    }
}
