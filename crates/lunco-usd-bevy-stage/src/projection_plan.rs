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

use std::borrow::Cow;
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
    /// Whether the prim is an authored placeable unit.
    pub selectable: bool,
    /// The composed catalog identity, when authored.
    pub catalog_entry_id: Option<String>,
    type_name: Option<String>,
    kind: Option<String>,
    property_names: Vec<String>,
    attributes: HashMap<String, Value>,
    attribute_types: HashMap<String, String>,
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
    invisible_or_guide: bool,
    bound_materials: HashMap<MaterialPurpose, String>,
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
    default_prim_subtree: Vec<usize>,
    children: HashMap<String, Vec<usize>>,
    /// Composed schema type → prim indices, built while the load is prepared.
    type_indices: HashMap<String, Vec<usize>>,
    /// Composed applied API schema → prim indices, built with the type index.
    api_schema_indices: HashMap<String, Vec<usize>>,
    collections: HashMap<(String, String), Vec<String>>,
    prim_indices: HashMap<String, usize>,
    stage_metadata: HashMap<String, Value>,
    time_codes_per_second: f64,
}

#[derive(Clone, Debug)]
struct InstanceNamespace {
    source_root: String,
    instance_root: String,
}

/// A Send-safe snapshot of the composed USD read surface for one asset.
///
/// Instance plans share this immutable snapshot and carry only a namespace
/// mapping plus root overrides. Creating a runtime instance is therefore
/// constant-time with respect to the number of prims in its source asset.
/// Runtime edits still use the canonical live stage as their owner.
#[derive(Clone, Debug, Default)]
pub struct UsdStageProjectionPlan {
    data: Arc<UsdStageProjectionData>,
    namespace: Option<InstanceNamespace>,
    instance_default_prim: Option<String>,
    instance_root_pose: Option<Transform>,
    instance_root_scale: Option<bevy::math::Vec3>,
    instance_root_attributes: HashMap<String, (String, Value)>,
}

impl UsdStageProjectionPlan {
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
        let paths = reader.prim_paths();
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
            let active = reader.is_active(&path);
            let transform = if active {
                crate::local_transform_at(&reader, &path, 0.0)
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
                    let samples = reader
                        .stage()
                        .prim(path.clone())
                        .attribute(name)
                        .time_samples()
                        .ok()??;
                    (!samples.is_empty()).then(|| (name.clone(), samples))
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
            let bound_materials = [MaterialPurpose::Render, MaterialPurpose::Physics]
                .into_iter()
                .filter_map(|purpose| {
                    reader
                        .bound_material(&path, purpose)
                        .map(|material| (purpose, material))
                })
                .collect();
            let has_component_collection = attribute_names
                .iter()
                .any(|name| name.starts_with("collection:components:"));
            let prim = UsdPrimProjectionPlan {
                path: path_string.clone(),
                transform,
                selectable: reader.boolean(&path, "lunco:spawnable").unwrap_or(false),
                catalog_entry_id: reader
                    .text(&path, "lunco:catalogId")
                    .filter(|id| !id.trim().is_empty()),
                type_name: reader.type_name(&path),
                kind: reader.kind(&path),
                property_names: attribute_names.clone(),
                attributes,
                attribute_types,
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
                invisible_or_guide: reader.is_invisible_or_guide(&path),
                bound_materials,
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

            if has_component_collection {
                if let Ok(members) = reader.collection_members(&path, "components") {
                    data.collections.insert(
                        (path_string.clone(), "components".to_string()),
                        members
                            .into_iter()
                            .map(|member| member.to_string())
                            .collect(),
                    );
                }
            }
        }
        for (index, prim) in data.prims.iter().enumerate() {
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
        if let Some(default_prim) = data.default_prim.as_deref() {
            let root_path = format!("/{}", default_prim.trim_start_matches('/'));
            data.default_prim_subtree = data
                .prims
                .iter()
                .enumerate()
                .filter_map(|(index, prim)| {
                    (prim.path == root_path
                        || prim
                            .path
                            .strip_prefix(&root_path)
                            .is_some_and(|suffix| suffix.starts_with('/')))
                    .then_some(index)
                })
                .collect();
        }
        Ok(Self {
            data: Arc::new(data),
            ..Self::default()
        })
    }

    /// Iterate prims with one composed USD schema type from the source stage.
    /// Instance callers receive these source paths; use `UsdRead` for paths in
    /// a remapped reference namespace.
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
        let source_parent = self.source_path(parent);
        self.data
            .children
            .get(source_parent.as_ref())
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
        if let Some(namespace) = &self.namespace {
            let path_text = path.as_str();
            let is_instance_path = path_text == namespace.instance_root
                || path_text
                    .strip_prefix(&namespace.instance_root)
                    .is_some_and(|suffix| suffix.starts_with('/'));
            if !is_instance_path {
                return None;
            }
        }
        let source_path = self.source_path(path.as_str());
        self.data
            .prim_indices
            .get(source_path.as_ref())
            .and_then(|index| self.data.prims.get(*index))
    }

