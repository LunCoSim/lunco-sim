use openusd::sdf::Path as SdfPath;

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
            Self::Render => openusd_schemas::shade::tokens::ALL_PURPOSE,
            Self::Physics => "physics",
        }
    }
}

/// Extract the owning prim from a USD property path.
pub fn parent_prim_path(target: &str) -> Option<SdfPath> {
    Some(SdfPath::new(target).ok()?.prim_path())
}

/// Resolve a standard native material binding, preserving native errors.
pub(crate) fn resolve_bound_material(
    reader: &StageView<'_>,
    prim: &SdfPath,
    purpose: MaterialPurpose,
) -> Result<Option<SdfPath>, String> {
    let native = reader
        .stage()
        .prim(prim)
        .map_err(|error| error.to_string())?;
    if !native.is_valid().map_err(|error| error.to_string())? {
        return Err(format!("{prim}: prim is absent from the live stage"));
    }
    let bound = openusd_schemas::shade::MaterialBindingAPI::from_prim_unchecked(native)
        .compute_bound_material(purpose.token())
        .map_err(|error| format!("{prim}: {error}"))?;
    if let Some(path) = &bound {
        openusd_schemas::shade::Material::get(reader.stage(), path.clone())
            .map_err(|error| format!("{path}: {error}"))?
            .ok_or_else(|| format!("{prim}: bound target {path} is not a Material"))?;
    }
    Ok(bound)
}

/// Resolve the universal surface terminal through native UsdShade graphs.
pub(crate) fn resolve_surface_shader(
    reader: &StageView<'_>,
    prim: &SdfPath,
) -> Result<Option<SdfPath>, String> {
    let Some(path) = resolve_bound_material(reader, prim, MaterialPurpose::Render)? else {
        return Ok(None);
    };
    resolve_material_surface_shader(reader, &path)
}

/// Resolve a material's universal surface terminal after its binding is known.
pub(crate) fn resolve_material_surface_shader(
    reader: &StageView<'_>,
    path: &SdfPath,
) -> Result<Option<SdfPath>, String> {
    let material = openusd_schemas::shade::Material::get(reader.stage(), path.clone())
        .map_err(|error| format!("{path}: {error}"))?
        .ok_or_else(|| format!("{path}: surface terminal target is not a Material"))?;
    let terminal = material
        .compute_surface_source(&[])
        .map_err(|error| format!("{path}: {error}"))?;
    let Some(terminal) = terminal else {
        if !material
            .surface_output()
            .attribute()
            .connections()
            .map_err(|error| format!("{path}: {error}"))?
            .is_empty()
        {
            return Err(format!(
                "{path}: surface connections have no producing shader"
            ));
        }
        return Ok(None);
    };
    let source = terminal
        .sources()
        .first()
        .ok_or_else(|| format!("{path}: native surface terminal has no source"))?;
    if source.shader().is_none() {
        return Err(format!(
            "{path}: surface endpoint {} is not a Shader",
            source.attribute().path()
        ));
    }
    Ok(Some(source.attribute().path().prim_path()))
}
