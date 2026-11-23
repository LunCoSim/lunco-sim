//! One place that registers **all** LunCo Bevy asset sources, so every binary
//! (luncosim, sandbox, web, model_viewer) gets the *same* schemes instead
//! of each `main()` hand-listing a divergent subset.
//!
//! Asset sources must be registered **before** `AssetPlugin`/`DefaultPlugins`
//! builds (Bevy snapshots the source registry at that point), so call
//! [`register_lunco_asset_sources`] right after `App::new()`, before
//! `.add_plugins(DefaultPlugins)`.

use bevy::prelude::*;
#[cfg(not(target_arch = "wasm32"))]
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};
#[cfg(target_arch = "wasm32")]
use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::collections::VecDeque;

use lunco_assets_core::lunco_source::lunco_asset_source;
use lunco_assets_core::twin_source::{TwinRoots, twin_asset_source};

const TWIN_ASSET_MOUNT_FAILED: &str = "twin-asset-mount-failed";
const TWIN_ASSET_UNMOUNT_FAILED: &str = "twin-asset-unmount-failed";

/// Mounts workspace Twins into the shared `twin://` asset source.
///
/// This is asset lifecycle, not USD lifecycle: Modelica-only lunica still
/// needs Twin-relative reads and Twin-scoped datasets, while a simulation host
/// must not depend on a particular scene domain to keep the source mounted.
pub struct TwinRootsPlugin;

/// A workspace Twin has an addressable `twin://` authority and its manifest
/// routes are installed before consumers load through that source.
///
/// [`lunco_workspace::TwinAdded`] announces workspace ownership, while this
/// event announces the asset boundary's stronger postcondition. Consumers must
/// use this event when their work requires a mounted Twin source; it is emitted
/// only after declared asset owners are ready, so scene reads cannot race routes.
#[derive(Event, Clone, Debug)]
pub struct TwinAssetMounted {
    /// Workspace identity of the mounted Twin.
    pub twin: lunco_workspace::TwinId,
    /// Exact mount-local source authority assigned by [`TwinRoots`]. Consumers
    /// use it for all loads; it is distinct from both display and source identity.
    pub authority: String,
    /// Human-readable name from the manifest or folder.
    pub display_name: String,
}

impl Plugin for TwinRootsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TwinRoots>()
            .add_observer(register_twin_root)
            .add_observer(unregister_twin_root);
        app.init_resource::<PendingTwinAssetMounts>();
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(Update, publish_ready_twin_asset_mounts);
        #[cfg(target_arch = "wasm32")]
        app.add_systems(Update, publish_ready_twin_asset_mounts);
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource, Default)]
struct PendingTwinAssetMounts(VecDeque<PendingTwinAssetMount>);

#[cfg(target_arch = "wasm32")]
#[derive(Resource)]
struct PendingTwinAssetMounts {
    pending: HashMap<lunco_workspace::TwinId, String>,
    sender: crossbeam_channel::Sender<PendingTwinAssetMountResult>,
    receiver: crossbeam_channel::Receiver<PendingTwinAssetMountResult>,
}

#[cfg(target_arch = "wasm32")]
impl Default for PendingTwinAssetMounts {
    fn default() -> Self {
        let (sender, receiver) = crossbeam_channel::unbounded();
        Self {
            pending: HashMap::new(),
            sender,
            receiver,
        }
    }
}

#[cfg(target_arch = "wasm32")]
struct PendingTwinAssetMountResult {
    twin_id: lunco_workspace::TwinId,
    authority: String,
    display_name: String,
    root: std::path::PathBuf,
    admitted_root: std::path::PathBuf,
    result: Result<Vec<lunco_assets_core::TwinAssetRoute>, String>,
}

