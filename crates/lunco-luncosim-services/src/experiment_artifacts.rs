//! Optional durable history; all source, runtime, root, and budgets are pinned
//! before bounded background work starts. Completion never projects live UI.
use bevy::prelude::*;
#[cfg(not(target_arch = "wasm32"))]
use lunco_core_runtime::AsyncWorkPriority;
use lunco_core_runtime::{AsyncWorkAdmission, AsyncWorkKey, AsyncWorkKind};
use lunco_experiments::artifact::{
    ArtifactOperation, ArtifactOutcome, ArtifactRequest, ArtifactResponse, ArtifactWorkerTransport,
};
use lunco_experiments::{
    ArtifactAdmission, ExperimentOrigin, ExperimentOrigins, ExperimentRegistry, ExperimentSettings,
    REGISTRY_CAP_PER_TWIN, RunArtifact, RunCompleted, RunResultLimits,
};
use lunco_workspace::{DocumentRuntimeOwner, WorkspaceResource};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Event, Clone)]
pub(crate) struct MaterializedArtifacts {
    pub runtime: DocumentRuntimeOwner,
    pub root: PathBuf,
    pub paths: Vec<PathBuf>,
}
struct PendingArtifact {
    runtime: DocumentRuntimeOwner,
    root: PathBuf,
    limits: RunResultLimits,
    expected_id: Option<lunco_experiments::ExperimentId>,
    origin: Option<ExperimentOrigin>,
    cancel: Arc<AtomicBool>,
    key: AsyncWorkKey,
    response: crossbeam_channel::Receiver<ArtifactResponse>,
}
#[derive(Resource)]
struct ArtifactJobs {
    next: u64,
    pending: BTreeMap<u64, PendingArtifact>,
}
impl Default for ArtifactJobs {
    fn default() -> Self {
        Self {
            next: 0,
            pending: BTreeMap::new(),
        }
    }
}
impl Drop for ArtifactJobs {
    fn drop(&mut self) {
        for job in self.pending.values() {
            job.cancel.store(true, Ordering::SeqCst);
        }
    }
}
impl ArtifactJobs {
    fn take_ready(&mut self) -> Vec<(PendingArtifact, Result<ArtifactOutcome, String>)> {
        let completions = self
            .pending
            .iter()
            .filter_map(|(&token, job)| match job.response.try_recv() {
                Ok(response) => Some((
                    token,
                    if response.token == token {
                        response.result
                    } else {
                        Err("artifact response does not match its admitted operation".into())
                    },
                )),
                Err(crossbeam_channel::TryRecvError::Empty) => None,
                Err(crossbeam_channel::TryRecvError::Disconnected) => Some((
                    token,
                    Err("artifact preparation disconnected before a terminal outcome".into()),
                )),
            })
            .collect::<Vec<_>>();
        completions
            .into_iter()
            .filter_map(|(token, result)| self.pending.remove(&token).map(|job| (job, result)))
            .collect()
    }
    fn admit(
        &mut self,
        runtime: DocumentRuntimeOwner,
        admission: ArtifactAdmission,
        expected_id: Option<lunco_experiments::ExperimentId>,
        origin: Option<ExperimentOrigin>,
        operation: ArtifactOperation,
        scheduler: &mut AsyncWorkAdmission,
        transport: Option<&ArtifactWorkerTransport>,
    ) -> Result<(), String> {
        if self.pending.len() >= REGISTRY_CAP_PER_TWIN {
            return Err("optional experiment artifact admission is full".into());
        }
        let token = self
            .next
            .checked_add(1)
            .ok_or("experiment artifact operation identity exhausted")?;
        self.next = token;
        let key = AsyncWorkKey::new(
            AsyncWorkKind::ExperimentArtifact,
            0,
            u128::from(token),
            0,
            token,
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let request = ArtifactRequest { token, operation };
        let (sender, response) = crossbeam_channel::bounded(1);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = transport;
            let cancelled = cancel.clone();
            let root = admission.root.clone();
            scheduler
                .submit(AsyncWorkPriority::Background, key, move || {
                    // The existing shared admission bounds codec work; the owning
                    // job cap bounds detached I/O requests as well.
                    let Some(pool) = bevy::tasks::IoTaskPool::try_get() else {
                        let _ = sender.try_send(ArtifactResponse {
                            token,
                            result: Err("artifact preparation requires the I/O pool".into()),
                        });
                        return;
                    };
                    pool.spawn(async move {
                        let result = process_native(request.operation, &cancelled, &root).await;
                        let _ = sender.try_send(ArtifactResponse { token, result });
                    })
                    .detach();
                })
                .map_err(|error| format!("artifact work was not admitted: {error:?}"))?;
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = scheduler;
            (transport
                .ok_or("artifact Web Worker transport is not installed")?
                .dispatch)(request, runtime.clone(), sender)?;
        }
        self.pending.insert(
            token,
            PendingArtifact {
                runtime,
                root: admission.root,
                limits: admission.limits,
                expected_id,
                origin,
                cancel,
                key,
                response,
            },
        );
        Ok(())
    }
}
#[cfg(not(target_arch = "wasm32"))]
async fn process_native(
    operation: ArtifactOperation,
    cancelled: &AtomicBool,
    root: &std::path::Path,
) -> Result<ArtifactOutcome, String> {
    use lunco_storage::Storage;
    if cancelled.load(Ordering::SeqCst) {
        return Err("artifact owner retired before I/O".into());
    }
    let storage = lunco_storage::FileStorage::new();
    match operation {
        ArtifactOperation::Read { path, limits } => {
            let path = confined_existing(root, &path)?;
            let bytes = storage
                .read_bounded(
                    &lunco_storage::StorageHandle::File(path),
                    limits.max_artifact_bytes,
                )
                .await
                .map_err(|error| error.to_string())?;
            lunco_experiments::decode_run_artifact(&bytes, limits).map(ArtifactOutcome::Read)
        }
        ArtifactOperation::Write {
            path,
            artifact,
            limits,
        } => {
            let bytes = lunco_experiments::encode_run_artifact(&artifact, limits)?;
            let parent = path.parent().ok_or("artifact destination has no parent")?;
            let relative = parent
                .strip_prefix(root)
                .map_err(|_| "artifact destination is outside its admitted root")?;
            // Validate an existing parent before creation, then confine the
            // created parent using the shared asset identity owner.
            lunco_assets_core::existing_path_within_root(root, relative)
                .map_err(|error| error.to_string())?;
            storage
                .ensure_directory(&lunco_storage::StorageHandle::File(parent.to_path_buf()))
                .await
                .map_err(|error| error.to_string())?;
            let parent = confined_existing(root, parent)?;
            let relative = path
                .strip_prefix(root)
                .map_err(|_| "artifact destination is outside its admitted root")?;
            lunco_assets_core::existing_path_within_root(root, relative)
                .map_err(|error| error.to_string())?;
            // Atomic Storage replacement addresses the leaf itself; it never
            // writes through an existing leaf symlink to its target.
            let path = parent.join(
                path.file_name()
                    .ok_or("artifact destination has no filename")?,
            );
            if cancelled.load(Ordering::SeqCst) {
                return Err("artifact owner retired before durable write".into());
            }
            storage
                .write(&lunco_storage::StorageHandle::File(path), &bytes)
                .await
                .map_err(|error| error.to_string())?;
            Ok(ArtifactOutcome::Written)
        }
        ArtifactOperation::List { directory, cap } => {
            let canonical = confined_existing(root, &directory)?;
            let listing = storage
                .read_directory_bounded(&lunco_storage::StorageHandle::File(canonical), cap)
                .await
                .map_err(|error| error.to_string())?;
            Ok(ArtifactOutcome::Listed {
                paths: listing
                    .entries
                    .into_iter()
                    .filter_map(|handle| {
                        handle
                            .as_file_path()
                            .and_then(|path| path.file_name())
                            .map(|name| directory.join(name))
                    })
                    .collect(),
                truncated: listing.truncated,
            })
        }
    }
}
#[cfg(not(target_arch = "wasm32"))]
fn confined_existing(root: &std::path::Path, path: &std::path::Path) -> Result<PathBuf, String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "artifact path is outside its admitted root")?;
    lunco_assets_core::existing_path_within_root(root, relative)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "optional artifact entry does not exist".into())
}
fn runtime_is_current(
    runtime: &DocumentRuntimeOwner,
    root: &std::path::Path,
    workspace: Option<&lunco_workspace::Workspace>,
    replica: Option<&lunco_workspace::ReplicationOwner>,
) -> bool {
    if !runtime.is_current(workspace, replica) {
        return false;
    }
    match runtime {
        DocumentRuntimeOwner::LocalTwin(twin) => workspace
            .and_then(|workspace| workspace.twin(*twin))
            .is_some_and(|twin| twin.root == root),
        DocumentRuntimeOwner::Replicated(lunco_workspace::ReplicationOwner::Twin { scene }) => {
            scene.root == root
        }
        _ => false,
    }
}
fn write_completed(
    mut completed: MessageReader<RunCompleted>,
    registry: Res<ExperimentRegistry>,
    origins: Res<ExperimentOrigins>,
    workspace: Option<Res<WorkspaceResource>>,
    connection: Option<Res<lunco_core_session::ClientConnection>>,
    replica: Option<Res<lunco_core_session::ReplicatedScene>>,
    mut jobs: ResMut<ArtifactJobs>,
    mut scheduler: ResMut<AsyncWorkAdmission>,
    transport: Option<Res<ArtifactWorkerTransport>>,
    role: Option<Res<lunco_core_session::NetworkRole>>,
) {
    let current =
        lunco_core_session::current_replication_owner(connection.as_deref(), replica.as_deref());
    for event in completed.read() {
        if matches!(
            role.as_deref(),
            Some(lunco_core_session::NetworkRole::Client)
        ) {
            if let Some(transport) = transport.as_deref() {
                (transport.discard)(event.experiment_id);
            }
            continue;
        }
        let Some(admission) = event.artifact_admission.clone() else {
            if let Some(transport) = transport.as_deref() {
                (transport.discard)(event.experiment_id);
            }
            continue;
        };
        let runtime = event.origin.runtime();
        if origins.get(&event.experiment_id) != Some(&event.origin)
            || !runtime_is_current(
                &runtime,
                &admission.root,
                workspace.as_deref().map(|workspace| &workspace.0),
                current.as_ref(),
            )
        {
            if let Some(transport) = transport.as_deref() {
                (transport.discard)(event.experiment_id);
            }
            continue;
        }
        let Some(experiment) = registry.get(event.experiment_id) else {
            if let Some(transport) = transport.as_deref() {
                (transport.discard)(event.experiment_id);
            }
            continue;
        };
        let result = (|| {
            let artifact = RunArtifact::from_experiment(experiment)?;
            let path = lunco_twin::results_dir(&admission.root)
                .join(format!("{}.json", event.experiment_id.as_artifact_stem()));
            jobs.admit(
                runtime,
                admission.clone(),
                Some(event.experiment_id),
                Some(event.origin.clone()),
                ArtifactOperation::Write {
                    path,
                    artifact,
                    limits: admission.limits,
                },
                &mut scheduler,
                transport.as_deref(),
            )
        })();
        if let Err(error) = result {
            if let Some(transport) = transport.as_deref() {
                (transport.discard)(event.experiment_id);
            }
            warn!("[experiment] optional artifact write rejected: {error}");
        }
    }
}
fn load_added_twin(
    event: On<lunco_workspace::TwinAdded>,
    workspace: Res<WorkspaceResource>,
    settings: Option<Res<ExperimentSettings>>,
    mut jobs: ResMut<ArtifactJobs>,
    mut scheduler: ResMut<AsyncWorkAdmission>,
    transport: Option<Res<ArtifactWorkerTransport>>,
) {
    let Some(settings) = settings else {
        return;
    };
    let Some(twin) = workspace.twin(event.twin) else {
        return;
    };
    let admission = ArtifactAdmission {
        root: twin.root.clone(),
        limits: settings.result_limits,
    };
    let directory = lunco_twin::results_dir(&admission.root);
    if let Err(error) = jobs.admit(
        DocumentRuntimeOwner::LocalTwin(event.twin),
        admission,
        None,
        None,
        ArtifactOperation::List {
            directory,
            cap: REGISTRY_CAP_PER_TWIN,
        },
        &mut scheduler,
        transport.as_deref(),
    ) {
        warn!("[experiment] optional artifact listing rejected: {error}");
    }
}
fn load_materialized(
    event: On<MaterializedArtifacts>,
    settings: Option<Res<ExperimentSettings>>,
    mut jobs: ResMut<ArtifactJobs>,
    mut scheduler: ResMut<AsyncWorkAdmission>,
    transport: Option<Res<ArtifactWorkerTransport>>,
) {
    let Some(settings) = settings else {
        return;
    };
    admit_paths(
        &mut jobs,
        &mut scheduler,
        transport.as_deref(),
        event.runtime.clone(),
        event.root.clone(),
        settings.result_limits,
        event.paths.clone(),
    );
}
fn admit_paths(
    jobs: &mut ArtifactJobs,
    scheduler: &mut AsyncWorkAdmission,
    transport: Option<&ArtifactWorkerTransport>,
    runtime: DocumentRuntimeOwner,
    root: PathBuf,
    limits: RunResultLimits,
    paths: Vec<PathBuf>,
) {
    for path in paths.into_iter().take(REGISTRY_CAP_PER_TWIN) {
        if path.parent() != Some(lunco_twin::results_dir(&root).as_path())
            || path.extension().and_then(|value| value.to_str()) != Some("json")
        {
            continue;
        }
        let Some(id) = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(lunco_experiments::ExperimentId::from_artifact_stem)
        else {
            continue;
        };
        if let Err(error) = jobs.admit(
            runtime.clone(),
            ArtifactAdmission {
                root: root.clone(),
                limits,
            },
            Some(id),
            None,
            ArtifactOperation::Read { path, limits },
            scheduler,
            transport,
        ) {
            warn!("[experiment] optional artifact read rejected: {error}");
        }
    }
}
fn commit_artifacts(
    mut jobs: ResMut<ArtifactJobs>,
    mut scheduler: ResMut<AsyncWorkAdmission>,
    transport: Option<Res<ArtifactWorkerTransport>>,
    workspace: Option<Res<WorkspaceResource>>,
    connection: Option<Res<lunco_core_session::ClientConnection>>,
    replica: Option<Res<lunco_core_session::ReplicatedScene>>,
    mut registry: ResMut<ExperimentRegistry>,
    mut origins: ResMut<ExperimentOrigins>,
    #[cfg(feature = "networking")] role: Option<Res<lunco_core_session::NetworkRole>>,
    #[cfg(feature = "networking")] rebuild: Option<
        ResMut<lunco_networking_sync::sync::RequestManifestRebuild>,
    >,
) {
    #[cfg(feature = "networking")]
    let mut rebuild = rebuild;
    let current =
        lunco_core_session::current_replication_owner(connection.as_deref(), replica.as_deref());
    for (job, result) in jobs.take_ready() {
        if job.cancel.load(Ordering::SeqCst)
            || !runtime_is_current(
                &job.runtime,
                &job.root,
                workspace.as_deref().map(|workspace| &workspace.0),
                current.as_ref(),
            )
        {
            continue;
        }
        match result {
            Err(error) => warn!("[experiment] optional artifact unavailable: {error}"),
            Ok(ArtifactOutcome::Listed { paths, truncated }) => {
                if truncated {
                    warn!(
                        "[experiment] history listing exceeds retained run cap; admitting first {REGISTRY_CAP_PER_TWIN} lexical entries"
                    );
                }
                admit_paths(
                    &mut jobs,
                    &mut scheduler,
                    transport.as_deref(),
                    job.runtime,
                    job.root,
                    job.limits,
                    paths,
                );
            }
            Ok(ArtifactOutcome::Read(artifact)) => {
                if job.expected_id != Some(artifact.experiment_id) {
                    warn!("[experiment] artifact filename and envelope UUID disagree");
                    continue;
                }
                let group = match &job.runtime {
                    DocumentRuntimeOwner::LocalTwin(twin) => {
                        lunco_experiments::TwinId(format!("workspace:{}", twin.raw()))
                    }
                    DocumentRuntimeOwner::Replicated(lunco_workspace::ReplicationOwner::Twin {
                        scene,
                    }) => {
                        lunco_experiments::TwinId(format!("replicated:{}", scene.host_twin.raw()))
                    }
                    _ => {
                        warn!("[experiment] archive has no admitted Twin history owner");
                        continue;
                    }
                };
                if let Err(error) = origins.restore_artifact(
                    &mut registry,
                    job.runtime,
                    group,
                    artifact,
                    job.limits,
                ) {
                    warn!("[experiment] optional history rejected: {error}");
                }
            }
            Ok(ArtifactOutcome::Written) => {
                if !job.expected_id.is_some_and(|id| {
                    origins.get(&id) == job.origin.as_ref() && registry.get(id).is_some()
                }) {
                    continue;
                }
                #[cfg(feature = "networking")]
                if matches!(role.as_deref(), Some(lunco_core_session::NetworkRole::Host)) {
                    if let Some(rebuild) = rebuild.as_deref_mut() {
                        rebuild.0 = true;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pending(
        token: u64,
        runtime: DocumentRuntimeOwner,
    ) -> (PendingArtifact, crossbeam_channel::Sender<ArtifactResponse>) {
        let (sender, response) = crossbeam_channel::bounded(1);
        (
            PendingArtifact {
                runtime,
                root: PathBuf::new(),
                limits: RunResultLimits::default(),
                expected_id: None,
                origin: None,
                cancel: Arc::new(AtomicBool::new(false)),
                key: AsyncWorkKey::new(
                    AsyncWorkKind::ExperimentArtifact,
                    0,
                    u128::from(token),
                    0,
                    token,
                ),
                response,
            },
            sender,
        )
    }
    #[test]
    fn artifact_routes_retire_exact_owner_and_fail_disconnected_preparation() {
        let outgoing = DocumentRuntimeOwner::LocalTwin(lunco_workspace::TwinId::new(1));
        let successor = DocumentRuntimeOwner::LocalTwin(lunco_workspace::TwinId::new(2));
        let mut jobs = ArtifactJobs::default();
        let mut scheduler = AsyncWorkAdmission::default();
        let (old, old_sender) = pending(1, outgoing.clone());
        let old_cancel = old.cancel.clone();
        let (new, new_sender) = pending(2, successor.clone());
        jobs.pending.insert(1, old);
        jobs.pending.insert(2, new);
        retire_runtime(&outgoing, &mut jobs, &mut scheduler);
        assert!(old_cancel.load(Ordering::SeqCst));
        assert!(
            old_sender
                .try_send(ArtifactResponse {
                    token: 1,
                    result: Ok(ArtifactOutcome::Written)
                })
                .is_err()
        );
        assert!(jobs.take_ready().is_empty());
        new_sender
            .try_send(ArtifactResponse {
                token: 2,
                result: Ok(ArtifactOutcome::Written),
            })
            .expect("successor route remains admitted");
        let mut outcomes = jobs.take_ready();
        assert_eq!(outcomes.len(), 1);
        let (job, result) = outcomes.pop().expect("successor completion");
        assert_eq!(job.runtime, successor);
        assert!(matches!(result, Ok(ArtifactOutcome::Written)));
        let (disconnected, sender) = pending(3, DocumentRuntimeOwner::Application);
        jobs.pending.insert(3, disconnected);
        drop(sender);
        let (_, result) = jobs.take_ready().pop().expect("disconnect is terminal");
        assert!(
            result
                .expect_err("disconnect diagnostic")
                .contains("disconnected")
        );
        let (misrouted, sender) = pending(4, DocumentRuntimeOwner::Application);
        jobs.pending.insert(4, misrouted);
        sender
            .try_send(ArtifactResponse {
                token: 5,
                result: Ok(ArtifactOutcome::Written),
            })
            .expect("misrouted response");
        assert!(
            jobs.take_ready()
                .pop()
                .expect("mismatch is terminal")
                .1
                .expect_err("mismatch diagnostic")
                .contains("admitted operation")
        );
        assert!(jobs.pending.is_empty());
    }
    #[cfg(unix)]
    #[test]
    fn artifact_io_rejects_directory_and_leaf_symlinks_outside_admitted_root() {
        use lunco_experiments::{
            ExperimentDefinition, RunBounds, RunMeta, RunResult, SourceContentIdentity,
        };
        let root = tempfile::tempdir().expect("owner root");
        let outside = tempfile::tempdir().expect("unrelated root");
        let root_path = root.path().canonicalize().expect("canonical owner");
        let directory = lunco_twin::results_dir(&root_path);
        let id = lunco_experiments::ExperimentId::new();
        let destination = directory.join(format!("{}.json", id.as_artifact_stem()));
        let artifact = RunArtifact {
            version: lunco_experiments::artifact::RUN_ARTIFACT_VERSION,
            experiment_id: id,
            definition: Arc::new(ExperimentDefinition {
                model_ref: lunco_experiments::ModelRef("Probe".into()),
                overrides: Default::default(),
                inputs: Default::default(),
                bounds: RunBounds::default(),
            }),
            name: "Probe".into(),
            color_hint: 0,
            created_at: std::time::UNIX_EPOCH,
            result: Arc::new(RunResult {
                times: vec![0.0],
                series: Default::default(),
                meta: RunMeta {
                    sample_count: 1,
                    source_content: Some(SourceContentIdentity::Available {
                        cid: "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku"
                            .parse()
                            .expect("inline CID"),
                    }),
                    ..Default::default()
                },
            }),
        };
        let cancelled = AtomicBool::new(false);
        let limits = RunResultLimits::default();
        std::os::unix::fs::symlink(outside.path(), &directory).expect("escaping directory");
        std::fs::write(
            outside
                .path()
                .join(destination.file_name().expect("filename")),
            b"{}",
        )
        .expect("outside fixture");
        for operation in [
            ArtifactOperation::List {
                directory: directory.clone(),
                cap: 1,
            },
            ArtifactOperation::Read {
                path: destination.clone(),
                limits,
            },
            ArtifactOperation::Write {
                path: destination.clone(),
                artifact: artifact.clone(),
                limits,
            },
        ] {
            assert!(
                bevy::tasks::block_on(process_native(operation, &cancelled, &root_path))
                    .expect_err("escape rejected")
                    .contains("outside")
            );
        }
        std::fs::remove_file(&directory).expect("remove escaping link");
        assert!(matches!(
            bevy::tasks::block_on(process_native(
                ArtifactOperation::Write {
                    path: destination.clone(),
                    artifact: artifact.clone(),
                    limits
                },
                &cancelled,
                &root_path
            ))
            .expect("create confined results"),
            ArtifactOutcome::Written
        ));
        assert!(matches!(
            bevy::tasks::block_on(process_native(
                ArtifactOperation::Read {
                    path: destination.clone(),
                    limits
                },
                &cancelled,
                &root_path
            ))
            .expect("confined history read"),
            ArtifactOutcome::Read(_)
        ));
        std::fs::remove_file(&destination).expect("remove fixture");
        std::os::unix::fs::symlink(
            outside
                .path()
                .join(destination.file_name().expect("filename")),
            &destination,
        )
        .expect("escaping leaf");
        for operation in [
            ArtifactOperation::Read {
                path: destination.clone(),
                limits,
            },
            ArtifactOperation::Write {
                path: destination,
                artifact,
                limits,
            },
        ] {
            assert!(
                bevy::tasks::block_on(process_native(operation, &cancelled, &root_path))
                    .expect_err("leaf escape rejected")
                    .contains("outside")
            );
        }
    }
}
fn retire_runtime(
    runtime: &DocumentRuntimeOwner,
    jobs: &mut ArtifactJobs,
    scheduler: &mut AsyncWorkAdmission,
) {
    let tokens = jobs
        .pending
        .iter()
        .filter_map(|(token, job)| (&job.runtime == runtime).then_some(*token))
        .collect::<Vec<_>>();
    for token in tokens {
        if let Some(job) = jobs.pending.remove(&token) {
            job.cancel.store(true, Ordering::SeqCst);
            scheduler.cancel_queued(job.key);
        }
    }
}
fn twin_closed(
    event: On<lunco_workspace::TwinClosed>,
    mut jobs: ResMut<ArtifactJobs>,
    mut scheduler: ResMut<AsyncWorkAdmission>,
) {
    let runtime = DocumentRuntimeOwner::LocalTwin(event.twin);
    retire_runtime(&runtime, &mut jobs, &mut scheduler);
}
fn replication_retired(
    event: On<lunco_core_session::ReplicationOwnerRetired>,
    mut jobs: ResMut<ArtifactJobs>,
    mut scheduler: ResMut<AsyncWorkAdmission>,
) {
    let runtime = DocumentRuntimeOwner::Replicated(event.owner.clone());
    retire_runtime(&runtime, &mut jobs, &mut scheduler);
}
pub(crate) fn install(app: &mut App) {
    if !app.is_plugin_added::<lunco_core_runtime::AsyncWorkAdmissionPlugin>() {
        app.add_plugins(lunco_core_runtime::AsyncWorkAdmissionPlugin);
    }
    app.init_resource::<ArtifactJobs>()
        .add_observer(load_added_twin)
        .add_observer(load_materialized)
        .add_observer(twin_closed)
        .add_observer(replication_retired);
    app.add_systems(
        Update,
        (write_completed, commit_artifacts.after(write_completed)).run_if(
            resource_exists::<ExperimentRegistry>
                .and_then(resource_exists::<ExperimentOrigins>)
                .and_then(resource_exists::<ExperimentSettings>),
        ),
    );
}
