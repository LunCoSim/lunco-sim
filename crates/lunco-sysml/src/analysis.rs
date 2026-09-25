//! Twin-scoped asynchronous SysML source-set analysis.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::{Arc, Mutex};

use bevy::asset::{AssetEvent, AssetLoadFailedEvent, AssetServer, Assets, Handle};
use bevy::prelude::*;
use lunco_assets_core::{TwinRoots, twin_uri};
use lunco_core_runtime::{AsyncWorkAdmission, AsyncWorkKey, AsyncWorkKind, AsyncWorkPriority};
use lunco_sysml_ast::SysmlAnalysis;
use lunco_workspace::{TwinClosed, WorkspaceResource};

use crate::SysmlSource;

/// Owner namespace used by scenarios that require a mounted Twin's SysML
/// analysis before their first initialization/start hook.
pub const TWIN_ANALYSIS_DEPENDENCY_OWNER: &str = "sysml.twin-analysis";

fn analysis_dependency_status(
    state: &TwinSysmlAnalysisState,
    operation_id: u64,
) -> lunco_core_runtime::SimulationDependencyStatus {
    match state {
        TwinSysmlAnalysisState::Pending => {
            lunco_core_runtime::SimulationDependencyStatus::Pending { operation_id }
        }
        TwinSysmlAnalysisState::Ready(analysis) => {
            lunco_core_runtime::SimulationDependencyStatus::Ready {
                source_revision: analysis.source_revision(),
            }
        }
        TwinSysmlAnalysisState::Failed(errors) => {
            lunco_core_runtime::SimulationDependencyStatus::Failed {
                operation_id,
                errors: errors.clone(),
            }
        }
    }
}

/// Request the semantic snapshot for the source set selected by Twin policy.
#[lunco_core::Command(default)]
pub struct PrepareTwinSysmlAnalysis {
    /// Workspace identity of the mounted Twin.
    pub twin_id: u64,
    /// Exact `twin://` authority assigned to the Twin.
    pub name: String,
    /// Complete ordered source selection from the Twin loading policy.
    pub relative_paths: Vec<String>,
}

#[derive(Clone)]
struct SourceHandle {
    relative_path: String,
    handle: Handle<SysmlSource>,
}

struct PendingAnalysis {
    twin_id: lunco_workspace::TwinId,
    root: std::path::PathBuf,
    name: String,
    operation: u64,
    sources: Vec<SourceHandle>,
    submitted: bool,
    capacity_revision: Option<u64>,
}

struct AnalysisCompletion {
    twin_id: lunco_workspace::TwinId,
    root: std::path::PathBuf,
    name: String,
    operation: u64,
    result: Result<Arc<SysmlAnalysis>, String>,
}

/// State visible to read-only analysis queries.
#[derive(Clone)]
pub enum TwinSysmlAnalysisState {
    /// No complete source set has been prepared yet.
    Pending,
    /// Immutable source-set snapshot committed by the SysML owner.
    Ready(Arc<SysmlAnalysis>),
    /// Source loading or analysis failed with an owner diagnostic.
    Failed(Vec<String>),
}

/// Prepared Twin snapshots and their in-flight worker operations.
#[derive(Resource)]
pub struct TwinSysmlAnalyses {
    states: BTreeMap<String, TwinSysmlAnalysisState>,
    owners: BTreeMap<String, (u64, std::path::PathBuf)>,
    source_sets: HashMap<
        u64,
        (
            lunco_workspace::TwinId,
            std::path::PathBuf,
            String,
            Vec<SourceHandle>,
        ),
    >,
    pending: HashMap<u64, PendingAnalysis>,
    completions: Arc<Mutex<Vec<AnalysisCompletion>>>,
    next_operation: u64,
}

impl Default for TwinSysmlAnalyses {
    fn default() -> Self {
        Self {
            states: BTreeMap::new(),
            owners: BTreeMap::new(),
            source_sets: HashMap::new(),
            pending: HashMap::new(),
            completions: Arc::default(),
            next_operation: 1,
        }
    }
}