    fn source_path<'a>(&self, path: &'a str) -> Cow<'a, str> {
        let Some(namespace) = &self.namespace else {
            return Cow::Borrowed(path);
        };
        if path == namespace.instance_root {
            return Cow::Owned(namespace.source_root.clone());
        }
        path.strip_prefix(&namespace.instance_root)
            .filter(|suffix| suffix.starts_with('/'))
            .map(|suffix| Cow::Owned(format!("{}{suffix}", namespace.source_root)))
            .unwrap_or(Cow::Borrowed(path))
    }

    fn instance_path<'a>(&self, path: &'a str) -> Cow<'a, str> {
        let Some(namespace) = &self.namespace else {
            return Cow::Borrowed(path);
        };
        if path == namespace.source_root {
            return Cow::Owned(namespace.instance_root.clone());
        }
        path.strip_prefix(&namespace.source_root)
            .filter(|suffix| suffix.starts_with('/'))
            .map(|suffix| Cow::Owned(format!("{}{suffix}", namespace.instance_root)))
            .unwrap_or(Cow::Borrowed(path))
    }

    fn source_path_is_in_instance(&self, path: &str) -> bool {
        self.namespace.as_ref().is_none_or(|namespace| {
            path == namespace.source_root
                || path
                    .strip_prefix(&namespace.source_root)
                    .is_some_and(|suffix| suffix.starts_with('/'))
        })
    }

    fn instance_property_path<'a>(&self, path: &'a str) -> Cow<'a, str> {
        if self.namespace.is_none() {
            return Cow::Borrowed(path);
        }
        let Some(separator) = path.find('.') else {
            return self.instance_path(path);
        };
        let mapped_prim = self.instance_path(&path[..separator]);
        if matches!(mapped_prim, Cow::Borrowed(_)) {
            return Cow::Borrowed(path);
        }
        Cow::Owned(format!("{}{}", mapped_prim, &path[separator..]))
    }

    fn is_instance_root(&self, path: &SdfPath) -> bool {
        self.namespace
            .as_ref()
            .is_some_and(|namespace| path.as_str() == namespace.instance_root)
    }

    fn root_source_prim(&self) -> Option<&UsdPrimProjectionPlan> {
        let namespace = self.namespace.as_ref()?;
        self.data
            .prim_indices
            .get(namespace.source_root.as_str())
            .and_then(|index| self.data.prims.get(*index))
    }

    /// Composed default prim name, without a leading slash.
    pub fn default_prim_name(&self) -> Option<&str> {
        self.instance_default_prim
            .as_deref()
            .or(self.data.default_prim.as_deref())
    }

    /// Number of composed prims in the immutable source snapshot.
    pub fn prim_count(&self) -> usize {
        self.data.prims.len()
    }

    /// Create the prepared read surface for one USD reference instance.
    ///
    /// A reference composes the source asset's default prim at the authored
    /// instance path. The source asset has already paid the async composition
    /// cost, so runtime projection must reuse that immutable plan instead of
    /// reading the live scene stage once per prim. The returned plan is still
    /// derived exclusively from the source asset plan; the live canonical stage
    /// remains the owner for subsequent edits.
    pub fn for_instance(&self, instance_root: &str) -> Result<Self> {
        let instance_root = SdfPath::new(instance_root)
            .map_err(|error| anyhow!("invalid USD instance root {instance_root}: {error}"))?;
        if !instance_root.is_abs() {
            anyhow::bail!("USD instance root must be absolute: {instance_root}");
        }
        if self.namespace.is_some() {
            anyhow::bail!("an instance namespace cannot be remapped a second time");
        }
        let source_root = self
            .data
            .default_prim
            .as_deref()
            .map(|default_prim| format!("/{}", default_prim.trim_start_matches('/')))
            .ok_or_else(|| anyhow!("prepared USD asset has no defaultPrim"))?;
        if !self.data.prim_indices.contains_key(&source_root) {
            anyhow::bail!("prepared USD asset default prim `{source_root}` is absent");
        }
        let instance_root = instance_root.to_string();
        Ok(Self {
            data: Arc::clone(&self.data),
            namespace: Some(InstanceNamespace {
                source_root,
                instance_root: instance_root.clone(),
            }),
            instance_default_prim: Some(instance_root.trim_start_matches('/').to_owned()),
            ..Self::default()
        })
    }

    /// Apply the explicit position and rotation authored for a runtime
    /// reference root, preserving the source root's composed scale.
    pub fn set_instance_root_pose(&mut self, pose: Transform) -> Result<()> {
        let root = self
            .root_source_prim()
            .ok_or_else(|| anyhow!("prepared instance root is absent"))?;
        if self.namespace.is_none() {
            anyhow::bail!("root pose overrides require an instance namespace");
        }
        let mut pose = pose;
        if let Some(source_transform) = root.transform {
            pose.scale = source_transform.scale;
        }
        if !pose.translation.is_finite() || !pose.rotation.is_finite() || !pose.scale.is_finite() {
            anyhow::bail!("prepared instance root pose is not finite");
        }
        self.instance_root_pose = Some(pose);
        Ok(())
    }

    /// Apply an authored USD scale override to the remapped instance root.
    pub fn set_instance_root_scale(&mut self, scale: [f64; 3]) -> Result<()> {
        self.root_source_prim()
            .ok_or_else(|| anyhow!("prepared instance root is absent"))?;
        if self.namespace.is_none() {
            anyhow::bail!("root scale overrides require an instance namespace");
        }
        if scale.iter().any(|component| !component.is_finite()) {
            anyhow::bail!("prepared instance root scale is not finite");
        }
        let scale = bevy::math::Vec3::new(scale[0] as f32, scale[1] as f32, scale[2] as f32);
        if !scale.is_finite() {
            anyhow::bail!("prepared instance root scale is outside the render range");
        }
        self.instance_root_scale = Some(scale);
        Ok(())
    }

    /// Add the runtime catalog identity to the remapped instance root's
    /// composed read facts.
    pub fn set_instance_root_string_attribute(
        &mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<()> {
        self.root_source_prim()
            .ok_or_else(|| anyhow!("prepared instance root is absent"))?;
        if self.namespace.is_none() {
            anyhow::bail!("root attributes require an instance namespace");
        }
        let name = name.into();
        if name.is_empty() {
            anyhow::bail!("prepared instance root attribute name is empty");
        }
        let value = value.into();
        self.instance_root_attributes
            .insert(name, ("string".to_owned(), Value::String(value)));
        Ok(())
    }

    fn schema_facts_for<'a>(
        &'a self,
        prims: impl Iterator<Item = &'a UsdPrimProjectionPlan>,
        type_names: &[&str],
        api_schemas: &[&str],
        attr_prefix: &str,
    ) -> Vec<UsdReadPrimFacts> {
        prims
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
                    path: SdfPath::new(&self.instance_path(&prim.path)).ok()?,
                    type_name: prim.type_name.clone(),
                    api_schemas: prim.api_schemas.clone(),
                    has_attr_prefix,
                })
            })
            .collect()
    }
}

