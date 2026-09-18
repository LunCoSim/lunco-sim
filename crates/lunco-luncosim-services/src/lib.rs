//! Application services for the production LunCoSim composition.
//!
//! This package owns lifecycle and integration services that are deliberately
//! outside the simulation substrate: startup Twin resolution, API/query
//! registration, networking, journal projection, and persisted experiment
//! artifacts. Keeping these services here means changing a transport or
//! application policy does not rebuild the generic simulation core.

use bevy::asset::AssetLoadFailedEvent;
#[cfg(feature = "networking")]
use bevy::asset::AssetServer;
use bevy::log::error;
#[cfg(feature = "networking")]
use bevy::log::info;
#[cfg(any(feature = "experiments", feature = "networking"))]
use bevy::log::warn;
use bevy::prelude::*;

#[cfg(feature = "networking")]
use lunco_usd_bevy_runtime_core::scene::LoadScene;
use lunco_usd_bevy_scene::UsdPrimPath;
use lunco_usd_bevy_stage::UsdStageAsset;

/// Production application-service composition.
pub struct LunCoSimServicesPlugin {
    /// Whether this host owns the headless server lifecycle.
    pub headless: bool,
    /// Explicit startup scene supplied by the application boundary.
    pub startup_scene: Option<String>,
}

impl Default for LunCoSimServicesPlugin {
    fn default() -> Self {
        Self {
            headless: false,
            startup_scene: None,
        }
    }
}

impl Plugin for LunCoSimServicesPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ScenePath(self.startup_scene.clone()))
            .add_systems(Startup, load_startup_scene_on_boot)
            .add_observer(startup_twin_scan_failguard)
            .add_systems(Update, startup_scene_failguard);

        if self.headless && !app.is_plugin_added::<lunco_workspace::WorkspacePlugin>() {
            app.add_plugins(lunco_workspace::WorkspacePlugin);
        }

        #[cfg(feature = "api-transport")]
        {
            if !app.is_plugin_added::<lunco_workspace_api::WorkspaceApiQueriesPlugin>() {
                app.add_plugins(lunco_workspace_api::WorkspaceApiQueriesPlugin);
            }
            if !app.is_plugin_added::<lunco_modelica_api::ModelicaApiQueriesPlugin>() {
                app.add_plugins(lunco_modelica_api::ModelicaApiQueriesPlugin);
            }
            app.add_plugins(lunco_api_transport::LunCoApiPlugin::default());
        }

        #[cfg(feature = "experiments")]
        {
            app.add_systems(
                Update,
                write_run_result_artifact.run_if(
                    resource_exists::<lunco_experiments::ExperimentRegistry>
                        .and_then(resource_exists::<lunco_workspace::WorkspaceResource>),
                ),
            );
            app.add_systems(
                Update,
                load_run_result_artifacts.run_if(
                    resource_changed::<lunco_experiments::ExperimentRegistry>
                        .and_then(resource_exists::<lunco_workspace::WorkspaceResource>),
                ),
            );
        }

        #[cfg(feature = "networking")]
        {
            let mode = lunco_networking::NetworkMode::resolve(self.headless);
            info!("[net] networking mode: {mode:?}");
            app.add_plugins(lunco_networking::LunCoNetworkingPlugin { mode });
            app.add_plugins(lunco_networking_core::prediction::NetcodePredictionPlugin);
            app.add_systems(Update, load_ready_scenario);
            app.add_systems(Update, replay_scenario_journal);
            app.add_systems(Update, replay_scenario_journal_modelica);
            #[cfg(feature = "experiments")]
            app.add_systems(Update, replay_scenario_journal_experiment);
            app.add_systems(Update, replay_scenario_journal_shader);
            app.add_systems(Update, replay_scenario_journal_obstacle);
            app.init_resource::<lunco_networking_sync::sync::PendingRunStatus>();
            app.init_resource::<lunco_networking_sync::sync::RequestManifestRebuild>();
            #[cfg(feature = "experiments")]
            app.add_systems(
                Update,
                request_rebuild_after_result
                    .run_if(resource_exists::<lunco_experiments::ExperimentRegistry>),
            );
            #[cfg(feature = "experiments")]
            app.add_systems(
                Update,
                (broadcast_run_status, apply_run_status)
                    .run_if(resource_exists::<lunco_experiments::ExperimentRegistry>),
            );
        }
    }
}

