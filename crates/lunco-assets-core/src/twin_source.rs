//! `twin://` asset source — reads each open Twin's scene and its **co-located**
//! assets relative to that Twin's root.
//!
//! Lives here next to the other asset-source plumbing ([`crate::cache_dir`],
//! [`crate::lunco_lib_path`], …): this crate is the home for "where assets live
//! and how Bevy reaches them".
//!
//! ## Why it exists
//! An external Twin (a scene in its own repo, outside the engine project) must
//! stay portable. The scene file stores only *relative* refs (`@terrain.glb@`)
//! and library refs (`@lunco://vessels/…@`) — never an absolute path. But Bevy's
//! `AssetServer` only reads from sources registered at app-build time, and on
//! the web there is no filesystem at all, so we can't lean on `std::fs`. So we
//! register ONE `twin://` source backed by a small **registry of Twin roots**,
//! reading through [`lunco_storage`] so the SAME scheme serves native and web.
//!
//! A root is an open Twin's directory OR a downloaded scenario's cache directory.
//! Both use the same stable logical source identity on every peer.
//!
//! ## Path shape — `twin://<mount>/<relative>`
//! The first path segment is an authority assigned to one mount lifetime; the
//! rest is relative to its root. Authorities are never rebound after unmount,
//! so cached assets and late readers cannot acquire another Twin's bytes.
//! [`stable_source_path`] converts the load address to the stable
//! logical source used for provenance. Dependencies and imports keep their
//! mount address. `twin://` is internal and never authored into a Twin file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use bevy::asset::io::{
    AssetReader, AssetReaderError, AssetSourceBuilder, ErasedAssetReader, PathStream, Reader,
    VecReader,
};
use bevy::prelude::*;

/// The asset-source scheme for Twin-root-relative assets — the name it is
/// registered under, both as a Bevy `AssetSource` and in the
/// [`SchemeRegistry`](crate::scheme_registry::SchemeRegistry).
pub const TWIN_SCHEME: &str = "twin";

// A display name becomes one URI component. Unreserved ASCII stays readable;
// percent and delimiters are encoded so distinct authored names stay distinct.
const TWIN_NAME_COMPONENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');
const TWIN_DOT_NAME_COMPONENT: &percent_encoding::AsciiSet = &TWIN_NAME_COMPONENT.add(b'.');

/// Failure of the authoritative open-Twin registry. A failed lock is not an
/// absent Twin: callers must not publish a mounted/unmounted postcondition
/// when the registry could not perform the requested mutation.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum TwinRootsError {
    #[error("Twin root registry is not installed in this host")]
    RegistryUnavailable,
    #[error("Twin root registry is unavailable because its lock is poisoned")]
    RegistryPoisoned,
    #[error("invalid Twin asset authority `{0}`")]
    InvalidAuthority(String),
    #[error("unknown Twin asset authority `{0}`")]
    UnknownAuthority(String),
    #[error("Twin asset authority namespace is exhausted")]
    AuthorityExhausted,
    #[error("requested logical Twin identity `{requested}` was assigned `{assigned}`")]
    LogicalIdentityMismatch { requested: String, assigned: String },
    #[error("invalid Twin overlay path `{0}`")]
    InvalidOverlayPath(String),
    #[error("Twin asset resolution failed ({0:?}): {1}")]
    AssetResolution(std::io::ErrorKind, String),
    #[error("Twin root resolution failed ({0:?}): {1}")]
    RootResolution(std::io::ErrorKind, String),
}

/// The `twin://<name>/<rel>` URI naming `rel` inside the Twin `name` — the ONE
/// place the scheme string is spelled into an address. Callers that hand-rolled
/// `format!("twin://{name}/{rel}")` duplicated resolution knowledge this crate
/// owns; a scheme rename must not require editing five crates.
///
/// `name` is the returned mount authority when loading. `rel` is normalized to
/// forward slashes and stripped of a leading `/`. Logical identity conversion
/// belongs to [`stable_source_path`], not this load-address helper.
pub fn twin_uri(name: impl AsRef<str>, rel: impl AsRef<Path>) -> String {
    let rel = crate::asset_path::slashed(rel);
    crate::asset_path::uri(
        TWIN_SCHEME,
        &format!("{}/{}", name.as_ref(), rel.trim_start_matches('/')),
    )
}

/// Split a `twin://<name>/<rel>` URI into its parts, or `None` when it carries a
/// different scheme (or no scheme at all). The parsing inverse of [`twin_uri`].
pub fn parse_twin_uri(uri: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = crate::asset_path::split_scheme(uri)?;
    split_twin_rel((scheme == TWIN_SCHEME).then_some(rest)?)
}

/// Split the scheme-stripped remainder of a `twin://` address — the
/// `<name>/<rel>` form an `AssetReader` or scheme handler receives — into the
/// Twin name and the Twin-relative path. [`parse_twin_uri`] for callers that
/// hold the full URI.
pub fn split_twin_rel(rest: &str) -> Option<(&str, &str)> {
    rest.split_once(['/', '\\'])
}

/// The key an overlay is stored under — the reader-facing relative path
/// `<name>/<rel>`, matching what [`AssetReader::read`] receives once the
/// `twin://` scheme is stripped.
fn overlay_key(name: &str, rel: &str) -> PathBuf {
    Path::new(name).join(rel)
}

/// Authoritative Twin mount registry shared by the asset reader and runtime.
/// One lock owns mount admission, composed-document overlays, and stable source
/// identities. Unmount drops roots and bytes; only authority-to-logical-name
/// metadata remains, so old handles retain diagnosable provenance without
/// retaining a closed Twin's resources or making its address readable again.
#[derive(Resource, Clone, Default)]
pub struct TwinRoots {
    registry: Arc<RwLock<TwinRootRegistry>>,
}

#[derive(Default)]
struct TwinRootRegistry {
    roots: HashMap<String, TwinMount>,
    identities: HashMap<String, String>,
    overlays: HashMap<PathBuf, Arc<Vec<u8>>>,
    next_mount: u64,
    revision: u64,
}

/// Immutable live mount paths for background native readers. No overlay bytes
/// or retired identity metadata are retained. Publication checks the revision
/// against the live registry before consuming derived results.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TwinRootsSnapshot {
    pub revision: u64,
    roots: HashMap<String, PathBuf>,
}
impl TwinRootsSnapshot {
    pub fn resolve_file(
        &self,
        name: &str,
        relative: &Path,
    ) -> Result<Option<PathBuf>, TwinRootsError> {
        match self.roots.get(name) {
            Some(root) => resolve_twin_relative_file(root, relative),
            None => Ok(None),
        }
    }
}

struct TwinMount {
    root: PathBuf,
    requested: String,
    admission: u64,
}