impl UsdRead for UsdStageProjectionPlan {
    fn type_name(&self, prim: &SdfPath) -> Option<String> {
        self.prim(prim).and_then(|prim| prim.type_name.clone())
    }

    fn kind(&self, prim: &SdfPath) -> Option<String> {
        self.prim(prim).and_then(|prim| prim.kind.clone())
    }

    fn attr_value(&self, prim: &SdfPath, name: &str) -> Option<Value> {
        if self.is_instance_root(prim)
            && let Some((_, value)) = self.instance_root_attributes.get(name)
        {
            return Some(value.clone());
        }
        self.prim(prim)
            .and_then(|prim| prim.attributes.get(name).cloned())
    }

    fn attr_type_name(&self, prim: &SdfPath, name: &str) -> Option<String> {
        if self.is_instance_root(prim)
            && let Some((type_name, _)) = self.instance_root_attributes.get(name)
        {
            return Some(type_name.clone());
        }
        self.prim(prim)
            .and_then(|prim| prim.attribute_types.get(name).cloned())
    }

    fn has_authored_attribute(&self, prim: &SdfPath, name: &str) -> bool {
        if self.is_instance_root(prim) && self.instance_root_attributes.contains_key(name) {
            return true;
        }
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
            .map(|target| self.instance_path(target).into_owned())
    }

