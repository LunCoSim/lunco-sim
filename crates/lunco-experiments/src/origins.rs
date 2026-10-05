//! Runtime admission attribution independent of persisted presentation grouping.
use crate::{
    Experiment, ExperimentId, ExperimentRegistry, ModelRef, ParamPath, ParamValue, RunBounds,
    TwinId,
};
use bevy::prelude::*;
use lunco_workspace::PinnedDocumentRuntimeOwner;
use std::collections::{BTreeMap, HashMap};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExperimentOrigin {
    LocalDocument(PinnedDocumentRuntimeOwner),
    Replicated(lunco_workspace::ReplicationOwner),
}
impl ExperimentOrigin {
    pub fn belongs_to_runtime(&self, runtime: &lunco_workspace::DocumentRuntimeOwner) -> bool {
        match self {
            Self::LocalDocument(source) => &source.runtime == runtime,
            Self::Replicated(owner) => {
                matches!(runtime, lunco_workspace::DocumentRuntimeOwner::Replicated(current) if current == owner)
            }
        }
    }
    pub fn replication_owner(&self) -> Option<&lunco_workspace::ReplicationOwner> {
        match self {
            Self::LocalDocument(source) => match &source.runtime {
                lunco_workspace::DocumentRuntimeOwner::Replicated(owner) => Some(owner),
                _ => None,
            },
            Self::Replicated(owner) => Some(owner),
        }
    }
    pub fn is_in_active_scope(
        &self,
        workspace: Option<&lunco_workspace::Workspace>,
        replication: Option<&lunco_workspace::ReplicationOwner>,
    ) -> bool {
        match self {
            Self::LocalDocument(source) => source.is_in_active_scope(workspace, replication),
            Self::Replicated(owner) => owner.is_in_active_scope(workspace, replication),
        }
    }
    pub fn local_document(&self) -> Option<&PinnedDocumentRuntimeOwner> {
        match self {
            Self::LocalDocument(source) => Some(source),
            Self::Replicated(_) => None,
        }
    }
}

