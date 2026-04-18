//! Registry of active visualization instances + the kind catalog.
//!
//! Two separate maps, for orthogonal concerns:
//!
//! * [`VisualizationRegistry`] — live instances: `VizId → VisualizationConfig`.
//!   Adding a plot = inserting a config here. The [`VizPanel`](crate::panel::VizPanel)
//!   looks up configs on each render.
//!
//! * [`VizKindCatalog`] — known viz-kind implementations:
//!   `VizKindId → Arc<dyn Visualization>`. Populated at app startup via
//!   [`AppVizExt::register_visualization`]. New viz kinds plug in by
//!   appending to this catalog.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bevy::prelude::*;

use crate::viz::{VisualizationConfig, VizId, VizKindId, Visualization};

/// Live instance map: one entry per open visualization.
#[derive(Resource, Default)]
pub struct VisualizationRegistry {
    instances: HashMap<VizId, VisualizationConfig>,
}

impl VisualizationRegistry {
    pub fn insert(&mut self, config: VisualizationConfig) -> VizId {
        let id = config.id;
        self.instances.insert(id, config);
        id
    }

    pub fn remove(&mut self, id: VizId) -> Option<VisualizationConfig> {
        self.instances.remove(&id)
    }

    pub fn get(&self, id: VizId) -> Option<&VisualizationConfig> {
        self.instances.get(&id)
    }

    pub fn get_mut(&mut self, id: VizId) -> Option<&mut VisualizationConfig> {
        self.instances.get_mut(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&VizId, &VisualizationConfig)> {
        self.instances.iter()
    }

    pub fn len(&self) -> usize {
        self.instances.len()
    }
}

/// One-shot "please auto-fit on next render" requests, keyed by
/// [`VizId`]. Toolbar buttons insert; viz kinds drain when they
/// render. Modeled as a side-channel resource (rather than a field on
/// `VisualizationConfig`) because fit is transient UI state, not
/// configuration worth persisting in a workspace file.
#[derive(Resource, Default)]
pub struct VizFitRequests {
    pending: HashSet<VizId>,
}

impl VizFitRequests {
    pub fn request(&mut self, id: VizId) {
        self.pending.insert(id);
    }
    /// Returns true and consumes the entry if a fit was requested
    /// for `id`. Idempotent on subsequent renders within the same
    /// frame.
    pub fn take(&mut self, id: VizId) -> bool {
        self.pending.remove(&id)
    }
}

/// Catalog of every viz kind the app knows how to render.
///
/// `Arc<dyn Visualization>` because a kind is shared-immutable state
/// — register once, render many instances.
#[derive(Resource, Default, Clone)]
pub struct VizKindCatalog {
    kinds: HashMap<VizKindId, Arc<dyn Visualization>>,
}

impl VizKindCatalog {
    pub fn register(&mut self, viz: Arc<dyn Visualization>) {
        self.kinds.insert(viz.kind_id(), viz);
    }

    pub fn get(&self, id: VizKindId) -> Option<Arc<dyn Visualization>> {
        self.kinds.get(&id).cloned()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&VizKindId, &Arc<dyn Visualization>)> {
        self.kinds.iter()
    }
}

/// `App` extension for registering viz kinds, mirroring
/// `lunco_workbench::WorkbenchAppExt::register_panel`.
pub trait AppVizExt {
    fn register_visualization<V: Visualization + Default>(&mut self) -> &mut Self;
}

impl AppVizExt for App {
    fn register_visualization<V: Visualization + Default>(&mut self) -> &mut Self {
        let viz: Arc<dyn Visualization> = Arc::new(V::default());
        if self.world().get_resource::<VizKindCatalog>().is_none() {
            self.insert_resource(VizKindCatalog::default());
        }
        self.world_mut()
            .resource_mut::<VizKindCatalog>()
            .register(viz);
        self
    }
}
