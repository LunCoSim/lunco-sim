//! One-shot query preparation. Source I/O and pure facts use the existing Bevy
//! task/reader boundary. Twin facts, pure policy and response preparation share
//! that job; publication consumes the exact fenced result.

use crate::{
    twin_lint::{TwinInspectionInput, TwinNamespaceSnapshot},
    validate::ValidationReport,
};
#[cfg(target_arch = "wasm32")]
use bevy::tasks::{AsyncComputeTaskPool, Task};
use bevy::{
    asset::{AssetPath, AssetServer},
    prelude::*,
    tasks::futures_lite::future,
};
use lunco_api::ApiQueryError;
use lunco_api_core::{ApiErrorCode, ApiValue, api_value};
use lunco_assets_core::TwinRoots;
use lunco_core_runtime::async_work::ExternalWorkPermit;
#[cfg(not(target_arch = "wasm32"))]
use lunco_core_runtime::async_work::QueuedWorkRequest;
use lunco_core_runtime::{AsyncWorkAdmission, AsyncWorkKey, AsyncWorkKind, AsyncWorkPriority};
use lunco_workspace::{DocumentRuntimeOwner, FileDocumentAdmission, WorkspaceResource};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, PoisonError},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueryKind {
    Asset,
    Sysml,
    AnalyzeSysml,
    Twin,
    TwinLint,
}