/// Sole retained origin registry, bounded by the experiment registry's removals.
/// Admission and replay validate row and origin together before any mutation.
#[derive(Resource, Default, Debug)]
pub struct ExperimentOrigins {
    origins: HashMap<ExperimentId, ExperimentOrigin>,
}
impl ExperimentOrigins {
    pub fn get(&self, id: &ExperimentId) -> Option<&ExperimentOrigin> {
        self.origins.get(id)
    }
    pub fn local_document(&self, id: &ExperimentId) -> Option<&PinnedDocumentRuntimeOwner> {
        self.get(id)?.local_document()
    }
    pub fn iter(&self) -> impl Iterator<Item = (&ExperimentId, &ExperimentOrigin)> {
        self.origins.iter()
    }
    pub fn remove(&mut self, id: ExperimentId) {
        self.origins.remove(&id);
    }
    #[allow(clippy::too_many_arguments)]
    pub fn insert_new(
        &mut self,
        registry: &mut ExperimentRegistry,
        origin: ExperimentOrigin,
        twin: TwinId,
        model: ModelRef,
        overrides: BTreeMap<ParamPath, ParamValue>,
        inputs: BTreeMap<ParamPath, ParamValue>,
        bounds: RunBounds,
    ) -> ExperimentId {
        let id = registry.insert_new(twin, model, overrides, inputs, bounds);
        self.origins.insert(id, origin);
        id
    }
    pub fn require(
        &self,
        registry: &ExperimentRegistry,
        id: ExperimentId,
        origin: &ExperimentOrigin,
    ) -> Result<(), String> {
        if registry.get(id).is_none() {
            return Err(format!("experiment {} is not registered", id.0));
        }
        if self.get(&id) != Some(origin) {
            return Err(format!(
                "experiment {} belongs to a different or missing runtime origin",
                id.0
            ));
        }
        Ok(())
    }
    pub fn import(
        &mut self,
        registry: &mut ExperimentRegistry,
        origin: ExperimentOrigin,
        exp: Experiment,
    ) -> Result<(), String> {
        if let Some(current) = registry.get(exp.id) {
            self.require(registry, exp.id, &origin)?;
            if current.twin_id != exp.twin_id {
                return Err(format!(
                    "experiment {} cannot change its presentation group",
                    exp.id.0
                ));
            }
            // Replayed definitions never reset retained terminal status/results.
            let current = registry.get_mut(exp.id).ok_or_else(|| {
                format!(
                    "experiment {} disappeared during definition admission",
                    exp.id.0
                )
            })?;
            current.model_ref = exp.model_ref;
            current.name = exp.name;
            current.overrides = exp.overrides;
            current.inputs = exp.inputs;
            current.bounds = exp.bounds;
            current.color_hint = exp.color_hint;
            return Ok(());
        }
        if self.get(&exp.id).is_some() {
            return Err(format!(
                "experiment {} has an orphaned runtime origin",
                exp.id.0
            ));
        }
        let id = exp.id;
        registry.insert_with_id(exp)?;
        self.origins.insert(id, origin);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(doc: u64) -> ExperimentOrigin {
        ExperimentOrigin::LocalDocument(PinnedDocumentRuntimeOwner {
            document: lunco_workspace::DocumentId::new(doc),
            runtime: lunco_workspace::DocumentRuntimeOwner::Application,
        })
    }
    fn experiment() -> Experiment {
        let mut registry = ExperimentRegistry::new();
        let id = registry.insert_new(
            TwinId("history".into()),
            ModelRef("Plant".into()),
            Default::default(),
            Default::default(),
            RunBounds::default(),
        );
        registry.get(id).expect("fixture").clone()
    }
    #[test]
    fn origin_admission_rejects_uuid_retagging_and_preserves_terminal_history() {
        let mut registry = ExperimentRegistry::new();
        let mut origins = ExperimentOrigins::default();
        let exp = experiment();
        let id = exp.id;
        origins
            .import(&mut registry, source(1), exp.clone())
            .expect("first admission");
        registry.set_status(id, crate::RunStatus::Cancelled);
        let error = origins
            .import(&mut registry, source(2), exp.clone())
            .expect_err("different document rejected");
        assert!(error.contains("different or missing runtime origin"));
        assert_eq!(origins.get(&id), Some(&source(1)));
        assert!(matches!(
            registry.get(id).expect("retained").status,
            crate::RunStatus::Cancelled
        ));
        origins
            .import(&mut registry, source(1), exp.clone())
            .expect("same owner definition replay");
        assert!(matches!(
            registry.get(id).expect("retained").status,
            crate::RunStatus::Cancelled
        ));
        assert!(origins.require(&registry, id, &source(2)).is_err());
        let mut foreign_bucket = exp;
        foreign_bucket.twin_id = TwinId("other".into());
        assert!(registry.insert_with_id(foreign_bucket).is_err());
        assert_eq!(registry.iter_all().count(), 1);
    }
    #[test]
    fn bounded_registry_removals_retire_origins() {
        let mut app = App::new();
        app.add_plugins(crate::ExperimentsPlugin);
        let mut ids = Vec::new();
        for index in 0..=crate::REGISTRY_CAP_PER_TWIN {
            let id =
                app.world_mut()
                    .resource_scope(|world, mut origins: Mut<ExperimentOrigins>| {
                        let mut registry = world.resource_mut::<ExperimentRegistry>();
                        let id = origins.insert_new(
                            &mut registry,
                            source(index as u64 + 1),
                            TwinId("history".into()),
                            ModelRef("Plant".into()),
                            Default::default(),
                            Default::default(),
                            RunBounds::default(),
                        );
                        registry.set_status(id, crate::RunStatus::Done { wall_time_ms: 1 });
                        id
                    });
            ids.push(id);
        }
        app.update();
        assert_eq!(
            app.world().resource::<ExperimentOrigins>().iter().count(),
            crate::REGISTRY_CAP_PER_TWIN
        );
        let evicted = ids
            .iter()
            .find(|id| {
                app.world()
                    .resource::<ExperimentRegistry>()
                    .get(**id)
                    .is_none()
            })
            .expect("eviction");
        assert!(
            app.world()
                .resource::<ExperimentOrigins>()
                .get(evicted)
                .is_none()
        );
        let deleted = *ids.last().expect("retained row");
        app.world_mut()
            .resource_mut::<ExperimentRegistry>()
            .delete(deleted);
        app.update();
        assert!(
            app.world()
                .resource::<ExperimentOrigins>()
                .get(&deleted)
                .is_none()
        );
    }
}
