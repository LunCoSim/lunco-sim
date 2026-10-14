//! Bevy asset boundary for resolver-backed composed USD stages.
//!
//! `UsdStageAsset` carries the send-safe layer recipe, source-read receipts,
//! and projection plan. The live OpenUSD stage is intentionally owned by
//! [`crate::canonical`], so this asset can cross Bevy's asynchronous loading
//! boundary without retaining `Rc`-backed OpenUSD handles.

use std::sync::Arc;

use anyhow::Result;
use bevy::asset::{AssetLoader, AssetServer, Handle, LoadContext, io::Reader};
use bevy::prelude::{Asset, TypePath};
#[cfg(not(target_arch = "wasm32"))]
use bevy::tasks::AsyncComputeTaskPool;

use crate::UsdStageProjectionPlan;
use crate::compose::{FetchedStageClosure, fetch_layer_closure};
use lunco_assets_core::asset_path::anchor_of;

/// A Bevy asset representing a loaded, composed USD stage.
///
/// The prepared plan is used for initial projection. When authoring or
/// incremental projection needs a live stage, [`crate::canonical`] opens the
/// same recipe on the main thread.
#[derive(Asset, TypePath, Clone)]
pub struct UsdStageAsset {
    /// The send-safe layer closure shared by initial projection and the live
    /// canonical stage. It is absent for an externally composed stage.
    /// Cloned handles share the immutable closure instead of copying its layer
    /// bytes on the app thread.
    pub recipe: Option<Arc<lunco_usd_compose::recipe::StageRecipe>>,
    /// Keeps source-read receipt assets alive so Bevy can reload this stage
    /// when one of its transitive USD layers changes.
    #[dependency]
    source_dependencies: Vec<Handle<UsdLayerReadReceipt>>,
    /// Structural hierarchy and default-time facts captured from the composed
    /// stage before the asset crosses the async boundary.
    pub projection_plan: Arc<UsdStageProjectionPlan>,
}

impl UsdStageAsset {
    /// Build an asset with the same prepared projection contract as the async
    /// loader.
    pub fn from_recipe(recipe: lunco_usd_compose::recipe::StageRecipe) -> Result<Self> {
        let projection_plan = UsdStageProjectionPlan::from_recipe(&recipe)?;
        Self::from_prepared_recipe(recipe, Vec::new(), projection_plan)
    }

    pub(crate) fn from_prepared_recipe(
        recipe: lunco_usd_compose::recipe::StageRecipe,
        source_dependencies: Vec<Handle<UsdLayerReadReceipt>>,
        projection_plan: UsdStageProjectionPlan,
    ) -> Result<Self> {
        projection_plan.validate()?;
        Ok(Self {
            recipe: Some(Arc::new(recipe)),
            source_dependencies,
            projection_plan: Arc::new(projection_plan),
        })
    }

    /// Build an asset from an already-composed native stage.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_composed_stage(stage: &openusd::usd::Stage) -> Result<Self> {
        let projection_plan = UsdStageProjectionPlan::from_stage(stage)?;
        projection_plan.validate()?;
        Ok(Self {
            recipe: None,
            source_dependencies: Vec::new(),
            projection_plan: Arc::new(projection_plan),
        })
    }
}

/// Labeled receipt that keeps a transitive layer read in Bevy's hot-reload graph.
/// Apps registering [`UsdLoader`] directly must also initialize this asset type.
#[derive(Asset, TypePath)]
pub struct UsdLayerReadReceipt;

/// Resolve a USD asset reference into its typed Bevy load address. Native
/// references require the stage's exact live Twin mount; externally composed
/// stages without an asset-server origin cannot admit native asset loads.
pub fn resolve_stage_asset_path(
    asset_server: &AssetServer,
    stage_id: bevy::asset::AssetId<UsdStageAsset>,
    asset_path: &str,
    roots: Option<&lunco_assets_core::TwinRoots>,
    prepared: Option<&lunco_assets_core::asset_path::PreparedAssetPaths>,
) -> Result<bevy::asset::AssetPath<'static>, lunco_assets_core::TwinRootsError> {
    let origin = asset_server.get_path(stage_id);
    lunco_assets_core::asset_path::load_asset_path(asset_path, origin.as_ref(), roots, prepared)
}

