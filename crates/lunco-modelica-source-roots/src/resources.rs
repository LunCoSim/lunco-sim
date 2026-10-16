//! Immutable language facts for the shared asset reader. This snapshot is a
//! projection of source-root admission, never another source or storage owner.

use super::{LoadState, SourceRootKind, SourceRootRegistry};
use bevy::asset::AssetPath;
use bevy::prelude::DetectChangesMut;
use lunco_assets_core::registered_read::AssetReadTarget;
use lunco_modelica_ast::resource_uri::ModelicaUri;
use std::path::{Path, PathBuf};

#[derive(Clone, Default)]
pub struct ResourceContext {
    registry: SourceRootRegistry,
}

impl ResourceContext {
    pub fn capture(registry: &SourceRootRegistry) -> Self {
        Self {
            registry: registry.clone(),
        }
    }

    /// Runs on the shared asset worker. URI syntax and package facts become a
    /// typed address; containment, I/O and byte limits stay in asset crates.
    pub fn resolve(&self, uri: &str) -> Result<AssetReadTarget, String> {
        let parsed = ModelicaUri::parse(uri)?.ok_or("not a Modelica URI")?;
        let relative = parsed.package_relative_resource()?;
        let namespace = rumoca_core::qualified_name_segments(&parsed.qualified)
            .next()
            .ok_or("empty package name")?;
        let owner = self.registry.resolve_source_root(namespace)?;
        let root = self
            .registry
            .roots
            .get(&owner)
            .ok_or("admitted source owner disappeared")?;
        if let LoadState::Failed(error) = &root.state {
            return Err(error.clone());
        }
        let first = parsed
            .resource
            .as_ref()
            .and_then(|path| path.iter().next())
            .and_then(|part| part.to_str())
            .ok_or("empty resource path")?;
        if matches!(root.kind, SourceRootKind::SourceLibrary) {
            let package =
                lunco_modelica_library::package_paths::package_declaration(&parsed.qualified)?;
            if package.members.iter().any(|member| member == first) {
                return Err(
                    "a resource path cannot start with a class name; use a qualified URI authority"
                        .into(),
                );
            }
            let source = lunco_modelica_library::package_paths::locate_library_file(namespace)
                .ok_or_else(|| format!("package `{namespace}` has no source file"))?;
            return lunco_assets_runtime::library::library_resource_path(&source, &relative)
                .map(AssetReadTarget::Registered)
                .map_err(|error| error.to_string());
        }
        if !matches!(root.state, LoadState::Ready) {
            return Err(format!(
                "Modelica source set `{owner}` is awaiting admission"
            ));
        }
        let sources = root.effective_sources();
        let packages = lunco_modelica_ast::resource_uri::package_sources_for_files(sources)?;
        let package = unique_package(&packages, &parsed.qualified)?;
        if package.members.iter().any(|member| member == first) {
            return Err(
                "a resource path cannot start with a class name; use a qualified URI authority"
                    .into(),
            );
        }
        let top = unique_package(&packages, namespace)?;
        match &root.kind {
            SourceRootKind::SourceLibrary => Err("source-library resource was not admitted".into()),
            SourceRootKind::Bundled { filename } => Ok(AssetReadTarget::Registered(
                AssetPath::from_path_buf(
                    Path::new("models")
                        .join(
                            Path::new(filename)
                                .parent()
                                .ok_or("source has no directory")?,
                        )
                        .join(relative),
                )
                .with_source(lunco_assets_core::LUNCO_SCHEME),
            )),
            SourceRootKind::BundledPackage { root } => {
                let source = lunco_assets_path::relative_path(&top.source_uri)
                    .ok_or("unsafe bundled package source")?;
                let parent = source.parent().ok_or("source has no directory")?;
                Ok(AssetReadTarget::Registered(
                    AssetPath::from_path_buf(
                        Path::new("models").join(root).join(parent).join(relative),
                    )
                    .with_source(lunco_assets_core::LUNCO_SCHEME),
                ))
            }
            SourceRootKind::Disk { .. } => directory_target(Path::new(&top.source_uri), relative),
            SourceRootKind::WorkspaceFile { path } => directory_target(path, relative),
            SourceRootKind::SessionDocument { .. } => {
                Err("save the package before resolving its external resources".into())
            }
        }
    }
}