#[cfg(feature = "networking")]
fn load_ready_scenario(
    role: Res<lunco_core_session::NetworkRole>,
    remote: Res<lunco_networking_sync::scenario_sync::RemoteScenarioManifest>,
    downloads: Res<lunco_networking_sync::scenario_sync::AssetDownloads>,
    // Twin roots: a downloaded scenario is mounted here as a root over its cache
    // dir, so it loads under the SAME `twin://<name>/<rel>` the host uses.
    twins: Res<lunco_assets_core::twin_source::TwinRoots>,
    // Last scenario revision we triggered a load for — reload only on change.
    mut last_loaded: Local<Option<[u8; 32]>>,
    mut commands: Commands,
) {
    if role.is_host() {
        return;
    }
    let Some(m) = remote.manifest.as_ref() else {
        return;
    };
    let Some(scene) = m.default_scene.as_deref() else {
        return; // scenario advertises no entry scene → nothing to auto-load
    };
    if *last_loaded == Some(m.revision) || !downloads.all_cached(m) {
        return;
    }
    // Mounting registers the scenario's cache dir as this twin's root (unless the
    // twin is already open locally, which keeps its own). Either way the URI is
    // the host's, so a client that already booted this scene re-triggers the SAME
    // asset path and `LoadScene` no-ops instead of remounting.
    //
    // Verified on a native host/client pair (`scripts/run_host_client.sh`): both
    // peers mount `twin://luncosim/sandbox_scene.usda`, and this load lands ~1 s
    // after the client's own boot load — INSIDE the spawn window, so the no-op
    // depends on `LoadScene`'s `SceneLoadInFlight` arm, not on its
    // already-spawned-prims arm.
    //
    // TODO(verify-web-client): the case this addressing exists for — a peer with
    // NO local checkout, resolving through the mounted cache dir — is still
    // unverified. A native pair takes the "twin already open locally" branch, so
    // it exercises URI agreement but never the cache-root mount. It fails
    // silently: a wrong root gives that peer its own `GlobalEntityId`s, so
    // possession and client prediction never bind while the scene still renders.
    let uri = match lunco_networking_sync::scenario_sync::mount_scenario_twin(
        &twins,
        &m.scenario_id,
        &m.name,
        scene,
    ) {
        Ok(uri) => uri,
        Err(error) => {
            lunco_core::trigger_runtime_error(
                &mut commands,
                "scenario-twin-mount-failed",
                format!("could not mount downloaded scenario Twin: {error}"),
            );
            return;
        }
    };
    info!("[net] scenario fully cached; loading entry scene (read-only): {scene}");
    commands.trigger(LoadScene {
        path: uri,
        root_prim: String::new(),
    });
    *last_loaded = Some(m.revision);
}

/// Scenario distribution Layer B: replay peers' live authored edits onto the
/// local scene. The journal plane converges every peer's journal
/// (`append_remote` + merge, bidirectional); this projects the merged Op entries
/// onto the local USD scene so *other* peers' edits become visible. Runs on
/// **both** roles now (full bidirectional collaboration):
///
/// - **Client** — projects entries AFTER the manifest's `journal_head` (the
///   downloaded snapshot's base — so history baked into the files isn't
///   double-applied), authored by another peer.
/// - **Host** — projects entries AFTER the head its own scenario manifest
///   advertises (see below); the `author != me` filter then selects only
///   client-authored edits (its own are already applied at author time), so the
///   host *sees* clients' edits.
///
/// Both roles therefore share ONE invariant: *the files on disk already reflect
/// history up to `journal_head`; replay only what came after.* The host used to
/// pass `base = None` (replay the whole log, trusting `author != me` to drop its
/// own edits). That holds only while the host's local author id equals the id
/// that wrote the journal. A twin authored anywhere else — another machine, an
/// earlier session, a downloaded twin, or merely a different `LUNCO_PEER_ID`
/// (which `scripts/run_host_client.sh` sets) — looks entirely foreign, so the
/// host re-applied its whole saved history on top of files that already contained
/// it: prims re-added, rovers churned. (Historically this also double-despawned a
/// wheel joint whose bodies were already gone and tripped avian's
/// `assert!(island.joint_count > 0)`; that is now structurally impossible — every
/// synthesized joint is owned by its chassis via `ChildOf`, so it dies exactly
/// once with the rover subtree. See `setup_physical_wheel`.)
///
/// The head is sampled ONCE, not read every frame: a mid-session manifest rebuild
/// advances `journal_head`, which would move the base past client entries this
/// frame has not projected yet.
///
/// Each entry applies once, without re-recording (`replay_op`). The
/// assembly-crate bridge: the only place that sees the wire state
/// (`RemoteScenarioManifest`), the journal, AND the USD registry.
///
/// Single active scene doc for now — multi-doc needs stable cross-peer
/// `DocumentId` mapping (a follow-up); `scene_ops_after` selects by author, not
/// by the entry's peer-local `doc` id, which single-scene makes irrelevant.
#[cfg(feature = "networking")]
fn replay_scenario_journal(
    role: Res<lunco_core_session::NetworkRole>,
    remote: Res<lunco_networking_sync::scenario_sync::RemoteScenarioManifest>,
    // Host-side only (inserted by `setup_host`) — the manifest this host serves.
    local_scenario: Option<Res<lunco_networking_sync::scenario_sync::ScenarioManifestResource>>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
    mut registry: ResMut<
        lunco_doc_bevy::DocumentRegistry<lunco_usd_document::document::UsdDocument>,
    >,
    // Entry ids already projected onto the scene (once-per-entry guard).
    mut applied: Local<std::collections::HashSet<lunco_twin_journal::EntryId>>,
    // The host's replay base, latched the first frame its manifest exists.
    mut host_base: Local<Option<Option<lunco_twin_journal::EntryId>>>,
) {
    let Some(journal) = journal else {
        return;
    };
    // Base head: the state the on-disk files already reflect. The host reads it
    // off the manifest it built (deferring until that build lands); a client
    // bases on the downloaded snapshot's head, or waits if no scenario is loaded.
    let base: Option<lunco_twin_journal::EntryId> = if role.is_host() {
        if host_base.is_none() {
            let Some(scenario) = local_scenario.as_ref() else {
                return; // no host manifest resource → nothing to base on yet
            };
            let Some(manifest) = scenario.manifest.as_ref() else {
                return; // manifest build still in flight → defer, don't replay history
            };
            *host_base = Some(lunco_networking_sync::scenario_sync::manifest_journal_head(
                Some(manifest),
            ));
        }
        host_base.as_ref().and_then(Clone::clone)
    } else {
        let Some(manifest) = remote.manifest.as_ref() else {
            return;
        };
        lunco_networking_sync::scenario_sync::manifest_journal_head(Some(manifest))
    };
    // Single active scene doc (scenario consume is single-scene for now).
    let docs: Vec<_> = registry.ids().collect();
    let [doc] = docs.as_slice() else {
        return;
    };
    let doc = *doc;
    let me = journal.local_author();
    let pending = lunco_networking_sync::journal_plane::scene_ops_after(
        &journal,
        base.as_ref(),
        &me,
        &applied,
    );
    for (id, op) in pending {
        registry.replay_op(doc, &op);
        applied.insert(id);
    }
}