impl TwinSysmlAnalyses {
    /// Read the current immutable result for one exact Twin authority.
    fn state(&self, name: &str) -> Option<TwinSysmlAnalysisState> {
        self.states.get(name).cloned()
    }

    /// Read a snapshot only when its authority still names the same mounted
    /// Twin identity and root.
    pub fn state_for(
        &self,
        name: &str,
        twin_id: lunco_workspace::TwinId,
        root: &Path,
    ) -> Option<TwinSysmlAnalysisState> {
        self.owners
            .get(name)
            .is_some_and(|(owner_id, owner_root)| *owner_id == twin_id.raw() && owner_root == root)
            .then(|| self.state(name))
            .flatten()
    }
}

fn build_analysis_snapshot(
    twin_name: &str,
    mut source_texts: Vec<(String, Arc<str>)>,
) -> Arc<SysmlAnalysis> {
    source_texts.sort_by(|left, right| left.0.cmp(&right.0));
    let mut revision_input = Vec::new();
    let files: Vec<_> = source_texts
        .into_iter()
        .map(|(path, text)| {
            let logical = twin_uri(twin_name, &path);
            revision_input.extend_from_slice(&(logical.len() as u64).to_le_bytes());
            revision_input.extend_from_slice(logical.as_bytes());
            revision_input.extend_from_slice(&(text.len() as u64).to_le_bytes());
            revision_input.extend_from_slice(text.as_bytes());
            (logical, text.to_string())
        })
        .collect();
    let source_revision = lunco_hash::fnv1a64(&revision_input);
    Arc::new(SysmlAnalysis::build(files, true, source_revision))
}

fn completion_matches(pending: &PendingAnalysis, completion: &AnalysisCompletion) -> bool {
    pending.twin_id == completion.twin_id
        && pending.operation == completion.operation
        && pending.root == completion.root
        && pending.name == completion.name
}

fn pending_work_key(pending: &PendingAnalysis) -> AsyncWorkKey {
    AsyncWorkKey::new(
        AsyncWorkKind::SysmlAnalysis,
        pending.twin_id.raw(),
        u128::from(pending.twin_id.raw()),
        0,
        pending.operation,
    )
}

fn retire_pending_analysis(pending: PendingAnalysis, admission: &mut AsyncWorkAdmission) {
    admission.cancel_queued(pending_work_key(&pending));
}

/// Register the explicit preparation command and async result owner.
pub(crate) fn register(app: &mut App) {
    if !app.is_plugin_added::<lunco_core_runtime::AsyncWorkAdmissionPlugin>() {
        app.add_plugins(lunco_core_runtime::AsyncWorkAdmissionPlugin);
    }
    app.init_resource::<TwinSysmlAnalyses>()
        .init_resource::<lunco_core_runtime::SimulationDependencyStates>()
        .add_systems(Update, prepare_ready_sysml_analyses)
        .add_observer(clear_closed_twin_analysis);
    app.world_mut()
        .resource_mut::<lunco_core_runtime::SimulationDependencyStates>()
        .register_owner(TWIN_ANALYSIS_DEPENDENCY_OWNER)
        .expect("static SysML analysis dependency owner is valid");
    register_all_commands(app);
}

fn twin_analysis_dependency_key(
    name: &str,
) -> Result<lunco_core_runtime::SimulationDependencyKey, String> {
    lunco_core_runtime::SimulationDependencyKey::new(
        TWIN_ANALYSIS_DEPENDENCY_OWNER,
        name.to_owned(),
    )
}

