//! Live ECS candidates for a composed USD prim path.

use crate::UsdPrimPath;
use bevy::prelude::*;
use lunco_usd_bevy_stage::UsdStageAsset;
use std::collections::HashMap;
use std::sync::Arc;

/// Lifecycle-maintained lookup for every ECS projection of a USD path.
/// Live simulation reads exclude preview ancestry; visual and structural edits
/// reconcile every explicit preview lease through the same indexed candidates.
#[derive(Resource, Default)]
pub struct UsdPrimIndex {
    by_stage: HashMap<AssetId<UsdStageAsset>, HashMap<Arc<str>, Vec<Entity>>>,
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
