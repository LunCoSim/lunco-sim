//! Send-safe USD projection data prepared at the asset boundary.
//!
//! OpenUSD's composed `Stage` is intentionally `!Send`, but the visual
//! projector does not need to retain OpenUSD handles after composition. This
//! module snapshots the composed read surface while the asset loader is on the
//! async path. The main thread then binds the owned facts to Bevy entities;
//! it does not parse USD, walk the hierarchy, resolve materials, or decode
//! transforms during initial scene materialisation.
//! Native composition and snapshot extraction run on Bevy's async-compute pool
//! after source-layer reads complete.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Result, anyhow};
use bevy::prelude::Transform;
use openusd::sdf::{Path as SdfPath, Value};
use openusd::usd::Stage;

use crate::{MaterialPurpose, StageView, UsdRead, UsdReadPrimFacts};
use lunco_usd_compose::recipe::StageRecipe;
use lunco_usd_data::metadata::AttrUiHint;

/// One composed prim's owned facts needed by the initial visual projection.
#[derive(Clone, Debug)]
pub struct UsdPrimProjectionPlan {
    /// The composed prim path.
    pub path: String,
    /// The authored local transform after the shared stage convention
    /// conversion. `None` means the prim omitted `xformOpOrder`; callers must
    /// preserve an existing ECS spawn pose for that USD identity case.
    pub transform: Option<Transform>,
    pub transform_matrix_d: Option<bevy::math::DMat4>,
    transform_might_be_time_varying: bool,
    resets_xform_stack: bool,
    /// Whether the prim is an authored placeable unit.
    pub selectable: bool,
    /// The composed catalog identity, when authored.
    pub catalog_entry_id: Option<String>,
    type_name: Option<String>,
    kind: Option<String>,
    property_names: Vec<String>,
    attributes: HashMap<String, Value>,
    attribute_types: HashMap<String, String>,
    asset_sources: HashMap<String, String>,
    sampled_asset_sources: HashMap<(String, u64), String>,
    authored_attributes: HashSet<String>,
    attr_ui_hints: HashMap<String, AttrUiHint>,
    api_schemas: Vec<String>,
    relationships: HashMap<String, Vec<String>>,
    connections: HashMap<String, Vec<String>>,
    time_samples: HashMap<String, Vec<f64>>,
    time_sample_values: HashMap<String, Vec<(f64, Value)>>,
    binary_asset_uri: Option<String>,
    documentation: Option<String>,
    active: bool,
    has_composition_arc: bool,
    purpose: openusd_schemas::geom::Purpose,
    effective_visibility: openusd_schemas::geom::PurposeVisibility,
    bound_materials: HashMap<MaterialPurpose, String>,
    bound_shader: Option<SdfPath>,
}

impl UsdPrimProjectionPlan {
    /// Composed USD schema type name, when the prim has one.
    pub fn type_name(&self) -> Option<&str> {
        self.type_name.as_deref()
    }

    /// Borrow a composed string, token, or asset-path attribute value.
    pub fn text_attribute(&self, name: &str) -> Option<&str> {
        self.attributes.get(name).and_then(Value::as_str)
    }

    /// Borrow the authored path of a composed USD asset attribute.
    pub fn asset_attribute(&self, name: &str) -> Option<&str> {
        match self.attributes.get(name)? {
            Value::AssetPath(path) => Some(path.as_str()),
            _ => None,
        }
    }

