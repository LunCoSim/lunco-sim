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
        let package = unique_package(root, &parsed.qualified)?;
        if package.members.iter().any(|member| member == first) {
            return Err(
                "a resource path cannot start with a class name; use a qualified URI authority"
                    .into(),
            );
        }
        let top = unique_package(root, namespace)?;
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
    root: &'a super::SourceRoot,
    namespace: &str,
) -> Result<&'a lunco_modelica_ast::resource_uri::PackageSource, String> {
    let mut matches = root
        .package_sources
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
        let packages = lunco_modelica_ast::resource_uri::source_set_package_sources([
            (document.definition(), "/owner/A/package.mo"),
            (child.definition(), "/owner/A/Separate.mo"),
        ]);
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
        let duplicate = registry.roots["source"].package_sources[0].clone();
        registry
            .roots
            .get_mut("source")
            .unwrap()
            .package_sources
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