/// Convert a Bevy load address to the stable, scheme-stripped source path used
/// by content provenance. Non-Twin sources require no registry; Twin sources
/// require an admitted authority in the supplied registry. Loads, imports, and
/// dependency anchors keep their mount-local address. Missing ownership must
/// fail without a raw-name identity fallback or a fabricated empty registry.
pub fn stable_source_path(
    path: &bevy::asset::AssetPath<'_>,
    roots: Option<&TwinRoots>,
) -> Result<String, TwinRootsError> {
    let source_path = crate::asset_path::slashed(path.path());
    if path.source() != &bevy::asset::io::AssetSourceId::Name(TWIN_SCHEME.into()) {
        return Ok(source_path);
    }
    let (authority, relative) = split_twin_rel(&source_path)
        .ok_or_else(|| TwinRootsError::InvalidAuthority(source_path.clone()))?;
    if !crate::asset_path::is_safe_relative_path(relative) {
        return Err(TwinRootsError::AssetResolution(
            std::io::ErrorKind::InvalidInput,
            format!("unsafe relative path `{relative}`"),
        ));
    }
    let logical = roots
        .ok_or(TwinRootsError::RegistryUnavailable)?
        .logical_name(authority)?;
    Ok(format!("{logical}/{relative}"))
}

#[cfg(not(target_arch = "wasm32"))]
fn canonical_root(root: &Path) -> Result<PathBuf, TwinRootsError> {
    std::fs::canonicalize(root).map_err(|error| {
        TwinRootsError::RootResolution(error.kind(), format!("{}: {error}", root.display()))
    })
}

#[cfg(target_arch = "wasm32")]
fn canonical_root(root: &Path) -> Result<PathBuf, TwinRootsError> {
    Ok(root.to_path_buf())
}

/// Resolve a Twin-relative path with the same authored-first, Twin-cache,
/// global-cache policy used by the `twin://` reader. The global cache is an
/// intentional final read source for every Twin; write ownership is controlled
/// separately by the manifest's `shared` field. The AssetServer, dataset
/// registry, and native runtime consumers must agree on which roots a logical
/// Twin path names.
fn resolve_twin_relative_path(
    root: &Path,
    relative: &Path,
) -> Result<Option<PathBuf>, TwinRootsError> {
    resolve_twin_relative_path_with_cache(root, relative, &crate::cache_dir())
}

fn resolve_twin_relative_path_with_cache(
    root: &Path,
    relative: &Path,
    shared: &Path,
) -> Result<Option<PathBuf>, TwinRootsError> {
    if !crate::asset_path::is_safe_relative_components(relative) {
        return Err(TwinRootsError::AssetResolution(
            std::io::ErrorKind::InvalidInput,
            format!("unsafe relative path `{}`", relative.display()),
        ));
    }
    let authored = root.join(relative);
    #[cfg(not(target_arch = "wasm32"))]
    match crate::existing_path_within_root(root, relative) {
        Ok(Some(path)) => return Ok(Some(path)),
        Ok(None) => {}
        Err(error) => {
            return Err(TwinRootsError::AssetResolution(
                error.kind(),
                format!("{}: {error}", authored.display()),
            ));
        }
    }
    #[cfg(target_arch = "wasm32")]
    if authored.exists() {
        return Ok(Some(authored.clone()));
    }

    let cache_root = crate::twin_cache_dir(root);
    let cached = cache_root.join(relative);
    #[cfg(not(target_arch = "wasm32"))]
    match crate::existing_path_within_root(&cache_root, relative) {
        Ok(Some(path)) => return Ok(Some(path)),
        Ok(None) => {}
        Err(error) => {
            return Err(TwinRootsError::AssetResolution(
                error.kind(),
                format!("{}: {error}", cached.display()),
            ));
        }
    }
    #[cfg(target_arch = "wasm32")]
    if cached.exists() {
        return Ok(Some(cached.clone()));
    }
    #[cfg(not(target_arch = "wasm32"))]
    match crate::existing_path_within_root(shared, relative) {
        Ok(Some(path)) => return Ok(Some(path)),
        Ok(None) => {}
        Err(error) => {
            return Err(TwinRootsError::AssetResolution(
                error.kind(),
                format!("{}: {error}", shared.join(relative).display()),
            ));
        }
    }
    #[cfg(target_arch = "wasm32")]
    if shared.join(relative).exists() {
        return Ok(Some(shared.join(relative)));
    }
    // Preserve the reader's useful missing-file diagnostic: authored is the
    // logical location a Twin-relative reference names.
    Ok(Some(authored))
}

pub(crate) fn resolve_twin_relative_file(
    root: &Path,
    relative: &Path,
) -> Result<Option<PathBuf>, TwinRootsError> {
    Ok(resolve_twin_relative_path(root, relative)?.filter(|path| path.is_file()))
}

pub(crate) fn resolve_twin_relative_directory(
    root: &Path,
    relative: &Path,
) -> Result<Option<PathBuf>, TwinRootsError> {
    // Domain manifests designate the admitted Twin directory itself with `.`.
    if relative == Path::new(".") {
        #[cfg(not(target_arch = "wasm32"))]
        return match lunco_storage::entry_kind_file_sync(root) {
            Ok(lunco_storage::StorageEntryKind::Directory) => Ok(Some(root.to_path_buf())),
            Ok(_) | Err(lunco_storage::StorageError::NotFound) => Ok(None),
            Err(error) => Err(TwinRootsError::AssetResolution(
                std::io::ErrorKind::Other,
                format!("{}: {error}", root.display()),
            )),
        };
        #[cfg(target_arch = "wasm32")]
        return Ok(Some(root.to_path_buf()));
    }
    Ok(resolve_twin_relative_path(root, relative)?.filter(|path| path.is_dir()))
}

impl TwinRoots {
    /// Capture current live authorities without reading filesystem metadata.
    pub fn snapshot(&self) -> Result<TwinRootsSnapshot, TwinRootsError> {
        let registry = self
            .registry
            .read()
            .map_err(|_| TwinRootsError::RegistryPoisoned)?;
        Ok(TwinRootsSnapshot {
            revision: registry.revision,
            roots: registry
                .roots
                .iter()
                .map(|(name, mount)| (name.clone(), mount.root.clone()))
                .collect(),
        })
    }

    pub fn revision(&self) -> Result<u64, TwinRootsError> {
        Ok(self
            .registry
            .read()
            .map_err(|_| TwinRootsError::RegistryPoisoned)?
            .revision)
    }
    fn clear_overlays_for_names(overlays: &mut HashMap<PathBuf, Arc<Vec<u8>>>, names: &[String]) {
        overlays.retain(|path, _| {
            path.components()
                .next()
                .and_then(|component| component.as_os_str().to_str())
                .is_none_or(|name| !names.iter().any(|removed| removed == name))
        });
    }

