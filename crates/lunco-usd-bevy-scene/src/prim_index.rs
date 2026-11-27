//! Live ECS candidates for a composed USD prim path.

use crate::UsdPrimPath;
use bevy::prelude::*;
use lunco_usd_bevy_stage::UsdStageAsset;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// Exact live USD mount selected by a source entity, independent of whether
/// the mount has acquired an editable document association.
#[derive(Clone, Copy, Debug)]
pub struct MountedUsdScope {
    root: Entity,
    stage: AssetId<UsdStageAsset>,
}

impl MountedUsdScope {
    pub fn from_source(world: &World, source: Entity) -> Result<Self, String> {
        let root = crate::scene_root_in_world(world, source)
            .map_err(|error| format!("cannot resolve source scene: {error:?}"))?
            .ok_or("source entity has no scene root")?;
        if crate::is_preview_only_entity(world, source)
            || !world
                .get_resource::<lunco_core::SceneMountState>()
                .is_some_and(|mounts| mounts.contains_root(root))
        {
            return Err("source entity does not belong to a mounted live scene".into());
        }
        let stage = world
            .get::<UsdPrimPath>(root)
            .ok_or("source scene has no USD stage")?
            .stage_handle
            .id();
        Ok(Self { root, stage })
    }

    pub fn root(self) -> Entity {
        self.root
    }
    pub fn stage(self) -> AssetId<UsdStageAsset> {
        self.stage
    }
}

/// Lifecycle-maintained lookup for every ECS projection of a USD path.
/// Live simulation reads exclude preview ancestry; visual and structural edits
/// reconcile every explicit preview lease through the same indexed candidates.
#[derive(Resource, Default)]
pub struct UsdPrimIndex {
    by_stage: HashMap<AssetId<UsdStageAsset>, BTreeMap<Arc<str>, Vec<Entity>>>,
    by_entity: HashMap<Entity, (AssetId<UsdStageAsset>, Arc<str>)>,
}

impl UsdPrimIndex {
    /// Every projection of one composed path on this stage, in stable entity order.
    pub fn in_stage(&self, stage: AssetId<UsdStageAsset>, path: &str) -> &[Entity] {
        self.by_stage
            .get(&stage)
            .and_then(|paths| paths.get(path))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// Resolve a unique live projection, excluding previews and optionally
    /// restricting the candidate to the exact source mount.
    pub fn unique_live_prim(
        &self,
        world: &World,
        stage: AssetId<UsdStageAsset>,
        root: Option<Entity>,
        path: &str,
    ) -> Result<Option<Entity>, String> {
        let mut found = None;
        for &entity in self.in_stage(stage, path) {
            if crate::is_preview_only_entity(world, entity) {
                continue;
            }
            if let Some(root) = root {
                let owner = crate::scene_root_in_world(world, entity)
                    .map_err(|error| format!("cannot resolve scene for {entity:?}: {error:?}"))?;
                if owner != Some(root) {
                    continue;
                }
            }
            if found.replace(entity).is_some() {
                return Err(format!("multiple live entities are bound to prim {path}"));
            }
        }
        Ok(found)
    }

    /// Indexed candidates beneath one exact composed namespace, including its root.
    /// The separator excludes siblings with a matching name prefix.
    pub fn in_subtree(&self, stage: AssetId<UsdStageAsset>, path: &str) -> Vec<Entity> {
        let Some(paths) = self.by_stage.get(&stage) else {
            return Vec::new();
        };
        let mut entities = self.in_stage(stage, path).to_vec();
        let prefix = format!("{}/", path.trim_end_matches('/'));
        for (candidate, members) in paths.range(Arc::<str>::from(prefix.as_str())..) {
            if !candidate.starts_with(&prefix) {
                break;
            }
            entities.extend(members.iter().copied());
        }
        entities
    }

    /// Candidates across all admitted stages. The caller supplies any ordering or mount policy.
    pub fn at_path<'a>(&'a self, path: &'a str) -> impl Iterator<Item = Entity> + 'a {
        self.by_stage
            .values()
            .filter_map(move |paths| paths.get(path))
            .flatten()
            .copied()
    }

    fn insert(&mut self, entity: Entity, prim: &UsdPrimPath) {
        let stage = prim.stage_handle.id();
        let path: Arc<str> = Arc::from(prim.path.as_str());
        let key = (stage, Arc::clone(&path));
        if self.by_entity.get(&entity) == Some(&key) {
            return;
        }
        self.remove(entity);
        let candidates = self
            .by_stage
            .entry(stage)
            .or_default()
            .entry(path)
            .or_default();
        match candidates.binary_search_by_key(&entity.to_bits(), |candidate| candidate.to_bits()) {
            Ok(_) => {}
            Err(index) => candidates.insert(index, entity),
        }
        self.by_entity.insert(entity, key);
    }

    fn remove(&mut self, entity: Entity) {
        let Some((stage, path)) = self.by_entity.remove(&entity) else {
            return;
        };
        let Some(paths) = self.by_stage.get_mut(&stage) else {
            return;
        };
        if let Some(candidates) = paths.get_mut(&path) {
            candidates.retain(|candidate| *candidate != entity);
            if candidates.is_empty() {
                paths.remove(&path);
            }
        }
        if paths.is_empty() {
            self.by_stage.remove(&stage);
        }
    }
}

fn index_live_usd_prim(
    trigger: On<Insert<UsdPrimPath>>,
    prims: Query<&UsdPrimPath>,
    mut index: ResMut<UsdPrimIndex>,
) {
    if let Ok(prim) = prims.get(trigger.entity) {
        index.insert(trigger.entity, prim);
    }
}

fn unindex_live_usd_prim(trigger: On<Remove<UsdPrimPath>>, mut index: ResMut<UsdPrimIndex>) {
    index.remove(trigger.entity);
}

/// Maintains the shared lookup when a projected identity is inserted, replaced or removed.
pub struct UsdPrimIndexPlugin;

impl Plugin for UsdPrimIndexPlugin {
    fn build(&self, app: &mut App) {
        let world = app.world_mut();
        let mut index = UsdPrimIndex::default();
        for (entity, prim) in world.query::<(Entity, &UsdPrimPath)>().iter(world) {
            index.insert(entity, prim);
        }
        app.insert_resource(index)
            .add_observer(index_live_usd_prim)
            .add_observer(unindex_live_usd_prim);
    }
}
