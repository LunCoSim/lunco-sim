use openusd::sdf::Path as SdfPath;

use crate::read::UsdReadObject;

/// Read the composed stage's `defaultPrim` metadata.
pub fn stage_default_prim(reader: &dyn UsdReadObject) -> Option<String> {
    reader.default_prim()
}

/// Resolve the empty scene-root sentinel against a composed default prim.
pub fn resolve_stage_prim_path(reader: &dyn UsdReadObject, path: &str) -> Option<String> {
    if path.is_empty() {
        return stage_default_prim(reader).map(|prim| format!("/{prim}"));
    }
    SdfPath::new(path).ok().filter(|path| path.is_abs())?;
    Some(path.to_owned())
}

/// Whether `path` is equal to, or below, the absolute `root` path.
pub fn is_descendant_or_self(path: &SdfPath, root: &str) -> bool {
    let root = root.trim_end_matches('/');
    let path = path.as_str();
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}