#[lunco_core::on_command(PrepareTwinSysmlAnalysis)]
fn prepare_twin_sysml_analysis(
    trigger: On<PrepareTwinSysmlAnalysis>,
    workspace: Option<Res<WorkspaceResource>>,
    roots: Option<Res<TwinRoots>>,
    asset_server: Option<Res<AssetServer>>,
    assets: Option<Res<Assets<SysmlSource>>>,
    mut analyses: ResMut<TwinSysmlAnalyses>,
    mut dependency_states: ResMut<lunco_core_runtime::SimulationDependencyStates>,
    mut admission: ResMut<AsyncWorkAdmission>,
) -> Result<lunco_command_contracts::Ack, String> {
    let request = trigger.event();
    let twin_id = lunco_workspace::TwinId::new(request.twin_id);
    let workspace = workspace.ok_or_else(|| "WorkspaceResource is not installed".to_owned())?;
    let twin = workspace
        .twin(twin_id)
        .ok_or_else(|| format!("workspace Twin {} is unavailable", request.twin_id))?;
    if workspace.active_twin != Some(twin_id) {
        return Err(format!("Twin {} is not active", request.twin_id));
    }
    let roots = roots.ok_or_else(|| "TwinRoots is not installed".to_owned())?;
    if !roots
        .name_for_root(&twin.root)
        .map_err(|error| error.to_string())?
        .is_some_and(|name| name == request.name)
    {
        return Err(format!(
            "Twin asset authority `{}` does not belong to Twin {}",
            request.name, request.twin_id
        ));
    }
    let mut expected = twin
        .discover_sysml_sources_checked()
        .map_err(|errors| errors.join("; "))?;
    if expected.iter().any(|path| path.to_str().is_none()) {
        return Err("Twin SysML source paths must be valid UTF-8".to_owned());
    }
    expected.sort();
    let requested: Vec<_> = request
        .relative_paths
        .iter()
        .map(std::path::PathBuf::from)
        .collect();
    let mut requested_sorted = requested;
    requested_sorted.sort();
    if expected != requested_sorted {
        return Err(format!(
            "Twin loading policy selected a SysML source set that differs from the checked manifest set (expected {}, received {})",
            expected.len(),
            requested_sorted.len()
        ));
    }

    if analyses
        .owners
        .get(&request.name)
        .is_some_and(|(owner_id, owner_root)| {
            *owner_id != request.twin_id || owner_root != &twin.root
        })
    {
        return Err(format!(
            "SysML analysis authority `{}` still belongs to a different mounted Twin; its TwinClosed cleanup has not completed",
            request.name
        ));
    }

    if expected.is_empty() {
        if let Some(previous) = analyses.pending.remove(&request.twin_id) {
            retire_pending_analysis(previous, &mut admission);
        }
        analyses.source_sets.remove(&request.twin_id);
        let analysis = build_analysis_snapshot(&request.name, Vec::new());
        let source_revision = analysis.source_revision();
        analyses.states.insert(
            request.name.clone(),
            TwinSysmlAnalysisState::Ready(analysis),
        );
        analyses
            .owners
            .insert(request.name.clone(), (request.twin_id, twin.root.clone()));
        dependency_states.publish(
            twin_analysis_dependency_key(&request.name)?,
            lunco_core_runtime::SimulationDependencyStatus::Ready { source_revision },
        )?;
        return Ok(lunco_command_contracts::Ack::new(
            lunco_command_contracts::OpId::new(),
        ));
    }
    let asset_server = asset_server.ok_or_else(|| "AssetServer is not installed".to_owned())?;
    if assets.is_none() {
        return Err("SysML source Assets are not installed".to_owned());
    }
    let operation = analyses.next_operation;
    analyses.next_operation = operation
        .checked_add(1)
        .ok_or_else(|| "SysML analysis operation id exhausted".to_owned())?;
    let sources: Vec<SourceHandle> = expected
        .into_iter()
        .map(|relative| SourceHandle {
            relative_path: relative
                .to_str()
                .expect("UTF-8 SysML path checked above")
                .to_owned(),
            handle: asset_server.load::<SysmlSource>(twin_uri(&request.name, &relative)),
        })
        .collect();
    analyses.source_sets.insert(
        request.twin_id,
        (
            twin_id,
            twin.root.clone(),
            request.name.clone(),
            sources.clone(),
        ),
    );
    if let Some(previous) = analyses.pending.remove(&request.twin_id) {
        retire_pending_analysis(previous, &mut admission);
    }
    analyses.pending.insert(
        request.twin_id,
        PendingAnalysis {
            twin_id,
            root: twin.root.clone(),
            name: request.name.clone(),
            operation,
            sources,
            submitted: false,
            capacity_revision: None,
        },
    );
    analyses
        .states
        .insert(request.name.clone(), TwinSysmlAnalysisState::Pending);
    analyses
        .owners
        .insert(request.name.clone(), (request.twin_id, twin.root.clone()));
    dependency_states.publish(
        twin_analysis_dependency_key(&request.name)?,
        lunco_core_runtime::SimulationDependencyStatus::Pending {
            operation_id: operation,
        },
    )?;
    Ok(lunco_command_contracts::Ack::new(
        lunco_command_contracts::OpId::new(),
    ))
}