/// Scenario distribution Layer B for **Modelica** — the parallel of
/// [`replay_scenario_journal`] for the model domain. The journal plane, its merge,
/// and the strategy-honoring op selector are all domain-generic; only this consume
/// leg is per-domain. Selects the merged, not-yet-applied `Modelica` op entries via
/// [`domain_ops_after`](lunco_networking_sync::journal_plane::domain_ops_after)
/// (`DomainKind::Modelica`) — so a scripted merge policy reorders Modelica replay
/// identically to USD — and applies each through the generic Modelica document
/// registry's `replay_op`
/// (no re-recording).
///
/// Resources are `Option`: the Modelica registry / journal aren't present in every
/// app configuration (a pure-USD headless build), so this no-ops when either is
/// absent. Single active model for now — the same cross-peer `DocumentId` limitation
/// the USD leg documents (selection is by author, which one open model makes
/// sufficient); with more than one open model it defers rather than misroute.
#[cfg(feature = "networking")]
fn replay_scenario_journal_modelica(
    role: Res<lunco_core_session::NetworkRole>,
    remote: Res<lunco_networking_sync::scenario_sync::RemoteScenarioManifest>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
    registry: Option<
        ResMut<lunco_doc_bevy::DocumentRegistry<lunco_modelica_document::ModelicaDocument>>,
    >,
    // Modelica-domain entry ids already projected (its own once-per-entry guard,
    // independent of the USD driver's applied-set).
    mut applied: Local<std::collections::HashSet<lunco_twin_journal::EntryId>>,
) {
    let (Some(journal), Some(mut registry)) = (journal, registry) else {
        return;
    };
    let base: Option<lunco_twin_journal::EntryId> = if role.is_host() {
        None
    } else {
        let Some(manifest) = remote.manifest.as_ref() else {
            return;
        };
        lunco_networking_sync::scenario_sync::manifest_journal_head(Some(manifest))
    };
    // Single active Modelica model (see doc note); >1 open model → defer.
    let docs: Vec<_> = registry.iter().map(|(id, _)| id).collect();
    let [doc] = docs.as_slice() else {
        return;
    };
    let doc = *doc;
    let me = journal.local_author();
    let pending = lunco_networking_sync::journal_plane::domain_ops_after(
        &journal,
        base.as_ref(),
        &me,
        &applied,
        lunco_twin_journal::DomainKind::Modelica,
    );
    for (id, op) in pending {
        registry.replay_op(doc, &op);
        applied.insert(id);
    }
}

