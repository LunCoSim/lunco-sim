use openusd::sdf::Path as SdfPath;

use crate::read::UsdReadObject;
use crate::view::StageView;

/// Which purpose of a USD material binding to resolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MaterialPurpose {
    /// All-purpose binding (`material:binding`) — the rendered look.
    Render,
    /// Purpose-specific binding (`material:binding:physics`).
    Physics,
}

impl MaterialPurpose {
    /// The USD binding purpose token.
    pub fn token(self) -> &'static str {
        match self {
            Self::Render => openusd::schemas::shade::tokens::PURPOSE_ALL,
            Self::Physics => "physics",
        }
    }
}

/// Extract the owning prim from a USD property path.
pub fn parent_prim_path(target: &str) -> Option<SdfPath> {
    Some(SdfPath::new(target).ok()?.prim_path())
}

/// Resolve the standard UsdShade material binding for one purpose.
pub fn resolve_bound_material(
    reader: &StageView<'_>,
    prim: &SdfPath,
    purpose: MaterialPurpose,
) -> Option<SdfPath> {
    openusd::schemas::shade::MaterialBindingAPI::on(reader.stage(), prim.clone())
        .compute_bound_material(purpose.token())
        .ok()
        .flatten()
}

/// Resolve the surface shader connected to the rendered material binding.
pub fn resolve_bound_shader(reader: &dyn UsdReadObject, mesh_path: &SdfPath) -> Option<SdfPath> {
    let mat_path = reader.bound_material(mesh_path, MaterialPurpose::Render)?;
    let mat_path = SdfPath::new(&mat_path).ok()?;
    let surface = reader.connection_source(&mat_path, "outputs:surface")?;
    parent_prim_path(&surface)
}