#[cfg(not(target_arch = "wasm32"))]
struct PendingTwinAssetMount {
    twin_id: lunco_workspace::TwinId,
    authority: String,
    display_name: String,
    root: std::path::PathBuf,
    admitted_root: std::path::PathBuf,
    task: Task<Result<Vec<lunco_assets_core::TwinAssetRoute>, String>>,
}

fn register_twin_root(
    trigger: On<lunco_workspace::TwinAdded>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut pending_opens: Option<ResMut<lunco_workspace::open::PendingTwinOpens>>,
    roots: Res<TwinRoots>,
    mut pending_mounts: ResMut<PendingTwinAssetMounts>,
    mut commands: Commands,
) {
    let Some(workspace) = workspace else {
        return;
    };
    let twin_id = trigger.event().twin;
    let Some(twin) = workspace.twin(twin_id) else {
        return;
    };
    let assigned = match roots.register_twin(twin) {
        Ok(assigned) => assigned,
        Err(error) => {
            if let Some(pending_opens) = pending_opens.as_mut() {
                pending_opens.finish_replacement_for_twin(twin_id);
            }
            lunco_core::trigger_runtime_error(
                &mut commands,
                TWIN_ASSET_MOUNT_FAILED,
                format!(
                    "could not mount Twin asset root {}: {error}",
                    twin.root.display()
                ),
            );
            return;
        }
    };
    info!(
        "[twin-roots] mounted Twin `{}` at {}",
        twin.display_name(),
        twin.root.display()
    );
    let admitted_root = match roots.root_for(&assigned) {
        Ok(Some(root)) => root,
        Ok(None) => {
            if let Some(pending_opens) = pending_opens.as_mut() {
                pending_opens.finish_replacement_for_twin(twin_id);
            }
            lunco_core::trigger_runtime_error(
                &mut commands,
                TWIN_ASSET_MOUNT_FAILED,
                format!("Twin asset authority `{assigned}` disappeared during admission"),
            );
            return;
        }
        Err(error) => {
            if let Some(pending_opens) = pending_opens.as_mut() {
                pending_opens.finish_replacement_for_twin(twin_id);
            }
            lunco_core::trigger_runtime_error(
                &mut commands,
                TWIN_ASSET_MOUNT_FAILED,
                format!("could not verify admitted Twin asset root: {error}"),
            );
            return;
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        if pending_mounts
            .0
            .iter()
            .any(|pending| pending.twin_id == twin_id)
        {
            return;
        }
        let Some(pool) = AsyncComputeTaskPool::try_get() else {
            if let Some(pending_opens) = pending_opens.as_mut() {
                pending_opens.finish_replacement_for_twin(twin_id);
            }
            lunco_core::trigger_runtime_error(
                &mut commands,
                TWIN_ASSET_MOUNT_FAILED,
                "cannot prepare the Twin asset route map because the I/O task pool is unavailable",
            );
            return;
        };
        let scope = lunco_assets_datasets::DatasetScope::Twin {
            source_identity: lunco_assets_core::logical_twin_source_identity(twin),
            authority: Some(assigned.clone()),
            display_name: twin.display_name(),
            root: twin.root.clone(),
        };
        let route_scope = scope.clone();
        pending_mounts.0.push_back(PendingTwinAssetMount {
            twin_id,
            authority: assigned,
            display_name: twin.display_name(),
            root: twin.root.clone(),
            admitted_root,
            task: pool.spawn(async move {
                lunco_assets_datasets::DatasetRegistry::read_twin_asset_routes(&route_scope)
            }),
        });
    }
    #[cfg(target_arch = "wasm32")]
    {
        if pending_mounts.pending.get(&twin_id) == Some(&assigned) {
            return;
        }
        pending_mounts.pending.insert(twin_id, assigned.clone());
        let scope = lunco_assets_datasets::DatasetScope::Twin {
            source_identity: lunco_assets_core::logical_twin_source_identity(twin),
            authority: Some(assigned.clone()),
            display_name: twin.display_name(),
            root: twin.root.clone(),
        };
        let sender = pending_mounts.sender.clone();
        let authority = assigned;
        let display_name = twin.display_name();
        let root = twin.root.clone();
        let admitted_root = admitted_root;
        wasm_bindgen_futures::spawn_local(async move {
            let result =
                lunco_assets_datasets::DatasetRegistry::read_twin_asset_routes(&scope).await;
            let _ = sender.send(PendingTwinAssetMountResult {
                twin_id,
                authority,
                display_name,
                root,
                admitted_root,
                result,
            });
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn publish_ready_twin_asset_mounts(
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    roots: Res<TwinRoots>,
    mut pending: ResMut<PendingTwinAssetMounts>,
    mut pending_opens: Option<ResMut<lunco_workspace::open::PendingTwinOpens>>,
    mut commands: Commands,
) {
    let Some(workspace) = workspace else {
        return;
    };
    let mut remaining = VecDeque::new();
    while let Some(mut mount) = pending.0.pop_front() {
        let Some(result) = future::block_on(future::poll_once(&mut mount.task)) else {
            remaining.push_back(mount);
            continue;
        };
        let current_root = workspace
            .twin(mount.twin_id)
            .map(|twin| twin.root.as_path());
        let live_root = match roots.root_for(&mount.authority) {
            Ok(root) => root,
            Err(error) => {
                if let Some(pending_opens) = pending_opens.as_mut() {
                    pending_opens.finish_replacement_for_twin(mount.twin_id);
                }
                lunco_core::trigger_runtime_error(
                    &mut commands,
                    TWIN_ASSET_MOUNT_FAILED,
                    format!("could not verify live Twin asset authority: {error}"),
                );
                continue;
            }
        };
        let still_open = current_root == Some(mount.root.as_path())
            && live_root.as_deref() == Some(mount.admitted_root.as_path());
        if !still_open {
            continue;
        }
        let routes = match result {
            Ok(routes) => routes,
            Err(error) => {
                if let Some(pending_opens) = pending_opens.as_mut() {
                    pending_opens.finish_replacement_for_twin(mount.twin_id);
                }
                lunco_core::trigger_runtime_error(
                    &mut commands,
                    TWIN_ASSET_MOUNT_FAILED,
                    format!(
                        "could not prepare Twin asset routes for {}: {error}",
                        mount.root.display()
                    ),
                );
                continue;
            }
        };
        if let Err(error) = roots.replace_asset_routes(&mount.authority, routes) {
            if let Some(pending_opens) = pending_opens.as_mut() {
                pending_opens.finish_replacement_for_twin(mount.twin_id);
            }
            lunco_core::trigger_runtime_error(
                &mut commands,
                TWIN_ASSET_MOUNT_FAILED,
                format!(
                    "could not publish Twin asset routes for {}: {error}",
                    mount.root.display()
                ),
            );
            continue;
        }
        commands.trigger(TwinAssetMounted {
            twin: mount.twin_id,
            authority: mount.authority,
            display_name: mount.display_name,
        });
    }
    pending.0 = remaining;
}

#[cfg(target_arch = "wasm32")]
fn publish_ready_twin_asset_mounts(
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    roots: Res<TwinRoots>,
    mut pending: ResMut<PendingTwinAssetMounts>,
    mut pending_opens: Option<ResMut<lunco_workspace::open::PendingTwinOpens>>,
    mut commands: Commands,
) {
    let Some(workspace) = workspace else {
        return;
    };
    while let Ok(mount) = pending.receiver.try_recv() {
        if pending.pending.get(&mount.twin_id) != Some(&mount.authority) {
            continue;
        }
        pending.pending.remove(&mount.twin_id);
        let current_root = workspace
            .twin(mount.twin_id)
            .map(|twin| twin.root.as_path());
        let live_root = match roots.root_for(&mount.authority) {
            Ok(root) => root,
            Err(error) => {
                if let Some(pending_opens) = pending_opens.as_mut() {
                    pending_opens.finish_replacement_for_twin(mount.twin_id);
                }
                lunco_core::trigger_runtime_error(
                    &mut commands,
                    TWIN_ASSET_MOUNT_FAILED,
                    format!("could not verify live Twin asset authority: {error}"),
                );
                continue;
            }
        };
        let still_open = current_root == Some(mount.root.as_path())
            && live_root.as_deref() == Some(mount.admitted_root.as_path());
        if !still_open {
            continue;
        }
        let routes = match mount.result {
            Ok(routes) => routes,
            Err(error) => {
                if let Some(pending_opens) = pending_opens.as_mut() {
                    pending_opens.finish_replacement_for_twin(mount.twin_id);
                }
                lunco_core::trigger_runtime_error(
                    &mut commands,
                    TWIN_ASSET_MOUNT_FAILED,
                    format!(
                        "could not prepare Twin asset routes for {}: {error}",
                        mount.root.display()
                    ),
                );
                continue;
            }
        };
        if let Err(error) = roots.replace_asset_routes(&mount.authority, routes) {
            if let Some(pending_opens) = pending_opens.as_mut() {
                pending_opens.finish_replacement_for_twin(mount.twin_id);
            }
            lunco_core::trigger_runtime_error(
                &mut commands,
                TWIN_ASSET_MOUNT_FAILED,
                format!(
                    "could not publish Twin asset routes for {}: {error}",
                    mount.root.display()
                ),
            );
            continue;
        }
        commands.trigger(TwinAssetMounted {
            twin: mount.twin_id,
            authority: mount.authority,
            display_name: mount.display_name,
        });
    }
}

fn unregister_twin_root(
    trigger: On<lunco_workspace::TwinClosed>,
    mut pending_opens: Option<ResMut<lunco_workspace::open::PendingTwinOpens>>,
    roots: Res<TwinRoots>,
    mut pending_mounts: ResMut<PendingTwinAssetMounts>,
    mut commands: Commands,
) {
    if let Some(pending_opens) = pending_opens.as_mut() {
        pending_opens.finish_replacement_for_twin(trigger.event().twin);
    }
    #[cfg(not(target_arch = "wasm32"))]
    pending_mounts
        .0
        .retain(|pending| pending.twin_id != trigger.event().twin);
    #[cfg(target_arch = "wasm32")]
    pending_mounts.pending.remove(&trigger.event().twin);
    if let Err(error) = roots.unregister_root(&trigger.event().root) {
        lunco_core::trigger_runtime_error(
            &mut commands,
            TWIN_ASSET_UNMOUNT_FAILED,
            format!("could not unmount Twin asset root: {error}"),
        );
    }
}

/// Register every LunCo asset source on `app` and insert the shared
/// [`TwinRoots`] resource. The composition root must call this exactly once,
/// before `DefaultPlugins` snapshots Bevy's asset sources.
///
/// | Scheme | Resolves to | Notes |
/// |---|---|---|
/// | `lunco://` | the runtime `assets/` root, then the shared global cache | the engine asset *library* (rovers, parts, downloaded binaries, cached textures) |
/// | `twin://<source-identity>/…` | the currently mounted Twin source resolved at admission | stable user-facing scene and asset addresses |
/// | `twin://<authority>/…` | one exact live mount; Twin `Assets.toml` routes declared paths to one owner | internal asset reads only |
///
/// `lunco://` is path-derived and stateless; `twin://` is separate only because
/// its reader is stateful (it shares [`TwinRoots`] with the resource).
///
/// A cached texture needs no scheme of its own: the cache is already
/// `lunco://`'s fallback, so `lunco://textures/earth.png` reaches the packaged
/// or downloaded copy through the same logical address.
///
/// A **downloaded scenario is just a Twin root** over its cache directory, so it
/// needs no scheme of its own. Each peer uses its local mount authority for
/// loading and the asset owner's stable logical source for content provenance.
/// Mount histories cannot change `Provenance::Content`-derived ids.
///
/// Returns the [`TwinRoots`] handle (already inserted as a resource) for callers
/// that want to pre-register a root before the first scene load.
pub fn register_lunco_asset_sources(app: &mut App) -> std::io::Result<TwinRoots> {
    let assets_dir = lunco_assets_core::assets_dir_abs()?;
    app.register_asset_source(
        crate::library::LIBRARY_SCHEME,
        crate::library::library_asset_source(),
    );

    app.register_asset_source(
        bevy::asset::io::AssetSourceId::Default,
        lunco_assets_core::lunco_source::directory_asset_source(&assets_dir),
    );

    // Engine asset *library* under a NAMED, location-independent scheme so a
    // scene living OUTSIDE the project (an external Twin) can still reference
    // shared parts: `@lunco://vessels/rovers/skid_rover.usda@`.
    //
    // Resolves `assets/` FIRST, then the download cache — so a large binary
    // pulled by `cargo run -p lunco-assets -- download` is reachable at its
    // logical `lunco://` address without any authored file naming the cache.
    app.register_asset_source(
        lunco_assets_core::LUNCO_SCHEME,
        lunco_asset_source(&assets_dir),
    );

    // `twin://` — a root keyed by mount authority: an open Twin's directory, or a
    // downloaded scenario's cache dir. Registered on EVERY platform; the reader
    // goes through `lunco_storage`, so on web it reads the OPFS tree.
    let twin_roots = TwinRoots::default();
    app.register_asset_source(
        lunco_assets_core::TWIN_SCHEME,
        twin_asset_source(&twin_roots),
    );
    app.insert_resource(twin_roots.clone());
    app.add_plugins(TwinRootsPlugin);

    // The read side of the SAME registration: every scheme that gets an
    // `AssetSource` above also declares where its bytes live locally, so callers
    // that must reach them without the `AssetServer` (scenario sync, shader
    // pre-validation, file dialogs) cannot disagree with the readers.
    let schemes = lunco_assets_core::scheme_registry::SchemeRegistry::default();
    schemes
        .register(lunco_assets_core::LUNCO_SCHEME, move |rel| {
            lunco_assets_core::engine_asset_local_path(&lunco_assets_path::uri(
                lunco_assets_core::LUNCO_SCHEME,
                rel,
            ))
            .map_err(Into::into)
        })
        .expect("register the canonical lunco asset scheme");
    schemes
        .register(crate::library::LIBRARY_SCHEME, |rest| {
            crate::library::library_resource_local_path(std::path::Path::new(rest))
                .map_err(Into::into)
        })
        .expect("register source-library asset scheme");
    let roots = twin_roots.clone();
    schemes
        .register(lunco_assets_core::TWIN_SCHEME, move |rest| {
            // `twin://<authority>/<rel>` — the authority selects the mounted root, so this handler is
            // stateful where `lunco://`'s is constant.
            let invalid = || {
                lunco_assets_core::SchemeRegistryError::AssetResolution(format!(
                    "invalid Twin asset `{rest}`"
                ))
            };
            let (name, rel) = lunco_assets_core::split_twin_rel(rest).ok_or_else(invalid)?;
            let rel = lunco_assets_path::relative_path(rel).ok_or_else(invalid)?;
            roots.resolve_file(name, &rel).map_err(|error| {
                lunco_assets_core::SchemeRegistryError::AssetResolution(error.to_string())
            })
        })
        .expect("register the canonical twin asset scheme");
    app.insert_resource(schemes);

    // The common boundary owns the library manifest. Scene catalogs and source
    // browsers require this authoritative listing; script loading itself
    // follows the referenced Rhai asset's Bevy dependency graph and does not
    // scan this manifest. Downloadable datasets are installed by the separate
    // `lunco-assets` runtime package so this source library stays lightweight.
    app.add_plugins(crate::discovery::AssetDiscoveryPlugin);

    Ok(twin_roots)
}

/// Run the application while retaining native asset-event receivers through
/// watcher retirement. A native watcher stops asynchronously when its source
/// drops; its last callback must still have a live receiver after App teardown.
/// Draining happens after the runner has ended, when no UI frame is waiting.
pub fn run_asset_app(app: &mut App) -> AppExit {
    #[cfg(not(target_arch = "wasm32"))]
    let receivers = {
        use bevy::asset::io::AssetSourceId;
        let schemes = match app
            .world()
            .resource::<lunco_assets_core::SchemeRegistry>()
            .schemes()
        {
            Ok(schemes) => schemes,
            Err(error) => {
                error!("Cannot retain asset watcher lifetime: {error}");
                return AppExit::error();
            }
        };
        let server = app.world().resource::<AssetServer>();
        let mut receivers = Vec::new();
        for id in std::iter::once(AssetSourceId::Default)
            .chain(schemes.into_iter().map(AssetSourceId::from))
        {
            let source = match server.get_source(id) {
                Ok(source) => source,
                Err(error) => {
                    error!("Registered asset source is unavailable at runner admission: {error}");
                    return AppExit::error();
                }
            };
            receivers.extend(source.event_receiver().cloned());
            receivers.extend(source.processed_event_receiver().cloned());
        }
        receivers
    };
    let exit = app.run();
    #[cfg(not(target_arch = "wasm32"))]
    for receiver in receivers {
        while receiver.recv_blocking().is_ok() {}
    }
    exit
}

/// Register the generic text asset type after Bevy's [`AssetPlugin`] has been
/// installed by the composition root.
pub fn register_lunco_asset_types(app: &mut App) {
    app.add_plugins(crate::TextAssetPlugin);
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_workspace::{TwinAdded, TwinClosed, WorkspaceResource};

    #[derive(Resource, Default)]
    struct MountedAuthorities(Vec<(lunco_workspace::TwinId, String)>);

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn runner_retains_receivers_until_watcher_callbacks_retire() {
        use bevy::asset::io::{
            AssetSourceBuilder, AssetSourceBuilders, AssetSourceEvent, AssetSourceId, AssetWatcher,
        };
        use std::sync::{
            Arc, Barrier,
            atomic::{AtomicBool, Ordering},
        };
        struct Watch(Option<Box<dyn FnOnce() + Send + Sync>>);
        impl AssetWatcher for Watch {}
        impl Drop for Watch {
            fn drop(&mut self) {
                std::thread::spawn(self.0.take().unwrap());
            }
        }
        let barrier = Arc::new(Barrier::new(2));
        let delivered = Arc::new(AtomicBool::new(false));
        let wait = barrier.clone();
        let sent = delivered.clone();
        let mut builders = AssetSourceBuilders::default();
        builders.insert(
            AssetSourceId::Default,
            AssetSourceBuilder::new(|| {
                Box::new(bevy::asset::io::file::FileAssetReader::new("unused"))
            })
            .with_watcher(move |sender| {
                let wait = wait.clone();
                let sent = sent.clone();
                Some(Box::new(Watch(Some(Box::new(move || {
                    wait.wait();
                    sent.store(
                        sender
                            .send_blocking(AssetSourceEvent::ModifiedAsset("changed.rhai".into()))
                            .is_ok(),
                        Ordering::Release,
                    );
                })))))
            }),
        );
        let mut app = App::new();
        app.init_resource::<lunco_assets_core::SchemeRegistry>();
        app.insert_resource(AssetServer::new(
            Arc::new(builders.build_sources(true, false)),
            bevy::asset::AssetServerMode::Unprocessed,
            true,
            Default::default(),
        ));
        app.set_runner(move |app| {
            drop(app);
            barrier.wait();
            AppExit::Success
        });
        assert!(matches!(run_asset_app(&mut app), AppExit::Success));
        assert!(
            delivered.load(Ordering::Acquire),
            "the callback can finish after its App and source drop"
        );
    }

    #[test]
    fn twin_root_follows_workspace_lifecycle_without_usd() {
        let dir = tempfile::tempdir().expect("temporary Twin root");
        std::fs::write(
            dir.path().join("Assets.toml"),
            "[sample]\nname = \"Sample\"\nurl = \"https://example.invalid/sample.bin\"\ndest = \"terrain/sample.bin\"\n",
        )
        .expect("write Twin asset manifest");
        std::fs::create_dir_all(dir.path().join("terrain")).expect("create authored directory");
        std::fs::write(
            dir.path().join("terrain/sample.bin"),
            b"stale authored copy",
        )
        .expect("write stale authored copy");
        let cached = lunco_assets_core::twin_cache_dir(dir.path()).join("terrain/sample.bin");
        std::fs::create_dir_all(cached.parent().expect("cache parent"))
            .expect("create Twin cache directory");
        std::fs::write(&cached, b"manifest-selected cache copy").expect("write cache artifact");
        let twin = match lunco_twin::TwinMode::open(dir.path()).expect("open Twin folder") {
            lunco_twin::TwinMode::Folder(twin) | lunco_twin::TwinMode::Twin(twin) => twin,
            lunco_twin::TwinMode::Orphan(_) => panic!("a directory must open as a Twin folder"),
        };
        let root = twin.root.clone();

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<WorkspaceResource>();
        app.add_plugins(TwinRootsPlugin);
        app.init_resource::<MountedAuthorities>();
        app.add_observer(
            |trigger: On<TwinAssetMounted>, mut mounted: ResMut<MountedAuthorities>| {
                mounted
                    .0
                    .push((trigger.event().twin, trigger.event().authority.clone()));
            },
        );
        app.update();

        let twin_id = app
            .world_mut()
            .resource_mut::<WorkspaceResource>()
            .add_twin(twin);
        app.world_mut().trigger(TwinAdded { twin: twin_id });
        let roots = app.world().resource::<TwinRoots>();
        let pending_authority = roots
            .authority_for_root(&root)
            .expect("read pending Twin registry")
            .expect("workspace Twin root is mounted before route preparation");
        assert_eq!(
            roots.resolve_file(
                &pending_authority,
                std::path::Path::new("terrain/sample.bin"),
            ),
            Err(lunco_assets_core::TwinRootsError::AssetRoutesUnavailable),
            "Twin content remains unreadable until its manifest owner map is ready"
        );
        for _ in 0..100 {
            app.update();
            if !app.world().resource::<MountedAuthorities>().0.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let roots = app.world().resource::<TwinRoots>();
        let assigned = roots
            .authority_for_root(&root)
            .expect("read Twin registry")
            .expect("workspace Twin is mounted in the asset source");
        assert_eq!(roots.root_for(&assigned), Ok(Some(root.clone())));
        assert_eq!(
            app.world().resource::<MountedAuthorities>().0,
            vec![(twin_id, assigned.clone())]
        );
        assert_eq!(
            roots.resolve_file(&assigned, std::path::Path::new("terrain/sample.bin")),
            Ok(Some(std::fs::canonicalize(cached).unwrap())),
            "the mount event waits until manifest routes select cache ownership"
        );

        let was_active = app.world().resource::<WorkspaceResource>().active_twin == Some(twin_id);
        app.world_mut()
            .resource_mut::<WorkspaceResource>()
            .close_twin(twin_id);
        app.world_mut().trigger(TwinClosed {
            twin: twin_id,
            root,
            was_active,
        });
        assert!(
            app.world()
                .resource::<TwinRoots>()
                .authorities()
                .expect("read Twin registry")
                .is_empty()
        );
    }
}