/// Per-domain journal consume leg for `DomainKind::Experiment` — projects a
/// peer's journaled experiment *definitions* (create / rename / bounds / params
/// / delete) onto the local `ExperimentRegistry`. Unlike the script/modelica
/// legs there is **no single-doc limitation**: every `ExperimentOp` carries its
/// own cross-peer-stable id (the authored UUID, replayed via `insert_with_id`),
/// so any number of experiments route correctly. Run results/status are NOT here
/// — they ride the content/presence planes. No-ops when registry/journal absent.
#[cfg(all(feature = "networking", feature = "experiments"))]
fn replay_scenario_journal_experiment(
    role: Res<lunco_core_session::NetworkRole>,
    remote: Res<lunco_networking_sync::scenario_sync::RemoteScenarioManifest>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
    registry: Option<ResMut<lunco_experiments::ExperimentRegistry>>,
    mut applied: Local<std::collections::HashSet<lunco_twin_journal::EntryId>>,
) {
    let (Some(journal), Some(mut registry)) = (journal, registry) else {
        return;
    };
    let base: Option<lunco_twin_journal::EntryId> = if role.is_host() {
        None
    } else {
        let Some(manifest) = remote.manifest.as_ref() else {
            return;
        };
        lunco_networking_sync::scenario_sync::manifest_journal_head(Some(manifest))
    };
    let me = journal.local_author();
    let pending = lunco_networking_sync::journal_plane::domain_ops_after(
        &journal,
        base.as_ref(),
        &me,
        &applied,
        lunco_twin_journal::DomainKind::Experiment,
    );
    for (id, op) in pending {
        lunco_modelica_core::experiment_journal::replay_experiment_op(&mut registry, &op);
        applied.insert(id);
    }
}

/// Per-domain journal consume leg for `DomainKind::Shader` — projects a peer's
/// journaled WGSL edits (`ShaderOp::SetSource`) onto the local `ShaderRegistry`
/// and **hot-reloads** the live `Assets<Shader>`, so a shader tweak on one machine
/// recompiles on every peer. No single-doc limitation: the op carries the shader
/// `path` (cross-peer-stable), so `apply_replayed` routes by path. `Assets<Shader>`
/// / `ShaderRegistry` are `Option` — a headless (no-render) relay host has neither
/// and simply no-ops (it still forwards the journal entry to GUI peers).
#[cfg(feature = "networking")]
fn replay_scenario_journal_shader(
    role: Res<lunco_core_session::NetworkRole>,
    remote: Res<lunco_networking_sync::scenario_sync::RemoteScenarioManifest>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
    registry: Option<ResMut<lunco_scene_authoring::shader_doc::ShaderRegistry>>,
    asset_server: Option<Res<AssetServer>>,
    shaders: Option<ResMut<Assets<bevy::shader::Shader>>>,
    mut applied: Local<std::collections::HashSet<lunco_twin_journal::EntryId>>,
) {
    let (Some(journal), Some(mut registry), Some(asset_server), Some(mut shaders)) =
        (journal, registry, asset_server, shaders)
    else {
        return;
    };
    let base: Option<lunco_twin_journal::EntryId> = if role.is_host() {
        None
    } else {
        let Some(manifest) = remote.manifest.as_ref() else {
            return;
        };
        lunco_networking_sync::scenario_sync::manifest_journal_head(Some(manifest))
    };
    let me = journal.local_author();
    let pending = lunco_networking_sync::journal_plane::domain_ops_after(
        &journal,
        base.as_ref(),
        &me,
        &applied,
        lunco_twin_journal::DomainKind::Shader,
    );
    for (id, op) in pending {
        if let Ok(shader_op) =
            serde_json::from_value::<lunco_scene_authoring::shader_doc::ShaderOp>(op)
        {
            if let Some((path, source)) = registry.apply_replayed(&shader_op) {
                // Use the same source/asset identity resolver as the local
                // command. A journal path may be bare while the peer's live
                // material is keyed under the explicit `lunco://` source.
                if let Err(error) = lunco_scene_authoring::properties::apply_shader_source_live(
                    &asset_server,
                    &mut shaders,
                    &path,
                    &source,
                ) {
                    warn!("SHADER_JOURNAL: live source apply failed: {error}");
                }
            }
        }
        applied.insert(id);
    }
}