    fn rel_targets(&self, prim: &SdfPath, name: &str) -> Vec<SdfPath> {
        self.prim(prim)
            .and_then(|prim| prim.relationships.get(name))
            .into_iter()
            .flatten()
            .map(|target| self.instance_path(target))
            .filter_map(|target| SdfPath::new(&target).ok())
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
            .into_iter()
            .map(|path| self.instance_property_path(&path).into_owned())
            .collect()
    }

    fn children(&self, prim: &SdfPath) -> Vec<SdfPath> {
        if self.prim(prim).is_none() {
            return Vec::new();
        }
        self.child_indices(prim.as_str())
            .iter()
            .filter_map(|index| self.data.prims.get(*index))
            .map(|prim| self.instance_path(&prim.path))
            .filter_map(|path| SdfPath::new(&path).ok())
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
        let source_prim = self.source_path(prim.as_str());
        self.data
            .collections
            .get(&(source_prim.into_owned(), instance_name.to_string()))
            .map(|members| {
                members
                    .iter()
                    .map(|member| self.instance_path(member))
                    .filter_map(|member| SdfPath::new(&member).ok())
                    .collect()
            })
            .ok_or_else(|| format!("collection {instance_name} is not authored on {prim}"))
    }

    fn prim_paths(&self) -> Vec<SdfPath> {
        let indices = if self.namespace.is_some() {
            self.data.default_prim_subtree.as_slice()
        } else {
            return self
                .data
                .prims
                .iter()
                .filter_map(|prim| SdfPath::new(&prim.path).ok())
                .collect();
        };
        indices
            .iter()
            .filter_map(|index| self.data.prims.get(*index))
            .filter_map(|prim| SdfPath::new(&self.instance_path(&prim.path)).ok())
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
            .filter(|prim| self.source_path_is_in_instance(&prim.path))
            .map(|prim| self.instance_path(&prim.path))
            .filter_map(|path| SdfPath::new(&path).ok())
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
        if self.namespace.is_some() {
            self.schema_facts_for(
                self.data
                    .default_prim_subtree
                    .iter()
                    .filter_map(|index| self.data.prims.get(*index)),
                type_names,
                api_schemas,
                attr_prefix,
            )
        } else {
            self.schema_facts_for(self.data.prims.iter(), type_names, api_schemas, attr_prefix)
        }
    }