/// Bevy loader that fetches and composes the available transitive USD layer
/// closure before publishing a [`UsdStageAsset`]. Missing transitive layers
/// remain unresolved USD arcs and are carried as runtime diagnostics.
#[derive(TypePath)]
pub struct UsdLoader {
    roots: Option<lunco_assets_core::TwinRoots>,
    document_inputs: DocumentStageInputs,
}

/// Optional native session layer for file-backed loads. Document inputs are
/// discovered through their current source lease in `DocumentStageInputs`.
#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct UsdStageLoadSettings {
    pub session_layer: Option<String>,
}

type InputState = std::sync::Mutex<Option<Arc<lunco_usd_compose::recipe::StageRecipe>>>;

/// A weak reader index for currently owned document inputs at exact source URIs.
/// The loader consults it on every admission, including reloads of cached assets.
#[derive(bevy::prelude::Resource, Clone, Default)]
pub struct DocumentStageInputs(
    Arc<std::sync::Mutex<std::collections::HashMap<String, std::sync::Weak<InputState>>>>,
);

impl DocumentStageInputs {
    pub fn register(
        &self,
        recipe: lunco_usd_compose::recipe::StageRecipe,
    ) -> Result<DocumentStageInputOwner> {
        anyhow::ensure!(
            !recipe.root_id.is_empty() && recipe.bytes.contains_key(&recipe.root_id),
            "document stage input requires its exact root bytes"
        );
        let mut entries = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("document input index lock poisoned"))?;
        anyhow::ensure!(
            entries
                .get(&recipe.root_id)
                .and_then(std::sync::Weak::upgrade)
                .is_none(),
            "document stage input already has an owner for {}",
            recipe.root_id
        );
        let root_id = recipe.root_id.clone();
        let state = Arc::new(std::sync::Mutex::new(Some(Arc::new(recipe))));
        entries.insert(root_id.clone(), Arc::downgrade(&state));
        Ok(DocumentStageInputOwner {
            state,
            inputs: self.clone(),
            root_id,
        })
    }

    fn reader(&self, root: &str) -> Result<Option<DocumentStageInputReader>> {
        let entries = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("document input index lock poisoned"))?;
        Ok(entries
            .get(root)
            .and_then(std::sync::Weak::upgrade)
            .map(DocumentStageInputReader))
    }

    fn validate_reader(
        &self,
        root: &str,
        captured: Option<&DocumentStageInputReader>,
    ) -> Result<()> {
        let current = self.reader(root)?;
        let unchanged = match (current.as_ref(), captured) {
            (None, None) => true,
            (Some(current), Some(captured)) => Arc::ptr_eq(&current.0, &captured.0),
            _ => false,
        };
        anyhow::ensure!(
            unchanged,
            "document stage input authority changed during preparation"
        );
        Ok(())
    }
}

/// The document lease owns publication and retirement of immutable native input.
pub struct DocumentStageInputOwner {
    state: Arc<InputState>,
    inputs: DocumentStageInputs,
    root_id: String,
}

/// A loader may capture input but cannot publish or retain its document lease.
#[derive(Clone)]
struct DocumentStageInputReader(Arc<InputState>);

impl DocumentStageInputOwner {
    pub fn publish(&self, recipe: lunco_usd_compose::recipe::StageRecipe) -> Result<()> {
        let mut current = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("document stage input lock poisoned"))?;
        let previous = current
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("document stage input lease retired"))?;
        anyhow::ensure!(
            previous.root_id == recipe.root_id,
            "document stage input cannot change its root identity"
        );
        if previous.as_ref() != &recipe {
            *current = Some(Arc::new(recipe));
        }
        Ok(())
    }
}

impl Drop for DocumentStageInputOwner {
    fn drop(&mut self) {
        if let Ok(mut current) = self.state.lock() {
            *current = None;
        }
        if let Ok(mut entries) = self.inputs.0.lock() {
            if entries
                .get(&self.root_id)
                .and_then(std::sync::Weak::upgrade)
                .is_some_and(|state| Arc::ptr_eq(&state, &self.state))
            {
                entries.remove(&self.root_id);
            }
        }
    }
}

impl DocumentStageInputReader {
    fn capture(&self) -> Result<Arc<lunco_usd_compose::recipe::StageRecipe>> {
        self.0
            .lock()
            .map_err(|_| anyhow::anyhow!("document stage input lock poisoned"))?
            .clone()
            .ok_or_else(|| anyhow::anyhow!("document stage input lease retired"))
    }

