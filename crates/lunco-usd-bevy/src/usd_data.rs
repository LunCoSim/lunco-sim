//! Typed reads over a composed USD scene on openusd `main`.
//!
//! openusd `main` removed `TextReader`. Composition still runs through an
//! openusd `Stage` (see [`crate::compose`]) using the in-memory
//! [`LuncoUsdResolver`](crate::resolver::LuncoUsdResolver), but `Stage` is
//! `!Send` (`Rc`-backed) so it can't live in Bevy ECS. The composed stage is
//! flattened to a Send-safe [`sdf::Data`] (`HashMap<Path, SpecData>`), and this
//! extension trait adds the ergonomic, typed reads the rest of the stack needs
//! directly on openusd's own data type — no separate reader object.
//!
//! These read *flattened, already-composed* data: references, variants, and
//! sublayers were resolved by the Stage before flattening, so a plain spec
//! lookup here returns the composed opinion.

use openusd::sdf::{self, Path, SpecType, Value};
use openusd::tf;

/// Ergonomic reads over a composed [`sdf::Data`]. Replaces the removed
/// `TextReader` query methods (`try_get` → [`field`](UsdDataExt::field),
/// `prim_children`, `prim_attribute_value`).
pub trait UsdDataExt {
    /// The raw value of field `key` on the spec at `path`, if present. (A
    /// missing spec or field is simply `None` — the old `try_get` returned a
    /// `Result<Option<_>>`; reading flattened in-memory data can't fail.)
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

    /// Whether a prim is active (`active` metadata; defaults to `true`, matching
    /// USD semantics).
    fn prim_is_active(&self, prim: &Path) -> bool;
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
            .or_else(|| self.field_as::<String>(prim, "typeName"))
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