/// Query preparation uses one closure read at a time; budgets are the same
/// owner limits as runtime USD composition and may be configured by the host.
#[derive(Resource)]
pub struct QueryPreparationLimits(pub lunco_usd_compose::recipe::StageClosureLimits);
impl Default for QueryPreparationLimits {
    fn default() -> Self {
        Self(lunco_usd_compose::recipe::StageClosureLimits {
            max_parallel_reads: 1,
            ..lunco_usd_compose::recipe::StageClosureLimits::default()
        })
    }
}
#[derive(Resource, Default)]
pub(crate) struct QueryPreparations(Mutex<PreparationState>);
#[derive(Default)]
struct PreparationState {
    next: u64,
    operations: HashMap<u64, Operation>,
}
struct Operation {
    kind: QueryKind,
    params: Option<ApiValue>,
    fence: OwnerFence,
    #[cfg(not(target_arch = "wasm32"))]
    task: NativePreparation,
    #[cfg(target_arch = "wasm32")]
    task: Task<Result<Prepared, String>>,
    #[cfg(target_arch = "wasm32")]
    _permit: Arc<ExternalWorkPermit>,
}
#[cfg(not(target_arch = "wasm32"))]
struct NativePreparation {
    result: std::sync::mpsc::Receiver<(Result<Prepared, String>, Arc<ExternalWorkPermit>)>,
    _request: QueuedWorkRequest,
    _lifetime: Arc<()>,
}
#[derive(Clone)]
struct OwnerFence {
    runtime: DocumentRuntimeOwner,
    mount: Option<(String, PathBuf)>,
    twin_input: Option<(lunco_workspace::TwinId, TwinInspectionInput)>,
    policies: Vec<(&'static str, Option<Arc<lunco_hooks::RegisteredHook>>)>,
    #[cfg(feature = "sysml-runtime")]
    sysml_analysis: Option<(
        String,
        lunco_workspace::TwinId,
        PathBuf,
        Arc<lunco_sysml_ast::SysmlAnalysis>,
    )>,
}
impl OwnerFence {
    fn lifetime_current(&self, world: &World) -> bool {
        let workspace = world.get_resource::<WorkspaceResource>();
        let replication = lunco_core_session::session::current_replication_owner_in(world);
        self.runtime.is_current(
            workspace.map(|workspace| &**workspace),
            replication.as_ref(),
        ) && self.mount.as_ref().is_none_or(|(authority, root)| {
            world.get_resource::<TwinRoots>().is_some_and(|roots| {
                roots.root_for(authority).ok().flatten().as_ref() == Some(root)
            })
        }) && self.policies.iter().all(|(domain, admitted)| {
            let current = lunco_hooks::get(&lunco_lint::hook_id(domain));
            match (admitted, current) {
                (None, None) => true,
                (Some(admitted), Some(current)) => Arc::ptr_eq(admitted, &current),
                _ => false,
            }
        })
    }
    fn is_current(&self, world: &World) -> bool {
        self.lifetime_current(world)
            && self.sysml_current(world)
            && self.twin_input.as_ref().is_none_or(|(id, input)| {
                world
                    .get_resource::<WorkspaceResource>()
                    .and_then(|workspace| workspace.twin(*id))
                    .is_some_and(|twin| TwinInspectionInput::capture(twin) == *input)
            })
    }
    fn sysml_current(&self, world: &World) -> bool {
        #[cfg(feature = "sysml-runtime")]
        if let Some((source_identity, id, root, admitted)) = &self.sysml_analysis {
            return world.get_resource::<lunco_sysml::TwinSysmlAnalyses>()
                .and_then(|analyses| analyses.state_for_source_identity(source_identity, *id, root))
                .is_some_and(|state| matches!(state,
                    lunco_sysml::TwinSysmlAnalysisState::Ready(current) if Arc::ptr_eq(admitted, &current)));
        }
        #[cfg(not(feature = "sysml-runtime"))]
        let _ = world;
        true
    }
}

#[derive(serde::Serialize)]
pub(crate) struct ReadRevision {
    path: String,
    cid: String,
}
enum PreparedFacts {
    Asset { report: ValidationReport },
    Twin(PreparedTwinResult),
}
struct Prepared {
    facts: PreparedFacts,
    source_runtime: Option<DocumentRuntimeOwner>,
    revisions: Vec<ReadRevision>,
    mounts: Vec<(String, PathBuf)>,
}
pub(crate) enum PreparationPoll {
    Pending {
        operation_id: u64,
    },
    Ready {
        operation_id: u64,
        report: ValidationReport,
        params: ApiValue,
        revisions: ApiValue,
    },
    ReadyTwin {
        operation_id: u64,
        result: PreparedTwinResult,
        permit: Arc<ExternalWorkPermit>,
    },
    Failed {
        operation_id: u64,
        diagnostic: String,
    },
}

pub(crate) enum PreparedTwinResult {
    Query {
        report: ApiValue,
        revisions: ApiValue,
    },
    Lint {
        twin: String,
        source_errors: Vec<String>,
        findings: Vec<lunco_doc::Diagnostic>,
    },
}

struct PolicyPreparation {
    kind: QueryKind,
    context: lunco_hooks::RuntimeExecutionContext,
    policies: Vec<(&'static str, Option<Arc<lunco_hooks::RegisteredHook>>)>,
}

impl PolicyPreparation {
    fn registration(&self, domain: &str) -> Option<&lunco_hooks::RegisteredHook> {
        self.policies
            .iter()
            .find(|(name, _)| *name == domain)
            .and_then(|(_, registration)| registration.as_deref())
    }
    fn finish_asset(&self, report: ValidationReport, text: &str) -> ValidationReport {
        if self.kind == QueryKind::AnalyzeSysml {
            return report;
        }
        let registration = self.registration(&report.kind);
        crate::validate::apply_lint_policy_with_registration(
            report,
            text,
            self.context,
            registration,
        )
    }
    fn finish_twin(
        &self,
        reference: &str,
        policy: &str,
        snapshot: TwinNamespaceSnapshot,
        registry_errors: Vec<String>,
        revisions: &[ReadRevision],
    ) -> Result<PreparedTwinResult, String> {
        let findings = lunco_lint::run_lint_with_registration(
            "twin",
            crate::twin_lint::facts(&snapshot, policy),
            self.context,
            self.registration("twin"),
        );
        match self.kind {
            QueryKind::Twin => {
                let report = crate::validate::finish_twin_report(
                    reference,
                    policy,
                    snapshot,
                    registry_errors,
                    findings,
                );
                Ok(PreparedTwinResult::Query {
                    report: lunco_api_core::api_value_from_serializable(&report)
                        .map_err(|error| error.to_string())?,
                    revisions: lunco_api_core::api_value_from_serializable(revisions)
                        .map_err(|error| error.to_string())?,
                })
            }
            QueryKind::TwinLint => {
                let mut source_errors = registry_errors;
                source_errors.extend(snapshot.read_errors);
                Ok(PreparedTwinResult::Lint {
                    twin: snapshot.twin,
                    source_errors,
                    findings,
                })
            }
            _ => Err("Twin policy preparation belongs to another query".to_owned()),
        }
    }
}

fn rejected(message: impl Into<String>) -> ApiQueryError {
    ApiQueryError::new(ApiErrorCode::CommandRejected, message)
}
fn envelope(id: u64, state: &str, report: ApiValue, revisions: ApiValue) -> ApiValue {
    api_value!({"operation_id": id, "state": state, "report": report, "source_revisions": revisions})
}
pub(crate) fn failed(id: u64, diagnostic: impl Into<String>) -> ApiValue {
    api_value!({"operation_id": id, "state": "failed", "diagnostic": diagnostic.into()})
}
fn capture_fence(
    world: &World,
    reference: &str,
    kind: QueryKind,
) -> Result<OwnerFence, ApiQueryError> {
    let workspace = world.get_resource::<WorkspaceResource>();
    let replication = lunco_core_session::session::current_replication_owner_in(world);
    let runtime = workspace.map_or(DocumentRuntimeOwner::Application, |workspace| {
        workspace.new_document_runtime_owner(replication.as_ref())
    });
    let mut mount = None;
    if let Some(identity) = lunco_assets_core::parse_twin_root_uri(reference)
        .or_else(|| lunco_assets_core::parse_twin_uri(reference).map(|(identity, _)| identity))
    {
        let roots = world
            .get_resource::<TwinRoots>()
            .ok_or_else(|| rejected("Twin asset registry is unavailable"))?;
        let authority = roots
            .mounted_authority_for_logical(identity)
            .map_err(|error| rejected(error.to_string()))?
            .ok_or_else(|| rejected(format!("Twin source `{identity}` is not mounted")))?;
        let root = roots
            .root_for(&authority)
            .map_err(|error| rejected(error.to_string()))?
            .ok_or_else(|| {
                rejected(format!(
                    "Twin source `{identity}` mount retired during admission"
                ))
            })?;
        mount = Some((authority, root));
    } else if let DocumentRuntimeOwner::LocalTwin(id) = &runtime {
        if let Some(twin) = workspace.and_then(|workspace| workspace.twin(*id)) {
            let roots = world
                .get_resource::<TwinRoots>()
                .ok_or_else(|| rejected("Active Twin asset registry is unavailable"))?;
            let authority = roots
                .authorities()
                .map_err(|error| rejected(error.to_string()))?
                .into_iter()
                .find(|name| roots.root_for(name).ok().flatten().as_ref() == Some(&twin.root))
                .ok_or_else(|| rejected("Active Twin has no current asset mount"))?;
            mount = Some((authority, twin.root.clone()));
        }
    }
    let domains: &[&'static str] = if matches!(kind, QueryKind::Twin | QueryKind::TwinLint) {
        &["twin"]
    } else {
        &["modelica", "usd", "sysml", "wesl", "rhai"]
    };
    Ok(OwnerFence {
        runtime,
        mount,
        twin_input: None,
        #[cfg(feature = "sysml-runtime")]
        sysml_analysis: None,
        policies: domains
            .iter()
            .map(|domain| (*domain, lunco_hooks::get(&lunco_lint::hook_id(domain))))
            .collect(),
    })
}

/// External query adapter: decode admission parameters or the exact poll ID once.
pub(crate) fn poll(
    world: &World,
    kind: QueryKind,
    params: &ApiValue,
) -> Result<PreparationPoll, ApiQueryError> {
    if let Some(value) = params.get("operation_id") {
        if params.get("path").is_some() {
            return Err(rejected(
                "Poll with operation_id only; path admits a new operation",
            ));
        }
        let id = match value {
            ApiValue::UInt(id) => *id,
            ApiValue::Int(id) if *id >= 0 => *id as u64,
            _ => return Err(rejected("operation_id must be a nonnegative integer")),
        };
        return poll_operation(world, kind, id);
    }
    let reference = lunco_api::api_param_str(params, "path").ok_or_else(|| {
        rejected("Initial validation query requires path; poll requires operation_id")
    })?;
    let policy = match params.get("policy") {
        Some(ApiValue::Str(policy)) => policy.as_str(),
        None => "warn",
        Some(_) => return Err(rejected("policy must be a string")),
    };
    let operation_id = admit_request(
        world,
        kind,
        reference.to_owned(),
        policy.to_owned(),
        Some(params.clone()),
        lunco_hooks::RuntimeExecutionContext::unclassified(),
    )?;
    Ok(PreparationPoll::Pending { operation_id })
}

/// Consume one exact typed operation; no internal API parameter envelope.
pub(crate) fn poll_operation(
    world: &World,
    kind: QueryKind,
    id: u64,
) -> Result<PreparationPoll, ApiQueryError> {
    let preparations = world
        .get_resource::<QueryPreparations>()
        .ok_or_else(|| rejected("Validation preparation owner is unavailable"))?;
    let mut state = preparations
        .0
        .lock()
        .map_err(|_| rejected("Validation preparation state is poisoned"))?;
    state
        .operations
        .retain(|_, operation| operation.fence.lifetime_current(world));
    let operation = state.operations.get_mut(&id).ok_or_else(|| {
        rejected(format!(
            "Unknown, consumed, or retired validation operation {id}"
        ))
    })?;
    if operation.kind != kind {
        return Err(rejected("operation_id belongs to a different query"));
    }
    #[cfg(not(target_arch = "wasm32"))]
    let (outcome, permit) = match operation.task.result.try_recv() {
        Ok(completion) => completion,
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            return Ok(PreparationPoll::Pending { operation_id: id });
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            state.operations.remove(&id);
            return Ok(PreparationPoll::Failed {
                operation_id: id,
                diagnostic: "Validation preparation ended without a result".to_owned(),
            });
        }
    };
    #[cfg(target_arch = "wasm32")]
    let Some(outcome) = future::block_on(future::poll_once(&mut operation.task)) else {
        return Ok(PreparationPoll::Pending { operation_id: id });
    };
    #[cfg(target_arch = "wasm32")]
    let permit = Arc::clone(&operation._permit);
    let operation = state
        .operations
        .remove(&id)
        .ok_or_else(|| rejected("Validation operation retired before consumption"))?;
    drop(state);
    if !operation.fence.is_current(world) {
        return Err(rejected(
            "Validation owner or policy retired before publication",
        ));
    }
    let prepared = match outcome {
        Ok(prepared) => prepared,
        Err(diagnostic) => {
            return Ok(PreparationPoll::Failed {
                operation_id: id,
                diagnostic,
            });
        }
    };
    let workspace = world.get_resource::<WorkspaceResource>();
    let replication = lunco_core_session::session::current_replication_owner_in(world);
    if prepared.source_runtime.as_ref().is_some_and(|owner| {
        !owner.is_current(
            workspace.map(|workspace| &**workspace),
            replication.as_ref(),
        )
    }) || prepared.mounts.iter().any(|(name, root)| {
        world
            .get_resource::<TwinRoots>()
            .is_none_or(|roots| roots.root_for(name).ok().flatten().as_ref() != Some(root))
    }) {
        return Err(rejected(
            "Validation source owner retired before publication",
        ));
    }
    return match prepared.facts {
        PreparedFacts::Asset { report } => {
            let revisions = lunco_api_core::api_value_from_serializable(&prepared.revisions)?;
            Ok(PreparationPoll::Ready {
                operation_id: id,
                report,
                params: operation
                    .params
                    .ok_or_else(|| rejected("Non-query operation returned file facts"))?,
                revisions,
            })
        }
        PreparedFacts::Twin(result) => Ok(PreparationPoll::ReadyTwin {
            operation_id: id,
            result,
            permit,
        }),
    };
}

pub(crate) fn admit_twin_lint(
    world: &World,
    reference: &str,
    policy: &str,
    context: lunco_hooks::RuntimeExecutionContext,
) -> Result<u64, ApiQueryError> {
    admit_request(
        world,
        QueryKind::TwinLint,
        reference.to_owned(),
        policy.to_owned(),
        None,
        context,
    )
}

fn admit_request(
    world: &World,
    kind: QueryKind,
    reference: String,
    policy: String,
    params: Option<ApiValue>,
    context: lunco_hooks::RuntimeExecutionContext,
) -> Result<u64, ApiQueryError> {
    let preparations = world
        .get_resource::<QueryPreparations>()
        .ok_or_else(|| rejected("Validation preparation owner is unavailable"))?;
    let mut state = preparations
        .0
        .lock()
        .map_err(|_| rejected("Validation preparation state is poisoned"))?;
    state
        .operations
        .retain(|_, operation| operation.fence.lifetime_current(world));
    let mut fence = capture_fence(world, &reference, kind)?;
    let source = admit_source(world, &reference, kind, &mut fence)?;
    let prepared_policy = PolicyPreparation {
        kind,
        context,
        policies: fence.policies.clone(),
    };
    let next = state
        .next
        .checked_add(1)
        .ok_or_else(|| rejected("Validation operation identity exhausted"))?;
    let admission = world
        .get_resource::<AsyncWorkAdmission>()
        .ok_or_else(|| rejected("Shared async admission is unavailable"))?;
    let roots = world.get_resource::<TwinRoots>().cloned();
    let server = world.get_resource::<AssetServer>().cloned();
    let policy = crate::twin_lint::policy_name(&policy)
        .map_err(rejected)?
        .to_owned();
    let limits = world
        .get_resource::<QueryPreparationLimits>()
        .ok_or_else(|| rejected("Validation resource budget is unavailable"))?
        .0;
    let key = AsyncWorkKey::new(AsyncWorkKind::ValidationPreparation, 0, 0, 0, next);
    let work = catch_preparation(prepare(
        reference.clone(),
        source,
        roots,
        server,
        policy,
        limits,
        prepared_policy,
    ));
    #[cfg(not(target_arch = "wasm32"))]
    let task = {
        let (sender, result) = std::sync::mpsc::sync_channel(1);
        let lifetime = Arc::new(());
        let alive = Arc::downgrade(&lifetime);
        let request = admission
            .submit_transport(AsyncWorkPriority::Interactive, key, move |permit| {
                if alive.upgrade().is_none() {
                    return;
                }
                let outcome = future::block_on(work);
                let _ = sender.send((outcome, Arc::new(permit)));
            })
            .map_err(|error| {
                rejected(format!(
                    "Validation preparation was not admitted: {error:?}"
                ))
            })?;
        NativePreparation {
            result,
            _request: request,
            _lifetime: lifetime,
        }
    };
    #[cfg(target_arch = "wasm32")]
    let (task, permit) = {
        let permit = Arc::new(
            admission
                .admit_external(AsyncWorkPriority::Interactive, key)
                .map_err(|error| {
                    rejected(format!(
                        "Validation preparation was not admitted: {error:?}"
                    ))
                })?,
        );
        let pool = AsyncComputeTaskPool::try_get()
            .ok_or_else(|| rejected("Validation task pool is unavailable"))?;
        let running_permit = Arc::clone(&permit);
        let task = pool.spawn(async move {
            let _running_permit = running_permit;
            work.await
        });
        (task, permit)
    };
    state.next = next;
    state.operations.insert(
        next,
        Operation {
            kind,
            params,
            fence,
            task,
            #[cfg(target_arch = "wasm32")]
            _permit: permit,
        },
    );
    Ok(next)
}
pub(crate) fn cancel(world: &World, id: u64) {
    if let Some(preparations) = world.get_resource::<QueryPreparations>() {
        preparations
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .operations
            .remove(&id);
    }
}

pub(crate) fn pending(id: u64) -> ApiValue {
    envelope(id, "pending", ApiValue::Unit, ApiValue::Unit)
}

pub(crate) fn report_envelope(id: u64, report: ApiValue, revisions: ApiValue) -> ApiValue {
    envelope(id, "ready", report, revisions)
}

async fn catch_preparation(
    future: impl std::future::Future<Output = Result<Prepared, String>>,
) -> Result<Prepared, String> {
    let mut future = std::pin::pin!(future);
    std::future::poll_fn(|context| {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            future.as_mut().poll(context)
        })) {
            Ok(poll) => poll,
            Err(_) => std::task::Poll::Ready(Err(
                "Validation preparation panicked before publication".to_owned(),
            )),
        }
    })
    .await
}