/// Per-domain journal consume leg for `DomainKind::ObstacleField` — installs a
/// peer's journaled obstacle-field spec onto the local `ObstacleFieldSpec`.
/// This is what replaced the former bespoke host→client
/// broadcast (`sync_obstacle_field_spec`): the spec now rides the journal plane,
/// so a tweak syncs BOTH directions and persists. No single-doc limitation (the
/// spec is a singleton). No-ops when the spec resource / journal are absent.
#[cfg(feature = "networking")]
fn replay_scenario_journal_obstacle(
    role: Res<lunco_core_session::NetworkRole>,
    remote: Res<lunco_networking_sync::scenario_sync::RemoteScenarioManifest>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
    spec: Option<ResMut<lunco_obstacle_field::ObstacleFieldSpec>>,
    mut applied: Local<std::collections::HashSet<lunco_twin_journal::EntryId>>,
) {
    let (Some(journal), Some(mut spec)) = (journal, spec) else {
        return;
    };
    let base: Option<lunco_twin_journal::EntryId> = if role.is_host() {
        None
    } else {
        let Some(manifest) = remote.manifest.as_ref() else {
            return;
        };
        lunco_networking_sync::scenario_sync::manifest_journal_head(Some(manifest))
    };
    let me = journal.local_author();
    let pending = lunco_networking_sync::journal_plane::domain_ops_after(
        &journal,
        base.as_ref(),
        &me,
        &applied,
        lunco_twin_journal::DomainKind::ObstacleField,
    );
    // Coalesce: a batch may carry several SetSpec ops (rapid slider drags); only
    // the LAST one matters — so install once.
    let mut last_spec = None;
    for (id, op) in pending {
        if let Some(new_spec) = lunco_obstacle_field::journal::replay_spec(&op) {
            last_spec = Some(new_spec);
        }
        applied.insert(id);
    }
    if let Some(new_spec) = last_spec {
        // Install the peer's spec. Sets the resource directly (NOT the
        // `UpdateObstacleFieldSpec` command), so no re-record.
        *spec = new_spec;
    }
}

/// Result-artifact writer: on `RunCompleted`, the host serializes the finished
/// `RunResult` to `<twin>/results/<experiment-id>.json` so it rides the **content
/// plane** — the twin file-walk CID's `results/` (a non-dot dir) and the manifest
/// sync ships it to peers. Host-authoritative: a Client never ran the sim, so it
/// writes nothing (it *receives* the artifact). The result is recovered from the
/// registry (core writes it there before `RunCompleted` fires — same pattern as
/// `project_run_results_to_ui`). JSON today; parquet is a deferred format swap
/// pending a wasm-reader spike (see `NETWORKING_STATE_SYNC_TAXONOMY_DESIGN.md`).
/// `RunResult` to `<twin>/results/<experiment-id>.json` through the cross-platform
/// [`lunco_storage`] layer (native file / wasm WebStorage). This is **core
/// persistence, not a networking concern** — a single-player run's results
/// survive a restart, and when networking is on the same file rides the content
/// plane to peers. Host/standalone only: a networked Client never ran the sim, so
/// it writes nothing (it *receives* the artifact). Recovered from the registry
/// (core writes it there before `RunCompleted` fires — same pattern as
/// `project_run_results_to_ui`). JSON today; parquet is a deferred format swap.
#[cfg(feature = "experiments")]
fn write_run_result_artifact(
    mut completed: MessageReader<lunco_experiments::RunCompleted>,
    registry: Res<lunco_experiments::ExperimentRegistry>,
    workspace: Res<lunco_workspace::WorkspaceResource>,
    role: Option<Res<lunco_core_session::NetworkRole>>,
) {
    if matches!(
        role.as_deref(),
        Some(lunco_core_session::NetworkRole::Client)
    ) {
        return;
    }
    for msg in completed.read() {
        let id = msg.experiment_id;
        let Some(result) = registry.get(id).and_then(|e| e.result.as_ref()) else {
            continue;
        };
        let Some(active) = workspace.active_twin else {
            continue;
        };
        let Some(twin) = workspace.twin(active) else {
            continue;
        };
        // The storage layer creates parent dirs on write (FileStorage tmp+rename;
        // WebStorage is key-based), so no explicit mkdir — all I/O goes through it.
        let dest =
            lunco_twin::results_dir(&twin.root).join(format!("{}.json", id.as_artifact_stem()));
        match serde_json::to_vec_pretty(result) {
            Ok(bytes) => match lunco_storage::write_file_sync(&dest, &bytes) {
                Ok(()) => info!("[experiment] wrote result artifact {dest:?}"),
                Err(e) => warn!("[experiment] result artifact write failed: {e}"),
            },
            Err(e) => warn!("[experiment] result serialize failed: {e}"),
        }
    }
}

