//! Standards-based reads for [`UsdGeomPointInstancer`].
//!
//! The reader keeps the USD schema as the authority: `prototypes`,
//! `protoIndices`, and `positions` are required, optional arrays must either be
//! empty/omitted or match the instance count, and `ids`/`invisibleIds` follow
//! the masking rules defined by OpenUSD. The returned transforms are canonical
//! Bevy transforms; rendering remains owned by the visual adapter.

use anyhow::{bail, Result};
use bevy::prelude::{Quat, Transform, Vec3};
use openusd::sdf::{Path as SdfPath, Value};

#[cfg(test)]
use crate::UsdStageProjectionPlan;

/// One evaluated entry in a standard [`UsdGeomPointInstancer`].
///
/// The transform includes the prototype root's local transform, matching
/// OpenUSD's default `IncludeProtoXform` behavior. The visual adapter can use
/// the prototype path to share the prototype's mesh and material handles.
#[derive(Clone, Debug, PartialEq)]
pub struct UsdPointInstancePlan {
    /// Zero-based position in the authored per-instance arrays.
    pub index: usize,
    /// Stable authored id, or the array index when `ids` is omitted.
    pub id: i64,
    /// Zero-based index into the ordered `prototypes` relationship.
    pub prototype_index: usize,
    /// Targeted prototype root prim.
    pub prototype_path: String,
    /// Instance transform in the engine's canonical basis.
    pub transform: Transform,
    /// Whether the instance survives the standard `invisibleIds` mask.
    pub visible: bool,
}

/// Read one composed `UsdGeomPointInstancer` at `time`.
///
/// This follows the schema and the OpenUSD transform contract rather than
/// inventing a LunCoSim metadata field: required arrays are present and
/// aligned, optional arrays are either empty/omitted or aligned, and
/// `orientationsf` takes precedence over `orientations` when non-empty. The
/// returned data is owned so it works for both the prepared worker snapshot and
/// the live canonical reader.
pub fn read_point_instancer<R: crate::UsdRead>(
    reader: &R,
    path: &SdfPath,
    time: f64,
) -> Result<Vec<UsdPointInstancePlan>> {
    if reader.type_name(path).as_deref() != Some("PointInstancer") {
        bail!("{} is not a UsdGeomPointInstancer", path.as_str());
    }

    let prototype_paths = reader
        .rel_targets(path, "prototypes")
        .into_iter()
        .map(|prototype| prototype.to_string())
        .collect::<Vec<_>>();
    if prototype_paths.is_empty() {
        bail!(
            "{} has no targets in required prototypes relationship",
            path.as_str()
        );
    }
    for prototype in &prototype_paths {
        let prototype_path = SdfPath::new(prototype).map_err(|error| {
            anyhow::anyhow!(
                "{} has invalid prototype target {prototype}: {error}",
                path.as_str()
            )
        })?;
        if !reader.has_prim(&prototype_path) {
            bail!("{} targets missing prototype {prototype}", path.as_str());
        }
    }

    let positions = required_points(reader, path, "positions", time)?;
    let proto_indices = required_ints(reader, path, "protoIndices", time)?;
    if positions.len() != proto_indices.len() {
        bail!(
            "{} positions length {} must match protoIndices length {}",
            path.as_str(),
            positions.len(),
            proto_indices.len()
        );
    }

    let scales = optional_vec3(reader, path, "scales", time)?;
    validate_optional_len(
        path,
        "scales",
        scales.as_ref().map(Vec::len),
        positions.len(),
    )?;

    let orientationsf = optional_quats(reader, path, "orientationsf", time)?;
    let orientations = optional_quats(reader, path, "orientations", time)?;
    let orientations = match orientationsf.filter(|values| !values.is_empty()) {
        Some(values) => Some(values),
        None => orientations.filter(|values| !values.is_empty()),
    };
    validate_optional_len(
        path,
        "orientations/orientationsf",
        orientations.as_ref().map(Vec::len),
        positions.len(),
    )?;

    let ids = optional_int64s(reader, path, "ids", time)?;
    validate_optional_len(path, "ids", ids.as_ref().map(Vec::len), positions.len())?;
    let invisible_ids = optional_int64s(reader, path, "invisibleIds", time)?.unwrap_or_default();

    let convention =
        crate::stage_convention(reader as &dyn crate::UsdReadObject).map_err(|error| {
            anyhow::anyhow!("{} has invalid stage convention: {error}", path.as_str())
        })?;

    positions
        .into_iter()
        .enumerate()
        .map(|(index, position)| {
            let proto_index = usize::try_from(proto_indices[index]).map_err(|_| {
                anyhow::anyhow!(
                    "{} protoIndices[{index}] = {} is negative",
                    path.as_str(),
                    proto_indices[index]
                )
            })?;
            let prototype_path = prototype_paths.get(proto_index).ok_or_else(|| {
                anyhow::anyhow!(
                    "{} protoIndices[{index}] = {proto_index} is outside prototypes[0..{}]",
                    path.as_str(),
                    prototype_paths.len()
                )
            })?;
            let prototype_sdf_path = SdfPath::new(prototype_path).map_err(|error| {
                anyhow::anyhow!("invalid prototype path {prototype_path}: {error}")
            })?;
            let prototype_transform = reader
                .local_transform_at(&prototype_sdf_path, time)
                .map_err(|error| anyhow::anyhow!("{prototype_path}: {error}"))?
                .unwrap_or_default();
            let orientation = orientations
                .as_ref()
                .map(|values| values[index])
                .unwrap_or(Quat::IDENTITY);
            let scale = scales
                .as_ref()
                .map(|values| values[index])
                .unwrap_or(Vec3::ONE);
            let instance_transform = Transform {
                translation: convention.point(Vec3::from_array(position)),
                rotation: convention.rotation(orientation),
                scale: convention.scale_vec(scale),
            };
            let id = ids
                .as_ref()
                .map(|values| values[index])
                .unwrap_or(index as i64);
            Ok(UsdPointInstancePlan {
                index,
                id,
                prototype_index: proto_index,
                prototype_path: prototype_path.clone(),
                transform: instance_transform.mul_transform(prototype_transform),
                visible: !invisible_ids.contains(&id),
            })
        })
        .collect()
}

