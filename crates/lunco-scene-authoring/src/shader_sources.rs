//! Asynchronous preparation of live WESL edits. Registered asset readers own
//! dependency resolution; only the newest edit for a live source may publish.

use bevy::{
    prelude::*,
    shader::Shader,
    tasks::{IoTaskPool, Task, futures_lite::future},
};
use lunco_assets_core::twin_source::TwinRoots;
use std::{collections::BTreeMap, path::PathBuf};

struct PendingSource {
    handle: Handle<Shader>,
    mount: Option<(String, PathBuf)>,
    task: Task<Result<Shader, String>>,
}

#[derive(Resource, Default)]
pub struct PendingShaderSources(BTreeMap<String, PendingSource>);

impl PendingShaderSources {
    pub fn cancel(&mut self, paths: &[String]) {
        self.0.retain(|path, _| !paths.contains(path));
    }

    pub fn queue(
        &mut self,
        server: &AssetServer,
        roots: Option<&TwinRoots>,
        paths: &[String],
        source: &str,
    ) -> Result<(), String> {
        // Resolve every lease before enqueuing any edit.
        let admitted = paths
            .iter()
            .map(|path| {
                let mount = if let Some((authority, _)) = lunco_assets_core::parse_twin_uri(path) {
                    let root = roots
                        .ok_or("Twin asset registry is unavailable")?
                        .root_of(authority)
                        .map_err(|error| error.to_string())?
                        .ok_or_else(|| format!("shader source mount `{authority}` is retired"))?;
                    Some((authority.to_owned(), root))
                } else {
                    None
                };
                let load_path =
                    lunco_assets_core::asset_path::load_asset_path(path, None, None, None)
                        .map_err(|error| error.to_string())?;
                Ok((path.clone(), mount, load_path))
            })
            .collect::<Result<Vec<_>, String>>()?;
        for (path, mount, load_path) in admitted {
            let handle = server.load::<Shader>(load_path);
            let pending_handle = handle.clone();
            let server = server.clone();
            let source = source.to_owned();
            let shader_path = path.clone();
            let task = IoTaskPool::get().spawn(async move {
                // Finish the native file load before publishing an edit so an
                // older load cannot overwrite the new source afterwards.
                server
                    .wait_for_asset(&pending_handle)
                    .await
                    .map_err(|error| error.to_string())?;
                Ok(Shader::from_wesl_with_dependencies(source, shader_path, &server).await)
            });
            // Dropping the superseded task cancels its publication.
            self.0.insert(
                path,
                PendingSource {
                    handle,
                    mount,
                    task,
                },
            );
        }
        Ok(())
    }
}

pub fn publish_shader_sources(
    mut pending: ResMut<PendingShaderSources>,
    roots: Option<Res<TwinRoots>>,
    mut shaders: ResMut<Assets<Shader>>,
) {
    pending.0.retain(|path, pending| {
        if let Some((authority, root)) = &pending.mount {
            match roots.as_deref().map(|roots| roots.root_of(authority)) {
                Some(Ok(Some(current))) if &current == root => {}
                Some(Err(error)) => {
                    warn!("SET_SHADER_SOURCE: {path}: {error}");
                    return false;
                }
                _ => return false,
            }
        }
        let Some(result) = future::block_on(future::poll_once(&mut pending.task)) else {
            return true;
        };
        match result {
            Ok(mut shader) => {
                let Some(current) = shaders.get(pending.handle.id()) else {
                    warn!("SET_SHADER_SOURCE: {path}: source asset was retired during preparation");
                    return false;
                };
                shader.shader_defs = current.shader_defs.clone();
                shader.validate_shader = current.validate_shader.clone();
                if let Err(error) = shaders.insert(pending.handle.id(), shader) {
                    warn!("SET_SHADER_SOURCE: {path}: {error}");
                }
            }
            Err(error) => warn!("SET_SHADER_SOURCE: {path}: preparation failed: {error}"),
        }
        false
    });
}

pub fn retire_shader_sources(
    closed: On<lunco_workspace::TwinClosed>,
    mut pending: ResMut<PendingShaderSources>,
) {
    pending.0.retain(|_, pending| {
        pending
            .mount
            .as_ref()
            .is_none_or(|(_, root)| root != &closed.root)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::{
        AssetApp, AssetMetaCheck, AssetPlugin,
        io::{
            AssetSourceBuilder,
            memory::{Dir, MemoryAssetReader},
        },
    };
    use std::{
        path::Path,
        time::{Duration, Instant},
    };

    #[test]
    fn asynchronous_source_publication_keeps_latest_dependencies_and_honors_cancellation() {
        let directory = Dir::default();
        directory.insert_asset(Path::new("root.wesl"), b"fn original() {}".as_slice());
        directory.insert_asset(
            Path::new("helper.wesl"),
            b"fn value() -> f32 { return 0.5; }".as_slice(),
        );
        let mut app = App::new();
        app.register_asset_source(
            "lunco",
            AssetSourceBuilder::new(move || {
                Box::new(MemoryAssetReader {
                    root: directory.clone(),
                })
            }),
        );
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            AssetPlugin {
                meta_check: AssetMetaCheck::Never,
                ..default()
            },
        ));
        app.init_asset::<Shader>()
            .init_asset_loader::<bevy::shader::ShaderLoader>();
        app.init_resource::<PendingShaderSources>()
            .add_systems(Update, publish_shader_sources);
        app.finish();
        app.cleanup();
        let server = app.world().resource::<AssetServer>().clone();
        let handle = server.load::<Shader>("lunco://root.wesl");
        let paths = vec!["lunco://root.wesl".to_owned()];
        let latest = "import super::helper::value; fn latest() -> f32 { return value(); }";
        let pending = &mut *app.world_mut().resource_mut::<PendingShaderSources>();
        pending
            .queue(&server, None, &paths, "fn superseded() {}")
            .unwrap();
        pending.queue(&server, None, &paths, latest).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            app.update();
            if app.world().resource::<PendingShaderSources>().0.is_empty() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "source preparation did not settle"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let shader = app
            .world()
            .resource::<Assets<Shader>>()
            .get(&handle)
            .unwrap();
        assert_eq!(shader.source.as_str(), latest);
        assert_eq!(shader.file_dependencies.len(), 1);
        let mut pending = app.world_mut().resource_mut::<PendingShaderSources>();
        pending
            .queue(&server, None, &paths, "fn cancelled() {}")
            .unwrap();
        pending.cancel(&paths);
        assert!(
            pending
                .queue(
                    &server,
                    None,
                    &["twin://retired/root.wesl".to_owned()],
                    latest
                )
                .is_err()
        );
        drop(pending);
        app.update();
        assert_eq!(
            app.world()
                .resource::<Assets<Shader>>()
                .get(&handle)
                .unwrap()
                .source
                .as_str(),
            latest
        );
    }
}
