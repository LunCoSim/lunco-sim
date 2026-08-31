//! Send-safe USD projection data prepared at the asset boundary.
//!
//! OpenUSD's composed `Stage` is intentionally `!Send`, but the visual
//! projector does not need to retain OpenUSD handles after composition. This
//! module snapshots the composed read surface while the asset loader is on the
//! async path. The main thread then binds the owned facts to Bevy entities;
//! it does not parse USD, walk the hierarchy, resolve materials, or decode
//! transforms during initial scene materialisation.

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};
use bevy::prelude::Transform;
use openusd::sdf::{Path as SdfPath, Value};
use openusd::usd::Stage;

use crate::{AttrUiHint, MaterialPurpose, StageRecipe, StageView, UsdRead};

/// One composed prim's owned facts needed by the initial visual projection.
#[derive(Clone, Debug)]
pub struct UsdPrimProjectionPlan {
    /// The composed prim path.
    pub path: String,
    /// The local transform after the shared stage convention conversion.
    pub transform: Transform,
    /// Whether the prim is an authored placeable unit.
    pub selectable: bool,
    /// The composed catalog identity, when authored.
    pub catalog_entry_id: Option<String>,
    type_name: Option<String>,
    kind: Option<String>,
    property_names: Vec<String>,
    attributes: HashMap<String, Value>,
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

/// A Send-safe snapshot of the composed USD read surface for one asset.
///
/// It is not a second source of truth: it is an immutable load transaction
/// produced from the composed stage and consumed only until the corresponding
/// canonical live stage has a later authored generation. Runtime edits use the
/// canonical stage directly, preserving the existing live-edit ownership.
#[derive(Clone, Debug, Default)]
pub struct UsdStageProjectionPlan {
    /// The composed default prim name, without a leading slash.
    pub default_prim: Option<String>,
    /// All composed prims in deterministic traversal order.
    pub prims: Vec<UsdPrimProjectionPlan>,
    /// Parent path → direct child indices into [`Self::prims`].
    pub children: HashMap<String, Vec<usize>>,
    collections: HashMap<(String, String), Vec<String>>,
    prim_indices: HashMap<String, usize>,
    stage_metadata: HashMap<String, Value>,
    time_codes_per_second: f64,
}

impl UsdStageProjectionPlan {
    /// Build the initial projection from the same composed OpenUSD stage that
    /// runtime readers use. The stage is local to this worker call and is
    /// dropped before the plan crosses the async asset boundary.
    pub(crate) fn from_recipe(recipe: &StageRecipe) -> Result<Self> {
        let (stage, _) = crate::compose::build_stage_with_resolver(recipe)?;
        Self::from_stage(&stage)
    }