fn required_points<R: crate::UsdRead>(
    reader: &R,
    path: &SdfPath,
    name: &str,
    time: f64,
) -> Result<Vec<[f32; 3]>> {
    match reader.attr_value_at(path, name, time) {
        Some(Value::Vec3fVec(values)) => Ok(values
            .into_iter()
            .map(|value| [value.x, value.y, value.z])
            .collect()),
        Some(_) => bail!("{} required {} must be point3f[]", path.as_str(), name),
        None => bail!("{} is missing required {}", path.as_str(), name),
    }
}

fn required_ints<R: crate::UsdRead>(
    reader: &R,
    path: &SdfPath,
    name: &str,
    time: f64,
) -> Result<Vec<i32>> {
    match reader.attr_value_at(path, name, time) {
        Some(Value::IntVec(values)) => Ok(values),
        Some(_) => bail!("{} required {} must be int[]", path.as_str(), name),
        None => bail!("{} is missing required {}", path.as_str(), name),
    }
}

fn optional_vec3<R: crate::UsdRead>(
    reader: &R,
    path: &SdfPath,
    name: &str,
    time: f64,
) -> Result<Option<Vec<Vec3>>> {
    match reader.attr_value_at(path, name, time) {
        Some(Value::Vec3fVec(values)) if values.is_empty() => Ok(None),
        Some(Value::Vec3fVec(values)) => Ok(Some(
            values
                .into_iter()
                .map(|value| Vec3::new(value.x, value.y, value.z))
                .collect(),
        )),
        Some(_) => bail!("{} optional {} must be float3[]", path.as_str(), name),
        None if reader.has_authored_attribute(path, name) => {
            bail!(
                "{} authored {} has an unsupported value type",
                path.as_str(),
                name
            )
        }
        None => Ok(None),
    }
}

fn optional_quats<R: crate::UsdRead>(
    reader: &R,
    path: &SdfPath,
    name: &str,
    time: f64,
) -> Result<Option<Vec<Quat>>> {
    match reader.attr_value_at(path, name, time) {
        Some(Value::QuatfVec(values)) => Ok(Some(
            values
                .into_iter()
                .map(|value| Quat::from_xyzw(value.x, value.y, value.z, value.w))
                .collect(),
        )),
        Some(Value::QuathVec(values)) => Ok(Some(
            values
                .into_iter()
                .map(|value| {
                    Quat::from_xyzw(
                        value.x.to_f32(),
                        value.y.to_f32(),
                        value.z.to_f32(),
                        value.w.to_f32(),
                    )
                })
                .collect(),
        )),
        Some(_) => bail!(
            "{} optional {} must be quatf[] or quath[]",
            path.as_str(),
            name
        ),
        None if reader.has_authored_attribute(path, name) => {
            bail!(
                "{} authored {} has an unsupported value type",
                path.as_str(),
                name
            )
        }
        None => Ok(None),
    }
}