    /// Register the asset authority for an opened workspace Twin.
    ///
    /// The workspace event remains the normal lifecycle trigger, but startup
    /// composition can admit a Twin and immediately request its default scene
    /// in the same observer dispatch. Exposing this operation on the asset
    /// owner lets that composition root establish the `twin://` authority
    /// before any domain observer resolves the scene; repeated registration is
    /// idempotent for the same root. The authored manifest/folder display name
    /// is percent-encoded as one logical-source URI component; metadata is not
    /// changed. Low-level `register` accepts already-encoded source components.
    pub fn register_twin(&self, twin: &lunco_twin::Twin) -> Result<String, TwinRootsError> {
        let name = twin
            .manifest
            .as_ref()
            .map(|manifest| manifest.name.clone())
            .filter(|name| !name.is_empty())
            .or_else(|| {
                twin.root
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "twin".to_string());
        let encode = if name == "." || name == ".." {
            TWIN_DOT_NAME_COMPONENT
        } else {
            TWIN_NAME_COMPONENT
        };
        let logical = percent_encoding::utf8_percent_encode(&name, encode).to_string();
        self.register(logical, twin.root.clone())
    }

    /// Admit a Twin root and return its mount-local load authority.
    ///
    /// Repeated admission of the same live root/logical name is idempotent.
    /// Simultaneous roots with the same requested logical name are disambiguated
    /// as `name-2`, `name-3`, … . After unmount, a new load authority is assigned
    /// even for the same folder, while its stable logical identity is unchanged.
    /// Callers must use the returned authority for all loads and overlays.
    #[must_use = "use the RETURNED mount authority to build `twin://` load URIs"]
    pub fn register(
        &self,
        name: impl Into<String>,
        root: impl Into<PathBuf>,
    ) -> Result<String, TwinRootsError> {
        let requested = name.into();
        if !crate::asset_path::is_safe_relative_path(&requested)
            || requested.contains(['/', '\\', ':', '#', '?'])
        {
            return Err(TwinRootsError::InvalidAuthority(requested));
        }
        let root = root.into();
        let canonical = canonical_root(&root)?;
        let mut registry = self
            .registry
            .write()
            .map_err(|_| TwinRootsError::RegistryPoisoned)?;
        if let Some((authority, _)) = registry
            .roots
            .iter()
            .find(|(_, mount)| mount.root == canonical && mount.requested == requested)
        {
            return Ok(authority.clone());
        }
        let mut logical = requested.clone();
        let mut n = 1u64;
        loop {
            let occupied = registry
                .roots
                .iter()
                .find(|(authority, _)| registry.identities.get(*authority) == Some(&logical));
            match occupied {
                Some(_) => {
                    n = n.checked_add(1).ok_or(TwinRootsError::AuthorityExhausted)?;
                    logical = format!("{requested}-{n}");
                }
                None => break,
            }
        }
        let mut authority = logical.clone();
        registry.next_mount = registry
            .next_mount
            .checked_add(1)
            .ok_or(TwinRootsError::AuthorityExhausted)?;
        while registry.identities.contains_key(&authority) {
            authority = format!("{logical}--mount-{}", registry.next_mount);
            if registry.identities.contains_key(&authority) {
                registry.next_mount = registry
                    .next_mount
                    .checked_add(1)
                    .ok_or(TwinRootsError::AuthorityExhausted)?;
            }
        }
        if logical != requested {
            warn!(
                "[twin-roots] name `{requested}` is already bound to a different folder — \
                 registering `{}` with logical name `{logical}`",
                root.display()
            );
        }
        let admission = registry.next_mount;
        registry.revision = registry
            .revision
            .checked_add(1)
            .ok_or(TwinRootsError::AuthorityExhausted)?;
        registry.roots.insert(
            authority.clone(),
            TwinMount {
                root: canonical,
                requested,
                admission,
            },
        );
        registry.identities.insert(authority.clone(), logical);
        Ok(authority)
    }

    /// Stable logical name of an admitted mount, including retired mounts.
    /// Identity metadata does not make a retired authority readable again.
    pub fn logical_name(&self, authority: &str) -> Result<String, TwinRootsError> {
        self.registry
            .read()
            .map_err(|_| TwinRootsError::RegistryPoisoned)?
            .identities
            .get(authority)
            .cloned()
            .ok_or_else(|| TwinRootsError::UnknownAuthority(authority.to_string()))
    }

    /// Current load authority for an exact logical name, when it is mounted.
    /// Used by scenario synchronization to reuse an editable local root instead
    /// of shadowing it with the downloaded copy.
    pub fn mounted_name_for_logical(
        &self,
        logical: &str,
    ) -> Result<Option<String>, TwinRootsError> {
        let registry = self
            .registry
            .read()
            .map_err(|_| TwinRootsError::RegistryPoisoned)?;
        Ok(registry
            .roots
            .keys()
            .find(|authority| {
                registry
                    .identities
                    .get(*authority)
                    .is_some_and(|name| name == logical)
            })
            .cloned())
    }

    /// Serve `bytes` in place of the on-disk file at `twin://<name>/<rel>`. The
    /// key matches the path the `AssetReader` receives (scheme stripped), so a
    /// subsequent `AssetServer` load/reload of `twin://<name>/<rel>` reads these
    /// bytes. Used by E1b to project a document's composed source into the live
    /// world; pass the same `(name, rel)` to [`clear_overlay`](Self::clear_overlay)
    /// to fall back to disk.
    pub fn set_overlay(
        &self,
        name: &str,
        rel: &str,
        bytes: Arc<Vec<u8>>,
    ) -> Result<(), TwinRootsError> {
        if !crate::asset_path::is_safe_relative_path(name)
            || !crate::asset_path::is_safe_relative_path(rel)
        {
            return Err(TwinRootsError::InvalidOverlayPath(format!("{name}/{rel}")));
        }
        let mut registry = self
            .registry
            .write()
            .map_err(|_| TwinRootsError::RegistryPoisoned)?;
        if !registry.roots.contains_key(name) {
            return Err(TwinRootsError::UnknownAuthority(name.to_string()));
        }
        registry.overlays.insert(overlay_key(name, rel), bytes);
        Ok(())
    }

    /// Drop the in-memory overlay for `twin://<name>/<rel>` so reads fall back
    /// to the on-disk file again.
    pub fn clear_overlay(&self, name: &str, rel: &str) -> Result<(), TwinRootsError> {
        if !crate::asset_path::is_safe_relative_path(name)
            || !crate::asset_path::is_safe_relative_path(rel)
        {
            return Err(TwinRootsError::InvalidOverlayPath(format!("{name}/{rel}")));
        }
        self.registry
            .write()
            .map_err(|_| TwinRootsError::RegistryPoisoned)?
            .overlays
            .remove(&overlay_key(name, rel));
        Ok(())
    }

    /// Return the live in-memory bytes for a Twin-relative file, when an
    /// Editor composition has projected an overlay for it.
    ///
    /// Native validators must check this boundary before reading a resolved
    /// filesystem path. Otherwise a dirty Editor document is silently replaced
    /// by its last saved bytes and validation can approve a stale revision.
    pub fn overlay_bytes(
        &self,
        name: &str,
        relative: &Path,
    ) -> Result<Option<Arc<Vec<u8>>>, TwinRootsError> {
        if !crate::asset_path::is_safe_relative_path(name)
            || !crate::asset_path::is_safe_relative_components(relative)
        {
            return Err(TwinRootsError::InvalidOverlayPath(format!(
                "{name}/{}",
                crate::asset_path::slashed(relative)
            )));
        }
        self.overlay_for(&overlay_key(name, &crate::asset_path::slashed(relative)))
    }

    /// Overlay bytes registered for the reader-facing relative `path`
    /// (`<name>/<rel>`), if any.
    fn overlay_for(&self, path: &Path) -> Result<Option<Arc<Vec<u8>>>, TwinRootsError> {
        let path = PathBuf::from(crate::asset_path::slashed(path));
        self.registry
            .read()
            .map(|registry| registry.overlays.get(&path).cloned())
            .map_err(|_| TwinRootsError::RegistryPoisoned)
    }

    /// Absolute root folder of an open Twin, by `twin://` authority. Public
    /// because a Twin's own `Assets.toml` (scanned on open by
    /// `lunco-assets`]) is addressed by filesystem path, not by URI.
    pub fn root_for(&self, name: &str) -> Result<Option<PathBuf>, TwinRootsError> {
        self.registry
            .read()
            .map(|registry| registry.roots.get(name).map(|mount| mount.root.clone()))
            .map_err(|_| TwinRootsError::RegistryPoisoned)
    }

    /// Return the first admitted live authority for an open Twin root.
    ///
    /// Names can be disambiguated when two open folders share the same
    /// authored/folder name, so consumers must resolve the assigned authority
    /// instead of reconstructing it from the manifest again.
    pub fn name_for_root(&self, root: impl AsRef<Path>) -> Result<Option<String>, TwinRootsError> {
        let target = canonical_root(root.as_ref())?;
        self.registry
            .read()
            .map_err(|_| TwinRootsError::RegistryPoisoned)
            .map(|registry| {
                registry
                    .roots
                    .iter()
                    .filter(|(_, mount)| mount.root == target)
                    .min_by_key(|(_, mount)| mount.admission)
                    .map(|(name, _)| name.clone())
            })
    }

    /// Names of all currently-open Twins, sorted (deterministic order — the
    /// map's own iteration order isn't).
    pub fn names(&self) -> Result<Vec<String>, TwinRootsError> {
        self.registry
            .read()
            .map(|registry| {
                let mut v: Vec<String> = registry.roots.keys().cloned().collect();
                v.sort();
                v
            })
            .map_err(|_| TwinRootsError::RegistryPoisoned)
    }

    /// Absolute root folder for an open Twin by name.
    pub fn root_of(&self, name: &str) -> Result<Option<PathBuf>, TwinRootsError> {
        self.root_for(name)
    }

    /// Resolve a Twin-relative file using the same authored-first, Twin-cache,
    /// shared-cache
    /// policy as the `twin://` AssetReader. Native consumers that need a concrete
    /// filesystem path must use this boundary rather than joining a Twin root
    /// themselves, because downloaded Twin assets live in `.cache`.
    pub fn resolve_file(
        &self,
        name: &str,
        relative: &Path,
    ) -> Result<Option<PathBuf>, TwinRootsError> {
        let Some(root) = self.root_for(name)? else {
            return Ok(None);
        };
        resolve_twin_relative_file(&root, relative)
    }

    /// Resolve a Twin-relative directory using the same authored-first,
    /// cache-second policy as the `twin://` AssetReader.
    ///
    /// Processed datasets such as DEM sites deliver a directory containing
    /// their runtime products. Directory consumers use this boundary instead
    /// of reconstructing the Twin cache path themselves.
    /// `.` designates the admitted Twin root for domain source directories.
    pub fn resolve_directory(
        &self,
        name: &str,
        relative: &Path,
    ) -> Result<Option<PathBuf>, TwinRootsError> {
        let Some(root) = self.root_for(name)? else {
            return Ok(None);
        };
        resolve_twin_relative_directory(&root, relative)
    }

    /// Whether a Twin-relative file or directory exists in the Twin's
    /// authored-tree, Twin-cache, shared-cache order, including a composed
    /// document overlay. Native readers only: on wasm the mounted OPFS tree
    /// cannot be probed synchronously and every candidate is reported present.
    pub fn resolve_existing(&self, name: &str, relative: &Path) -> Result<bool, TwinRootsError> {
        if self
            .overlay_for(&overlay_key(name, &crate::asset_path::slashed(relative)))?
            .is_some()
        {
            return Ok(true);
        }
        let Some(root) = self.root_for(name)? else {
            return Err(TwinRootsError::UnknownAuthority(name.to_owned()));
        };
        #[cfg(not(target_arch = "wasm32"))]
        return Ok(resolve_twin_relative_path(&root, relative)?
            .is_some_and(|path| path.is_file() || path.is_dir()));
        #[cfg(target_arch = "wasm32")]
        {
            let _ = root;
            Ok(true)
        }
    }

    /// The "primary" open Twin as `(name, root)` — the alphabetically-first
    /// registered Twin, used as the default destination for newly created or
    /// imported assets when the caller doesn't name a Twin. `None` if no Twin
    /// is open.
    pub fn primary(&self) -> Result<Option<(String, PathBuf)>, TwinRootsError> {
        let Some(name) = self.names()?.into_iter().next() else {
            return Ok(None);
        };
        Ok(self.root_for(&name)?.map(|root| (name, root)))
    }

    /// Retire every registered Twin authority backed by `root`, including its
    /// composed-document overlays. A closed workspace Twin must not remain a
    /// valid source for a late asset request from the outgoing scene.
    pub fn unregister_root(&self, root: impl AsRef<Path>) -> Result<(), TwinRootsError> {
        let target = canonical_root(root.as_ref())?;
        let mut registry = self
            .registry
            .write()
            .map_err(|_| TwinRootsError::RegistryPoisoned)?;
        let removed: Vec<_> = registry
            .roots
            .iter()
            .filter(|(_, mount)| mount.root == target)
            .map(|(name, _)| name.clone())
            .collect();
        if !removed.is_empty() {
            registry.revision = registry
                .revision
                .checked_add(1)
                .ok_or(TwinRootsError::AuthorityExhausted)?;
        }
        for name in &removed {
            registry.roots.remove(name);
        }
        Self::clear_overlays_for_names(&mut registry.overlays, &removed);
        Ok(())
    }

    /// Retire one synthetic or user-session Twin authority by its exact name,
    /// including its composed-document overlays. This is distinct from
    /// [`unregister_root`](Self::unregister_root): several authorities may
    /// intentionally point at the same directory, so a document view must not
    /// tear down an unrelated Twin merely because their roots match.
    pub fn unregister_name(&self, name: &str) -> Result<(), TwinRootsError> {
        let mut registry = self
            .registry
            .write()
            .map_err(|_| TwinRootsError::RegistryPoisoned)?;
        if registry.roots.contains_key(name) {
            registry.revision = registry
                .revision
                .checked_add(1)
                .ok_or(TwinRootsError::AuthorityExhausted)?;
            registry.roots.remove(name);
        }
        Self::clear_overlays_for_names(&mut registry.overlays, &[name.to_string()]);
        Ok(())
    }
}

/// Read a Twin-root file through the storage backend. The ONLY native/web
/// divergence in this source: native = `FileStorage` (std::fs, via the sync
/// wrapper — this runs on Bevy's async IO pool); web = `OpfsStorage` (async OPFS
/// read), which is the same tree the networking client writes a downloaded
/// scenario into. Going through storage is what lets `twin://` serve a downloaded
/// scenario on the web, where there is no filesystem.
fn storage_read_error(path: &Path, error: lunco_storage::StorageError) -> AssetReaderError {
    match error {
        lunco_storage::StorageError::NotFound => AssetReaderError::NotFound(path.to_path_buf()),
        error => AssetReaderError::from(std::io::Error::other(format!(
            "could not read Twin asset `{}`: {error}",
            path.display()
        ))),
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn read_bytes(full: &Path) -> Result<Vec<u8>, AssetReaderError> {
    lunco_storage::read_file_sync(full).map_err(|error| storage_read_error(full, error))
}

#[cfg(target_arch = "wasm32")]
async fn read_bytes(full: &Path) -> Result<Vec<u8>, AssetReaderError> {
    lunco_storage::OpfsStorage::new()
        .read(&lunco_storage::StorageHandle::File(full.to_path_buf()))
        .await
        .map_err(|error| storage_read_error(full, error))
}

/// Build the `twin://` [`AssetSourceBuilder`] over `roots`. Register in each
/// binary BEFORE `AssetPlugin` builds, and insert the same `roots` handle as a
/// resource so the Twin-open flow can register roots.
pub fn twin_asset_source(roots: &TwinRoots) -> AssetSourceBuilder {
    let roots = roots.clone();
    AssetSourceBuilder::new(move || {
        Box::new(TwinReader {
            roots: roots.clone(),
        }) as Box<dyn ErasedAssetReader>
    })
}

/// `AssetReader` that splits `<name>/<rel>`, looks the Twin root up by name, and
/// reads `<root>/<rel>` into memory (`VecReader`). In-memory reading sidesteps
/// the lifetime dance of returning a borrowed file handle from an `async fn` in
/// the trait, and matches how the wasm file readers already work.
struct TwinReader {
    roots: TwinRoots,
}

impl TwinReader {
    /// Resolve `twin://`-relative `<name>/<rel>` to an absolute filesystem path.
    ///
    /// Rejects path traversal: only `Normal` components are joined, so a scene can
    /// never reach outside its Twin root. That guard is not optional — a Twin root
    /// may be a **downloaded scenario's cache directory**, whose relative paths were
    /// authored by a remote host, and escaping it would let a peer read arbitrary
    /// local files. Shipped assets are addressed by scheme (`@lunco://…@`), so no
    /// authored ref needs to climb out (verified across the shipped tree and the
    /// twins: zero `@../…@` refs).
    /// A Twin's DOWNLOADED assets live in its own `.cache/` (see
    /// [`crate::twin_cache_dir`]), so a Twin-relative path resolves whether the
    /// file is committed in the Twin or was fetched from that Twin's
    /// `Assets.toml`. Search-path selection happens before this reader, in
    /// native preparation. Authored files
    /// win: the cache is a materialisation of a declaration, never an override
    /// of something the author checked in.
    fn resolve(&self, path: &Path) -> Result<Option<PathBuf>, TwinRootsError> {
        let mut comps = path.components();
        let Some(name) = comps
            .next()
            .and_then(|component| component.as_os_str().to_str())
        else {
            return Ok(None);
        };
        let mut rel = PathBuf::new();
        for comp in comps {
            rel.push(comp.as_os_str());
        }
        self.roots.resolve_file(name, &rel)
    }
}

fn asset_reader_error(error: TwinRootsError) -> AssetReaderError {
    AssetReaderError::from(std::io::Error::other(error.to_string()))
}

impl AssetReader for TwinReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        let Some(path) = crate::asset_path::relative_path(&crate::asset_path::slashed(path)) else {
            return Err::<VecReader, _>(AssetReaderError::NotFound(path.to_path_buf()));
        };
        // In-memory overlay wins over the on-disk file (E1b: a scene document's
        // composed source projected into the live world). Keyed by the exact
        // reader-facing `<name>/<rel>` path.
        if let Some(bytes) = self
            .roots
            .overlay_for(path.as_path())
            .map_err(asset_reader_error)?
        {
            return Ok(VecReader::new((*bytes).clone()));
        }
        let Some(full) = self.resolve(path.as_path()).map_err(asset_reader_error)? else {
            return Err::<VecReader, _>(AssetReaderError::NotFound(path.to_path_buf()));
        };
        let bytes = read_bytes(&full).await?;
        Ok(VecReader::new(bytes))
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        // Twin assets ship no `.meta` sidecars. The `VecReader` annotation pins
        // the opaque return type even though this branch only ever errs.
        Err::<VecReader, _>(AssetReaderError::NotFound(path.to_path_buf()))
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        Err(AssetReaderError::NotFound(path.to_path_buf()))
    }

    async fn is_directory<'a>(&'a self, path: &'a Path) -> Result<bool, AssetReaderError> {
        let slashed = crate::asset_path::slashed(path);
        let Some((name, relative)) = split_twin_rel(&slashed) else {
            return Ok(false);
        };
        let relative = crate::asset_path::relative_path(relative)
            .ok_or_else(|| AssetReaderError::NotFound(path.to_path_buf()))?;
        Ok(self
            .roots
            .resolve_directory(name, &relative)
            .map_err(asset_reader_error)?
            .is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twin_mount_snapshot_revision_retires_equal_root_reopens() {
        let root = tempfile::tempdir().expect("generic root");
        let roots = TwinRoots::default();
        let empty = roots.snapshot().unwrap();
        let authority = roots.register("generic", root.path()).unwrap();
        let admitted = roots.snapshot().unwrap();
        assert!(admitted.revision > empty.revision);
        assert_eq!(roots.register("generic", root.path()).unwrap(), authority);
        assert_eq!(roots.revision().unwrap(), admitted.revision);
        roots.unregister_name(&authority).unwrap();
        let retired = roots.snapshot().unwrap();
        assert!(retired.revision > admitted.revision);
        assert!(!retired.roots.contains_key(&authority));
        assert_eq!(
            admitted.roots.get(&authority),
            Some(&canonical_root(root.path()).unwrap()),
            "immutable preparation retains only its admitted path snapshot"
        );
        let reopened = roots.register("generic", root.path()).unwrap();
        assert_ne!(reopened, authority);
        assert!(roots.revision().unwrap() > retired.revision);
        roots.unregister_name(&authority).unwrap();
        assert_eq!(
            roots.revision().unwrap(),
            retired.revision + 1,
            "retiring an already absent authority is idempotent"
        );
    }

    #[test]
    fn twin_uri_normalizes_windows_relative_paths() {
        assert_eq!(
            twin_uri("Fixture Twin", Path::new(r"sim\scenes\traverse.usda")),
            "twin://Fixture Twin/sim/scenes/traverse.usda"
        );
    }

    #[test]
    fn parses_a_windows_authored_twin_uri() {
        assert_eq!(
            parse_twin_uri(r"twin://fixture\sim\scenes\traverse.usda"),
            Some(("fixture", r"sim\scenes\traverse.usda"))
        );
    }

    /// The overlay must be keyed identically to the path the `AssetReader`
    /// receives for `twin://<name>/<rel>` (scheme stripped) — otherwise an
    /// `AssetServer` load would miss it and read the on-disk file.
    #[test]
    fn overlay_keyed_by_reader_facing_path() {
        let roots = TwinRoots::default();
        let root = tempfile::tempdir().expect("Twin root");
        let name = roots
            .register("moonbase", root.path())
            .expect("register root");
        let bytes = Arc::new(b"#usda 1.0\n".to_vec());
        roots
            .set_overlay(&name, "scenes/luncosim.usda", bytes.clone())
            .expect("set overlay");

        assert_eq!(
            roots
                .overlay_bytes("moonbase", Path::new("scenes/luncosim.usda"))
                .expect("read named overlay")
                .as_deref(),
            Some(&*bytes),
            "native consumers use the same overlay key as the AssetReader"
        );

        // The reader receives `moonbase/scenes/luncosim.usda` (scheme stripped).
        assert_eq!(
            roots
                .overlay_for(Path::new("moonbase/scenes/luncosim.usda"))
                .expect("read overlay registry")
                .as_deref(),
            Some(&*bytes),
            "overlay hit for the exact reader-facing path"
        );
        assert_eq!(
            roots
                .overlay_for(Path::new(r"moonbase\scenes\luncosim.usda"))
                .expect("read overlay registry")
                .as_deref(),
            Some(&*bytes),
            "a Windows reader-facing path finds the slash-normalized overlay"
        );
        assert!(
            roots
                .overlay_for(Path::new("moonbase/other.usda"))
                .expect("read overlay registry")
                .is_none(),
            "no overlay for an unrelated path"
        );

        roots
            .clear_overlay("moonbase", "scenes/luncosim.usda")
            .expect("clear overlay");
        assert!(
            roots
                .overlay_for(Path::new("moonbase/scenes/luncosim.usda"))
                .expect("read overlay registry")
                .is_none(),
            "cleared overlay falls back to disk"
        );
    }

    #[test]
    fn unsafe_overlay_paths_are_not_stored_or_read() {
        let roots = TwinRoots::default();
        assert!(matches!(
            roots.set_overlay("moonbase", "../outside.usda", Arc::new(b"secret".to_vec())),
            Err(TwinRootsError::InvalidOverlayPath(_))
        ));
        assert!(matches!(
            roots.set_overlay("../outside", "scene.usda", Arc::new(b"secret".to_vec())),
            Err(TwinRootsError::InvalidOverlayPath(_))
        ));
        assert!(matches!(
            roots.overlay_bytes("moonbase", Path::new("../outside.usda")),
            Err(TwinRootsError::InvalidOverlayPath(_))
        ));

        assert!(
            roots
                .overlay_for(Path::new("moonbase/../outside.usda"))
                .expect("read overlay registry")
                .is_none()
        );
        assert!(
            roots
                .overlay_for(Path::new("../outside/scene.usda"))
                .expect("read overlay registry")
                .is_none()
        );
    }

    /// Two unrelated folders can carry the same name (`twin.toml` name, or a
    /// basename like `scenes`). Registering the second must NOT repoint the
    /// first — that silently broke every `twin://first/…` read already in
    /// flight, with no diagnostic.
    #[test]
    fn same_name_different_root_does_not_repoint() {
        let roots = TwinRoots::default();
        let first_root = tempfile::tempdir().expect("first Twin root");
        let second_root = tempfile::tempdir().expect("second Twin root");

        let a = roots
            .register("scenes", first_root.path())
            .expect("register first root");
        let b = roots
            .register("scenes", second_root.path())
            .expect("register second root");

        assert_eq!(a, "scenes");
        assert_ne!(b, a, "second root must not take the first root's name");
        assert_eq!(
            roots.root_of(&a),
            Ok(Some(first_root.path().to_path_buf())),
            "first Twin still resolves to its own folder"
        );
        assert_eq!(
            roots.root_of(&b),
            Ok(Some(second_root.path().to_path_buf())),
            "second Twin resolves to its own folder under the assigned name"
        );
        assert_eq!(roots.logical_name(&a), Ok("scenes".to_string()));
        assert_eq!(roots.logical_name(&b), Ok("scenes-2".to_string()));
        roots.unregister_name(&a).expect("retire first duplicate");
        assert_eq!(
            roots
                .register("scenes", second_root.path())
                .expect("repeat second admission"),
            b,
            "a live duplicate keeps its admitted logical identity after the other Twin closes"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn register_twin_encodes_display_names_without_rebinding_retired_mounts() {
        futures_lite::future::block_on(async {
            let parent = tempfile::tempdir().expect("temporary parent");
            let folder_name = "Path # % Мир";
            let folder = parent.path().join(folder_name);
            lunco_storage::ensure_directory_sync(&folder).expect("Twin folder");
            let relative = Path::new("payload # % Мир.txt");
            lunco_storage::write_file_sync(&folder.join(relative), b"literal filename")
                .expect("payload");
            let lunco_twin::TwinMode::Folder(mut twin) =
                lunco_twin::TwinMode::open(&folder).expect("open folder")
            else {
                panic!("expected plain folder Twin");
            };
            let roots = TwinRoots::default();
            let expected = "Path%20%23%20%25%20%D0%9C%D0%B8%D1%80";
            let first = roots.register_twin(&twin).expect("special folder mount");
            assert_eq!(first, expected);
            assert_eq!(
                roots.register_twin(&twin).expect("idempotent folder mount"),
                first
            );
            assert!(twin.manifest.is_none());
            let origin =
                bevy::asset::AssetPath::from_path_buf(Path::new(&first).join("entry.usda"))
                    .with_source(TWIN_SCHEME);
            let file_uri = lunco_storage::file_path_to_uri(&twin.root.join(relative))
                .expect("native payload URI");
            let prepared = crate::asset_path::PreparedAssetPaths::prepare_on_worker(
                [file_uri.clone()],
                Some(origin.clone()),
                Some(&roots),
            );
            let payload = crate::asset_path::load_asset_path(
                &file_uri,
                Some(&origin),
                Some(&roots),
                Some(&prepared),
            )
            .expect("special-name native asset admission");
            assert_eq!(payload.path(), Path::new(&first).join(relative));
            assert_eq!(payload.label(), None);
            let reader = TwinReader {
                roots: roots.clone(),
            };
            let mut bytes = Vec::new();
            AssetReader::read(&reader, payload.path())
                .await
                .expect("literal payload reader")
                .read_to_end(&mut bytes)
                .await
                .expect("literal payload bytes");
            assert_eq!(bytes, b"literal filename");

            roots.unregister_name(&first).expect("retire folder mount");
            let reopened = roots.register_twin(&twin).expect("reopen folder mount");
            assert_ne!(reopened, first);
            assert_eq!(
                roots
                    .logical_name(&reopened)
                    .expect("stable logical identity"),
                expected
            );
            assert!(matches!(
                AssetReader::read(&reader, payload.path()).await,
                Err(AssetReaderError::NotFound(_))
            ));
            assert!(
                matches!(crate::asset_path::load_asset_path(&file_uri, Some(&origin), Some(&roots), None),
                Err(TwinRootsError::UnknownAuthority(authority)) if authority == first)
            );
            roots
                .unregister_name(&reopened)
                .expect("retire reopened mount");

            for (display, expected) in [
                ("plain-name_1.0~", "plain-name_1.0~"),
                (
                    r"A/B?C\D:E # % 月",
                    "A%2FB%3FC%5CD%3AE%20%23%20%25%20%E6%9C%88",
                ),
                ("#", "%23"),
                ("%23", "%2523"),
                (".", "%2E"),
                ("..", "%2E%2E"),
            ] {
                twin.manifest = Some(lunco_twin::TwinManifest::new(display));
                let authority = roots
                    .register_twin(&twin)
                    .expect("manifest display-name mount");
                assert_eq!(
                    roots
                        .logical_name(&authority)
                        .expect("manifest logical identity"),
                    expected
                );
                assert_eq!(
                    twin.manifest.as_ref().unwrap().name,
                    display,
                    "display metadata is unchanged"
                );
                roots
                    .unregister_name(&authority)
                    .expect("retire manifest mount");
            }
            for invalid in ["a/b", r"a\b", "a:b", "a#b", "a?b", ".", ".."] {
                assert!(
                    matches!(
                        roots.register(invalid, &folder),
                        Err(TwinRootsError::InvalidAuthority(_))
                    ),
                    "low-level registration must reject {invalid:?}"
                );
            }
        });
    }

    /// Repeated admission while the same mount is live is idempotent.
    #[test]
    fn reregistering_same_root_reuses_the_name() {
        let roots = TwinRoots::default();
        let root = tempfile::tempdir().expect("Twin root");

        let first = roots
            .register("moonbase", root.path())
            .expect("register root");
        let again = roots
            .register("moonbase", root.path())
            .expect("re-register root");

        assert_eq!(first, "moonbase");
        assert_eq!(
            again, first,
            "live admission must reuse the existing authority"
        );
        assert_eq!(
            roots.names().expect("read Twin registry").len(),
            1,
            "no duplicate registration"
        );
    }

    #[test]
    fn retired_mounts_keep_identity_without_roots_bytes_or_rebound_load_addresses() {
        let root = tempfile::tempdir().expect("Twin root");
        let replacement = tempfile::tempdir().expect("replacement Twin root");
        let roots = TwinRoots::default();
        let first = roots
            .register("fixture", root.path())
            .expect("mount first root");
        let first_path =
            bevy::asset::AssetPath::parse(&twin_uri(&first, "data/payload.txt")).into_owned();
        roots
            .set_overlay(&first, "data/payload.txt", Arc::new(b"first".to_vec()))
            .expect("first overlay");
        assert!(
            roots
                .overlay_bytes(&first, Path::new("data/payload.txt"))
                .expect("read first overlay")
                .is_some()
        );

        roots
            .unregister_root(root.path())
            .expect("retire first mount");
        assert!(roots.root_for(&first).expect("read retired root").is_none());
        assert!(
            roots
                .overlay_bytes(&first, Path::new("data/payload.txt"))
                .expect("read retired overlay")
                .is_none()
        );
        assert!(matches!(
            roots.set_overlay(&first, "data/payload.txt", Arc::new(b"late".to_vec())),
            Err(TwinRootsError::UnknownAuthority(_))
        ));
        assert_eq!(
            stable_source_path(&first_path, Some(&roots)).expect("retired source identity"),
            "fixture/data/payload.txt"
        );

        let reopened = roots
            .register("fixture", root.path())
            .expect("reopen same folder");
        assert_ne!(
            reopened, first,
            "Bevy path identities cannot survive a mount lifetime"
        );
        assert_eq!(
            roots
                .register("fixture", root.path())
                .expect("repeat live admission"),
            reopened
        );
        let reopened_path =
            bevy::asset::AssetPath::parse(&twin_uri(&reopened, "data/payload.txt")).into_owned();
        assert_eq!(
            stable_source_path(&reopened_path, Some(&roots)),
            stable_source_path(&first_path, Some(&roots))
        );
        roots
            .unregister_name(&reopened)
            .expect("retire reopened mount");

        let next = roots
            .register("fixture", replacement.path())
            .expect("mount replacement folder");
        assert_ne!(next, first);
        assert_ne!(next, reopened);
        assert_eq!(
            roots.logical_name(&next).expect("replacement logical name"),
            "fixture"
        );
        assert_eq!(
            roots
                .mounted_name_for_logical("fixture")
                .expect("find logical mount"),
            Some(next.clone())
        );
        assert!(
            roots
                .resolve_file(&first, Path::new("data/payload.txt"))
                .expect("retired file lookup")
                .is_none()
        );
        let next_uri = twin_uri(&next, "scripts/main.rhai");
        assert_eq!(
            lunco_assets_path::canonicalize("helpers.rhai", &next_uri),
            twin_uri(&next, "scripts/helpers.rhai")
        );
        assert_eq!(
            crate::asset_path::source_relative_uri(
                &bevy::asset::AssetPath::parse(&next_uri),
                "textures/albedo.png"
            ),
            Some(twin_uri(&next, "textures/albedo.png"))
        );
    }

    #[test]
    fn provenance_sources_agree_across_local_mount_history_and_reject_unadmitted_names() {
        let host_root = tempfile::tempdir().expect("host root");
        let client_root = tempfile::tempdir().expect("client cache root");
        let host = TwinRoots::default();
        let client = TwinRoots::default();
        let old = host
            .register("fixture", host_root.path())
            .expect("host first admission");
        host.unregister_name(&old).expect("host unmount");
        let host_name = host
            .register("fixture", host_root.path())
            .expect("host reopen");
        let client_name = client
            .register("fixture", client_root.path())
            .expect("client admission");
        assert_ne!(host_name, client_name);
        let host_path =
            bevy::asset::AssetPath::parse(&format!("twin://{host_name}\\scenes\\main.usda"))
                .into_owned();
        let client_path =
            bevy::asset::AssetPath::parse(&twin_uri(&client_name, "scenes/main.usda")).into_owned();
        assert_eq!(
            stable_source_path(&host_path, Some(&host)).expect("host provenance"),
            "fixture/scenes/main.usda"
        );
        assert_eq!(
            stable_source_path(&host_path, Some(&host)),
            stable_source_path(&client_path, Some(&client))
        );
        assert!(matches!(
            stable_source_path(
                &bevy::asset::AssetPath::parse("twin://unknown/scenes/main.usda"),
                Some(&host)
            ),
            Err(TwinRootsError::UnknownAuthority(_))
        ));
        assert!(matches!(
            stable_source_path(
                &bevy::asset::AssetPath::parse(&twin_uri(&host_name, "../outside.usda")),
                Some(&host)
            ),
            Err(TwinRootsError::AssetResolution(
                std::io::ErrorKind::InvalidInput,
                _
            ))
        ));
        assert_eq!(
            stable_source_path(
                &bevy::asset::AssetPath::parse(r"lunco://scenes\main.usda"),
                None
            ),
            Ok("scenes/main.usda".to_string())
        );
        assert_eq!(
            stable_source_path(&bevy::asset::AssetPath::parse("scenes/main.usda"), None),
            Ok("scenes/main.usda".to_string())
        );
        assert!(matches!(
            stable_source_path(&client_path, None),
            Err(TwinRootsError::RegistryUnavailable)
        ));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn reader_rejects_retired_alias_instead_of_reading_replacement_bytes() {
        futures_lite::future::block_on(async {
            let first_root = tempfile::tempdir().expect("first root");
            let second_root = tempfile::tempdir().expect("second root");
            lunco_storage::write_file_sync(&first_root.path().join("payload.txt"), b"first")
                .expect("first bytes");
            lunco_storage::write_file_sync(&second_root.path().join("payload.txt"), b"second")
                .expect("second bytes");
            let roots = TwinRoots::default();
            let first = roots
                .register("fixture", first_root.path())
                .expect("first mount");
            let reader = TwinReader {
                roots: roots.clone(),
            };
            let first_path = PathBuf::from(format!("{first}/payload.txt"));
            let mut bytes = Vec::new();
            AssetReader::read(&reader, &first_path)
                .await
                .expect("first reader")
                .read_to_end(&mut bytes)
                .await
                .expect("first read");
            assert_eq!(bytes, b"first");
            roots.unregister_name(&first).expect("retire first");
            let second = roots
                .register("fixture", second_root.path())
                .expect("second mount");
            assert!(matches!(
                AssetReader::read(&reader, &first_path).await,
                Err(AssetReaderError::NotFound(_))
            ));
            bytes.clear();
            AssetReader::read(&reader, Path::new(&format!("{second}/payload.txt")))
                .await
                .expect("second reader")
                .read_to_end(&mut bytes)
                .await
                .expect("second read");
            assert_eq!(bytes, b"second");
        });
    }

    #[test]
    fn resolve_file_finds_downloaded_twin_assets_without_exposing_cache_in_authored_paths() {
        let twin = tempfile::tempdir().expect("temporary Twin root");
        let cached = crate::twin_cache_dir(twin.path()).join("terrain/luna2");
        lunco_storage::ensure_directory_sync(cached.parent().expect("cached parent"))
            .expect("cache directory");
        lunco_storage::write_file_sync(&cached, b"cached terrain").expect("cached asset");
        let roots = TwinRoots::default();
        let name = roots.register("luna2", twin.path()).expect("register root");

        assert_eq!(
            roots.resolve_file(&name, Path::new("terrain/luna2")),
            Ok(Some(cached)),
            "logical Twin paths must resolve downloaded assets through the cache"
        );

        let authored = twin.path().join("terrain/luna2");
        lunco_storage::ensure_directory_sync(authored.parent().expect("authored parent"))
            .expect("authored directory");
        lunco_storage::write_file_sync(&authored, b"authored terrain").expect("authored asset");
        assert_eq!(
            roots.resolve_file(&name, Path::new("terrain/luna2")),
            Ok(Some(authored)),
            "authored Twin files take precedence over materialized cache files"
        );
    }

    #[test]
    fn resolve_directory_finds_processed_twin_assets_without_reconstructing_cache_paths() {
        let twin = tempfile::tempdir().expect("temporary Twin root");
        let cached = crate::twin_cache_dir(twin.path()).join("terrain/luna2");
        lunco_storage::ensure_directory_sync(&cached.join("materials/textures"))
            .expect("cached processed directory");
        lunco_storage::write_file_sync(
            &cached.join("materials/textures/heightmap.tif"),
            b"processed terrain",
        )
        .expect("cached processed asset");
        let roots = TwinRoots::default();
        let name = roots.register("luna2", twin.path()).expect("register root");

        assert_eq!(
            roots.resolve_directory(&name, Path::new("terrain/luna2")),
            Ok(Some(cached)),
            "processed Twin directories must resolve through the asset boundary"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn directory_root_designator_retains_mount_ownership_and_rejects_parent_escape() {
        let twin = tempfile::tempdir().unwrap();
        let roots = TwinRoots::default();
        let name = roots.register("native-root", twin.path()).unwrap();
        assert_eq!(
            roots.resolve_directory(&name, Path::new(".")).unwrap(),
            roots.root_for(&name).unwrap()
        );
        assert!(
            roots
                .resolve_directory(&name, Path::new("../outside"))
                .is_err()
        );
        assert!(roots.resolve_file(&name, Path::new(".")).is_err());
        roots.unregister_name(&name).unwrap();
        assert_eq!(
            roots.resolve_directory(&name, Path::new(".")).unwrap(),
            None
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn twin_resolution_reads_a_shared_cache_without_a_twin_local_copy() {
        let twin = tempfile::tempdir().expect("temporary Twin root");
        let shared = tempfile::tempdir().expect("temporary global cache");
        let relative = Path::new("terrain/shared/heightmap.tif");
        let shared_file = shared.path().join(relative);
        lunco_storage::ensure_directory_sync(shared_file.parent().expect("shared parent"))
            .expect("shared directory");
        lunco_storage::write_file_sync(&shared_file, b"shared terrain").expect("shared asset");

        assert_eq!(
            resolve_twin_relative_path_with_cache(twin.path(), relative, shared.path())
                .expect("resolve shared Twin asset"),
            Some(shared_file),
            "a Twin must consume a shared logical asset without materialising a local copy"
        );
    }

    #[test]
    fn unregistering_a_root_removes_its_authority_and_overlays() {
        let roots = TwinRoots::default();
        let root = tempfile::tempdir().expect("Twin root");
        let name = roots
            .register("moonbase", root.path())
            .expect("register root");
        roots
            .set_overlay(&name, "scene.usda", Arc::new(b"#usda 1.0\n".to_vec()))
            .expect("set overlay");

        roots.unregister_root(root.path()).expect("unregister root");

        assert!(roots.names().expect("read Twin registry").is_empty());
        assert!(roots.root_of(&name).expect("read Twin registry").is_none());
        assert!(
            roots
                .overlay_for(Path::new("moonbase/scene.usda"))
                .expect("read overlay registry")
                .is_none()
        );
    }

    #[test]
    fn unregistering_a_name_preserves_another_authority_on_the_same_root() {
        let roots = TwinRoots::default();
        let root = tempfile::tempdir().expect("Twin root");
        let twin = roots
            .register("moonbase", root.path())
            .expect("register root");
        let session = roots
            .register("__viewport_1", root.path())
            .expect("register session root");
        assert_eq!(
            roots
                .name_for_root(root.path())
                .expect("find open Twin mount"),
            Some(twin.clone())
        );
        roots
            .set_overlay(&twin, "scene.usda", Arc::new(b"twin".to_vec()))
            .expect("set Twin overlay");
        roots
            .set_overlay(&session, "scene.usda", Arc::new(b"session".to_vec()))
            .expect("set session overlay");

        roots.unregister_name(&session).expect("unregister session");

        assert!(roots.root_of(&twin).expect("read Twin registry").is_some());
        assert!(
            roots
                .root_of(&session)
                .expect("read Twin registry")
                .is_none()
        );
        assert!(
            roots
                .overlay_for(Path::new("moonbase/scene.usda"))
                .expect("read overlay registry")
                .is_some()
        );
        assert!(
            roots
                .overlay_for(Path::new("__viewport_1/scene.usda"))
                .expect("read overlay registry")
                .is_none()
        );
    }

    #[test]
    fn poisoned_registry_rejects_mount_mutation_and_reads() {
        let twin = tempfile::tempdir().expect("temporary Twin root");
        let roots = TwinRoots::default();
        let name = roots
            .register("moonbase", twin.path())
            .expect("register root");
        let registry = roots.registry.clone();
        std::thread::spawn(move || {
            let _guard = registry.write().expect("registry lock");
            panic!("poison registry for the transaction test");
        })
        .join()
        .expect_err("the transaction test must poison the registry lock");

        assert_eq!(
            roots.unregister_name(&name),
            Err(TwinRootsError::RegistryPoisoned)
        );
        assert_eq!(
            roots.root_for(&name),
            Err(TwinRootsError::RegistryPoisoned),
            "unavailable ownership must fail visibly instead of fabricating a root"
        );
    }

    #[cfg(unix)]
    #[test]
    fn reader_resolution_rejects_symlinks_that_leave_a_twin_root() {
        let root = tempfile::tempdir().expect("temporary Twin root");
        let outside = tempfile::tempdir().expect("temporary outside root");
        let secret = outside.path().join("secret.usda");
        lunco_storage::write_file_sync(&secret, b"#usda 1.0").expect("secret");
        lunco_storage::create_file_symlink_sync(&secret, &root.path().join("linked.usda"))
            .expect("symlink");

        let roots = TwinRoots::default();
        let name = roots
            .register("example", root.path())
            .expect("register root");
        let reader = TwinReader { roots };
        assert!(matches!(
            reader.resolve(Path::new(&format!("{name}/linked.usda"))),
            Err(TwinRootsError::AssetResolution(
                std::io::ErrorKind::PermissionDenied,
                _
            ))
        ));
    }
}