    fn validate_capture(
        &self,
        captured: &Arc<lunco_usd_compose::recipe::StageRecipe>,
    ) -> Result<()> {
        let current = self.capture()?;
        anyhow::ensure!(
            Arc::ptr_eq(&current, captured),
            "document stage input changed during preparation"
        );
        Ok(())
    }
}

impl bevy::prelude::FromWorld for UsdLoader {
    fn from_world(world: &mut bevy::prelude::World) -> Self {
        let document_inputs = world.get_resource_or_init::<DocumentStageInputs>().clone();
        Self {
            document_inputs,
            roots: world
                .get_resource::<lunco_assets_core::TwinRoots>()
                .cloned(),
        }
    }
}

impl AssetLoader for UsdLoader {
    type Asset = UsdStageAsset;
    type Settings = UsdStageLoadSettings;
    type Error = anyhow::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        // Preserve the named Bevy asset source when anchoring relative USD
        // arcs. The asset-path module owns both scheme reconstruction and
        // platform separator normalization.
        let origin = load_context.path().clone().into_owned();
        let roots = self.roots.clone();
        let root_asset_path = anchor_of(load_context.path());
        let input_reader = self.document_inputs.reader(&root_asset_path)?;
        let captured = input_reader
            .as_ref()
            .map(DocumentStageInputReader::capture)
            .transpose()?;
        let input = match &captured {
            Some(input) => {
                anyhow::ensure!(
                    input.root_id == root_asset_path,
                    "document input root does not match its asset source"
                );
                (**input).clone()
            }
            None => {
                let mut bytes = Vec::new();
                reader.read_to_end(&mut bytes).await?;
                let mut input = lunco_usd_compose::recipe::StageRecipe::new(
                    root_asset_path.clone(),
                    std::collections::HashMap::from([(root_asset_path.clone(), bytes)]),
                );
                input.session_layer = settings.session_layer.clone();
                input
            }
        };
        let fetched = fetch_layer_closure(load_context, input, roots.as_ref()).await?;
        let FetchedStageClosure {
            recipe,
            source_dependencies,
        } = fetched;
        #[cfg(not(target_arch = "wasm32"))]
        let (recipe, projection_plan) = {
            AsyncComputeTaskPool::get()
                .spawn(async move {
                    let mut projection_plan = UsdStageProjectionPlan::from_recipe(&recipe)?;
                    projection_plan.prepare_native_asset_paths(Some(origin), roots.as_ref());
                    if let Some(paths) = projection_plan.native_asset_paths_snapshot() {
                        paths.validate_owner(roots.as_ref())?;
                    }
                    Ok::<_, anyhow::Error>((recipe, projection_plan))
                })
                .await?
        };
        #[cfg(target_arch = "wasm32")]
        let (recipe, projection_plan) = {
            let mut projection_plan = UsdStageProjectionPlan::from_recipe(&recipe)?;
            projection_plan.prepare_native_asset_paths(Some(origin), roots.as_ref());
            if let Some(paths) = projection_plan.native_asset_paths_snapshot() {
                paths.validate_owner(roots.as_ref())?;
            }
            (recipe, projection_plan)
        };
        self.document_inputs
            .validate_reader(&root_asset_path, input_reader.as_ref())?;
        if let (Some(owner), Some(captured)) = (&input_reader, &captured) {
            owner.validate_capture(captured)?;
        }
        UsdStageAsset::from_prepared_recipe(recipe, source_dependencies, projection_plan)
    }

    fn extensions(&self) -> &[&str] {
        &["usd", "usda", "usdc"]
    }
}

#[cfg(test)]
mod document_input_tests {
    use super::*;

