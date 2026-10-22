//! Authored wheel render ownership, independent of collision geometry.

use lunco_usd_bevy_stage::read::UsdReadObject;
use openusd::sdf::Path as SdfPath;
use openusd_schemas::geom::{Purpose, PurposeVisibility};

/// Select an explicit render child, or the wheel prim's own geometry.
/// A render root is centered on the axle; offset submeshes belong below it.
pub fn wheel_render_root(
    reader: &dyn UsdReadObject,
    wheel: &SdfPath,
) -> Result<Option<SdfPath>, String> {
    let mut roots = Vec::new();
    for child in reader.children(wheel) {
        if reader.has_authored_attribute(&child, "purpose")
            && reader.purpose(&child)? == Purpose::Render
        {
            roots.push(child);
        }
    }
    if roots.len() > 1 {
        return Err("wheel requires one render root; group its visual parts below one child with purpose=render".into());
    }
    let Some(root) = roots.pop() else {
        return Ok(None);
    };
    if !matches!(reader.type_name(wheel).as_deref(), Some("Xform" | "Scope"))
        && reader.effective_visibility(wheel)? != PurposeVisibility::Invisible
    {
        return Err("wheel geometry and its render child would overlap; hide the collision proxy with VisibilityAPI proxyVisibility=invisible".into());
    }
    let pose = lunco_usd_bevy_stage::local_transform_at(reader, &root, 0.0)
        .map_err(|error| error.to_string())?;
    if pose.is_some_and(|pose| pose.translation != bevy_math::Vec3::ZERO) {
        return Err("wheel render root must be centered on the axle; place offset visual parts below that root".into());
    }
    if lunco_usd_bevy_stage::read_xform_op_order(reader, &root)
        .is_some_and(|order| order.iter().any(|op| op == "!resetXformStack!"))
    {
        return Err("wheel render root must retain its wheel-local transform stack".into());
    }
    Ok(Some(root))
}