fn unique_package<'a>(
    packages: &'a [lunco_modelica_ast::resource_uri::PackageSource],
    namespace: &str,
) -> Result<&'a lunco_modelica_ast::resource_uri::PackageSource, String> {
    let mut matches = packages
        .iter()
        .filter(|package| package.namespace == namespace);
    let package = matches
        .next()
        .ok_or_else(|| format!("package `{namespace}` has no admitted declaration"))?;
    if matches.next().is_some() {
        return Err(format!("package `{namespace}` has ambiguous declarations"));
    }
    Ok(package)
}

fn directory_target(source: &Path, relative: PathBuf) -> Result<AssetReadTarget, String> {
    let parent = source
        .parent()
        .ok_or("source file has no owning directory")?;
    Ok(AssetReadTarget::Directory {
        directory: lunco_assets_core::AssetDirectory::File(parent.to_path_buf()),
        relative,
    })
}

/// Read-only bridge for consumers outside ECS, including egui byte loaders.
/// Workers clone an immutable snapshot before resolving; publication never
/// waits for resource I/O while holding this lock.
#[derive(bevy::prelude::Resource, Clone, Default)]
pub struct ResourceContextHandle(
    std::sync::Arc<std::sync::RwLock<std::sync::Arc<ResourceContext>>>,
);

impl ResourceContextHandle {
    pub fn resolve(&self, uri: &str) -> Result<AssetReadTarget, String> {
        let context = self
            .0
            .read()
            .map_err(|_| "Modelica resource context is unavailable")?
            .clone();
        context.resolve(uri)
    }
}