fn optional_int64s<R: crate::UsdRead>(
    reader: &R,
    path: &SdfPath,
    name: &str,
    time: f64,
) -> Result<Option<Vec<i64>>> {
    match reader.attr_value_at(path, name, time) {
        Some(Value::Int64Vec(values)) => Ok((!values.is_empty()).then_some(values)),
        Some(_) => bail!("{} optional {} must be int64[]", path.as_str(), name),
        None if reader.has_authored_attribute(path, name) => {
            bail!(
                "{} authored {} has an unsupported value type",
                path.as_str(),
                name
            )
        }
        None => Ok(None),
    }
}

fn validate_optional_len(
    path: &SdfPath,
    name: &str,
    length: Option<usize>,
    expected: usize,
) -> Result<()> {
    if let Some(length) = length {
        if length != expected {
            bail!(
                "{} {} length {} must match instance count {}",
                path.as_str(),
                name,
                length,
                expected
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_SCENE: &str = r#"#usda 1.0
(
    defaultPrim = "World"
    metersPerUnit = 1
    upAxis = "Y"
)
def Xform "World"
{
    def Cube "Blade"
    {
        double size = 2
    }
    def PointInstancer "Blades"
    {
        rel prototypes = [</World/Blade>]
        int[] protoIndices = [0, 0]
        point3f[] positions = [(1, 2, 3), (4, 5, 6)]
        quath[] orientations = [(1, 0, 0, 0), (0, 0, 0, 1)]
        float3[] scales = [(1, 1, 1), (2, 2, 2)]
        int64[] ids = [100, 200]
        int64[] invisibleIds = [200]
    }
}
"#;

    fn plan(source: &str) -> UsdStageProjectionPlan {
        let recipe = lunco_usd_core::StageRecipe::from_source("point-instancer.usda", source);
        UsdStageProjectionPlan::from_recipe(&recipe).expect("projection plan builds")
    }

    #[test]
    fn reads_standard_arrays_and_openusd_masking() {
        let plan = plan(VALID_SCENE);
        let path = SdfPath::new("/World/Blades").unwrap();

        let instances = read_point_instancer(&plan, &path, 0.0).expect("point instancer reads");
        assert_eq!(instances.len(), 2);
        assert_eq!(instances[0].prototype_path, "/World/Blade");
        assert_eq!(instances[0].id, 100);
        assert!(instances[0].visible);
        assert_eq!(instances[1].id, 200);
        assert!(!instances[1].visible);
        assert_eq!(
            instances[0].transform.translation,
            bevy::prelude::Vec3::new(1., 2., 3.)
        );
        assert_eq!(instances[1].transform.scale, bevy::prelude::Vec3::splat(2.));
    }

    #[test]
    fn rejects_required_array_length_mismatch() {
        let source = VALID_SCENE.replace("(4, 5, 6)", "(4, 5, 6), (7, 8, 9)");
        let plan = plan(&source);
        let path = SdfPath::new("/World/Blades").unwrap();

        let error = read_point_instancer(&plan, &path, 0.0).expect_err("mismatch must fail");
        assert!(error.to_string().contains("positions"));
        assert!(error.to_string().contains("protoIndices"));
    }

    #[test]
    fn rejects_out_of_range_prototype_index() {
        let source =
            VALID_SCENE.replace("int[] protoIndices = [0, 0]", "int[] protoIndices = [0, 1]");
        let plan = plan(&source);
        let path = SdfPath::new("/World/Blades").unwrap();

        let error = read_point_instancer(&plan, &path, 0.0).expect_err("bad index must fail");
        assert!(error.to_string().contains("protoIndices[1]"));
    }

    #[test]
    fn preserves_the_transform_contract_type() {
        fn assert_transform(_: Transform) {}

        let plan = plan(VALID_SCENE);
        let path = SdfPath::new("/World/Blades").unwrap();
        let instance = &read_point_instancer(&plan, &path, 0.0).unwrap()[0];
        assert_transform(instance.transform);
    }
}