/// Result-artifact loader — the consume half of persistence/ship-artifact. For
/// each known experiment that lacks a trajectory, reads
/// `<twin>/results/<id>.json` through [`lunco_storage`] (cross-platform, no
/// directory listing — bounded by the registry cap) and loads it. This restores a
/// single-player run's results after a restart AND makes a networked peer *see*
/// the host's results once their file syncs.
///
/// Change-driven on [`ExperimentRegistry`] mutation (a definition synced, a run
/// completed, a status update) — so a just-synced result file is picked up on the
/// next registry change (e.g. the presence status flip) rather than by polling.
#[cfg(feature = "experiments")]
fn load_run_result_artifacts(
    mut registry: ResMut<lunco_experiments::ExperimentRegistry>,
    workspace: Res<lunco_workspace::WorkspaceResource>,
) {
    let Some(active) = workspace.active_twin else {
        return;
    };
    let Some(root) = workspace
        .twin(active)
        .map(|t| lunco_twin::results_dir(&t.root))
    else {
        return;
    };
    // Ids known but resultless — the only candidates worth a storage read.
    let want: Vec<lunco_experiments::ExperimentId> = registry
        .iter_all()
        .filter(|e| e.result.is_none())
        .map(|e| e.id)
        .collect();
    for id in want {
        let path = root.join(format!("{}.json", id.as_artifact_stem()));
        let Ok(bytes) = lunco_storage::read_file_sync(&path) else {
            continue; // not present (yet)
        };
        match serde_json::from_slice::<lunco_experiments::RunResult>(&bytes) {
            Ok(result) => {
                let wall = result.meta.wall_time_ms;
                registry.set_result(id, result);
                registry.set_status(
                    id,
                    lunco_experiments::RunStatus::Done { wall_time_ms: wall },
                );
                info!(
                    "[experiment] loaded result artifact for {}",
                    id.as_artifact_stem()
                );
            }
            Err(e) => warn!(
                "[experiment] result artifact parse failed for {}: {e}",
                id.as_artifact_stem()
            ),
        }
    }
}

/// Networking distribution trigger: when a run finishes on the host, ask for an
/// immediate scenario-manifest rebuild so already-connected peers pull the
/// just-written result artifact now (serviced by `service_manifest_rebuild_request`
/// in lunco-networking). The write itself is the core persistence system's job;
/// this only nudges distribution. Host-only.
#[cfg(all(feature = "networking", feature = "experiments"))]
fn request_rebuild_after_result(
    mut completed: MessageReader<lunco_experiments::RunCompleted>,
    role: Option<Res<lunco_core_session::NetworkRole>>,
    mut rebuild: ResMut<lunco_networking_sync::sync::RequestManifestRebuild>,
) {
    if !matches!(role.as_deref(), Some(lunco_core_session::NetworkRole::Host)) {
        return;
    }
    if completed.read().count() > 0 {
        rebuild.0 = true;
    }
}

/// Presence broadcast: the host relays experiment run-status transitions
/// (Running progress → Done/Failed/Cancelled) to clients over the wire, so a
/// peer watches a run advance live. Ephemeral — progress rides the lossy
/// `ControlStream`, terminal states the reliable `CommandBus` (so the final
/// flip is never dropped). Host-only; the assembly crate maps `RunStatus` to the
/// primitive `RunStatusMsg` here (keeping networking free of an experiments dep).
#[cfg(all(feature = "networking", feature = "experiments"))]
fn broadcast_run_status(
    role: Option<Res<lunco_core_session::NetworkRole>>,
    mut outbox: ResMut<lunco_networking_sync::sync::SyncOutbox>,
    mut progress: MessageReader<lunco_experiments::RunProgress>,
    mut completed: MessageReader<lunco_experiments::RunCompleted>,
    mut failed: MessageReader<lunco_experiments::RunFailed>,
    mut cancelled: MessageReader<lunco_experiments::RunCancelled>,
    registry: Res<lunco_experiments::ExperimentRegistry>,
) {
    if !matches!(role.as_deref(), Some(lunco_core_session::NetworkRole::Host)) {
        return;
    }
    use lunco_command_contracts::SyncChannel;
    use lunco_networking_sync::sync::{RunStatusMsg, SyncEnvelope};
    let msg = |id: lunco_experiments::ExperimentId,
               phase: u8,
               t_current: f64,
               wall_time_ms: u64,
               error: String| {
        SyncEnvelope::RunStatus(RunStatusMsg {
            experiment_id: id.uuid_bytes(),
            phase,
            t_current,
            wall_time_ms,
            error,
        })
    };
    for m in progress.read() {
        outbox.0.push((
            SyncChannel::ControlStream,
            msg(m.experiment_id, 2, m.t_current, 0, String::new()),
        ));
    }
    for m in completed.read() {
        let wall = registry
            .get(m.experiment_id)
            .and_then(|e| e.result.as_ref())
            .map(|r| r.meta.wall_time_ms)
            .unwrap_or(0);
        outbox.0.push((
            SyncChannel::CommandBus,
            msg(m.experiment_id, 3, 0.0, wall, String::new()),
        ));
    }
    for m in failed.read() {
        outbox.0.push((
            SyncChannel::CommandBus,
            msg(m.experiment_id, 4, 0.0, 0, m.error.clone()),
        ));
    }
    for m in cancelled.read() {
        outbox.0.push((
            SyncChannel::CommandBus,
            msg(m.experiment_id, 5, 0.0, 0, String::new()),
        ));
    }
}