pub(super) fn publish_resource_context(
    registry: bevy::prelude::Res<SourceRootRegistry>,
    mut handle: bevy::prelude::ResMut<ResourceContextHandle>,
    mut commands: bevy::prelude::Commands,
) {
    let snapshot = std::sync::Arc::new(ResourceContext::capture(&registry));
    match handle.0.write() {
        Ok(mut context) => *context = snapshot,
        Err(_) => lunco_core::trigger_runtime_error(
            &mut commands,
            "modelica-resource-context-unavailable",
            "Modelica resource context lock is poisoned",
        ),
    }
    // This change edge invalidates dependent UI byte/image caches.
    handle.set_changed();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn asset_resolution_contract_relocation() {
        use lunco_modelica_ast::resource_uri::SourceFileFacts;
        fn facts(uri: &str, source: &str) -> SourceFileFacts {
            let parsed = rumoca_compile::compile::ParsedSourceDocument::parse(uri, source).unwrap();
            SourceFileFacts::parsed(parsed.definition(), uri)
        }
        let old_uri = "/owner/models/Probe.mo";
        let new_uri = "/owner/models/Renamed.mo";
        let before = facts(old_uri, "package Probe end Probe;");
        let after = facts(new_uri, "package Renamed end Renamed;");
        let doc = lunco_doc::DocumentId::new(1);
        let mut registry = SourceRootRegistry::default();
        registry.register_disk_root("source", PathBuf::from("/owner/models"));
        assert!(registry.complete_load(
            "source",
            0,
            vec!["Probe".into()],
            vec![before.clone()],
            None
        ));
        registry
            .roots
            .get_mut("source")
            .unwrap()
            .refresh_document_source(doc, before.clone(), true);
        let mut app = bevy::prelude::App::new();
        app.insert_resource(registry);
        app.init_resource::<super::super::PendingSourceRootReloads>();
        app.add_observer(super::super::relocate_source_roots);
        app.world_mut().trigger(lunco_workspace::FileRenamed {
            twin: lunco_workspace::TwinId::new(1),
            old_abs: PathBuf::from(old_uri),
            new_abs: PathBuf::from(new_uri),
            is_dir: false,
        });
        let mut registry = app.world_mut().resource_mut::<SourceRootRegistry>();
        assert!(!registry.complete_load("source", 0, vec!["Probe".into()], vec![before], None));
        let root = registry.roots.get_mut("source").unwrap();
        assert_eq!(root.source_files[0].source_uri, new_uri);
        assert_eq!(root.document_sources[&doc].packages[0].source_uri, new_uri);
        // Automatic save can precede parsing. Its exact source identity must
        // be replaced by the completed parse rather than surviving as an error.
        let mut pending = root.document_sources[&doc].clone();
        pending.error = Some("current parse pending".into());
        root.refresh_document_source(doc, pending, true);
        root.refresh_document_source(doc, after.clone(), true);
        root.document_sources.remove(&doc);
        let operation = root.operation_id;
        assert!(registry.complete_load(
            "source",
            operation,
            vec!["Renamed".into()],
            vec![after],
            None
        ));
        let context = ResourceContext::capture(&registry);
        assert!(
            context
                .resolve("modelica://Renamed/Resources/plot.png")
                .is_ok()
        );
        assert!(
            context
                .resolve("modelica://Probe/Resources/plot.png")
                .is_err()
        );
        let root = registry.roots.get_mut("source").unwrap();
        root.state = LoadState::Loading {
            progress: 0.0,
            started: web_time::Instant::now(),
        };
        let saved = facts(
            new_uri,
            "package Renamed package Saved end Saved; end Renamed;",
        );
        root.refresh_document_source(doc, saved, true);
        drop(registry);
        app.world_mut().trigger(lunco_workspace::FileRenamed {
            twin: lunco_workspace::TwinId::new(1),
            old_abs: PathBuf::from("/owner/models"),
            new_abs: PathBuf::from("/owner/moved"),
            is_dir: true,
        });
        let mut registry = app.world_mut().resource_mut::<SourceRootRegistry>();
        let root = registry.roots.get_mut("source").unwrap();
        assert!(
            matches!(&root.kind, SourceRootKind::Disk { root_dir } if root_dir == Path::new("/owner/moved"))
        );
        assert_eq!(root.source_files[0].source_uri, "/owner/moved/Renamed.mo");
        assert!(
            root.pending_saved_sources
                .contains_key("/owner/moved/Renamed.mo")
        );
        assert!(!root.pending_saved_sources.contains_key(new_uri));
        root.document_sources.remove(&doc);
        let current = root.operation_id;
        assert!(!registry.complete_load("source", operation, vec!["Probe".into()], vec![], None));
        let relocated = facts("/owner/moved/Renamed.mo", "package Renamed end Renamed;");
        assert!(registry.complete_load(
            "source",
            current,
            vec!["Renamed".into()],
            vec![relocated],
            None
        ));
        assert!(
            ResourceContext::capture(&registry)
                .resolve("modelica://Renamed.Saved/Resources/plot.png")
                .is_ok()
        );
        match ResourceContext::capture(&registry)
            .resolve("modelica://Renamed/Resources/plot.png")
            .unwrap()
        {
            AssetReadTarget::Directory { directory, .. } => {
                assert_eq!(directory.path(), Path::new("/owner/moved"))
            }
            _ => panic!("expected relocated directory"),
        }
    }

    #[test]
    fn asset_resolution_contract_saved_admission() {
        let mut registry = SourceRootRegistry::default();
        registry.register_disk_root("source", PathBuf::from("/owner"));
        let uri = "/owner/A.mo";
        let old = rumoca_compile::compile::ParsedSourceDocument::parse(
            uri,
            "package A package Before end Before; end A;",
        )
        .unwrap();
        let saved = rumoca_compile::compile::ParsedSourceDocument::parse(
            uri,
            "package A package After end After; end A;",
        )
        .unwrap();
        let id = lunco_doc::DocumentId::new(1);
        registry
            .roots
            .get_mut("source")
            .unwrap()
            .refresh_document_source(
                id,
                lunco_modelica_ast::resource_uri::SourceFileFacts::parsed(saved.definition(), uri),
                true,
            );
        registry
            .roots
            .get_mut("source")
            .unwrap()
            .document_sources
            .remove(&id);
        assert!(
            ResourceContext::capture(&registry)
                .resolve("modelica://A.After/file.bin")
                .unwrap_err()
                .contains("awaiting admission")
        );
        assert!(registry.complete_load(
            "source",
            0,
            vec!["A".into()],
            vec![lunco_modelica_ast::resource_uri::SourceFileFacts::parsed(
                old.definition(),
                uri
            )],
            None
        ));
        let context = ResourceContext::capture(&registry);
        assert!(context.resolve("modelica://A.After/file.bin").is_ok());
        assert!(context.resolve("modelica://A.Before/file.bin").is_err());
    }

    #[test]
    fn asset_resolution_contract() {
        let mut registry = SourceRootRegistry::default();
        registry.register_disk_root("source", PathBuf::from("/owner"));
        let document = rumoca_compile::compile::ParsedSourceDocument::parse(
            "package.mo",
            "package A package B model Child end Child; end B; end A;",
        )
        .unwrap();
        let child = rumoca_compile::compile::ParsedSourceDocument::parse(
            "B.mo",
            "within A; model Separate end Separate;",
        )
        .unwrap();
        let packages = vec![
            lunco_modelica_ast::resource_uri::SourceFileFacts::parsed(
                document.definition(),
                "/owner/A/package.mo",
            ),
            lunco_modelica_ast::resource_uri::SourceFileFacts::parsed(
                child.definition(),
                "/owner/A/Separate.mo",
            ),
        ];
        assert!(registry.complete_load("source", 0, vec!["A".into()], packages, None));
        let context = ResourceContext::capture(&registry);
        let resolved = context
            .resolve("MoDeLiCa://A.B/Resources/literal%20%23%20%25.png")
            .unwrap();
        match resolved {
            AssetReadTarget::Directory {
                directory,
                relative,
            } => {
                assert_eq!(directory.path(), Path::new("/owner/A"));
                assert_eq!(relative, Path::new("B/Resources/literal # %.png"));
            }
            _ => panic!("expected the admitted package directory"),
        }
        assert!(
            context
                .resolve("modelica://A/B/Resources/image.png")
                .unwrap_err()
                .contains("class name")
        );
        assert!(context.resolve("modelica://A.B/Child/file.bin").is_err());
        assert!(context.resolve("modelica://A/Separate/file.bin").is_err());
        assert!(context.resolve("modelica://A.Missing/file.bin").is_err());
        let mut duplicate = registry.roots["source"]
            .source_files
            .iter()
            .find(|file| file.packages.iter().any(|package| package.namespace == "A"))
            .unwrap()
            .clone();
        duplicate.source_uri = "/owner/A/duplicate.mo".into();
        for package in &mut duplicate.packages {
            package.source_uri = duplicate.source_uri.clone();
        }
        registry
            .roots
            .get_mut("source")
            .unwrap()
            .source_files
            .push(duplicate);
        assert!(
            ResourceContext::capture(&registry)
                .resolve("modelica://A/file.bin")
                .unwrap_err()
                .contains("ambiguous")
        );
        registry.register_disk_root("another", PathBuf::from("/other"));
        assert!(registry.complete_load("another", 0, vec!["A".into()], Vec::new(), None));
        assert!(
            ResourceContext::capture(&registry)
                .resolve("modelica://A/file.bin")
                .unwrap_err()
                .contains("multiple")
        );
        registry.roots.clear();
        assert!(
            ResourceContext::capture(&registry)
                .resolve("modelica://A/file.bin")
                .unwrap_err()
                .contains("not registered")
        );
    }
}