lunco_core::register_commands!(prepare_twin_sysml_analysis);

fn prepare_ready_sysml_analyses(
    assets: Option<Res<Assets<SysmlSource>>>,
    mut failures: MessageReader<AssetLoadFailedEvent<SysmlSource>>,
    mut asset_events: MessageReader<AssetEvent<SysmlSource>>,
    mut analyses: ResMut<TwinSysmlAnalyses>,
    mut dependency_states: ResMut<lunco_core_runtime::SimulationDependencyStates>,
    mut admission: ResMut<AsyncWorkAdmission>,
) {
    let failed_assets: HashMap<_, _> = failures
        .read()
        .map(|failure| (failure.id, failure.error.to_string()))
        .collect();
    let (changed_assets, removed_assets): (HashSet<_>, HashSet<_>) = asset_events.read().fold(
        (HashSet::new(), HashSet::new()),
        |(mut changed, mut removed), event| {
            match event {
                AssetEvent::Added { id } | AssetEvent::Modified { id } => {
                    changed.insert(*id);
                }
                AssetEvent::Removed { id } => {
                    removed.insert(*id);
                }
                _ => {}
            }
            (changed, removed)
        },
    );

    let mut changed_twins: Vec<_> = analyses
        .source_sets
        .iter()
        .filter(|(_, (_, _, _, sources))| {
            sources.iter().any(|source| {
                changed_assets.contains(&source.handle.id())
                    || removed_assets.contains(&source.handle.id())
            })
        })
        .map(|(raw_id, _)| *raw_id)
        .collect();
    changed_twins.sort_unstable();
    for raw_id in changed_twins {
        let Some((twin_id, root, name, sources)) = analyses.source_sets.get(&raw_id).cloned()
        else {
            continue;
        };
        let operation = analyses.next_operation;
        let Some(next_operation) = operation.checked_add(1) else {
            if let Some(previous) = analyses.pending.remove(&raw_id) {
                retire_pending_analysis(previous, &mut admission);
            }
            let state = TwinSysmlAnalysisState::Failed(vec![
                "SysML analysis operation id exhausted".into(),
            ]);
            analyses.states.insert(name.clone(), state.clone());
            if let Ok(key) = twin_analysis_dependency_key(&name) {
                if let Err(error) =
                    dependency_states.publish(key, analysis_dependency_status(&state, operation))
                {
                    bevy::log::error!("[sysml-analysis] {error}");
                }
            }
            continue;
        };
        analyses.next_operation = next_operation;
        if let Some(previous) = analyses.pending.remove(&raw_id) {
            retire_pending_analysis(previous, &mut admission);
        }
        analyses.pending.insert(
            raw_id,
            PendingAnalysis {
                twin_id,
                root,
                name: name.clone(),
                operation,
                sources,
                submitted: false,
                capacity_revision: None,
            },
        );
        analyses
            .states
            .insert(name.clone(), TwinSysmlAnalysisState::Pending);
        if let Ok(key) = twin_analysis_dependency_key(&name) {
            if let Err(error) = dependency_states.publish(
                key,
                lunco_core_runtime::SimulationDependencyStatus::Pending {
                    operation_id: operation,
                },
            ) {
                bevy::log::error!("[sysml-analysis] {error}");
            }
        }
    }

    let completion_sender = Arc::clone(&analyses.completions);
    let mut pending_ids: Vec<_> = analyses.pending.keys().copied().collect();
    pending_ids.sort_unstable();
    let mut state_updates = Vec::new();
    let mut terminal_pending_ids = Vec::new();
    for raw_id in pending_ids {
        let Some(pending) = analyses.pending.get_mut(&raw_id) else {
            continue;
        };
        if pending.submitted {
            continue;
        }
        if let Some((source, error)) = pending.sources.iter().find_map(|source| {
            failed_assets
                .get(&source.handle.id())
                .map(|error| (source.relative_path.clone(), error.clone()))
        }) {
            let message = format!(
                "Twin `{}` SysML source `{source}` failed to load: {error}",
                pending.name
            );
            state_updates.push((
                pending.name.clone(),
                pending.operation,
                TwinSysmlAnalysisState::Failed(vec![message]),
            ));
            terminal_pending_ids.push(raw_id);
            continue;
        }
        if pending
            .sources
            .iter()
            .any(|source| changed_assets.contains(&source.handle.id()))
        {
            continue;
        }
        let Some(assets) = assets.as_deref() else {
            state_updates.push((
                pending.name.clone(),
                pending.operation,
                TwinSysmlAnalysisState::Failed(vec![
                    "SysML source asset storage was removed during analysis".to_owned(),
                ]),
            ));
            terminal_pending_ids.push(raw_id);
            continue;
        };
        if let Some(source) = pending.sources.iter().find(|source| {
            removed_assets.contains(&source.handle.id()) && assets.get(&source.handle).is_none()
        }) {
            state_updates.push((
                pending.name.clone(),
                pending.operation,
                TwinSysmlAnalysisState::Failed(vec![format!(
                    "Twin `{}` SysML source `{}` failed to load: the loaded source asset was removed",
                    pending.name, source.relative_path
                )]),
            ));
            terminal_pending_ids.push(raw_id);
            continue;
        }
        let Some(source_texts) = pending
            .sources
            .iter()
            .map(|source| {
                assets
                    .get(&source.handle)
                    .map(|asset| (source.relative_path.clone(), Arc::clone(&asset.text)))
            })
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };

        let capacity_revision = admission.capacity_revision();
        if pending.capacity_revision == Some(capacity_revision) {
            continue;
        }
        let key = pending_work_key(pending);
        let sender = Arc::clone(&completion_sender);
        let twin_id = pending.twin_id;
        let root = pending.root.clone();
        let name = pending.name.clone();
        let operation = pending.operation;
        let job = move || {
            let result = catch_unwind(AssertUnwindSafe(|| {
                build_analysis_snapshot(&name, source_texts)
            }))
            .map_err(|_| "SysML analysis worker panicked".to_owned());
            let mut queued = sender
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            queued.push(AnalysisCompletion {
                twin_id,
                root,
                name,
                operation,
                result,
            });
        };
        match admission.submit(AsyncWorkPriority::Interactive, key, job) {
            Ok(()) | Err(lunco_core_runtime::AsyncWorkRejection::DuplicateKey) => {
                pending.submitted = true;
            }
            Err(lunco_core_runtime::AsyncWorkRejection::QueueFull) => {
                pending.capacity_revision = Some(capacity_revision);
            }
            Err(lunco_core_runtime::AsyncWorkRejection::NativeDispatcherUnavailable) => {
                state_updates.push((
                    pending.name.clone(),
                    pending.operation,
                    TwinSysmlAnalysisState::Failed(vec![
                        "SysML analysis requires a native worker; this host has no Web Worker transport".to_owned(),
                    ]),
                ));
                terminal_pending_ids.push(raw_id);
            }
        }
    }
    for (name, operation, state) in state_updates {
        if let Ok(key) = twin_analysis_dependency_key(&name) {
            if let Err(error) =
                dependency_states.publish(key, analysis_dependency_status(&state, operation))
            {
                bevy::log::error!("[sysml-analysis] {error}");
            }
        }
        analyses.states.insert(name, state);
    }
    terminal_pending_ids.sort_unstable();
    terminal_pending_ids.dedup();
    for raw_id in terminal_pending_ids {
        if let Some(pending) = analyses.pending.remove(&raw_id) {
            retire_pending_analysis(pending, &mut admission);
        }
    }

    let completions = {
        let mut queue = analyses
            .completions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut ready = std::mem::take(&mut *queue);
        ready.sort_by_key(|completion| (completion.twin_id.raw(), completion.operation));
        ready
    };
    for completion in completions {
        let Some(pending) = analyses.pending.get(&completion.twin_id.raw()) else {
            continue;
        };
        if !completion_matches(pending, &completion) {
            continue;
        }
        let state = match completion.result {
            Ok(analysis) => TwinSysmlAnalysisState::Ready(analysis),
            Err(error) => TwinSysmlAnalysisState::Failed(vec![error]),
        };
        if let Ok(key) = twin_analysis_dependency_key(&completion.name) {
            if let Err(error) = dependency_states.publish(
                key,
                analysis_dependency_status(&state, completion.operation),
            ) {
                bevy::log::error!("[sysml-analysis] {error}");
            }
        }
        analyses.states.insert(completion.name.clone(), state);
        if let Some(pending) = analyses.pending.remove(&completion.twin_id.raw()) {
            retire_pending_analysis(pending, &mut admission);
        }
    }
}