/// Presence apply (client): drain host-sent run-status updates into the local
/// `ExperimentRegistry` so a synced experiment's row advances Running → Done.
/// Won't clobber a `Done` already loaded from the result artifact (the artifact
/// carries the trajectory; a late progress packet must not downgrade it).
#[cfg(all(feature = "networking", feature = "experiments"))]
fn apply_run_status(
    mut pending: ResMut<lunco_networking_sync::sync::PendingRunStatus>,
    mut registry: ResMut<lunco_experiments::ExperimentRegistry>,
) {
    if pending.0.is_empty() {
        return;
    }
    for m in std::mem::take(&mut pending.0) {
        let id = lunco_experiments::ExperimentId::from_uuid_bytes(m.experiment_id);
        let already_done = matches!(
            registry.get(id).map(|e| &e.status),
            Some(lunco_experiments::RunStatus::Done { .. })
        );
        if already_done && m.phase != 3 {
            continue;
        }
        let status = match m.phase {
            1 => lunco_experiments::RunStatus::Queued,
            2 => lunco_experiments::RunStatus::Running {
                t_current: m.t_current,
            },
            3 => lunco_experiments::RunStatus::Done {
                wall_time_ms: m.wall_time_ms,
            },
            4 => lunco_experiments::RunStatus::Failed {
                error: m.error,
                partial: false,
            },
            5 => lunco_experiments::RunStatus::Cancelled,
            _ => lunco_experiments::RunStatus::Pending,
        };
        registry.set_status(id, status);
    }
}

/// Resource that holds the optional asset-source-relative path of the scene to
/// load on Startup. `None` means an intentionally empty world shell. It is
/// supplied by the application boundary before the service composition starts.
#[derive(Resource)]
pub struct ScenePath(pub Option<String>);

/// Load the explicitly requested startup scene.
fn load_startup_scene_on_boot(world: &mut World) {
    let scene_path = world.resource::<ScenePath>().0.clone();

    // WEB: do NOT load a startup scene here. The generated page's autoload hook
    // (index.html → a `LoadScene` command) loads the deployment's default twin
    // (moonbase) directly. A second built-in `sandbox_scene` load here raced that
    // autoload: the twin reload's cleanup despawned the sandbox_scene entity while
    // `sync_usd_visuals` still had a deferred `insert::<UsdPrimPath>` queued for it
    // → "Entity despawned" panic → aborted wasm → dark viewport. The filesystem
    // twin-resolve below is meaningless in the browser anyway (no `twin.toml` FS).
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(scene_path) = scene_path {
        load_startup_scene(world, scene_path);
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (world, scene_path);
        info!(
            "[luncosim] web startup: no built-in scene load — the page autoload hook loads the default twin directly"
        );
    }
}

/// Native/headless startup-scene load: resolve the enclosing Twin folder for
/// `scene_path` (walk up to a `twin.toml`) and enqueue its workspace scan. The
/// scan and indexing stay off the UI thread; its completion registers the Twin
/// and mounts the selected scene through the normal doc-first path. Invalid or
/// orphaned roots report an error and do not load a base-only scene. Web skips
/// this — its autoload hook loads the deployment twin directly (see
/// [`setup_luncosim`]).
#[cfg(not(target_arch = "wasm32"))]
fn load_startup_scene(world: &mut World, scene_path: String) {
    // Resolve the absolute path to find the enclosing Twin folder. This is
    // deliberately shared by shipped scenes and external Twin roots: a CLI
    // spelling must never change which document root gets mounted.
    let abs_path = resolve_scene_cli_path(&scene_path);

    // The root that owns this scene — nearest `twin.toml` ancestor, else the
    // containing folder. Shared with the runtime open path (`OpenFile` →
    // `spawn_twin_from_scene`) so boot and commands cannot disagree about what
    // "the root" is for a given file.
    let twin_root = lunco_twin::root_for_file(&abs_path);

    let scene_file = abs_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    world.insert_resource(StartupSceneGuard {
        file: scene_file.clone(),
    });

    let rel_scene_path = abs_path
        .strip_prefix(&twin_root)
        .map(lunco_assets_path::slashed)
        .unwrap_or_else(|_| scene_file.clone());
    let Some(mut pending) = world.get_resource_mut::<lunco_workspace::open::PendingTwinOpens>()
    else {
        error!(
            "[luncosim] startup scene `{scene_path}` cannot begin: workspace open pipeline is not installed"
        );
        return;
    };

    // `TwinMode::open` walks and indexes the entire root synchronously, so it
    // must use the same asynchronous scan owner as an interactive OpenFile.
    // The completion path registers the asset authority before its
    // TwinAssetMounted event and therefore preserves the doc-first overlay.
    lunco_workspace::open::spawn_twin_scan(
        &twin_root,
        &mut pending,
        "StartupScene",
        Some(rel_scene_path),
        lunco_workspace::open::TwinOpenMode::Replace,
    );
    info!(
        "[luncosim] queued startup Twin scan for `{}` (scene `{scene_file}`)",
        twin_root.display()
    );
    // `--scene` is doc-backed through the same path as any workspace Twin: the
    // asset-mounted event emitted after `TwinAdded` runs the doc-first mount
    // (`open_usd_docs_on_twin_asset_mounted` → `drain_pending_twin_docs`), and
    // terrain edits stay on the incremental re-bake — `LiveRebuildExempt` +
    // `edit_confined_to_exempt_subtree` keep a terrain-confined USD edit from
    // ever reloading the scene.
}