enum Source {
    Logical(AssetPath<'static>),
    Native {
        path: PathBuf,
        admission: FileDocumentAdmission,
        scope: Option<NativeReadScope>,
    },
    MountedTwin {
        authority: String,
        input: TwinInspectionInput,
    },
    NativeTwin {
        path: PathBuf,
        admission: FileDocumentAdmission,
    },
    #[cfg(feature = "sysml-runtime")]
    SysmlAnalysis(Arc<lunco_sysml_ast::SysmlAnalysis>),
}

/// Captured native composition scope. Authored source identities remain
/// separate from the runtime mount fenced by the admitting owner.
struct NativeReadScope {
    root: PathBuf,
    logical_identity: String,
    scope: lunco_assets_core::TwinReadScope,
}

fn native_read_scope(
    world: &World,
    fence: &OwnerFence,
) -> Result<Option<NativeReadScope>, ApiQueryError> {
    fence
        .mount
        .as_ref()
        .map(|(authority, root)| {
            let roots = world.get_resource::<TwinRoots>().ok_or_else(|| {
                rejected("Native source scope requires its admitted Twin registry")
            })?;
            Ok(NativeReadScope {
                root: root.clone(),
                logical_identity: roots
                    .logical_identity_for_authority(authority)
                    .map_err(|error| rejected(error.to_string()))?,
                scope: roots
                    .read_scope(authority)
                    .map_err(|error| rejected(error.to_string()))?,
            })
        })
        .transpose()
}

fn admit_source(
    world: &World,
    reference: &str,
    kind: QueryKind,
    fence: &mut OwnerFence,
) -> Result<Source, ApiQueryError> {
    if matches!(kind, QueryKind::Twin | QueryKind::TwinLint) {
        if lunco_assets_core::parse_twin_root_uri(reference).is_some() {
            let (authority, root) = fence.mount.as_ref().expect("Twin fence captured").clone();
            let workspace = world
                .get_resource::<WorkspaceResource>()
                .ok_or_else(|| rejected("Mounted Twin validation requires the Workspace"))?;
            let (id, twin) = workspace
                .twins()
                .find(|(_, twin)| twin.root == root)
                .ok_or_else(|| rejected("Twin mount has no indexed Workspace owner"))?;
            let input = TwinInspectionInput::capture(twin);
            fence.twin_input = Some((id, input.clone()));
            return Ok(Source::MountedTwin { authority, input });
        }
        #[cfg(target_arch = "wasm32")]
        return Err(rejected(
            "Browser ValidateTwin requires a mounted stable twin:// source URI",
        ));
    }
    let workspace = world.get_resource::<WorkspaceResource>();
    let replication = lunco_core_session::session::current_replication_owner_in(world);
    let admission = FileDocumentAdmission::capture(
        workspace.map(|workspace| &**workspace),
        replication.as_ref(),
    );
    if matches!(kind, QueryKind::Twin | QueryKind::TwinLint) {
        let path = match lunco_storage::file_uri_to_path(reference)
            .map_err(|error| rejected(error.to_string()))?
        {
            Some(path) => path,
            None if !reference.contains("://") => PathBuf::from(reference),
            None => {
                return Err(rejected(
                    "Twin directory validation requires a native path, standard file URI, or mounted stable twin:// source URI",
                ));
            }
        };
        return Ok(Source::NativeTwin { path, admission });
    }
    if matches!(kind, QueryKind::Sysml | QueryKind::AnalyzeSysml) {
        if let Some(name) = lunco_assets_core::parse_twin_root_uri(reference) {
            #[cfg(feature = "sysml-runtime")]
            {
                let (id, twin, _, _) = crate::validate::mounted_sysml_twin(world, name, reference)
                    .map_err(|report| rejected(report.errors.join("; ")))?;
                let analyses = world
                    .get_resource::<lunco_sysml::TwinSysmlAnalyses>()
                    .ok_or_else(|| {
                        rejected("Mounted Twin SysML preparation owner is unavailable")
                    })?;
                let analysis = crate::validate::prepared_twin_sysml_analysis(
                    analyses, name, reference, id, twin,
                )
                .map_err(|report| rejected(report.errors.join("; ")))?;
                fence.sysml_analysis = Some((
                    name.to_owned(),
                    id,
                    twin.root.clone(),
                    Arc::clone(&analysis),
                ));
                return Ok(Source::SysmlAnalysis(analysis));
            }
            #[cfg(not(feature = "sysml-runtime"))]
            {
                let _ = name;
                return Err(rejected(
                    "Mounted Twin SysML validation requires its asynchronous SysML preparation owner",
                ));
            }
        }
        let native = lunco_storage::file_uri_to_path(reference)
            .map_err(|error| rejected(error.to_string()))?;
        let path = native
            .as_deref()
            .unwrap_or_else(|| std::path::Path::new(reference));
        if !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                matches!(extension.to_ascii_lowercase().as_str(), "sysml" | "kerml")
            })
        {
            return Err(rejected(
                "SysML source validation requires .sysml or .kerml",
            ));
        }
    }
    if let Some(path) =
        lunco_storage::file_uri_to_path(reference).map_err(|error| rejected(error.to_string()))?
    {
        return Ok(Source::Native {
            path,
            admission,
            scope: native_read_scope(world, fence)?,
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    if !reference.contains("://") {
        return Ok(Source::Native {
            path: PathBuf::from(reference),
            admission,
            scope: native_read_scope(world, fence)?,
        });
    }
    let load_reference = match lunco_assets_core::parse_twin_uri(reference) {
        Some((_, relative)) => {
            let (authority, _) = fence
                .mount
                .as_ref()
                .ok_or_else(|| rejected("Twin file admission has no captured source mount"))?;
            lunco_assets_core::twin_uri(authority, relative)
        }
        None => reference.to_owned(),
    };
    let path = lunco_assets_core::asset_path::load_asset_path(
        &load_reference,
        None,
        world.get_resource::<TwinRoots>(),
        None,
    )
    .map_err(|error| rejected(error.to_string()))?;
    Ok(Source::Logical(path))
}

async fn prepare(
    reference: String,
    source: Source,
    roots: Option<TwinRoots>,
    server: Option<AssetServer>,
    policy: String,
    limits: lunco_usd_compose::recipe::StageClosureLimits,
    prepared_policy: PolicyPreparation,
) -> Result<Prepared, String> {
    let limit = limits.max_bytes;
    match source {
        #[cfg(feature = "sysml-runtime")]
        Source::SysmlAnalysis(analysis) => {
            let text = analysis
                .files()
                .iter()
                .map(|file| file.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            let revisions = analysis
                .files()
                .iter()
                .map(|file| ReadRevision {
                    path: file.name.clone(),
                    cid: lunco_hash::content::cid(file.text.as_bytes()).to_string(),
                })
                .collect();
            let report = prepared_policy.finish_asset(
                crate::validate::finish_sysml_report(&reference, analysis),
                &text,
            );
            Ok(Prepared {
                facts: PreparedFacts::Asset { report },
                source_runtime: None,
                revisions,
                mounts: Vec::new(),
            })
        }
        Source::MountedTwin { authority, input } => {
            let server = server.ok_or("Registered asset server is unavailable")?;
            let prepared = prepare_twin_sources(
                &input,
                Some(&authority),
                None,
                roots.as_ref(),
                &server,
                limits,
            )
            .await?;
            Ok(Prepared {
                facts: PreparedFacts::Twin(prepared_policy.finish_twin(
                    &reference,
                    &policy,
                    inspect_prepared_twin(&input, &prepared.sources),
                    input.registry_errors,
                    &prepared.revisions,
                )?),
                source_runtime: None,
                revisions: prepared.revisions,
                mounts: prepared.mounts,
            })
        }
        Source::NativeTwin { path, admission } => {
            #[cfg(not(target_arch = "wasm32"))]
            {
                let resolved = admission.clone().resolve(&path)?;
                let twin = match lunco_twin::TwinMode::open(&resolved.path)
                    .map_err(|error| error.to_string())?
                {
                    lunco_twin::TwinMode::Twin(twin) | lunco_twin::TwinMode::Folder(twin) => twin,
                    _ => return Err("Twin validation requires a directory".to_owned()),
                };
                let mut input = TwinInspectionInput::capture(&twin);
                if let Err(errors) = twin.discover_sysml_sources_checked() {
                    input.registry_errors.extend(
                        errors
                            .into_iter()
                            .map(|error| format!("Twin SysML source set: {error}")),
                    );
                }
                let prepared = prepare_twin_sources(
                    &input,
                    None,
                    Some(admission),
                    roots.as_ref(),
                    &server.ok_or("Asset server is unavailable")?,
                    limits,
                )
                .await?;
                return Ok(Prepared {
                    facts: PreparedFacts::Twin(prepared_policy.finish_twin(
                        &reference,
                        &policy,
                        inspect_prepared_twin(&input, &prepared.sources),
                        input.registry_errors,
                        &prepared.revisions,
                    )?),
                    source_runtime: Some(resolved.runtime),
                    revisions: prepared.revisions,
                    mounts: prepared.mounts,
                });
            }
            #[cfg(target_arch = "wasm32")]
            {
                let _ = (path, admission);
                Err("Browser Twin validation requires an admitted indexed mount".to_owned())
            }
        }
        Source::Logical(path) => {
            let server = server.ok_or("Registered asset server is unavailable")?;
            let mut bytes = lunco_assets_core::registered_read::read_registered_asset_bytes(
                &server, &path, limit,
            )
            .await
            .map_err(|error| error.to_string())?;
            let id = lunco_assets_core::asset_path::anchor_of(&path);
            let recipe = if crate::validate::is_usd_path(path.path()) {
                Some(
                    lunco_usd_bevy_stage::compose::fetch_layer_closure_from_asset_reader(
                        &server,
                        path.clone(),
                        std::mem::take(&mut bytes),
                        roots.as_ref(),
                        limits,
                    )
                    .await
                    .map_err(|error| error.to_string())?,
                )
            } else {
                None
            };
            let (revisions, mounts) = read_identities(
                &id,
                &bytes,
                recipe.as_ref(),
                ReadIdentityScope::Registered {
                    origin: &path,
                    roots: roots.as_ref(),
                },
            )?;
            let (report, text) =
                crate::validate::validate_prepared_bytes(&reference, path.path(), bytes, recipe);
            Ok(Prepared {
                facts: PreparedFacts::Asset {
                    report: prepared_policy.finish_asset(report, &text),
                },
                source_runtime: None,
                revisions,
                mounts,
            })
        }
        Source::Native {
            path,
            admission,
            scope,
        } => {
            #[cfg(not(target_arch = "wasm32"))]
            let path = if path.is_absolute() {
                path
            } else {
                lunco_assets_core::resolve_asset_input(
                    &reference,
                    lunco_storage::StorageEntryKind::File,
                )
                .map_err(|error| error.to_string())?
            };
            let (resolved, mut bytes) = admission.read_bounded(&path, limit).await?;
            let scope = scope
                .as_ref()
                .filter(|scope| resolved.path.starts_with(&scope.root));
            let id = lunco_storage::file_path_to_uri(&resolved.path)
                .map_err(|error| error.to_string())?;
            let recipe = if crate::validate::is_usd_path(&resolved.path) {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    Some(
                        lunco_usd_compose::recipe_from_bytes_with_roots(
                            &id,
                            std::mem::take(&mut bytes),
                            Some(
                                crate::validate::engine_assets_root()
                                    .map_err(|error| error.to_string())?
                                    .as_path(),
                            ),
                            scope.map(|scope| &scope.scope),
                            limits,
                        )
                        .map_err(|error| error.to_string())?,
                    )
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let roots = roots
                        .as_ref()
                        .ok_or("USD file validation requires the current registered asset mount")?;
                    let mut logical = None;
                    for name in roots.authorities().map_err(|error| error.to_string())? {
                        if let Some(root) =
                            roots.root_for(&name).map_err(|error| error.to_string())?
                        {
                            if let Ok(relative) = resolved.path.strip_prefix(root) {
                                logical = Some(lunco_assets_core::twin_uri(&name, relative));
                                break;
                            }
                        }
                    }
                    let logical = logical.ok_or("USD file has no admitted async asset source; use its logical lunco:// or twin:// address")?;
                    let path = lunco_assets_core::asset_path::load_asset_path(
                        &logical,
                        None,
                        Some(roots),
                        None,
                    )
                    .map_err(|error| error.to_string())?;
                    Some(
                        lunco_usd_bevy_stage::compose::fetch_layer_closure_from_asset_reader(
                            &server.ok_or("Registered asset server is unavailable")?,
                            path,
                            std::mem::take(&mut bytes),
                            Some(roots),
                            limits,
                        )
                        .await
                        .map_err(|error| error.to_string())?,
                    )
                }
            } else {
                None
            };
            #[cfg(not(target_arch = "wasm32"))]
            let identity_scope = ReadIdentityScope::Native {
                logical_identity: scope.map(|scope| scope.logical_identity.as_str()),
            };
            #[cfg(target_arch = "wasm32")]
            let origin = recipe
                .as_ref()
                .map(|recipe| {
                    lunco_assets_core::asset_path::load_asset_path(
                        &recipe.root_id,
                        None,
                        roots.as_ref(),
                        None,
                    )
                    .map_err(|error| error.to_string())
                })
                .transpose()?;
            #[cfg(target_arch = "wasm32")]
            let identity_scope = match origin.as_ref() {
                Some(origin) => ReadIdentityScope::Registered {
                    origin,
                    roots: roots.as_ref(),
                },
                None => ReadIdentityScope::Native {
                    logical_identity: scope.map(|scope| scope.logical_identity.as_str()),
                },
            };
            let (revisions, mounts) =
                read_identities(&id, &bytes, recipe.as_ref(), identity_scope)?;
            let (report, text) =
                crate::validate::validate_prepared_bytes(&reference, &resolved.path, bytes, recipe);
            Ok(Prepared {
                facts: PreparedFacts::Asset {
                    report: prepared_policy.finish_asset(report, &text),
                },
                source_runtime: Some(resolved.runtime),
                revisions,
                mounts,
            })
        }
    }
}

enum ReadIdentityScope<'a> {
    Registered {
        origin: &'a AssetPath<'static>,
        roots: Option<&'a TwinRoots>,
    },
    Native {
        logical_identity: Option<&'a str>,
    },
}

fn read_identities(
    root_id: &str,
    bytes: &[u8],
    recipe: Option<&lunco_usd_compose::recipe::StageRecipe>,
    scope: ReadIdentityScope<'_>,
) -> Result<(Vec<ReadRevision>, Vec<(String, PathBuf)>), String> {
    let mut revisions = if let Some(recipe) = recipe {
        recipe
            .bytes
            .iter()
            .map(|(path, bytes)| ReadRevision {
                path: path.clone(),
                cid: lunco_hash::content::cid(bytes).to_string(),
            })
            .collect::<Vec<_>>()
    } else {
        vec![ReadRevision {
            path: root_id.to_owned(),
            cid: lunco_hash::content::cid(bytes).to_string(),
        }]
    };
    revisions.sort_by(|left, right| left.path.cmp(&right.path));
    let mut mounts = Vec::new();
    for revision in &revisions {
        if let Some((identity, _)) = lunco_assets_core::parse_twin_uri(&revision.path) {
            let (origin, roots) = match &scope {
                ReadIdentityScope::Native { logical_identity } => {
                    if *logical_identity != Some(identity) {
                        return Err(format!(
                            "Twin source `{identity}` is outside the admitted native scope"
                        ));
                    }
                    continue;
                }
                ReadIdentityScope::Registered { origin, roots } => (
                    *origin,
                    roots.ok_or("Registered Twin dependency has no registry")?,
                ),
            };
            let address = lunco_assets_core::asset_path::load_asset_path(
                &revision.path,
                Some(origin),
                Some(roots),
                None,
            )
            .map_err(|error| error.to_string())?;
            let address = lunco_assets_core::asset_path::anchor_of(&address);
            let (authority, _) = lunco_assets_core::parse_twin_uri(&address)
                .ok_or("Registered Twin dependency lost its load authority")?;
            let root = roots
                .root_for(authority)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Twin source mount retired during preparation".to_owned())?;
            if !mounts.iter().any(|(name, _)| name == authority) {
                mounts.push((authority.to_owned(), root));
            }
        }
    }
    Ok((revisions, mounts))
}
enum PreparedTwinSource {
    Modelica(String),
    Usd {
        entries: Vec<crate::twin_lint::NamespaceEntry>,
        errors: Vec<String>,
    },
}
struct PreparedTwinSources {
    sources: HashMap<PathBuf, Result<PreparedTwinSource, String>>,
    revisions: Vec<ReadRevision>,
    mounts: Vec<(String, PathBuf)>,
}

fn retain_read_revisions(
    retained: &mut HashMap<String, String>,
    revisions: impl IntoIterator<Item = ReadRevision>,
) -> Result<(), String> {
    for revision in revisions {
        match retained.entry(revision.path) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(revision.cid);
            }
            std::collections::hash_map::Entry::Occupied(entry) if entry.get() != &revision.cid => {
                return Err(format!(
                    "Source `{}` changed during Twin preparation",
                    entry.key()
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

async fn prepare_twin_sources(
    input: &TwinInspectionInput,
    authority: Option<&str>,
    admission: Option<FileDocumentAdmission>,
    roots: Option<&TwinRoots>,
    server: &AssetServer,
    limits: lunco_usd_compose::recipe::StageClosureLimits,
) -> Result<PreparedTwinSources, String> {
    if input.files.len() > limits.max_layers {
        return Err(
            "Twin indexed source count exceeds the shared closure preparation budget".to_owned(),
        );
    }
    // USD bytes live only through this source's inspection. The persistent
    // source-byte allowance is occupied only by retained Modelica text.
    let mut retained_bytes = 0_usize;
    let mut read_revisions = HashMap::<String, String>::new();
    let mut prepared = PreparedTwinSources {
        sources: HashMap::new(),
        revisions: Vec::new(),
        mounts: Vec::new(),
    };
    for relative in &input.files {
        let usd = crate::validate::is_usd_path(relative);
        if !usd
            && relative
                .extension()
                .and_then(|extension| extension.to_str())
                != Some("mo")
        {
            continue;
        }
        let result = async {
            let (id, bytes, recipe) = if let Some(authority) = authority {
                let id = lunco_assets_core::twin_uri(authority, relative);
                let path = lunco_assets_core::asset_path::load_asset_path(&id, None, roots, None)
                    .map_err(|error| error.to_string())?;
                let mut bytes = lunco_assets_core::registered_read::read_registered_asset_bytes(
                    server,
                    &path,
                    limits.max_bytes.saturating_sub(retained_bytes),
                )
                .await
                .map_err(|error| error.to_string())?;
                let recipe = if usd {
                    Some(
                        lunco_usd_bevy_stage::compose::fetch_layer_closure_from_asset_reader(
                            server,
                            path,
                            std::mem::take(&mut bytes),
                            roots,
                            lunco_usd_compose::recipe::StageClosureLimits {
                                max_bytes: limits.max_bytes.saturating_sub(retained_bytes),
                                ..limits
                            },
                        )
                        .await
                        .map_err(|error| error.to_string())?,
                    )
                } else {
                    None
                };
                (id, bytes, recipe)
            } else {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let (resolved, mut bytes) = admission
                        .clone()
                        .ok_or("Native Twin source admission is unavailable")?
                        .read_bounded(
                            &input.root.join(relative),
                            limits.max_bytes.saturating_sub(retained_bytes),
                        )
                        .await?;
                    let id = lunco_storage::file_path_to_uri(&resolved.path)
                        .map_err(|error| error.to_string())?;
                    let recipe = if usd {
                        Some(
                            lunco_usd_compose::recipe_from_bytes_with_roots(
                                &id,
                                std::mem::take(&mut bytes),
                                Some(
                                    crate::validate::engine_assets_root()
                                        .map_err(|error| error.to_string())?
                                        .as_path(),
                                ),
                                Some(
                                    &lunco_assets_core::TwinReadScope::declared_with_routes(
                                        &input.source_name,
                                        &input.root,
                                        lunco_assets_datasets::DatasetRegistry::read_twin_asset_routes(
                                            &lunco_assets_datasets::DatasetScope::Twin {
                                                source_identity: input.source_name.clone(),
                                                authority: None,
                                                display_name: input.source_name.clone(),
                                                root: input.root.clone(),
                                            },
                                        )
                                        .map_err(|error| error.to_string())?,
                                    )
                                    .map_err(|error| error.to_string())?,
                                ),
                                lunco_usd_compose::recipe::StageClosureLimits {
                                    max_bytes: limits.max_bytes.saturating_sub(retained_bytes),
                                    ..limits
                                },
                            )
                            .map_err(|error| error.to_string())?,
                        )
                    } else {
                        None
                    };
                    (id, bytes, recipe)
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let _ = &admission;
                    return Err("Native Twin sources cannot enter browser preparation".to_owned());
                }
            };
            let origin = authority
                .map(|_| {
                    lunco_assets_core::asset_path::load_asset_path(&id, None, roots, None)
                        .map_err(|error| error.to_string())
                })
                .transpose()?;
            let scope = match origin.as_ref() {
                Some(origin) => ReadIdentityScope::Registered { origin, roots },
                None => ReadIdentityScope::Native {
                    logical_identity: Some(&input.source_name),
                },
            };
            let (revisions, mounts) = read_identities(&id, &bytes, recipe.as_ref(), scope)?;
            retain_read_revisions(&mut read_revisions, revisions)?;
            for mount in mounts {
                if !prepared.mounts.contains(&mount) {
                    prepared.mounts.push(mount);
                }
            }
            match recipe {
                Some(recipe) => {
                    let mut entries = Vec::new();
                    let mut errors = recipe
                        .dependency_diagnostics
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>();
                    match lunco_usd_bevy_stage::canonical::CanonicalStage::from_recipe(&recipe) {
                        Ok(stage) => {
                            crate::twin_lint::inspect_usd_stage(&stage, relative, &mut entries)
                        }
                        Err(error) => errors.push(format!("{}: {error}", relative.display())),
                    }
                    Ok(PreparedTwinSource::Usd { entries, errors })
                }
                None => {
                    let text = String::from_utf8(bytes).map_err(|error| error.to_string())?;
                    retained_bytes = retained_bytes
                        .checked_add(text.len())
                        .ok_or("Twin source byte budget overflow")?;
                    if retained_bytes > limits.max_bytes {
                        return Err(
                            "Twin source text exceeds the shared preparation byte budget"
                                .to_owned(),
                        );
                    }
                    Ok(PreparedTwinSource::Modelica(text))
                }
            }
        }
        .await;
        prepared.sources.insert(relative.clone(), result);
    }
    prepared.revisions = read_revisions
        .into_iter()
        .map(|(path, cid)| ReadRevision { path, cid })
        .collect();
    prepared
        .revisions
        .sort_by(|left, right| (&left.path, &left.cid).cmp(&(&right.path, &right.cid)));
    Ok(prepared)
}
fn inspect_prepared_twin(
    input: &TwinInspectionInput,
    sources: &HashMap<PathBuf, Result<PreparedTwinSource, String>>,
) -> TwinNamespaceSnapshot {
    crate::twin_lint::inspect_input(
        input,
        |relative| match sources.get(relative) {
            Some(Ok(PreparedTwinSource::Modelica(text))) => Ok(text.clone()),
            Some(Ok(PreparedTwinSource::Usd { .. })) => {
                Err("USD facts have no Modelica source text".to_owned())
            }
            Some(Err(error)) => Err(error.clone()),
            None => Err("Indexed source was not prepared".to_owned()),
        },
        |relative, entries, errors| match sources.get(relative) {
            Some(Ok(PreparedTwinSource::Usd {
                entries: prepared_entries,
                errors: prepared_errors,
            })) => {
                entries.extend(prepared_entries.iter().cloned());
                errors.extend(prepared_errors.iter().cloned());
            }
            Some(Ok(PreparedTwinSource::Modelica(_))) => {
                errors.push(format!(
                    "{}: Modelica source has no USD facts",
                    relative.display()
                ));
            }
            Some(Err(error)) => {
                errors.push(format!("{}: {error}", relative.display()));
            }
            None => {
                errors.push(format!(
                    "{}: indexed USD source was not prepared",
                    relative.display()
                ));
            }
        },
    )
}

pub(crate) fn retire(world: &mut World) {
    if let Some(preparations) = world.get_resource::<QueryPreparations>() {
        preparations
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .operations
            .retain(|_, operation| operation.fence.lifetime_current(world));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_revisions_are_unique_and_reject_changed_content() {
        let mut retained = HashMap::new();
        let original = lunco_hash::content::cid(b"original").to_string();
        let revision = || ReadRevision {
            path: "part.usda".to_owned(),
            cid: original.clone(),
        };
        retain_read_revisions(&mut retained, [revision(), revision()]).unwrap();
        assert_eq!(retained.len(), 1);
        assert_eq!(retained.get("part.usda"), Some(&original));
        let error = retain_read_revisions(
            &mut retained,
            [ReadRevision {
                path: "part.usda".to_owned(),
                cid: lunco_hash::content::cid(b"changed").to_string(),
            }],
        )
        .unwrap_err();
        assert_eq!(error, "Source `part.usda` changed during Twin preparation");
        assert_eq!(retained.get("part.usda"), Some(&original));
    }

    #[test]
    fn source_revisions_preserve_actual_bytes() {
        let (revisions, mounts) = read_identities(
            "payload.rhai",
            b"let value = 1;",
            None,
            ReadIdentityScope::Native {
                logical_identity: None,
            },
        )
        .unwrap();
        assert_eq!(revisions.len(), 1);
        assert_eq!(
            revisions[0].cid,
            lunco_hash::content::cid(b"let value = 1;").to_string()
        );
        assert!(mounts.is_empty());
        assert_ne!(
            revisions[0].cid,
            lunco_hash::content::cid(b"let value = 2;").to_string()
        );
    }

    #[test]
    fn preparation_panics_return_terminal_errors() {
        let outcome = future::block_on(catch_preparation(async {
            panic!("preparation seam panic")
        }));
        assert!(matches!(outcome, Err(error) if error.contains("panicked before publication")));
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn logical_revisions_retain_their_native_or_registered_source_scope() {
        let logical = "twin://preparation/part.usda";
        let recipe = lunco_usd_compose::recipe::StageRecipe::new(
            logical.to_owned(),
            HashMap::from([(logical.to_owned(), b"#usda 1.0".to_vec())]),
        );
        let (revisions, mounts) = read_identities(
            logical,
            &[],
            Some(&recipe),
            ReadIdentityScope::Native {
                logical_identity: Some("preparation"),
            },
        )
        .unwrap();
        assert_eq!(revisions[0].path, logical);
        assert!(mounts.is_empty());
        assert!(
            read_identities(
                logical,
                &[],
                Some(&recipe),
                ReadIdentityScope::Native {
                    logical_identity: Some("foreign")
                }
            )
            .is_err()
        );

        let folder = tempfile::tempdir().unwrap();
        let roots = TwinRoots::default();
        let authority = roots
            .register_authority("preparation", folder.path())
            .unwrap();
        let origin = lunco_assets_core::asset_path::load_asset_path(
            &lunco_assets_core::twin_uri(&authority, "root.usda"),
            None,
            Some(&roots),
            None,
        )
        .unwrap();
        let (revisions, mounts) = read_identities(
            logical,
            &[],
            Some(&recipe),
            ReadIdentityScope::Registered {
                origin: &origin,
                roots: Some(&roots),
            },
        )
        .unwrap();
        assert_eq!(revisions[0].path, logical);
        assert_eq!(
            mounts,
            vec![(
                authority.clone(),
                roots.root_for(&authority).unwrap().unwrap()
            )]
        );
        roots.unregister_authority(&authority).unwrap();
        roots
            .register_authority("preparation", folder.path())
            .unwrap();
        assert!(
            read_identities(
                logical,
                &[],
                Some(&recipe),
                ReadIdentityScope::Registered {
                    origin: &origin,
                    roots: Some(&roots)
                }
            )
            .is_err()
        );
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn retired_mount_cannot_publish_into_reopened_root() {
        let folder = tempfile::tempdir().unwrap();
        let roots = TwinRoots::default();
        let authority = roots
            .register_authority("preparation", folder.path())
            .unwrap();
        let mut world = World::new();
        world.insert_resource(roots.clone());
        let reference = "twin://preparation/source.usda";
        let mut fence = capture_fence(&world, reference, QueryKind::Asset).unwrap();
        let source = admit_source(&world, reference, QueryKind::Asset, &mut fence).unwrap();
        let Source::Logical(path) = source else {
            panic!("Twin file must use its registered source");
        };
        assert_eq!(
            lunco_assets_core::asset_path::anchor_of(&path),
            lunco_assets_core::twin_uri(&authority, "source.usda")
        );
        assert!(fence.is_current(&world));
        roots.unregister_authority(&authority).unwrap();
        let replacement = roots
            .register_authority("preparation", folder.path())
            .unwrap();
        assert_ne!(replacement, authority);
        assert!(!fence.is_current(&world));
    }
}