    /// Read a composed boolean attribute.
    pub fn boolean_attribute(&self, name: &str) -> Option<bool> {
        match self.attributes.get(name)? {
            Value::Bool(value) => Some(*value),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct UsdStageProjectionData {
    default_prim: Option<String>,
    prims: Vec<UsdPrimProjectionPlan>,
    children: HashMap<String, Vec<usize>>,
    /// Composed schema type → prim indices, built while the load is prepared.
    type_indices: HashMap<String, Vec<usize>>,
    /// Composed applied API schema → prim indices, built with the type index.
    api_schema_indices: HashMap<String, Vec<usize>>,
    collections: HashMap<(String, String), Result<Vec<String>, String>>,
    prim_indices: HashMap<String, usize>,
    stage_metadata: HashMap<String, Value>,
    time_codes_per_second: f64,
}

/// A Send-safe snapshot of the composed USD read surface for one asset.
///
/// Paths, relationships, bindings and transforms are already composed by native
/// USD. Runtime edits use the canonical live stage as their owner.
#[derive(Clone, Debug, Default)]
pub struct UsdStageProjectionPlan {
    native_paths: Option<Arc<lunco_assets_core::asset_path::PreparedAssetPaths>>,
    data: Arc<UsdStageProjectionData>,
}

impl UsdStageProjectionPlan {
    /// Share the immutable native admission snapshot with its live stage owner.
    pub fn native_asset_paths_snapshot(
        &self,
    ) -> Option<Arc<lunco_assets_core::asset_path::PreparedAssetPaths>> {
        self.native_paths.clone()
    }

    /// Prepare native asset references through the authoritative asset owner.
    /// Called only on the same worker that prepares this composed snapshot.
    pub fn prepare_native_asset_paths(
        &mut self,
        origin: Option<bevy::asset::AssetPath<'static>>,
        roots: Option<&lunco_assets_core::TwinRoots>,
    ) {
        let references =
            crate::native_paths::native_references_for_prims(self, UsdRead::prim_paths(self));
        self.native_paths = Some(Arc::new(
            lunco_assets_core::asset_path::PreparedAssetPaths::prepare_on_worker(
                references, origin, roots,
            ),
        ));
    }

    /// Build the initial projection from the same composed OpenUSD stage that
    /// runtime readers use. The stage is local to this worker call and is
    /// dropped before the plan crosses the async asset boundary.
    pub fn from_recipe(recipe: &StageRecipe) -> Result<Self> {
        let (stage, _) = crate::compose::build_stage_with_resolver(recipe)?;
        Self::from_stage(&stage)
    }

    /// Snapshot an already-composed stage into the owned read surface used by
    /// an external native adapter. The caller retains the live stage separately
    /// for explicit authoring; this plan owns no OpenUSD handles.
    pub fn from_stage(stage: &Stage) -> Result<Self> {
        let reader = StageView::new(stage);
        let mut paths = Vec::new();
        let mut seen = std::collections::HashSet::new();
        stage
            .traverse(openusd::usd::PrimPredicate::DEFAULT, |path| {
                if seen.insert(path.clone()) {
                    paths.push(path.clone());
                }
            })
            .map_err(|error| anyhow!("native stage traversal failed: {error}"))?;
        // Explicit queries retain inactive identity and metadata. Keep the
        // projection region identical to native default traversal, including
        // its payload and abstract-prim boundaries.
        let mut all_paths = Vec::new();
        stage
            .traverse(openusd::usd::PrimPredicate::ALL, |path| {
                if !seen.contains(path) {
                    all_paths.push(path.clone());
                }
            })
            .map_err(|error| anyhow!("native stage traversal failed: {error}"))?;
        for path in all_paths {
            if !reader.is_active(&path).map_err(|error| anyhow!(error))? {
                paths.push(path);
            }
        }
        let mut data = UsdStageProjectionData {
            default_prim: reader.default_prim(),
            time_codes_per_second: reader.time_codes_per_second(),
            stage_metadata: ["upAxis", "metersPerUnit"]
                .into_iter()
                .filter_map(|name| {
                    reader
                        .stage_metadata_value(name)
                        .map(|value| (name.into(), value))
                })
                .collect(),
            ..UsdStageProjectionData::default()
        };

        for path in paths {
            let path_string = path.to_string();
            let active = reader
                .is_active(&path)
                .map_err(|error| anyhow!("{path_string}: {error}"))?;
            let transform = if active {
                crate::local_transform_at(&reader, &path, 0.0)
                    .map_err(|error| anyhow!("{path_string}: {error}"))?
            } else {
                None
            };
            let transform_matrix_d = if active {
                crate::local_transform_matrix_d_at(&reader, &path, 0.0)
                    .map_err(|error| anyhow!("{path_string}: {error}"))?
            } else {
                None
            };
            let attribute_names = reader.attr_names(&path);
            let attributes = attribute_names
                .iter()
                .filter_map(|name| {
                    reader
                        .attr_value(&path, name)
                        .map(|value| (name.clone(), value))
                })
                .collect::<HashMap<_, _>>();
            let attribute_types = attribute_names
                .iter()
                .filter_map(|name| {
                    reader
                        .attr_type_name(&path, name)
                        .map(|type_name| (name.clone(), type_name))
                })
                .collect::<HashMap<_, _>>();
            let authored_attributes = attribute_names
                .iter()
                .filter(|name| reader.has_authored_attribute(&path, name))
                .cloned()
                .collect();
            let attr_ui_hints = attribute_names
                .iter()
                .filter_map(|name| {
                    reader
                        .attr_ui_hint(&path, name)
                        .map(|hint| (name.clone(), hint))
                })
                .collect();
            let api_schemas = reader.api_schemas(&path);
            let connections = attribute_names
                .iter()
                .map(|name| (name.clone(), reader.connections(&path, name)))
                .filter(|(_, values)| !values.is_empty())
                .collect();
            let time_samples = attribute_names
                .iter()
                .map(|name| (name.clone(), reader.time_sample_times(&path, name)))
                .filter(|(_, values)| !values.is_empty())
                .collect();
            let time_sample_values = attribute_names
                .iter()
                .filter_map(|name| {
                    let mut times = reader.time_sample_times(&path, name);
                    if times.is_empty() {
                        return None;
                    }
                    if !times.contains(&0.0) {
                        times.push(0.0);
                    }
                    let values = times
                        .into_iter()
                        .filter_map(|time| {
                            reader
                                .attr_value_at(&path, name, time)
                                .map(|value| (time, value))
                        })
                        .collect();
                    Some((name.clone(), values))
                })
                .collect();
            let relationships = reader
                .relationship_names(&path)
                .into_iter()
                .filter_map(|name| {
                    let targets = reader
                        .rel_targets(&path, &name)
                        .into_iter()
                        .map(|target| target.to_string())
                        .collect::<Vec<_>>();
                    (!targets.is_empty()).then_some((name, targets))
                })
                .collect();
            let mut bound_materials = HashMap::new();
            for purpose in [MaterialPurpose::Render, MaterialPurpose::Physics] {
                if let Some(material) = reader
                    .bound_material(&path, purpose)
                    .map_err(|error| anyhow!(error))?
                {
                    bound_materials.insert(purpose, material);
                }
            }
            let collection_names: std::collections::BTreeSet<String> = api_schemas
                .iter()
                .filter_map(|schema: &String| {
                    schema.strip_prefix("CollectionAPI:").map(str::to_owned)
                })
                .chain(attribute_names.iter().filter_map(|name| {
                    name.strip_prefix("collection:")
                        .and_then(|rest| rest.split_once(':'))
                        .map(|(instance, _)| instance.to_owned())
                }))
                .collect();
            let asset_sources = attribute_names
                .iter()
                .filter(|name| {
                    matches!(
                        attribute_types.get(*name).map(String::as_str),
                        Some("asset" | "asset[]")
                    )
                })
                .filter_map(|name| {
                    reader
                        .asset_source_layer(&path, name, None)
                        .map(|source| (name.clone(), source))
                })
                .collect();
            let sampled_asset_sources = attribute_names
                .iter()
                .filter(|name| {
                    matches!(
                        attribute_types.get(*name).map(String::as_str),
                        Some("asset" | "asset[]")
                    )
                })
                .flat_map(|name| {
                    reader
                        .time_sample_times(&path, name)
                        .into_iter()
                        .filter_map(|time| {
                            reader
                                .asset_source_layer(&path, name, Some(time))
                                .map(|source| ((name.clone(), time.to_bits()), source))
                        })
                        .collect::<Vec<_>>()
                })
                .collect();
            let prim = UsdPrimProjectionPlan {
                path: path_string.clone(),
                transform,
                transform_matrix_d,
                selectable: reader.boolean(&path, "lunco:spawnable").unwrap_or(false),
                catalog_entry_id: reader
                    .text(&path, "lunco:catalogId")
                    .filter(|id| !id.trim().is_empty()),
                type_name: reader.type_name(&path),
                kind: reader.kind(&path),
                property_names: attribute_names.clone(),
                attributes,
                attribute_types,
                asset_sources,
                sampled_asset_sources,
                authored_attributes,
                attr_ui_hints,
                api_schemas,
                relationships,
                connections,
                time_samples,
                time_sample_values,
                binary_asset_uri: reader.binary_asset_uri(&path),
                documentation: reader.documentation(&path),
                active,
                has_composition_arc: reader
                    .has_composition_arc(&path)
                    .map_err(|error| anyhow!(error))?,
                transform_might_be_time_varying: reader
                    .transform_might_be_time_varying(&path)
                    .map_err(|error| anyhow!(error))?,
                resets_xform_stack: reader
                    .resets_xform_stack(&path)
                    .map_err(|error| anyhow!(error))?,
                purpose: reader.purpose(&path).map_err(|error| anyhow!(error))?,
                effective_visibility: reader
                    .effective_visibility(&path)
                    .map_err(|error| anyhow!(error))?,
                bound_materials,
                bound_shader: reader.bound_shader(&path).map_err(|error| anyhow!(error))?,
            };
            let index = data.prims.len();
            data.prim_indices.insert(path_string.clone(), index);
            data.prims.push(prim);

            if active {
                if let Some(parent) = path.parent() {
                    data.children
                        .entry(parent.to_string())
                        .or_default()
                        .push(index);
                }
            }

            for instance in collection_names {
                let members = reader
                    .collection_members(&path, &instance)
                    .map(|members| members.into_iter().map(|p| p.to_string()).collect());
                data.collections
                    .insert((path_string.clone(), instance), members);
            }
        }
        for diagnostic in stage.composition_errors() {
            bevy::log::warn!(target: "usd-bevy", %diagnostic, "USD composition diagnostic");
        }
        for (index, prim) in data.prims.iter().enumerate() {
            if !prim.active {
                continue;
            }
            if let Some(type_name) = &prim.type_name {
                data.type_indices
                    .entry(type_name.clone())
                    .or_default()
                    .push(index);
            }
            for schema in &prim.api_schemas {
                data.api_schema_indices
                    .entry(schema.clone())
                    .or_default()
                    .push(index);
            }
        }
        Ok(Self {
            data: Arc::new(data),
            ..Self::default()
        })
    }

    /// Iterate prims with one composed USD schema type from the source stage.
    pub fn prims_of_type(
        &self,
        type_name: &str,
    ) -> impl Iterator<Item = &UsdPrimProjectionPlan> + '_ {
        self.data
            .type_indices
            .get(type_name)
            .into_iter()
            .flatten()
            .filter_map(|index| self.data.prims.get(*index))
    }

    /// Return direct children in composed USD order.
    pub(crate) fn child_indices(&self, parent: &str) -> &[usize] {
        self.data
            .children
            .get(parent)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Validate all prepared transforms before they become ECS state.
    pub fn validate(&self) -> Result<()> {
        for prim in &self.data.prims {
            if let Some(transform) = prim.transform {
                let t = transform.translation;
                let s = transform.scale;
                let q = transform.rotation;
                if !t.is_finite() || !s.is_finite() || !q.is_finite() {
                    anyhow::bail!(
                        "{}: composed transform contains a non-finite value",
                        prim.path
                    );
                }
            }
        }
        Ok(())
    }

    /// Return the prepared prim for a composed path.
    pub(crate) fn prim(&self, path: &SdfPath) -> Option<&UsdPrimProjectionPlan> {
        self.data
            .prim_indices
            .get(path.as_str())
            .and_then(|index| self.data.prims.get(*index))
    }

    fn transform_snapshot(
        &self,
        path: &SdfPath,
        time: f64,
    ) -> Result<&UsdPrimProjectionPlan, crate::TransformReadError> {
        let plan = self.prim(path).ok_or_else(|| crate::TransformReadError {
            prim: path.to_string(),
        })?;
        if time != 0.0
            && plan
                .time_samples
                .keys()
                .any(|name| name.starts_with("xformOp:"))
        {
            bevy::log::error!(%path, %time, "continuous USD transforms require the native temporal stage");
            return Err(crate::TransformReadError {
                prim: path.to_string(),
            });
        }
        Ok(plan)
    }

    /// Composed default prim name, without a leading slash.
    pub fn default_prim_name(&self) -> Option<&str> {
        self.data.default_prim.as_deref()
    }

    /// Number of composed prims in the immutable source snapshot.
    pub fn prim_count(&self) -> usize {
        self.data.prims.len()
    }

    fn schema_facts_for<'a>(
        &'a self,
        prims: impl Iterator<Item = &'a UsdPrimProjectionPlan>,
        type_names: &[&str],
        api_schemas: &[&str],
        attr_prefix: &str,
    ) -> Vec<UsdReadPrimFacts> {
        prims
            .filter(|prim| prim.active)
            .filter_map(|prim| {
                let matches_type = prim
                    .type_name
                    .as_deref()
                    .is_some_and(|name| type_names.contains(&name));
                let matches_api = prim
                    .api_schemas
                    .iter()
                    .any(|name| api_schemas.contains(&name.as_str()));
                let has_attr_prefix = !attr_prefix.is_empty()
                    && prim
                        .property_names
                        .iter()
                        .any(|name| name.starts_with(attr_prefix));
                if !matches_type && !matches_api && !has_attr_prefix {
                    return None;
                }
                Some(UsdReadPrimFacts {
                    path: SdfPath::new(&prim.path).ok()?,
                    type_name: prim.type_name.clone(),
                    api_schemas: prim.api_schemas.clone(),
                    has_attr_prefix,
                })
            })
            .collect()
    }
}

impl UsdRead for UsdStageProjectionPlan {
    fn native_asset_paths(&self) -> Option<&lunco_assets_core::asset_path::PreparedAssetPaths> {
        self.native_paths.as_deref()
    }

    fn type_name(&self, prim: &SdfPath) -> Option<String> {
        self.prim(prim).and_then(|prim| prim.type_name.clone())
    }

    fn kind(&self, prim: &SdfPath) -> Option<String> {
        self.prim(prim).and_then(|prim| prim.kind.clone())
    }

    fn asset_source_layer(&self, prim: &SdfPath, name: &str, time: Option<f64>) -> Option<String> {
        let prim = self.prim(prim)?;
        match time {
            None => prim.asset_sources.get(name).cloned(),
            Some(time) => prim
                .sampled_asset_sources
                .get(&(name.to_owned(), time.to_bits()))
                .cloned(),
        }
    }

    fn attr_value(&self, prim: &SdfPath, name: &str) -> Option<Value> {
        self.prim(prim)
            .and_then(|prim| prim.attributes.get(name).cloned())
    }

    fn attr_type_name(&self, prim: &SdfPath, name: &str) -> Option<String> {
        self.prim(prim)
            .and_then(|prim| prim.attribute_types.get(name).cloned())
    }

    fn has_authored_attribute(&self, prim: &SdfPath, name: &str) -> bool {
        self.prim(prim)
            .is_some_and(|prim| prim.authored_attributes.contains(name))
    }

    fn documentation(&self, prim: &SdfPath) -> Option<String> {
        self.prim(prim).and_then(|prim| prim.documentation.clone())
    }

    fn has_api_schema(&self, prim: &SdfPath, schema: &str) -> bool {
        self.prim(prim)
            .is_some_and(|prim| prim.api_schemas.iter().any(|name| name == schema))
    }

    fn api_schemas(&self, prim: &SdfPath) -> Vec<String> {
        self.prim(prim)
            .map(|prim| prim.api_schemas.clone())
            .unwrap_or_default()
    }

    fn rel_target(&self, prim: &SdfPath, name: &str) -> Option<String> {
        self.prim(prim)
            .and_then(|prim| prim.relationships.get(name))
            .and_then(|values| values.first())
            .cloned()
    }

    fn rel_targets(&self, prim: &SdfPath, name: &str) -> Vec<SdfPath> {
        self.prim(prim)
            .and_then(|prim| prim.relationships.get(name))
            .into_iter()
            .flatten()
            .filter_map(|target| SdfPath::new(target).ok())
            .collect()
    }

    fn relationship_names(&self, prim: &SdfPath) -> Vec<String> {
        self.prim(prim)
            .map(|prim| prim.relationships.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn connections(&self, prim: &SdfPath, name: &str) -> Vec<String> {
        self.prim(prim)
            .and_then(|prim| prim.connections.get(name).cloned())
            .unwrap_or_default()
    }

    fn children(&self, prim: &SdfPath) -> Vec<SdfPath> {
        if self.prim(prim).is_none() {
            return Vec::new();
        }
        self.child_indices(prim.as_str())
            .iter()
            .filter_map(|index| self.data.prims.get(*index))
            .filter_map(|prim| SdfPath::new(&prim.path).ok())
            .collect()
    }

    fn collection_members(
        &self,
        prim: &SdfPath,
        instance_name: &str,
    ) -> Result<Vec<SdfPath>, String> {
        if self.prim(prim).is_none() {
            return Err(format!("prim {prim} does not exist"));
        }
        self.data
            .collections
            .get(&(prim.to_string(), instance_name.to_string()))
            .map(|members| {
                members.as_ref().map_err(Clone::clone).and_then(|members| {
                    members
                        .iter()
                        .map(|member| SdfPath::new(member).map_err(|error| error.to_string()))
                        .collect()
                })
            })
            .ok_or_else(|| format!("collection {instance_name} is not authored on {prim}"))?
    }

    fn prim_paths(&self) -> Vec<SdfPath> {
        self.data
            .prims
            .iter()
            .filter(|prim| prim.active)
            .filter_map(|prim| SdfPath::new(&prim.path).ok())
            .collect()
    }

    fn prim_paths_matching(&self, type_names: &[&str], api_schemas: &[&str]) -> Vec<SdfPath> {
        let mut indices = Vec::new();
        for type_name in type_names {
            indices.extend(
                self.data
                    .type_indices
                    .get(*type_name)
                    .into_iter()
                    .flatten()
                    .copied(),
            );
        }
        for schema in api_schemas {
            indices.extend(
                self.data
                    .api_schema_indices
                    .get(*schema)
                    .into_iter()
                    .flatten()
                    .copied(),
            );
        }
        indices.sort_unstable();
        indices.dedup();
        indices
            .into_iter()
            .filter_map(|index| self.data.prims.get(index))
            .filter_map(|prim| SdfPath::new(&prim.path).ok())
            .collect()
    }

    fn prim_schema_facts_matching(
        &self,
        type_names: &[&str],
        api_schemas: &[&str],
        attr_prefix: &str,
    ) -> Vec<UsdReadPrimFacts> {
        if type_names.is_empty() && api_schemas.is_empty() && attr_prefix.is_empty() {
            return Vec::new();
        }
        self.schema_facts_for(self.data.prims.iter(), type_names, api_schemas, attr_prefix)
    }

    fn attr_names(&self, prim: &SdfPath) -> Vec<String> {
        self.prim(prim)
            .map(|prim| prim.property_names.clone())
            .unwrap_or_default()
    }

    fn any_attr_with_prefix(&self, prim: &SdfPath, prefix: &str) -> bool {
        self.prim(prim).is_some_and(|prim| {
            prim.property_names
                .iter()
                .any(|name| name.starts_with(prefix))
        })
    }

    fn attr_value_at(&self, prim: &SdfPath, name: &str, time: f64) -> Option<Value> {
        let prim_plan = self.prim(prim)?;
        if let Some(samples) = prim_plan.time_sample_values.get(name) {
            if let Some((_, value)) = samples.iter().find(|(sample_time, _)| *sample_time == time) {
                return Some(value.clone());
            }
            bevy::log::error!(%prim, %name, %time,
                "continuous USD sampling requires the native temporal stage");
            return None;
        }
        prim_plan.attributes.get(name).cloned()
    }

    fn local_transform_matrix_d_at(
        &self,
        prim: &SdfPath,
        time: f64,
    ) -> Result<Option<bevy::math::DMat4>, crate::TransformReadError> {
        Ok(self.transform_snapshot(prim, time)?.transform_matrix_d)
    }

    fn resets_xform_stack(&self, prim: &SdfPath) -> Result<bool, crate::TransformReadError> {
        self.prim(prim)
            .map(|prim| prim.resets_xform_stack)
            .ok_or_else(|| crate::TransformReadError {
                prim: prim.to_string(),
            })
    }

    fn transform_might_be_time_varying(
        &self,
        prim: &SdfPath,
    ) -> Result<bool, crate::TransformReadError> {
        self.prim(prim)
            .map(|prim| prim.transform_might_be_time_varying)
            .ok_or_else(|| crate::TransformReadError {
                prim: prim.to_string(),
            })
    }

    fn local_transform_at(
        &self,
        prim: &SdfPath,
        time: f64,
    ) -> Result<Option<Transform>, crate::TransformReadError> {
        Ok(self.transform_snapshot(prim, time)?.transform)
    }

    fn effective_visibility(
        &self,
        prim: &SdfPath,
    ) -> Result<openusd_schemas::geom::PurposeVisibility, String> {
        self.prim(prim)
            .map(|prim| prim.effective_visibility)
            .ok_or_else(|| format!("{prim}: prim is absent from the prepared stage"))
    }

    fn purpose(&self, prim: &SdfPath) -> Result<openusd_schemas::geom::Purpose, String> {
        self.prim(prim)
            .map(|prim| prim.purpose)
            .ok_or_else(|| format!("{prim}: prim is absent from the prepared stage"))
    }

    fn bound_shader(&self, prim: &SdfPath) -> Result<Option<SdfPath>, String> {
        self.prim(prim)
            .map(|prim| prim.bound_shader.clone())
            .ok_or_else(|| format!("{prim}: prim is absent from the prepared stage"))
    }

    fn bound_material(
        &self,
        prim: &SdfPath,
        purpose: MaterialPurpose,
    ) -> Result<Option<String>, String> {
        self.prim(prim)
            .map(|prim| prim.bound_materials.get(&purpose).cloned())
            .ok_or_else(|| format!("{prim}: prim is absent from the prepared stage"))
    }

    fn binary_asset_uri(&self, prim: &SdfPath) -> Option<String> {
        self.prim(prim)
            .and_then(|prim| prim.binary_asset_uri.clone())
    }

    fn is_active(&self, prim: &SdfPath) -> Result<bool, String> {
        Ok(self.prim(prim).is_some_and(|prim| prim.active))
    }

    fn has_composition_arc(&self, prim: &SdfPath) -> Result<bool, String> {
        Ok(self.prim(prim).is_some_and(|prim| prim.has_composition_arc))
    }

    fn has_prim(&self, prim: &SdfPath) -> bool {
        self.prim(prim).is_some()
    }

    fn default_prim(&self) -> Option<String> {
        self.data.default_prim.clone()
    }

    fn attr_ui_hint(&self, prim: &SdfPath, name: &str) -> Option<AttrUiHint> {
        self.prim(prim)
            .and_then(|prim| prim.attr_ui_hints.get(name).cloned())
    }

    fn has_time_samples(&self, prim: &SdfPath, name: &str) -> bool {
        self.prim(prim)
            .is_some_and(|prim| prim.time_samples.contains_key(name))
    }

    fn time_codes_per_second(&self) -> f64 {
        self.data.time_codes_per_second
    }

    fn time_sample_times(&self, prim: &SdfPath, name: &str) -> Vec<f64> {
        self.prim(prim)
            .and_then(|prim| prim.time_samples.get(name).cloned())
            .unwrap_or_default()
    }

    fn stage_metadata_value(&self, name: &str) -> Option<Value> {
        self.data.stage_metadata.get(name).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_composition_arc_facts_survive_prepared_admission() {
        let recipe = StageRecipe::from_source(
            "scene.usda",
            r#"#usda 1.0
(metersPerUnit = 1)
def Xform "Source" { def Cube "Child" {} }
def Xform "Instance" (prepend references = </Source>) {}
"#,
        );
        let (stage, _) = crate::compose::build_stage_with_resolver(&recipe).unwrap();
        let live = StageView::new(&stage);
        let prepared = UsdStageProjectionPlan::from_stage(&stage).unwrap();
        for path in [
            "/Source",
            "/Source/Child",
            "/Instance",
            "/Instance/Child",
            "/Missing",
        ] {
            let path = SdfPath::new(path).unwrap();
            assert_eq!(
                prepared.has_composition_arc(&path).unwrap(),
                live.has_composition_arc(&path).unwrap(),
                "{path}"
            );
        }
        assert!(
            !prepared
                .has_composition_arc(&SdfPath::new("/Source").unwrap())
                .unwrap()
        );
        assert!(
            prepared
                .has_composition_arc(&SdfPath::new("/Instance/Child").unwrap())
                .unwrap()
        );
    }

    #[test]
    fn native_surface_terminals_survive_prepared_admission() {
        let source = r#"#usda 1.0
def Cube "Shape" (prepend apiSchemas = ["MaterialBindingAPI"]) {
    rel material:binding = </Mat>
}
def Cube "Unbound" {}
def Material "Mat" {
    token outputs:surface.connect = </Mat/Graph.outputs:surface>
    def NodeGraph "Graph" {
        token outputs:surface.connect = </Mat/Preview.outputs:surface>
    }
    def Shader "Preview" {
        uniform token info:id = "UsdPreviewSurface"
        token outputs:surface
    }
}
"#;
        let (stage, _) = crate::compose::build_stage_with_resolver(&StageRecipe::from_source(
            "scene.usda",
            source,
        ))
        .unwrap();
        let live = StageView::new(&stage);
        let plan = UsdStageProjectionPlan::from_stage(&stage).unwrap();
        for reader in [&live as &dyn crate::UsdReadObject, &plan] {
            assert_eq!(
                reader
                    .bound_shader(&SdfPath::new("/Shape").unwrap())
                    .unwrap(),
                Some(SdfPath::new("/Mat/Preview").unwrap())
            );
            assert!(
                reader
                    .bound_shader(&SdfPath::new("/Unbound").unwrap())
                    .unwrap()
                    .is_none()
            );
            assert!(
                reader
                    .bound_shader(&SdfPath::new("/Missing").unwrap())
                    .is_err()
            );
        }
        let cycle = source.replace("</Mat/Preview.outputs:surface>", "</Mat.outputs:surface>");
        let (stage, _) = crate::compose::build_stage_with_resolver(&StageRecipe::from_source(
            "scene.usda",
            &cycle,
        ))
        .unwrap();
        let error = StageView::new(&stage)
            .bound_shader(&SdfPath::new("/Shape").unwrap())
            .unwrap_err();
        assert!(error.contains("no producing shader"), "{error}");
        assert!(UsdStageProjectionPlan::from_stage(&stage).is_err());
    }

    #[test]
    fn native_reset_facts_survive_prepared_admission() {
        let recipe = StageRecipe::from_source(
            "scene.usda",
            r#"#usda 1.0
(metersPerUnit = 1)
def Xform "Parent" {
    double3 xformOp:translate = (10,0,0)
    uniform token[] xformOpOrder = ["xformOp:translate"]
    def Cube "Reset" {
        double3 xformOp:translate:discarded = (100,0,0)
        double3 xformOp:translate:after = (2,0,0)
        uniform token[] xformOpOrder = ["xformOp:translate:discarded","!resetXformStack!","xformOp:translate:after"]
    }
}
"#,
        );
        let (stage, _) = crate::compose::build_stage_with_resolver(&recipe).unwrap();
        let live = StageView::new(&stage);
        let plan = UsdStageProjectionPlan::from_stage(&stage).unwrap();
        for (path, reset) in [("/Parent", false), ("/Parent/Reset", true)] {
            let path = SdfPath::new(path).unwrap();
            assert_eq!(live.resets_xform_stack(&path).unwrap(), reset);
            assert_eq!(plan.resets_xform_stack(&path).unwrap(), reset);
        }
        let path = SdfPath::new("/Parent/Reset").unwrap();
        for reader in [&live as &dyn crate::UsdReadObject, &plan] {
            let matrix = crate::world_transform_matrix_d_at(reader, &path, 0.0).unwrap();
            assert_eq!(
                matrix.transform_point3(bevy::math::DVec3::ZERO),
                bevy::math::DVec3::new(2.0, 0.0, 0.0)
            );
        }
        let missing = SdfPath::new("/Missing").unwrap();
        assert!(live.resets_xform_stack(&missing).is_err());
        assert!(plan.resets_xform_stack(&missing).is_err());
    }

    #[test]
    fn inactive_identity_is_queryable_but_never_projected() {
        let recipe = StageRecipe::from_source(
            "scene.usda",
            r#"#usda 1.0
def Xform "Active" {}
def Xform "Inactive" (active = false) {}
"#,
        );
        let (stage, _) = crate::compose::build_stage_with_resolver(&recipe).unwrap();
        let plan = UsdStageProjectionPlan::from_stage(&stage).unwrap();
        let inactive = SdfPath::new("/Inactive").unwrap();
        assert!(plan.has_prim(&inactive));
        assert!(!plan.is_active(&inactive).unwrap());
        assert_eq!(plan.type_name(&inactive).as_deref(), Some("Xform"));
        assert!(!plan.prim_paths().contains(&inactive));
        assert!(
            plan.prims_of_type("Xform")
                .all(|prim| prim.path != "/Inactive")
        );
        assert!(
            plan.prim_schema_facts_matching(&["Xform"], &[], "")
                .iter()
                .all(|prim| prim.path != inactive)
        );
        assert!(
            plan.child_indices("/")
                .iter()
                .all(|index| plan.data.prims[*index].path != "/Inactive")
        );
    }

    #[test]
    fn native_xform_variation_survives_prepared_admission() {
        let recipe = StageRecipe::from_source(
            "scene.usda",
            r#"#usda 1.0
def Xform "Ordered" {
    float xformOp:rotateZ:spin.timeSamples = {0: 0, 1: 90}
    uniform token[] xformOpOrder = ["xformOp:rotateZ:spin"]
}
def Xform "Unordered" {
    float xformOp:rotateZ.timeSamples = {0: 0, 1: 90}
    uniform token[] xformOpOrder = []
}
"#,
        );
        let (stage, _) = crate::compose::build_stage_with_resolver(&recipe).unwrap();
        let live = StageView::new(&stage);
        let plan = UsdStageProjectionPlan::from_stage(&stage).unwrap();
        for (path, varying) in [("/Ordered", true), ("/Unordered", false)] {
            let path = SdfPath::new(path).unwrap();
            assert_eq!(
                live.transform_might_be_time_varying(&path).unwrap(),
                varying
            );
            assert_eq!(
                plan.transform_might_be_time_varying(&path).unwrap(),
                varying
            );
        }
        let missing = SdfPath::new("/Missing").unwrap();
        assert!(live.transform_might_be_time_varying(&missing).is_err());
        assert!(plan.transform_might_be_time_varying(&missing).is_err());

        let malformed = StageRecipe::from_source(
            "invalid.usda",
            "#usda 1.0\ndef Xform \"Invalid\" { uniform bool xformOpOrder = true }",
        );
        let (stage, _) = crate::compose::build_stage_with_resolver(&malformed).unwrap();
        assert!(
            StageView::new(&stage)
                .transform_might_be_time_varying(&SdfPath::new("/Invalid").unwrap())
                .is_err()
        );
        assert!(UsdStageProjectionPlan::from_stage(&stage).is_err());
    }

    #[test]
    fn native_activity_read_errors_are_not_active_defaults() {
        let recipe = StageRecipe::from_source(
            "scene.usda",
            r#"#usda 1.0
def Scope "Good" {}
def Scope "Inactive" (active = false) { def Scope "Child" {} }
def Scope "Invalid" {}
"#,
        );
        let mut data = lunco_usd_compose::parse_usda(
            std::str::from_utf8(&recipe.bytes["scene.usda"]).unwrap(),
        )
        .unwrap();
        data.spec_mut(&SdfPath::new("/Invalid").unwrap())
            .unwrap()
            .add("active", openusd::sdf::Value::String("invalid".into()));
        let bytes = openusd::sdf::LayerRegistry::find_by_extension("usdc")
            .unwrap()
            .write_bytes(&data)
            .unwrap()
            .bytes
            .as_ref()
            .to_vec();
        let recipe = StageRecipe::new(
            "scene.usdc",
            std::collections::HashMap::from([("scene.usdc".to_owned(), bytes)]),
        );
        let (stage, _) = crate::compose::build_stage_with_resolver(&recipe).unwrap();
        let live = StageView::new(&stage);
        assert_eq!(
            live.is_active(&SdfPath::new("/Good").unwrap()).unwrap(),
            true
        );
        assert_eq!(
            live.is_active(&SdfPath::new("/Inactive/Child").unwrap())
                .unwrap(),
            false
        );
        assert!(live.is_active(&SdfPath::new("/Invalid").unwrap()).is_err());
        assert!(UsdStageProjectionPlan::from_stage(&stage).is_err());
    }

    #[test]
    fn prepared_curve_tokens_use_native_schema_values() {
        let recipe = StageRecipe::from_source(
            "scene.usda",
            r#"#usda 1.0
def BasisCurves "Curve" {}
def BasisCurves "CameraPath" (prepend apiSchemas = ["LunCoCameraPathAPI"]) {}
def NurbsPatch "Surface" {}
def Scope "NonCurve" {}
"#,
        );
        let plan = UsdStageProjectionPlan::from_recipe(&recipe).unwrap();
        let (stage, _) = crate::compose::build_stage_with_resolver(&recipe).unwrap();
        let live = StageView::new(&stage);
        for reader in [&live as &dyn crate::read::UsdReadObject, &plan] {
            for (path, name, expected, allowed) in [
                ("/Curve", "type", "cubic", &["linear", "cubic"][..]),
                ("/Curve", "basis", "bezier", &["bezier", "catmullRom"][..]),
                (
                    "/Curve",
                    "wrap",
                    "nonperiodic",
                    &["nonperiodic", "periodic", "pinned"][..],
                ),
                (
                    "/Surface",
                    "orientation",
                    "rightHanded",
                    &["rightHanded", "leftHanded"][..],
                ),
                (
                    "/CameraPath",
                    "lunco:path:clock",
                    "sim",
                    &["sim", "real"][..],
                ),
            ] {
                assert_eq!(
                    crate::read::read_curve_token(
                        reader,
                        &SdfPath::new(path).unwrap(),
                        name,
                        allowed
                    )
                    .unwrap(),
                    expected
                );
            }
            assert!(
                crate::read::read_curve_token(
                    reader,
                    &SdfPath::new("/NonCurve").unwrap(),
                    "basis",
                    &["bezier", "catmullRom"]
                )
                .is_err()
            );
        }
    }

    #[test]
    fn prepared_visibility_preserves_native_opinions_and_read_errors() {
        use openusd_schemas::geom::PurposeVisibility as V;
        let recipe = StageRecipe::from_source(
            "visibility.usda",
            r#"#usda 1.0
def Xform "Guide" (prepend apiSchemas = ["VisibilityAPI"]) {
    uniform token purpose = "guide"
    uniform token guideVisibility = "visible"
    def Cube "Shown" {}
    def Cube "Suppressed" (prepend apiSchemas = ["VisibilityAPI"]) {
        uniform token guideVisibility = "invisible"
    }
}
def Xform "Hidden" {
    token visibility = "invisible"
    def Cube "Child" (prepend apiSchemas = ["VisibilityAPI"]) {
        uniform token purpose = "guide"
        uniform token guideVisibility = "visible"
    }
}
"#,
        );
        let (stage, _) = crate::compose::build_stage_with_resolver(&recipe).unwrap();
        let live = StageView::new(&stage);
        let prepared = UsdStageProjectionPlan::from_stage(&stage).unwrap();
        for (path, expected) in [
            ("/Guide/Shown", V::Visible),
            ("/Guide/Suppressed", V::Invisible),
            ("/Hidden/Child", V::Invisible),
        ] {
            let path = SdfPath::new(path).unwrap();
            assert_eq!(live.effective_visibility(&path), Ok(expected));
            assert_eq!(
                prepared.effective_visibility(&path),
                live.effective_visibility(&path)
            );
        }
        let missing = SdfPath::new("/Missing").unwrap();
        assert!(live.effective_visibility(&missing).is_err());
        assert!(prepared.effective_visibility(&missing).is_err());

        let invalid = StageRecipe::from_source(
            "invalid.usda",
            r#"#usda 1.0
def Cube "Invalid" (prepend apiSchemas = ["VisibilityAPI"]) {
    uniform token purpose = "guide"
    uniform token guideVisibility = "invalid-visibility"
}
"#,
        );
        let (stage, _) = crate::compose::build_stage_with_resolver(&invalid).unwrap();
        assert!(
            StageView::new(&stage)
                .effective_visibility(&SdfPath::new("/Invalid").unwrap())
                .is_err()
        );
        assert!(UsdStageProjectionPlan::from_stage(&stage).is_err());
    }

    #[test]
    fn prepared_purpose_preserves_native_inheritance_and_missing_prim_errors() {
        let recipe = StageRecipe::from_source(
            "purpose.usda",
            r#"#usda 1.0
def Xform "Root" {
    uniform token purpose = "guide"
    def Cube "Inherited" {}
    def Cube "Override" { uniform token purpose = "default" }
    def Xform "Proxy" {
        uniform token purpose = "proxy"
        def Cube "Shape" {}
    }
}
"#,
        );
        let (stage, _) = crate::compose::build_stage_with_resolver(&recipe).unwrap();
        let live = StageView::new(&stage);
        let prepared = UsdStageProjectionPlan::from_stage(&stage).unwrap();
        for (path, expected) in [
            ("/Root/Inherited", openusd_schemas::geom::Purpose::Guide),
            ("/Root/Override", openusd_schemas::geom::Purpose::Default),
            ("/Root/Proxy/Shape", openusd_schemas::geom::Purpose::Proxy),
        ] {
            let path = SdfPath::new(path).unwrap();
            assert_eq!(live.purpose(&path), Ok(expected));
            assert_eq!(prepared.purpose(&path), live.purpose(&path));
        }
        let missing = SdfPath::new("/Root/Missing").unwrap();
        assert!(live.purpose(&missing).is_err());
        assert!(prepared.purpose(&missing).is_err());
    }

    #[test]
    fn snapshots_composed_hierarchy_and_time_samples() {
        let recipe = StageRecipe::from_source(
            "scene.usda",
            "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\n\
def Xform \"World\"\n\
{\n\
    def Xform \"Child\"\n\
    {\n\
        double3 xformOp:translate.timeSamples = {\n\
            0: (1, 2, 3),\n\
            10: (11, 2, 3),\n\
        }\n\
        uniform token[] xformOpOrder = [\"xformOp:translate\"]\n\
    }\n\
}\n",
        );
        let plan = UsdStageProjectionPlan::from_recipe(&recipe).expect("projection plan builds");
        let world = SdfPath::new("/World").unwrap();
        let child = SdfPath::new("/World/Child").unwrap();

        assert_eq!(plan.default_prim_name(), Some("World"));
        assert_eq!(plan.data.children.get("/World"), Some(&vec![1]));
        assert_eq!(
            plan.time_sample_times(&child, "xformOp:translate"),
            vec![0.0, 10.0]
        );
        assert_eq!(
            plan.scalar_at::<[f64; 3]>(&child, "xformOp:translate", 0.0),
            Some([1.0, 2.0, 3.0])
        );
        assert_eq!(plan.children(&world), vec![child]);
    }

    #[test]
    fn prepared_reader_preserves_unauthored_transform_as_usd_identity() {
        let recipe = StageRecipe::from_source(
            "scene.usda",
            r#"#usda 1.0
(
    defaultPrim = "World"
)
def Xform "World"
{
    def Xform "Authored"
    {
        double3 xformOp:translate = (1, 2, 3)
        uniform token[] xformOpOrder = ["xformOp:translate"]
    }
    def Xform "Unauthored"
    {
    }
}
"#,
        );
        let plan = UsdStageProjectionPlan::from_recipe(&recipe).expect("projection plan builds");
        let authored = SdfPath::new("/World/Authored").unwrap();
        let unauthored = SdfPath::new("/World/Unauthored").unwrap();

        assert!(
            plan.local_transform_at(&authored, 0.0)
                .expect("authored prim exists")
                .is_some()
        );
        assert_eq!(
            plan.local_transform_at(&unauthored, 0.0)
                .expect("unauthored prim exists"),
            None,
            "an omitted xformOpOrder must remain distinguishable from authored identity"
        );
    }

    #[test]
    fn prepared_reader_preserves_usd_attribute_roles() {
        let recipe = StageRecipe::from_source(
            "scene.usda",
            r#"#usda 1.0
(
    defaultPrim = "World"
)
def Xform "World"
{
    color3f[] primvars:displayColor = [(1, 0, 0)]
    point3f point = (1, 2, 3)
}
"#,
        );
        let plan = UsdStageProjectionPlan::from_recipe(&recipe).expect("projection plan builds");
        let world = SdfPath::new("/World").unwrap();

        assert_eq!(
            plan.attr_type_name(&world, "primvars:displayColor")
                .as_deref(),
            Some("color3f[]")
        );
        assert_eq!(
            plan.attr_type_name(&world, "point").as_deref(),
            Some("point3f")
        );
    }

    #[test]
    fn prepared_reader_matches_live_composed_network_and_material_reads() {
        let recipe = StageRecipe::from_source(
            "scene.usda",
            r#"#usda 1.0
(
    defaultPrim = "Test"
)
def Xform "Test"
{
    def Xform "Vehicle" (
        prepend apiSchemas = ["CollectionAPI:components"]
    )
    {
        uniform token collection:components:expansionRule = "explicitOnly"
        prepend rel collection:components:includes = [</Test/Vehicle/Chassis>]
        def Xform "Chassis" (
            prepend apiSchemas = ["MaterialBindingAPI"]
        )
        {
            float inputs:voltage
            float outputs:power = 42
            rel material:binding = </Test/Vehicle/Looks/Chassis>
        }
        def Xform "Thermal" (
            prepend apiSchemas = ["CollectionAPI:components"]
        )
        {
            uniform token collection:components:expansionRule = "explicitOnly"
            prepend rel collection:components:includes = [</Test/Vehicle/Chassis>]
        }
        def Scope "Looks"
        {
            def Material "Chassis" {}
        }
    }
}
"#,
        );
        let plan = UsdStageProjectionPlan::from_recipe(&recipe).expect("projection plan builds");
        let (live_stage, _) =
            crate::compose::build_stage_with_resolver(&recipe).expect("live stage builds");
        let live = StageView::new(&live_stage);

        for path in live.prim_paths() {
            assert_eq!(
                live.api_schemas(&path),
                plan.api_schemas(&path),
                "prepared applied API schemas differ at {path}"
            );
        }

        for root in ["/Test/Vehicle", "/Test/Vehicle/Thermal"] {
            let root = SdfPath::new(root).expect("test root path");
            assert_eq!(
                live.collection_members(&root, "components"),
                plan.collection_members(&root, "components"),
                "prepared collection differs at {root}"
            );
            let live_attrs = live.attr_names(&root);
            assert_eq!(
                live_attrs,
                plan.attr_names(&root),
                "prepared property names differ at {root}"
            );
            for attr in live_attrs {
                assert_eq!(
                    live.connections(&root, &attr),
                    plan.connections(&root, &attr),
                    "prepared connection differs at {root}.{attr}"
                );
            }

            for path in live
                .prim_paths()
                .into_iter()
                .filter(|path| path.as_str().starts_with(root.as_str()))
            {
                for purpose in [MaterialPurpose::Render, MaterialPurpose::Physics] {
                    assert_eq!(
                        live.bound_material(&path, purpose),
                        plan.bound_material(&path, purpose),
                        "prepared material binding differs at {path} for {purpose:?}"
                    );
                }
                assert_eq!(
                    live.bound_shader(&path),
                    plan.bound_shader(&path),
                    "prepared shader binding differs at {path}"
                );
                let live_attrs = live.attr_names(&path);
                assert_eq!(
                    live_attrs,
                    plan.attr_names(&path),
                    "prepared property names differ at {path}"
                );
                for attr in live_attrs {
                    assert_eq!(
                        live.connections(&path, &attr),
                        plan.connections(&path, &attr),
                        "prepared connection differs at {path}.{attr}"
                    );
                }
            }
        }
    }
}