/// Resolve a `--scene` argument without forcing every Twin into the engine's
/// shipped `assets/` tree.
///
/// Resolution order is intentionally deterministic:
///
/// 1. absolute filesystem path;
/// 2. existing path relative to the process working directory;
/// 3. an explicit `assets/...` spelling relative to the process working
///    directory (useful when invoking the binary from the repository root);
/// 4. the normal asset-root-relative spelling used by packaged launches.
///
/// We do not canonicalize here. The Twin opener should receive the user's
/// path, including a custom Twin's symlink/layout, and report a precise error
/// if it does not exist.
#[cfg(not(target_arch = "wasm32"))]
fn resolve_scene_cli_path(input: &str) -> std::path::PathBuf {
    use std::path::Path;

    let path = Path::new(input);
    if path.is_absolute() {
        return path.to_path_buf();
    }

    if let Ok(cwd) = std::env::current_dir() {
        let cwd_relative = cwd.join(path);
        if cwd_relative.exists() {
            return cwd_relative;
        }
    }

    if let Ok(without_assets) = path.strip_prefix(lunco_assets_core::ASSETS_DIR_NAME) {
        let asset_spelling = lunco_assets_core::assets_dir_abs().join(without_assets);
        if asset_spelling.exists() {
            return asset_spelling;
        }
    }

    lunco_assets_core::assets_dir_abs().join(path)
}

/// Tracks an explicitly requested startup scene so the two startup failguards
/// can turn a silent scan or asset-load failure into a loud, fatal error. It is
/// removed once the scene has loaded (or failed), so later runtime `LoadScene`s
/// (API / UI) — which must NOT crash the app on a bad request — are unaffected.
#[derive(Resource)]
struct StartupSceneGuard {
    /// File name of the explicitly requested startup scene.
    file: String,
}

/// Fail loud if the explicit `--scene` Twin scan or USD scene fails at startup.
///
/// The bug this guards: `--scene` paths are relative to the `assets/` source
/// root; prefixing `assets/` doubles it (`assets/assets/…`), the asset is not
/// found, and the app *silently* boots a scene-less world. Here a matching
/// `TWIN_OPEN_FAILED` / `AssetLoadFailedEvent<UsdStageAsset>` → clear error +
/// non-zero exit. Disarms on success (scene produced `UsdPrimPath` entities) so
/// runtime loads are safe.
fn startup_twin_scan_failguard(
    trigger: On<lunco_telemetry_core::TelemetryEvent>,
    guard: Option<Res<StartupSceneGuard>>,
    mut commands: Commands,
) {
    let Some(guard) = guard else { return };
    if trigger.event().name != lunco_workspace::open::TWIN_OPEN_FAILED {
        return;
    }
    let lunco_telemetry_core::TelemetryValue::String(detail) = &trigger.event().data else {
        return;
    };
    if !detail.starts_with("StartupScene failed:") {
        return;
    }
    error!("Startup scene `{}` failed to scan: {detail}", guard.file);
    commands.write_message(AppExit::error());
    commands.remove_resource::<StartupSceneGuard>();
}

fn startup_scene_failguard(
    guard: Option<Res<StartupSceneGuard>>,
    mut failures: MessageReader<AssetLoadFailedEvent<UsdStageAsset>>,
    scene: Query<(), With<UsdPrimPath>>,
    mut exit: MessageWriter<AppExit>,
    mut commands: Commands,
) {
    let Some(guard) = guard else { return };

    for failed in failures.read() {
        let is_startup_scene =
            failed.path.path().file_name().and_then(|s| s.to_str()) == Some(guard.file.as_str());
        if is_startup_scene {
            error!(
                "Startup scene `{}` failed to load: {}. \
                 NOTE: `--scene` is relative to the `assets/` source root — do NOT prefix \
                 `assets/` (use `scenes/luncosim/sandbox_scene.usda`, not `assets/scenes/...`).",
                guard.file, failed.error,
            );
            exit.write(AppExit::error());
            commands.remove_resource::<StartupSceneGuard>();
            return;
        }
    }

    // Scene loaded (entities exist) → disarm so a later runtime LoadScene
    // failure (API/UI) never trips this fatal guard.
    if !scene.is_empty() {
        commands.remove_resource::<StartupSceneGuard>();
    }
}
