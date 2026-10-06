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
    /// Durable historical trajectory, with no fabricated source document.
    Archived(lunco_workspace::DocumentRuntimeOwner),
}
#[derive(Clone, Debug)]
pub struct ArtifactAdmission {
    pub root: std::path::PathBuf,
    pub limits: crate::RunResultLimits,
}
impl ExperimentOrigin {
    pub fn runtime(&self) -> lunco_workspace::DocumentRuntimeOwner {
        match self {
            Self::LocalDocument(source) => source.runtime.clone(),
            Self::Replicated(owner) => {
                lunco_workspace::DocumentRuntimeOwner::Replicated(owner.clone())
            }
            Self::Archived(runtime) => runtime.clone(),
        }
    }
    pub fn artifact_admission(
        &self,
        workspace: Option<&lunco_workspace::Workspace>,
        limits: crate::RunResultLimits,
    ) -> Option<ArtifactAdmission> {
        let root = match self.runtime() {
            lunco_workspace::DocumentRuntimeOwner::LocalTwin(twin) => {
                workspace?.twin(twin)?.root.clone()
            }
            lunco_workspace::DocumentRuntimeOwner::Replicated(
                lunco_workspace::ReplicationOwner::Twin { scene },
            ) => scene.root,
            _ => return None,
        };
        Some(ArtifactAdmission { root, limits })
    }
    pub fn belongs_to_runtime(&self, runtime: &lunco_workspace::DocumentRuntimeOwner) -> bool {
        match self {
            Self::LocalDocument(source) => &source.runtime == runtime,
            Self::Archived(owner) => owner == runtime,
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
            Self::Archived(lunco_workspace::DocumentRuntimeOwner::Replicated(owner)) => Some(owner),
            Self::Archived(_) => None,
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
            Self::Archived(runtime) => match runtime {
                lunco_workspace::DocumentRuntimeOwner::Application => {
                    workspace.is_none_or(|workspace| workspace.active_twin.is_none())
                        && !matches!(
                            replication,
                            Some(lunco_workspace::ReplicationOwner::Twin { .. })
                        )
                }
                lunco_workspace::DocumentRuntimeOwner::LocalTwin(twin) => {
                    workspace.is_some_and(|workspace| {
                        workspace.active_twin == Some(*twin) && workspace.twin(*twin).is_some()
                    }) && !matches!(
                        replication,
                        Some(lunco_workspace::ReplicationOwner::Twin { .. })
                    )
                }
                lunco_workspace::DocumentRuntimeOwner::Replicated(owner) => {
                    owner.is_in_active_scope(workspace, replication)
                }
            },
        }
    }
    pub fn local_document(&self) -> Option<&PinnedDocumentRuntimeOwner> {
        match self {
            Self::LocalDocument(source) => Some(source),
            Self::Replicated(_) | Self::Archived(_) => None,
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
    /// Restore durable history without assigning a source document or emitting
    /// a live completion. Existing rows require their exact source identity.
    pub fn restore_artifact(
        &mut self,
        registry: &mut ExperimentRegistry,
        runtime: lunco_workspace::DocumentRuntimeOwner,
        group: TwinId,
        artifact: crate::RunArtifact,
        limits: crate::RunResultLimits,
    ) -> Result<(), String> {
        artifact.validate(limits)?;
        let id = artifact.experiment_id;
        if let Some(current) = registry.get(id) {
            let origin = self
                .get(&id)
                .ok_or("existing experiment has no runtime origin")?;
            if !origin.belongs_to_runtime(&runtime)
                || crate::ExperimentDefinition::from(current) != *artifact.definition
            {
                return Err(format!(
                    "artifact {} conflicts with its runtime origin or execution definition",
                    id.0
                ));
            }
            if let Some(result) = current.result.as_ref() {
                if result.meta.source_content != artifact.result.meta.source_content {
                    return Err(format!(
                        "artifact {} conflicts with its compiled-source identity",
                        id.0
                    ));
                }
                return Ok(());
            }
            if !matches!(origin, ExperimentOrigin::Replicated(_)) || !current.status.is_terminal() {
                return Err(format!(
                    "artifact {} cannot attach to an unverified or executing current source",
                    id.0
                ));
            }
            let mut result = artifact.result;
            std::sync::Arc::make_mut(&mut result).meta.restored_history = true;
            let wall_time_ms = result.meta.wall_time_ms;
            let row = registry
                .get_mut(id)
                .ok_or("historic row disappeared during admission")?;
            row.result = Some(result);
            row.status = crate::RunStatus::Done { wall_time_ms };
            return Ok(());
        }
        if self.get(&id).is_some() {
            return Err("artifact UUID has an orphaned origin".into());
        }
        let mut result = artifact.result;
        std::sync::Arc::make_mut(&mut result).meta.restored_history = true;
        let definition = artifact.definition;
        let experiment = Experiment {
            id,
            twin_id: group,
            model_ref: definition.model_ref.clone(),
            name: artifact.name,
            overrides: definition.overrides.clone(),
            inputs: definition.inputs.clone(),
            bounds: definition.bounds.clone(),
            status: crate::RunStatus::Done {
                wall_time_ms: result.meta.wall_time_ms,
            },
            result: Some(result),
            created_at: artifact.created_at,
            color_hint: artifact.color_hint,
        };
        registry.insert_with_id(experiment)?;
        self.origins.insert(id, ExperimentOrigin::Archived(runtime));
        Ok(())
    }
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
            if !current.has_same_execution_definition(&exp) {
                return Err(format!(
                    "experiment {} creation conflicts with its registered execution definition",
                    exp.id.0
                ));
            }
            // An identical Create never rewinds later edits, status, or results.
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
        let mut conflict = exp.clone();
        conflict.bounds.t_end = 2.0;
        let error = origins
            .import(&mut registry, source(1), conflict.clone())
            .expect_err("Create cannot edit even an unadmitted row");
        assert!(error.contains("creation conflicts"));
        registry.set_partial_result(
            id,
            crate::RunResult {
                times: vec![0.0],
                series: Default::default(),
                meta: crate::RunMeta {
                    sample_count: 1,
                    ..Default::default()
                },
            },
        );
        registry.set_name(id, "Retained label".into());
        registry.get_mut(id).expect("fixture").color_hint = 7;
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
        let retained = registry.get(id).expect("retained");
        assert_eq!(retained.name, "Retained label");
        assert_eq!(retained.color_hint, 7);
        assert_eq!(
            retained.result.as_ref().expect("retained result").times,
            vec![0.0]
        );
        assert!(origins.import(&mut registry, source(1), conflict).is_err());
        for field in 0..3 {
            let mut conflict = exp.clone();
            match field {
                0 => conflict.model_ref = ModelRef("OtherPlant".into()),
                1 => {
                    conflict
                        .overrides
                        .insert(ParamPath("gain".into()), ParamValue::Real(3.0));
                }
                _ => {
                    conflict
                        .inputs
                        .insert(ParamPath("drive".into()), ParamValue::Bool(true));
                }
            }
            assert!(origins.import(&mut registry, source(1), conflict).is_err());
        }
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

    #[test]
    fn archived_admission_preserves_source_and_runtime_conflict_boundaries() {
        let mut registry = ExperimentRegistry::new();
        let mut origins = ExperimentOrigins::default();
        let mut row = experiment();
        let id = row.id;
        row.result = Some(std::sync::Arc::new(crate::RunResult {
            times: vec![0.0],
            series: Default::default(),
            meta: crate::RunMeta {
                sample_count: 1,
                source_content: Some(crate::SourceContentIdentity::Available {
                    cid: lunco_hash::content::cid(b"compiled source"),
                }),
                ..Default::default()
            },
        }));
        row.status = crate::RunStatus::Done { wall_time_ms: 1 };
        let artifact = crate::RunArtifact::from_experiment(&row).expect("durable snapshot");
        let runtime =
            lunco_workspace::DocumentRuntimeOwner::LocalTwin(lunco_workspace::TwinId::new(1));
        origins
            .restore_artifact(
                &mut registry,
                runtime.clone(),
                TwinId("current-runtime-group".into()),
                artifact.clone(),
                Default::default(),
            )
            .expect("history");
        assert_eq!(
            origins.get(&id),
            Some(&ExperimentOrigin::Archived(runtime.clone()))
        );
        assert!(origins.local_document(&id).is_none());
        assert!(
            registry
                .get(id)
                .expect("row")
                .result
                .as_ref()
                .expect("history")
                .meta
                .restored_history
        );
        assert_eq!(
            registry.get(id).expect("row").twin_id,
            TwinId("current-runtime-group".into())
        );
        assert!(
            origins
                .restore_artifact(
                    &mut registry,
                    lunco_workspace::DocumentRuntimeOwner::LocalTwin(lunco_workspace::TwinId::new(
                        2
                    )),
                    TwinId("other".into()),
                    artifact.clone(),
                    Default::default()
                )
                .unwrap_err()
                .contains("runtime origin")
        );
        let mut changed = artifact.clone();
        std::sync::Arc::make_mut(&mut changed.definition)
            .bounds
            .t_end = 2.0;
        assert!(
            origins
                .restore_artifact(
                    &mut registry,
                    runtime.clone(),
                    TwinId("current-runtime-group".into()),
                    changed,
                    Default::default()
                )
                .unwrap_err()
                .contains("execution definition")
        );
        let mut changed = artifact.clone();
        std::sync::Arc::make_mut(&mut changed.result)
            .meta
            .source_content = Some(crate::SourceContentIdentity::Available {
            cid: lunco_hash::content::cid(b"different source"),
        });
        assert!(
            origins
                .restore_artifact(
                    &mut registry,
                    runtime,
                    TwinId("current-runtime-group".into()),
                    changed,
                    Default::default()
                )
                .unwrap_err()
                .contains("compiled-source identity")
        );
        let mut current = ExperimentRegistry::new();
        let mut current_origins = ExperimentOrigins::default();
        let mut row = row;
        row.result = None;
        row.status = crate::RunStatus::Pending;
        current_origins
            .import(&mut current, source(1), row)
            .expect("current source row");
        assert!(
            current_origins
                .restore_artifact(
                    &mut current,
                    lunco_workspace::DocumentRuntimeOwner::Application,
                    TwinId("history".into()),
                    artifact,
                    Default::default()
                )
                .unwrap_err()
                .contains("unverified or executing")
        );
        assert!(current.get(id).expect("source").result.is_none());
    }
}