    /// Snapshot an already-composed stage into the owned read surface used by
    /// an external native adapter. The caller retains the live stage separately
    /// for explicit authoring; this plan owns no OpenUSD handles.
    pub(crate) fn from_stage(stage: &Stage) -> Result<Self> {
        let reader = StageView::new(stage);
        let paths = reader.prim_paths();
        let mut plan = Self {
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
            ..Self::default()
        };

        for path in paths {
            let path_string = path.to_string();
            let active = reader.is_active(&path);
            let transform = if active {
                crate::read_transform_from_usd(&reader, &path)
                    .map_err(|error| anyhow!("{path_string}: {error}"))?
            } else {
                Transform::IDENTITY
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
            let index = plan.prims.len();
            plan.prim_indices.insert(path_string.clone(), index);
            plan.prims.push(prim);

            if active {
                if let Some(parent) = path.parent() {
                    plan.children
                        .entry(parent.to_string())
                        .or_default()
                        .push(index);
                }
            }

            if has_component_collection {
                if let Ok(members) = reader.collection_members(&path, "components") {
                    plan.collections.insert(
                        (path_string.clone(), "components".to_string()),
                        members
                            .into_iter()
                            .map(|member| member.to_string())
                            .collect(),
                    );
                }
            }
        }
        Ok(plan)
    }

    /// Return direct children in composed USD order.
    pub(crate) fn child_indices(&self, parent: &str) -> &[usize] {
        self.children.get(parent).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Validate all prepared transforms before they become ECS state.
    pub(crate) fn validate(&self) -> Result<()> {
        for prim in &self.prims {
            let t = prim.transform.translation;
            let s = prim.transform.scale;
            let q = prim.transform.rotation;
            if !t.is_finite() || !s.is_finite() || !q.is_finite() {
                anyhow::bail!(
                    "{}: composed transform contains a non-finite value",
                    prim.path
                );
            }
        }
        Ok(())
    }

    /// Return the prepared prim for a composed path.
    pub(crate) fn prim(&self, path: &SdfPath) -> Option<&UsdPrimProjectionPlan> {
        self.prim_indices
            .get(path.as_str())
            .and_then(|index| self.prims.get(*index))
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
        self.prim(prim)
            .and_then(|prim| prim.attributes.get(name).cloned())
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
            .and_then(|values| values.first().cloned())
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
        self.child_indices(prim.as_str())
            .iter()
            .filter_map(|index| self.prims.get(*index))
            .filter_map(|prim| SdfPath::new(&prim.path).ok())
            .collect()
    }

    fn collection_members(
        &self,
        prim: &SdfPath,
        instance_name: &str,
    ) -> Result<Vec<SdfPath>, String> {
        self.collections
            .get(&(prim.to_string(), instance_name.to_string()))
            .map(|members| {
                members
                    .iter()
                    .filter_map(|member| SdfPath::new(member).ok())
                    .collect()
            })
            .ok_or_else(|| format!("collection {instance_name} is not authored on {prim}"))
    }

    fn prim_paths(&self) -> Vec<SdfPath> {
        self.prims
            .iter()
            .filter_map(|prim| SdfPath::new(&prim.path).ok())
            .collect()
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
        let Some(prim_plan) = self.prim(prim) else {
            return None;
        };
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
        self.prim(prim)
            .map(|prim| Some(prim.transform))
            .ok_or_else(|| crate::TransformReadError {
                prim: prim.to_string(),
            })
    }

    fn is_invisible_or_guide(&self, prim: &SdfPath) -> bool {
        self.prim(prim).is_some_and(|prim| prim.invisible_or_guide)
    }

    fn bound_material(&self, prim: &SdfPath, purpose: MaterialPurpose) -> Option<String> {
        self.prim(prim)
            .and_then(|prim| prim.bound_materials.get(&purpose).cloned())
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
        self.default_prim.clone()
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
        self.time_codes_per_second
    }

    fn time_sample_times(&self, prim: &SdfPath, name: &str) -> Vec<f64> {
        self.prim(prim)
            .and_then(|prim| prim.time_samples.get(name).cloned())
            .unwrap_or_default()
    }

    fn stage_metadata_value(&self, name: &str) -> Option<Value> {
        self.stage_metadata.get(name).cloned()
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[cfg(not(target_arch = "wasm32"))]
    fn sandbox_recipe() -> StageRecipe {
        use std::collections::HashMap;
        use std::path::Path;

        let scene = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/scenes/luncosim/sandbox_scene.usda");
        let assets_root = lunco_assets::shipped_asset_root(&scene).expect("shipped asset root");
        let relative = scene
            .strip_prefix(assets_root)
            .expect("sandbox is below shipped asset root");
        let root_id = lunco_assets::engine_asset_uri(&lunco_assets::asset_path::slashed(relative));
        let mut bytes = HashMap::from([(
            root_id.clone(),
            lunco_assets::read_asset_file_bytes(&scene).expect("read sandbox scene"),
        )]);
        let mut queue = vec![root_id.clone()];
        while let Some(id) = queue.pop() {
            let raw = bytes.get(&id).expect("queued layer bytes").clone();
            for child in lunco_usd_compose::child_layer_ids(&id, &raw).expect("read layer arcs") {
                if bytes.contains_key(&child) {
                    continue;
                }
                let child_bytes =
                    lunco_assets::read_asset_bytes_with_twin_root(&child, Some(assets_root), None)
                        .expect("read composed sandbox dependency");
                bytes.insert(child.clone(), child_bytes);
                queue.push(child);
            }
        }
        StageRecipe { root_id, bytes }
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

        assert_eq!(plan.default_prim.as_deref(), Some("World"));
        assert_eq!(plan.children.get("/World"), Some(&vec![1]));
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

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn prepared_reader_preserves_sandbox_network_and_material_reads() {
        use crate::{StageView, UsdRead};
        use openusd::usd::Stage;

        let recipe = sandbox_recipe();
        let plan = UsdStageProjectionPlan::from_recipe(&recipe).expect("projection plan builds");
        let live = Stage::builder()
            .resolver(lunco_usd_compose::LuncoUsdResolver::new(
                recipe.bytes.clone(),
            ))
            .open(&recipe.root_id)
            .expect("live stage builds");
        let live = StageView::new(&live);

        for path in live.prim_paths() {
            assert_eq!(
                live.api_schemas(&path),
                plan.api_schemas(&path),
                "prepared applied API schemas differ at {path}"
            );
        }

        for root in [
            "/SandboxScene/Skid_Physical_1",
            "/SandboxScene/Skid_Battery_Thermal_1",
            "/SandboxScene/Skid_Battery_Thermal_1/Thermal",
        ] {
            let root = SdfPath::new(root).expect("sandbox root path");
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