fn clear_closed_twin_analysis(
    trigger: On<TwinClosed>,
    mut analyses: ResMut<TwinSysmlAnalyses>,
    mut dependency_states: ResMut<lunco_core_runtime::SimulationDependencyStates>,
    mut admission: ResMut<AsyncWorkAdmission>,
) {
    let twin_id = trigger.event().twin;
    if let Some(pending) = analyses.pending.remove(&twin_id.raw()) {
        retire_pending_analysis(pending, &mut admission);
    }
    let removed: Vec<_> = analyses
        .owners
        .iter()
        .filter(|(_, (owner_id, _))| *owner_id == twin_id.raw())
        .map(|(name, _)| name.clone())
        .collect();
    for name in removed {
        analyses.owners.remove(&name);
        analyses.states.remove(&name);
        if let Ok(key) = twin_analysis_dependency_key(&name) {
            dependency_states.retire(&key);
        }
    }
    analyses.source_sets.remove(&twin_id.raw());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twin_analysis_registers_its_scenario_readiness_owner() {
        let mut app = App::new();
        register(&mut app);
        assert!(
            app.world()
                .resource::<lunco_core_runtime::SimulationDependencyStates>()
                .owner_is_registered(TWIN_ANALYSIS_DEPENDENCY_OWNER)
        );

        let snapshot = build_analysis_snapshot(
            "analysis-fixture",
            vec![("a.sysml".into(), Arc::from("package A { part def Rover; }"))],
        );
        assert_eq!(
            analysis_dependency_status(&TwinSysmlAnalysisState::Ready(snapshot.clone()), 7),
            lunco_core_runtime::SimulationDependencyStatus::Ready {
                source_revision: snapshot.source_revision(),
            }
        );
        assert_eq!(
            analysis_dependency_status(
                &TwinSysmlAnalysisState::Failed(vec!["source failed".to_owned()]),
                8,
            ),
            lunco_core_runtime::SimulationDependencyStatus::Failed {
                operation_id: 8,
                errors: vec!["source failed".to_owned()],
            }
        );
    }

    #[test]
    fn source_set_analysis_is_content_revisioned_and_order_independent() {
        let first = build_analysis_snapshot(
            "analysis-fixture",
            vec![
                ("b.kerml".into(), Arc::from("package B { part def Body; }")),
                ("a.sysml".into(), Arc::from("package A { part def Rover; }")),
            ],
        );
        let reordered = build_analysis_snapshot(
            "analysis-fixture",
            vec![
                ("a.sysml".into(), Arc::from("package A { part def Rover; }")),
                ("b.kerml".into(), Arc::from("package B { part def Body; }")),
            ],
        );
        let edited = build_analysis_snapshot(
            "analysis-fixture",
            vec![
                (
                    "a.sysml".into(),
                    Arc::from("package A { part def Lander; }"),
                ),
                ("b.kerml".into(), Arc::from("package B { part def Body; }")),
            ],
        );

        assert_eq!(*first, *reordered);
        assert_eq!(first.source_revision(), reordered.source_revision());
        assert_ne!(first.source_revision(), edited.source_revision());
    }

    #[test]
    fn worker_result_is_accepted_only_for_its_exact_twin_operation() {
        let twin_id = lunco_workspace::TwinId::new(7);
        let root = std::path::PathBuf::from("/fixture/twin");
        let pending = PendingAnalysis {
            twin_id,
            root: root.clone(),
            name: "analysis-fixture".into(),
            operation: 12,
            sources: Vec::new(),
            submitted: true,
            capacity_revision: None,
        };
        let result = || {
            Arc::new(SysmlAnalysis::build(
                std::iter::empty::<(String, String)>(),
                false,
                0,
            ))
        };
        let current = AnalysisCompletion {
            twin_id,
            root: root.clone(),
            name: "analysis-fixture".into(),
            operation: 12,
            result: Ok(result()),
        };
        let stale = AnalysisCompletion {
            twin_id,
            root,
            name: "analysis-fixture".into(),
            operation: 11,
            result: Ok(result()),
        };

        assert!(completion_matches(&pending, &current));
        assert!(!completion_matches(&pending, &stale));
    }

    #[test]
    fn analysis_snapshot_lookup_requires_the_current_twin_identity_and_root() {
        let twin_id = lunco_workspace::TwinId::new(7);
        let root = std::path::PathBuf::from("/fixture/twin");
        let mut analyses = TwinSysmlAnalyses::default();
        analyses
            .owners
            .insert("analysis-fixture".into(), (twin_id.raw(), root.clone()));
        analyses.states.insert(
            "analysis-fixture".into(),
            TwinSysmlAnalysisState::Ready(build_analysis_snapshot("analysis-fixture", Vec::new())),
        );

        assert!(
            analyses
                .state_for("analysis-fixture", twin_id, &root)
                .is_some()
        );
        assert!(
            analyses
                .state_for("analysis-fixture", lunco_workspace::TwinId::new(8), &root)
                .is_none()
        );
        assert!(
            analyses
                .state_for("analysis-fixture", twin_id, Path::new("/fixture/other"))
                .is_none()
        );
    }

    #[test]
    fn retiring_analysis_cancels_only_its_queued_work() {
        let twin_id = lunco_workspace::TwinId::new(7);
        let pending = PendingAnalysis {
            twin_id,
            root: std::path::PathBuf::from("/fixture/twin"),
            name: "analysis-fixture".into(),
            operation: 12,
            sources: Vec::new(),
            submitted: false,
            capacity_revision: None,
        };
        let mut admission = AsyncWorkAdmission::default();
        admission
            .submit(
                AsyncWorkPriority::Interactive,
                pending_work_key(&pending),
                || {},
            )
            .expect("queued analysis is admitted");
        retire_pending_analysis(pending, &mut admission);

        assert_eq!(admission.snapshot().cancelled, 1);
        assert_eq!(admission.snapshot().queued, [0, 0, 0]);
    }
}