    #[test]
    fn source_input_survives_late_admission_and_reload() {
        use crate::UsdRead;
        use bevy::asset::{
            AssetApp, AssetPlugin, Assets,
            io::{
                AssetSourceBuilder, AssetSourceId,
                memory::{Dir, MemoryAssetReader},
            },
        };
        use bevy::prelude::{App, MinimalPlugins};
        use std::{
            path::Path,
            time::{Duration, Instant},
        };

        let files = Dir::default();
        let source =
            "#usda 1.0\n(defaultPrim = \"Root\")\ndef Xform \"Root\" { double value = 1 }\n";
        files.insert_asset_text(Path::new("scene.usda"), source);
        let mut app = App::new();
        app.register_asset_source(
            AssetSourceId::Name(lunco_assets_core::LUNCO_SCHEME.into()),
            AssetSourceBuilder::new(move || {
                Box::new(MemoryAssetReader {
                    root: files.clone(),
                })
            }),
        );
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        app.init_asset::<UsdStageAsset>()
            .init_asset_loader::<UsdLoader>();
        app.finish();
        app.cleanup();
        let server = app.world().resource::<AssetServer>().clone();
        let first = server.load::<UsdStageAsset>("lunco://scene.usda");
        let deadline = Instant::now() + Duration::from_secs(10);
        while app
            .world()
            .resource::<Assets<UsdStageAsset>>()
            .get(&first)
            .is_none()
        {
            assert!(
                Instant::now() < deadline,
                "initial source admission timed out"
            );
            app.update();
            std::thread::sleep(Duration::from_millis(1));
        }
        let original = app
            .world()
            .resource::<Assets<UsdStageAsset>>()
            .get(&first)
            .unwrap()
            .projection_plan
            .clone();
        let mut input =
            lunco_usd_compose::recipe::StageRecipe::from_source("lunco://scene.usda", source);
        input.session_layer = Some("lunco://session.usda".to_owned());
        input.bytes.insert(
            "lunco://session.usda".to_owned(),
            b"#usda 1.0\nover \"Root\" { double value = 42 }\n".to_vec(),
        );
        let inputs = app.world().resource::<DocumentStageInputs>().clone();
        let _owner = inputs.register(input).unwrap();
        let late = server.load::<UsdStageAsset>("lunco://scene.usda");
        assert_eq!(
            first.id(),
            late.id(),
            "document publication retains asset identity"
        );
        server.reload("lunco://scene.usda");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            app.update();
            let assets = app.world().resource::<Assets<UsdStageAsset>>();
            if let Some(asset) = assets.get(&first) {
                if !Arc::ptr_eq(&asset.projection_plan, &original) {
                    assert_eq!(
                        asset
                            .projection_plan
                            .real(&openusd::sdf::Path::new("/Root").unwrap(), "value"),
                        Some(42.0),
                        "reload consumes the current native session input"
                    );
                    break;
                }
            }
            assert!(Instant::now() < deadline, "source reload timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn recipe(root: &str) -> lunco_usd_compose::recipe::StageRecipe {
        lunco_usd_compose::recipe::StageRecipe::from_source(root, "#usda 1.0\n")
    }

    #[test]
    fn layered_stage_input_fences_revisions_identity_and_retirement() {
        let inputs = DocumentStageInputs::default();
        inputs.validate_reader("scene.usda", None).unwrap();
        let owner = inputs.register(recipe("scene.usda")).unwrap();
        assert!(inputs.validate_reader("scene.usda", None).is_err());
        assert!(inputs.register(recipe("scene.usda")).is_err());
        let reader = inputs.reader("scene.usda").unwrap().unwrap();
        let first = reader.capture().unwrap();
        reader.validate_capture(&first).unwrap();
        assert!(owner.publish(recipe("other.usda")).is_err());
        reader.validate_capture(&first).unwrap();
        owner.publish(recipe("scene.usda")).unwrap();
        reader.validate_capture(&first).unwrap();
        owner
            .publish(lunco_usd_compose::recipe::StageRecipe::from_source(
                "scene.usda",
                "#usda 1.0\n(defaultPrim = \"World\")\ndef Xform \"World\" {}\n",
            ))
            .unwrap();
        assert!(reader.validate_capture(&first).is_err());
        let second = reader.capture().unwrap();
        reader.validate_capture(&second).unwrap();
        drop(owner);
        inputs.validate_reader("scene.usda", None).unwrap();
        assert!(inputs.validate_reader("scene.usda", Some(&reader)).is_err());
        let replacement = inputs.register(recipe("scene.usda")).unwrap();
        assert!(inputs.validate_reader("scene.usda", Some(&reader)).is_err());
        inputs
            .reader("scene.usda")
            .unwrap()
            .unwrap()
            .capture()
            .unwrap();
        drop(replacement);
        assert!(reader.capture().is_err());
        assert!(reader.validate_capture(&second).is_err());
    }
}