    fn attr_names(&self, prim: &SdfPath) -> Vec<String> {
        let mut names = self
            .prim(prim)
            .map(|prim| prim.property_names.clone())
            .unwrap_or_default();
        if self.is_instance_root(prim) {
            for name in self.instance_root_attributes.keys() {
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }
            names.sort();
        }
        names
    }

    fn any_attr_with_prefix(&self, prim: &SdfPath, prefix: &str) -> bool {
        (self.is_instance_root(prim)
            && self
                .instance_root_attributes
                .keys()
                .any(|name| name.starts_with(prefix)))
            || self.prim(prim).is_some_and(|prim| {
                prim.property_names
                    .iter()
                    .any(|name| name.starts_with(prefix))
            })
    }

    fn attr_value_at(&self, prim: &SdfPath, name: &str, time: f64) -> Option<Value> {
        let prim_plan = self.prim(prim)?;
        if self.is_instance_root(prim)
            && let Some((_, value)) = self.instance_root_attributes.get(name)
        {
            return Some(value.clone());
        }
        if let Some(samples) = prim_plan.time_sample_values.get(name) {
            return openusd::usd::evaluate(samples, time, openusd::usd::InterpolationType::Linear);
        }
        prim_plan.attributes.get(name).cloned()
    }

    fn local_transform_at(
        &self,
        prim: &SdfPath,
        _time: f64,
    ) -> Result<Option<Transform>, crate::TransformReadError> {
        let prim_plan = self.prim(prim).ok_or_else(|| crate::TransformReadError {
            prim: prim.to_string(),
        })?;
        if self.is_instance_root(prim) {
            let mut transform = self.instance_root_pose.or(prim_plan.transform);
            if let Some(scale) = self.instance_root_scale {
                transform.get_or_insert(Transform::IDENTITY).scale = scale;
            }
            return Ok(transform);
        }
        Ok(prim_plan.transform)
    }

    fn is_invisible_or_guide(&self, prim: &SdfPath) -> bool {
        self.prim(prim).is_some_and(|prim| prim.invisible_or_guide)
    }

    fn bound_material(&self, prim: &SdfPath, purpose: MaterialPurpose) -> Option<String> {
        self.prim(prim)
            .and_then(|prim| prim.bound_materials.get(&purpose))
            .map(|material| self.instance_path(material).into_owned())
    }

    fn binary_asset_uri(&self, prim: &SdfPath) -> Option<String> {
        self.prim(prim)
            .and_then(|prim| prim.binary_asset_uri.clone())
    }

    fn is_active(&self, prim: &SdfPath) -> bool {
        self.prim(prim).is_some_and(|prim| prim.active)
    }

    fn has_prim(&self, prim: &SdfPath) -> bool {
        self.prim(prim).is_some()
    }

