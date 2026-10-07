//! One-shot query preparation. Source I/O and pure facts use the existing Bevy
//! task/reader boundary; admitted policy executes serially when consumed.

use crate::{
    twin_lint::{TwinInspectionInput, TwinNamespaceSnapshot},
    validate::ValidationReport,
};
use bevy::{
    asset::{AssetPath, AssetServer},
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures_lite::future},
};
use lunco_api::ApiQueryError;
use lunco_api_core::{ApiErrorCode, ApiValue, api_value};
use lunco_assets_core::TwinRoots;
use lunco_core_runtime::async_work::ExternalWorkPermit;
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
    reference: String,
    fence: OwnerFence,
    task: Task<Result<Prepared, String>>,
    _permit: Arc<ExternalWorkPermit>,
}
#[derive(Clone)]
struct OwnerFence {
    runtime: DocumentRuntimeOwner,
    mount: Option<(String, PathBuf)>,
    twin_input: Option<(lunco_workspace::TwinId, TwinInspectionInput)>,
    policies: Vec<(&'static str, Option<Arc<lunco_hooks::RegisteredHook>>)>,
}
impl OwnerFence {
    fn lifetime_current(&self, world: &World) -> bool {
        let workspace = world.get_resource::<WorkspaceResource>();
        let replication = lunco_core_session::session::current_replication_owner_in(world);
        self.runtime.is_current(
            workspace.map(|workspace| &**workspace),
            replication.as_ref(),
        ) && self.mount.as_ref().is_none_or(|(authority, root)| {
            world
                .get_resource::<TwinRoots>()
                .is_some_and(|roots| roots.root_of(authority).ok().flatten().as_ref() == Some(root))
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
            && self.twin_input.as_ref().is_none_or(|(id, input)| {
                world
                    .get_resource::<WorkspaceResource>()
                    .and_then(|workspace| workspace.twin(*id))
                    .is_some_and(|twin| TwinInspectionInput::capture(twin) == *input)
            })
    }
}

#[derive(serde::Serialize)]
pub(crate) struct ReadRevision {
    path: String,
    cid: String,
}
enum PreparedFacts {
    Asset {
        report: ValidationReport,
        text: String,
    },
    Twin {
        snapshot: TwinNamespaceSnapshot,
        registry_errors: Vec<String>,
        policy: String,
    },
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
        snapshot: TwinNamespaceSnapshot,
        registry_errors: Vec<String>,
        policy: String,
        reference: String,
        revisions: Vec<ReadRevision>,
        permit: Arc<ExternalWorkPermit>,
    },
    Failed {
        operation_id: u64,
        diagnostic: String,
    },
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
fn twin_authority(reference: &str) -> Option<&str> {
    reference
        .strip_prefix("twin://")
        .map(|rest| rest.split('/').next().unwrap_or(rest))
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
    if let Some(authority) = twin_authority(reference) {
        let roots = world
            .get_resource::<TwinRoots>()
            .ok_or_else(|| rejected("Twin asset registry is unavailable"))?;
        let root = roots
            .root_of(authority)
            .map_err(|error| rejected(error.to_string()))?
            .ok_or_else(|| rejected(format!("Twin mount `{authority}` is retired or unknown")))?;
        mount = Some((authority.to_owned(), root));
    } else if let DocumentRuntimeOwner::LocalTwin(id) = &runtime {
        if let Some(twin) = workspace.and_then(|workspace| workspace.twin(*id)) {
            let roots = world
                .get_resource::<TwinRoots>()
                .ok_or_else(|| rejected("Active Twin asset registry is unavailable"))?;
            let authority = roots
                .names()
                .map_err(|error| rejected(error.to_string()))?
                .into_iter()
                .find(|name| roots.root_of(name).ok().flatten().as_ref() == Some(&twin.root))
                .ok_or_else(|| rejected("Active Twin has no current asset mount"))?;
            mount = Some((authority, twin.root.clone()));
        }
    }
    let domains: &[&'static str] = if matches!(kind, QueryKind::Twin | QueryKind::TwinLint) {
        &["twin"]
    } else {
        &["modelica", "usd", "sysml", "wgsl", "rhai"]
    };
    Ok(OwnerFence {
        runtime,
        mount,
        twin_input: None,
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
    let Some(outcome) = future::block_on(future::poll_once(&mut operation.task)) else {
        return Ok(PreparationPoll::Pending { operation_id: id });
    };
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
            .is_none_or(|roots| roots.root_of(name).ok().flatten().as_ref() != Some(root))
    }) {
        return Err(rejected(
            "Validation source owner retired before publication",
        ));
    }
    return match prepared.facts {
        PreparedFacts::Asset { report, text } => {
            let revisions = lunco_api_core::api_value_from_serializable(&prepared.revisions)?;
            let report = if kind == QueryKind::AnalyzeSysml {
                report
            } else {
                crate::validate::apply_lint_policy(report, &text)
            };
            Ok(PreparationPoll::Ready {
                operation_id: id,
                report,
                params: operation
                    .params
                    .ok_or_else(|| rejected("Non-query operation returned file facts"))?,
                revisions,
            })
        }
        PreparedFacts::Twin {
            snapshot,
            registry_errors,
            policy,
        } => Ok(PreparationPoll::ReadyTwin {
            operation_id: id,
            snapshot,
            registry_errors,
            policy,
            reference: operation.reference,
            revisions: prepared.revisions,
            permit: Arc::clone(&operation._permit),
        }),
    };
}

pub(crate) fn admit_twin_lint(
    world: &World,
    reference: &str,
    policy: &str,
) -> Result<u64, ApiQueryError> {
    admit_request(
        world,
        QueryKind::TwinLint,
        reference.to_owned(),
        policy.to_owned(),
        None,
    )
}

fn admit_request(
    world: &World,
    kind: QueryKind,
    reference: String,
    policy: String,
    params: Option<ApiValue>,
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
    let next = state
        .next
        .checked_add(1)
        .ok_or_else(|| rejected("Validation operation identity exhausted"))?;
    let admission = world
        .get_resource::<AsyncWorkAdmission>()
        .ok_or_else(|| rejected("Shared async admission is unavailable"))?;
    let permit = Arc::new(
        admission
            .admit_external(
                AsyncWorkPriority::Interactive,
                AsyncWorkKey::new(AsyncWorkKind::ValidationPreparation, 0, 0, 0, next),
            )
            .map_err(|error| {
                rejected(format!(
                    "Validation preparation was not admitted: {error:?}"
                ))
            })?,
    );
    let pool = AsyncComputeTaskPool::try_get()
        .ok_or_else(|| rejected("Validation task pool is unavailable"))?;
    let roots = world.get_resource::<TwinRoots>().cloned();
    let server = world.get_resource::<AssetServer>().cloned();
    let policy = crate::twin_lint::policy_name(&policy)
        .map_err(rejected)?
        .to_owned();
    let limits = world
        .get_resource::<QueryPreparationLimits>()
        .ok_or_else(|| rejected("Validation resource budget is unavailable"))?
        .0;
    let running_permit = Arc::clone(&permit);
    let source_reference = reference.clone();
    let task = pool.spawn(async move {
        let _running_permit = running_permit;
        catch_preparation(prepare(
            source_reference,
            source,
            roots,
            server,
            policy,
            limits,
        ))
        .await
    });
    state.next = next;
    state.operations.insert(
        next,
        Operation {
            kind,
            params,
            reference,
            fence,
            task,
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
    },
    MountedTwin {
        authority: String,
        input: TwinInspectionInput,
    },
    NativeTwin {
        path: PathBuf,
        admission: FileDocumentAdmission,
    },
}
fn admit_source(
    world: &World,
    reference: &str,
    kind: QueryKind,
    fence: &mut OwnerFence,
) -> Result<Source, ApiQueryError> {
    if matches!(kind, QueryKind::Twin | QueryKind::TwinLint) {
        if let Some(authority) = reference
            .strip_prefix("twin://")
            .filter(|name| !name.is_empty() && !name.contains('/'))
        {
            let root = fence.mount.as_ref().expect("Twin fence captured").1.clone();
            let workspace = world
                .get_resource::<WorkspaceResource>()
                .ok_or_else(|| rejected("Mounted Twin validation requires the Workspace"))?;
            let (id, twin) = workspace
                .twins()
                .find(|(_, twin)| twin.root == root)
                .ok_or_else(|| rejected("Twin mount has no indexed Workspace owner"))?;
            let input = TwinInspectionInput::capture(twin);
            fence.twin_input = Some((id, input.clone()));
            return Ok(Source::MountedTwin {
                authority: authority.to_owned(),
                input,
            });
        }
        #[cfg(target_arch = "wasm32")]
        return Err(rejected(
            "Browser ValidateTwin requires the current mounted twin:// authority",
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
                    "Twin directory validation requires a native path, standard file URI, or mounted twin:// authority",
                ));
            }
        };
        return Ok(Source::NativeTwin { path, admission });
    }
    if matches!(kind, QueryKind::Sysml | QueryKind::AnalyzeSysml) {
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
        return Ok(Source::Native { path, admission });
    }
    #[cfg(not(target_arch = "wasm32"))]
    if !reference.contains("://") {
        return Ok(Source::Native {
            path: PathBuf::from(reference),
            admission,
        });
    }
    let path = lunco_assets_core::asset_path::load_asset_path(
        reference,
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
) -> Result<Prepared, String> {
    let limit = limits.max_bytes;
    match source {
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
                facts: PreparedFacts::Twin {
                    snapshot: inspect_prepared_twin(&input, &prepared.sources),
                    registry_errors: input.registry_errors,
                    policy,
                },
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
                    facts: PreparedFacts::Twin {
                        snapshot: inspect_prepared_twin(&input, &prepared.sources),
                        registry_errors: input.registry_errors,
                        policy,
                    },
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
            let mut bytes =
                lunco_usd_bevy_stage::compose::read_registered_asset_bytes(&server, &path, limit)
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
            let (revisions, mounts) =
                read_identities(&id, &bytes, recipe.as_ref(), roots.as_ref())?;
            let (report, text) =
                crate::validate::validate_prepared_bytes(&reference, path.path(), bytes, recipe);
            Ok(Prepared {
                facts: PreparedFacts::Asset { report, text },
                source_runtime: None,
                revisions,
                mounts,
            })
        }
        Source::Native { path, admission } => {
            #[cfg(not(target_arch = "wasm32"))]
            let path = if path.is_file() {
                path
            } else {
                lunco_assets_core::engine_asset_local_path(&path.to_string_lossy())
                    .map_err(|error| error.to_string())?
                    .filter(|path| path.is_file())
                    .unwrap_or(path)
            };
            let (resolved, mut bytes) = admission.read_bounded(&path, limit).await?;
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
                            None,
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
                    for name in roots.names().map_err(|error| error.to_string())? {
                        if let Some(root) =
                            roots.root_of(&name).map_err(|error| error.to_string())?
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
            let (revisions, mounts) =
                read_identities(&id, &bytes, recipe.as_ref(), roots.as_ref())?;
            let (report, text) =
                crate::validate::validate_prepared_bytes(&reference, &resolved.path, bytes, recipe);
            Ok(Prepared {
                facts: PreparedFacts::Asset { report, text },
                source_runtime: Some(resolved.runtime),
                revisions,
                mounts,
            })
        }
    }
}

fn read_identities(
    root_id: &str,
    bytes: &[u8],
    recipe: Option<&lunco_usd_compose::recipe::StageRecipe>,
    roots: Option<&TwinRoots>,
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
        if let Some(authority) = twin_authority(&revision.path) {
            let root = roots
                .ok_or("Twin registry unavailable at source preparation")?
                .root_of(authority)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| {
                    format!("Twin source mount `{authority}` retired during preparation")
                })?;
            if !mounts.iter().any(|(name, _)| name == authority) {
                mounts.push((authority.to_owned(), root));
            }
        }
    }
    Ok((revisions, mounts))
}
struct PreparedTwinSource {
    text: Option<String>,
    recipe: Option<lunco_usd_compose::recipe::StageRecipe>,
}
struct PreparedTwinSources {
    sources: HashMap<PathBuf, Result<PreparedTwinSource, String>>,
    revisions: Vec<ReadRevision>,
    mounts: Vec<(String, PathBuf)>,
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
    let mut retained_bytes = 0_usize;
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
                let mut bytes = lunco_usd_bevy_stage::compose::read_registered_asset_bytes(
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
                                Some(&input.root),
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
            let owned_bytes = if let Some(recipe) = &recipe {
                recipe.bytes.values().try_fold(0_usize, |sum, bytes| {
                    sum.checked_add(bytes.len())
                        .ok_or("Twin source byte budget overflow")
                })?
            } else {
                bytes.len()
            };
            retained_bytes = retained_bytes
                .checked_add(owned_bytes)
                .ok_or("Twin source byte budget overflow")?;
            if retained_bytes > limits.max_bytes {
                return Err("Twin sources exceed the shared preparation byte budget".to_owned());
            }
            let (revisions, mounts) = read_identities(&id, &bytes, recipe.as_ref(), roots)?;
            prepared.revisions.extend(revisions);
            prepared.mounts.extend(mounts);
            let text = if usd {
                None
            } else {
                Some(String::from_utf8(bytes).map_err(|error| error.to_string())?)
            };
            Ok(PreparedTwinSource { text, recipe })
        }
        .await;
        prepared.sources.insert(relative.clone(), result);
    }
    prepared
        .revisions
        .sort_by(|left, right| (&left.path, &left.cid).cmp(&(&right.path, &right.cid)));
    prepared
        .revisions
        .dedup_by(|left, right| left.path == right.path && left.cid == right.cid);
    Ok(prepared)
}
fn inspect_prepared_twin(
    input: &TwinInspectionInput,
    sources: &HashMap<PathBuf, Result<PreparedTwinSource, String>>,
) -> TwinNamespaceSnapshot {
    crate::twin_lint::inspect_input(
        input,
        |relative| match sources.get(relative) {
            Some(Ok(source)) => source
                .text
                .clone()
                .ok_or_else(|| "Source has no prepared text".to_owned()),
            Some(Err(error)) => Err(error.clone()),
            None => Err("Indexed source was not prepared".to_owned()),
        },
        |relative, entries, errors| {
            let recipe = match sources.get(relative) {
                Some(Ok(source)) => match &source.recipe {
                    Some(recipe) => recipe,
                    None => {
                        errors.push(format!(
                            "{}: missing prepared USD recipe",
                            relative.display()
                        ));
                        return;
                    }
                },
                Some(Err(error)) => {
                    errors.push(format!("{}: {error}", relative.display()));
                    return;
                }
                None => {
                    errors.push(format!(
                        "{}: indexed USD source was not prepared",
                        relative.display()
                    ));
                    return;
                }
            };
            errors.extend(
                recipe
                    .dependency_diagnostics
                    .iter()
                    .map(ToString::to_string),
            );
            match lunco_usd_bevy_stage::canonical::CanonicalStage::from_recipe(recipe) {
                Ok(stage) => crate::twin_lint::inspect_usd_stage(&stage, relative, entries),
                Err(error) => errors.push(format!("{}: {error}", relative.display())),
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
    fn source_revisions_preserve_actual_bytes() {
        let (revisions, mounts) =
            read_identities("payload.rhai", b"let value = 1;", None, None).unwrap();
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
    fn retired_mount_cannot_publish_into_reopened_root() {
        let folder = tempfile::tempdir().unwrap();
        let roots = TwinRoots::default();
        let authority = roots.register("preparation", folder.path()).unwrap();
        let root = roots.root_of(&authority).unwrap().unwrap();
        let fence = OwnerFence {
            runtime: DocumentRuntimeOwner::Application,
            mount: Some((authority.clone(), root)),
            twin_input: None,
            policies: Vec::new(),
        };
        let mut world = World::new();
        world.insert_resource(roots.clone());
        assert!(fence.is_current(&world));
        roots.unregister_name(&authority).unwrap();
        let replacement = roots.register("preparation", folder.path()).unwrap();
        assert_ne!(replacement, authority);
        assert!(!fence.is_current(&world));
    }
}
