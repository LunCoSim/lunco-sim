//! One place that registers **all** LunCo Bevy asset sources, so every binary
//! (luncosim, sandbox, web, model_viewer) gets the *same* schemes instead
//! of each `main()` hand-listing a divergent subset.
//!
//! Asset sources must be registered **before** `AssetPlugin`/`DefaultPlugins`
//! builds (Bevy snapshots the source registry at that point), so call
//! [`register_lunco_asset_sources`] right after `App::new()`, before
//! `.add_plugins(DefaultPlugins)`.

use bevy::prelude::*;

use crate::lunco_source::lunco_asset_source;
use crate::twin_source::{twin_asset_source, TwinRoots};

/// Mounts workspace Twins into the shared `twin://` asset source.
///
/// This is asset lifecycle, not USD lifecycle: Modelica-only lunica still
/// needs Twin-relative reads and Twin-scoped datasets, while a simulation host
/// must not depend on a particular scene domain to keep the source mounted.
pub struct TwinRootsPlugin;

impl Plugin for TwinRootsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TwinRoots>()
            .add_observer(register_twin_root)
            .add_observer(unregister_twin_root);
    }
}

fn twin_authority(twin: &lunco_twin::Twin) -> String {
    twin.manifest
        .as_ref()
        .map(|manifest| manifest.name.clone())
        .filter(|name| !name.is_empty())
        .or_else(|| {
            twin.root
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "twin".to_string())
}

fn register_twin_root(
    trigger: On<lunco_workspace::TwinAdded>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    roots: Res<TwinRoots>,
) {
    let Some(workspace) = workspace else {
        return;
    };
    let twin_id = trigger.event().twin;
    let Some(twin) = workspace.twin(twin_id) else {
        return;
    };
    let assigned = roots.register(twin_authority(twin), &twin.root);
    info!(
        "[twin-roots] mounted `{assigned}` at {}",
        twin.root.display()
    );
}

fn unregister_twin_root(trigger: On<lunco_workspace::TwinClosed>, roots: Res<TwinRoots>) {
    roots.unregister_root(&trigger.event().root);
}

/// Register every LunCo asset source on `app` and insert the shared
/// [`TwinRoots`] resource. The composition root must call this exactly once,
/// before `DefaultPlugins` snapshots Bevy's asset sources.
///
/// | Scheme | Resolves to | Notes |
/// |---|---|---|
/// | `lunco://` | `<cwd>/assets`, then `<cache>` | the engine asset *library* (rovers, parts, downloaded binaries, cached textures) |
/// | `twin://<name>/…` | open Twin roots | Twin scenes AND downloaded scenarios — native fs + web OPFS, via `lunco_storage` |
///
/// `lunco://` is path-derived and stateless; `twin://` is separate only because
/// its reader is stateful (it shares [`TwinRoots`] with the resource).
///
/// A cached texture needs no scheme of its own either: the cache is already
/// `lunco://`'s fallback, so `lunco://textures/earth.png` reaches exactly what a
/// `cached_textures://earth.png` did — and on the web it resolves to where
/// `build_web.sh` actually stages the file, which the cache-rooted spelling did
/// not.
///
/// A **downloaded scenario is just a Twin root** over its cache directory, so it
/// needs no scheme of its own: one `twin://<name>/<rel>` names the scene on every
/// peer regardless of where that peer's bytes live. That is what keeps
/// `Provenance::Content`-derived ids identical across host and client.
///
/// Returns the [`TwinRoots`] handle (already inserted as a resource) for callers
/// that want to pre-register a root before the first scene load.
pub fn register_lunco_asset_sources(app: &mut App) -> TwinRoots {
    let assets_dir = crate::assets_dir_abs();

    // Engine asset *library* under a NAMED, location-independent scheme so a
    // scene living OUTSIDE the project (an external Twin) can still reference
    // shared parts: `@lunco://vessels/rovers/skid_rover.usda@`.
    //
    // Resolves `assets/` FIRST, then the download cache — so a large binary
    // pulled by `cargo run -p lunco-assets -- download` is reachable at its
    // logical `lunco://` address without any authored file naming the cache.
    // (This replaced `lunco-lib://`, which addressed the cache directly and so
    // baked a machine-local location into shipped `.usda` files.)
    app.register_asset_source(crate::LUNCO_SCHEME, lunco_asset_source(&assets_dir));

    // `twin://` — a named root, keyed by Twin name: an open Twin's directory, or a
    // downloaded scenario's cache dir. Registered on EVERY platform; the reader
    // goes through `lunco_storage`, so on web it reads the OPFS tree.
    let twin_roots = TwinRoots::default();
    app.register_asset_source(crate::TWIN_SCHEME, twin_asset_source(&twin_roots));
    app.insert_resource(twin_roots.clone());
    app.add_plugins(TwinRootsPlugin);

    // The read side of the SAME registration: every scheme that gets an
    // `AssetSource` above also declares where its bytes live locally, so callers
    // that must reach them without the `AssetServer` (scenario sync, shader
    // pre-validation, file dialogs) cannot disagree with the readers.
    let schemes = crate::scheme_registry::SchemeRegistry::default();
    schemes.register(crate::LUNCO_SCHEME, move |rel| {
        if !crate::asset_path::is_safe_relative_path(rel) {
            return None;
        }
        let authored = assets_dir.join(rel);
        #[cfg(not(target_arch = "wasm32"))]
        if authored.exists() {
            return crate::existing_path_within_root(&assets_dir, std::path::Path::new(rel))
                .filter(|path| path.is_file());
        }
        Some(authored)
    });
    let roots = twin_roots.clone();
    schemes.register(crate::TWIN_SCHEME, move |rest| {
        // `twin://<name>/<rel>` — the name selects the root, so this handler is
        // stateful where `lunco://`'s is constant.
        let (name, rel) = crate::split_twin_rel(rest)?;
        let rel = crate::asset_path::relative_path(rel)?;
        roots.resolve_file(name, &rel)
    });
    app.insert_resource(schemes);

    // Declared-dataset registry: owns every download, scans each open Twin's
    // `Assets.toml`, and is the ONLY thing in the engine that fetches. Lives
    // here so every app that registers asset sources gets it — a domain crate
    // that had to remember to add it would eventually forget and grow its own
    // downloader.
    // The same common boundary also owns the library manifest. Script imports,
    // scene catalogs, and source browsers all require this authoritative file
    // listing; installing it here prevents a host-specific missing-resource
    // panic when a domain plugin preloads scripts before its UI is visible.
    app.add_plugins(crate::discovery::AssetDiscoveryPlugin);
    app.add_plugins(crate::datasets::DatasetsPlugin);

    twin_roots
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_workspace::{TwinAdded, TwinClosed, WorkspaceResource};

    #[test]
    fn twin_root_follows_workspace_lifecycle_without_usd() {
        let dir = tempfile::tempdir().expect("temporary Twin root");
        let twin = match lunco_twin::TwinMode::open(dir.path()).expect("open Twin folder") {
            lunco_twin::TwinMode::Folder(twin) | lunco_twin::TwinMode::Twin(twin) => twin,
            lunco_twin::TwinMode::Orphan(_) => panic!("a directory must open as a Twin folder"),
        };
        let root = twin.root.clone();

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<WorkspaceResource>();
        app.add_plugins(TwinRootsPlugin);
        app.update();

        let twin_id = app
            .world_mut()
            .resource_mut::<WorkspaceResource>()
            .add_twin(twin);
        app.world_mut().trigger(TwinAdded { twin: twin_id });

        let roots = app.world().resource::<TwinRoots>();
        let assigned = roots
            .name_for_root(&root)
            .expect("workspace Twin is mounted in the asset source");
        assert_eq!(roots.root_for(&assigned), Some(root.clone()));

        let was_active = app.world().resource::<WorkspaceResource>().active_twin == Some(twin_id);
        app.world_mut()
            .resource_mut::<WorkspaceResource>()
            .close_twin(twin_id);
        app.world_mut().trigger(TwinClosed {
            twin: twin_id,
            root,
            was_active,
        });
        assert!(app.world().resource::<TwinRoots>().names().is_empty());
    }
}
