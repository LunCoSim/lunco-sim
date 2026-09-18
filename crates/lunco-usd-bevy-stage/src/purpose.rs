use openusd::sdf::Path as SdfPath;

use crate::read::UsdReadObject;

/// The standard `UsdGeomImageable.purpose` value resolved for a composed prim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Default,
    Render,
    Proxy,
    Guide,
}

/// Read the composed stage's `defaultPrim` metadata.
pub fn stage_default_prim(reader: &dyn UsdReadObject) -> Option<String> {
    reader.default_prim()
}

/// Resolve the composed, inherited `UsdGeomImageable.purpose` value.
pub fn effective_purpose(reader: &dyn UsdReadObject, path: &SdfPath) -> Purpose {
    let mut cur = Some(path.clone());
    while let Some(p) = cur {
        if p.is_abs_root() {
            break;
        }
        match reader.text(&p, "purpose").as_deref() {
            Some("guide") => return Purpose::Guide,
            Some("proxy") => return Purpose::Proxy,
            Some("render") => return Purpose::Render,
            Some("default") => return Purpose::Default,
            _ => {}
        }
        cur = p.parent();
    }
    Purpose::Default
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