    fn default_prim(&self) -> Option<String> {
        self.instance_default_prim
            .clone()
            .or_else(|| self.data.default_prim.clone())
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
            plan.scalar_at::<[f64; 3]>(&child, "xformOp:translate", 5.0),
            Some([6.0, 2.0, 3.0])
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
    fn instance_plan_remaps_the_composed_namespace_without_rebuilding_usd() {
        let recipe = StageRecipe::from_source(
            "rover.usda",
            "#usda 1.0\n(\n    defaultPrim = \"Rover\"\n)\n\
def Xform \"Rover\" (\n\
    prepend apiSchemas = [\"CollectionAPI:components\"]\n\
)\n\
{\n\
    uniform token collection:components:expansionRule = \"explicitOnly\"\n\
    prepend rel collection:components:includes = [</Rover/Body>]\n\
    def Cube \"Body\" (\n\
        prepend apiSchemas = [\"MaterialBindingAPI\"]\n\
    )\n\
    {\n\
        rel material:binding = </Rover/Looks/Body>\n\
        rel route:target = </Rover/Body>\n\
        float outputs:signal = 1\n\
        float inputs:signal.connect = </Rover/Body.outputs:signal>\n\
    }\n\
    def Scope \"Looks\"\n\
    {\n\
        def Material \"Body\" {}\n\
    }\n\
}\n\
def Scope \"OutsideDefaultPrim\" {}\n",
        );
        let source = UsdStageProjectionPlan::from_recipe(&recipe).expect("projection plan builds");
        let instance = source
            .for_instance("/Traverse/rover_1")
            .expect("instance view shares and remaps the prepared plan");
        let root = SdfPath::new("/Traverse/rover_1").unwrap();
        let body = SdfPath::new("/Traverse/rover_1/Body").unwrap();
        let looks = SdfPath::new("/Traverse/rover_1/Looks").unwrap();

        assert!(Arc::ptr_eq(&source.data, &instance.data));
        assert_eq!(instance.prim_count(), source.prim_count());
        assert_eq!(instance.default_prim_name(), Some("Traverse/rover_1"));
        assert!(instance.has_prim(&root));
        assert!(instance.has_prim(&body));
        assert_eq!(instance.children(&root), vec![body.clone(), looks]);
        assert_eq!(
            instance.bound_material(&body, MaterialPurpose::Render),
            Some("/Traverse/rover_1/Looks/Body".to_owned()),
            "instance material bindings must follow the canonical namespace remap"
        );
        assert_eq!(
            instance.rel_target(&body, "route:target").as_deref(),
            Some("/Traverse/rover_1/Body")
        );
        assert_eq!(
            instance.connections(&body, "inputs:signal"),
            vec!["/Traverse/rover_1/Body.outputs:signal"]
        );
        assert_eq!(
            instance.collection_members(&root, "components").unwrap(),
            vec![body.clone()]
        );

        let mut with_overrides = instance.clone();
        with_overrides
            .set_instance_root_pose(Transform::from_xyz(1.0, 2.0, 3.0))
            .expect("root pose is valid");
        with_overrides
            .set_instance_root_scale([2.0, 3.0, 4.0])
            .expect("root scale is valid");
        with_overrides
            .set_instance_root_string_attribute("lunco:catalogId", "rover.test")
            .expect("catalog identity is valid");
        let root_transform = with_overrides
            .local_transform_at(&root, 0.0)
            .expect("instance root exists")
            .expect("root pose is projected");
        assert_eq!(
            root_transform.translation,
            bevy::math::Vec3::new(1.0, 2.0, 3.0)
        );
        assert_eq!(root_transform.scale, bevy::math::Vec3::new(2.0, 3.0, 4.0));
        assert_eq!(
            with_overrides.text(&root, "lunco:catalogId").as_deref(),
            Some("rover.test")
        );
        assert!(!instance.has_prim(&SdfPath::new("/Rover").unwrap()));
        let outside = SdfPath::new("/OutsideDefaultPrim").unwrap();
        assert!(source.has_prim(&outside));
        assert!(!instance.has_prim(&outside));
        assert!(
            instance
                .prim_paths()
                .iter()
                .all(|path| path.as_str() != "/OutsideDefaultPrim")
        );
        assert!(
            instance
                .prim_paths_matching(&["Scope"], &[])
                .iter()
                .all(|path| path.as_str() != "/OutsideDefaultPrim")
        );
        assert!(
            instance
                .prim_schema_facts_matching(&["Scope"], &[], "")
                .iter()
                .all(|facts| facts.path.as_str() != "/OutsideDefaultPrim")
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
                    crate::resolve_bound_shader(&live, &path),
                    crate::resolve_bound_shader(&plan, &path),
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
