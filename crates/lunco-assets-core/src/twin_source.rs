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
//! ## Internal asset path — `twin://<authority>/<relative>`
//! Asset-reader addresses use an opaque authority assigned to one mount
//! lifetime. Authorities are never rebound after unmount, so cached assets and
//! late readers cannot acquire another Twin's bytes. [`stable_source_path`]
//! converts the load address to the stable logical source identity used for
//! provenance. User-facing scene URIs use that logical identity and resolve to
//! the current authority only at `LoadScene` admission. Twin files author
//! relative and `lunco://` references, never mount authorities.

use std::collections::{HashMap, HashSet};
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
pub use lunco_assets_path::TWIN_SCHEME;

/// Immutable native-read scope captured from one admitted Twin. Publication
/// still validates the caller's source lease; an authority from another Twin
/// can never be interpreted relative to this scope's directory.
#[derive(Clone, Debug)]
pub struct TwinReadScope {
    authority: String,
    logical_identity: String,
    root: PathBuf,
}

impl TwinReadScope {
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
    pub(crate) fn accepts(&self, source_identity: &str) -> bool {
        source_identity == self.authority || source_identity == self.logical_identity
    }
    /// Admit a standalone source through the same registry used by the app.
    pub fn declared(logical_identity: &str, root: &Path) -> Result<Self, TwinRootsError> {
        let roots = TwinRoots::default();
        let authority = roots.register_authority(logical_identity, root)?;
        roots.read_scope(&authority)
    }
    pub fn for_twin(twin: &lunco_twin::Twin) -> Result<Self, TwinRootsError> {
        let roots = TwinRoots::default();
        let authority = roots.register_twin(twin)?;
        roots.read_scope(&authority)
    }
}

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
    #[error("invalid Twin logical source identity `{0}`")]
    InvalidLogicalIdentity(String),
    #[error("unknown Twin asset authority `{0}`")]
    UnknownAuthority(String),
    #[error("Twin source identity is not mounted")]
    SourceNotMounted,
    #[error("Twin source identity conflicts with a mount authority")]
    SourceIdentityConflictsWithMountAuthority,
    #[error("Twin root registry sequence is exhausted")]
    RegistryExhausted,
    #[error("Twin source identity `{0}` is already mounted from another root")]
    DuplicateSourceIdentity(String),
    #[error("invalid Twin display name")]
    InvalidDisplayName,
    #[error("invalid Twin overlay path `{0}`")]
    InvalidOverlayPath(String),
    #[error("asset resolution failed ({0:?}): {1}")]
    AssetResolution(std::io::ErrorKind, String),
    #[error("Twin root resolution failed ({0:?}): {1}")]
    RootResolution(std::io::ErrorKind, String),
}

/// Stable logical source component derived from the Twin's authored identity.
/// Manifest UUIDs provide cross-session and cross-peer identity; older manifests
/// and plain folders retain their encoded display component.
pub fn logical_twin_source_identity(twin: &lunco_twin::Twin) -> String {
    let display_name = twin.display_name();
    if let Some(uuid) = twin.manifest.as_ref().and_then(|manifest| manifest.uuid) {
        return format!("uuid-{}", uuid.simple());
    }
    let encode = if display_name == "." || display_name == ".." {
        TWIN_DOT_NAME_COMPONENT
    } else {
        TWIN_NAME_COMPONENT
    };
    percent_encoding::utf8_percent_encode(&display_name, encode).to_string()
}

/// The `twin://<authority>/<rel>` URI naming `rel` inside the mounted Twin — the ONE
/// place the scheme string is spelled into an address. Callers that hand-rolled
/// `format!("twin://{authority}/{rel}")` duplicated resolution knowledge this crate
/// owns; a scheme rename must not require editing five crates.
///
/// `authority` is the returned mount authority when loading. `rel` is
/// normalized to forward slashes and stripped of a leading `/`. Logical
/// identity conversion belongs to [`stable_source_path`], not this helper.
pub fn twin_uri(authority: impl AsRef<str>, rel: impl AsRef<Path>) -> String {
    let rel = crate::asset_path::slashed(rel);
    crate::asset_path::uri(
        TWIN_SCHEME,
        &format!("{}/{}", authority.as_ref(), rel.trim_start_matches('/')),
    )
}

/// Replace unknown opaque Twin mount authorities in public diagnostics with a
/// generic source marker. Known authorities should first pass through
/// [`TwinRoots::stable_source_text`] so they retain their stable identity.
pub fn redact_opaque_twin_authorities(text: &str) -> String {
    redact_opaque_twin_authorities_except(text, &HashSet::new())
}

fn redact_opaque_twin_authorities_except(text: &str, known_sources: &HashSet<String>) -> String {
    let mut output = String::with_capacity(text.len());
    let mut run_start = None;
    for (index, character) in text.char_indices() {
        if character.is_ascii_hexdigit() {
            run_start.get_or_insert(index);
        } else if let Some(start) = run_start.take() {
            let run = &text[start..index];
            if run.len() == 32 && text[..start].ends_with("twin://") && !known_sources.contains(run)
            {
                output.push_str("[Twin source]");
            } else {
                output.push_str(run);
            }
            output.push(character);
        } else {
            output.push(character);
        }
    }
    if let Some(start) = run_start {
        let run = &text[start..];
        if run.len() == 32 && text[..start].ends_with("twin://") && !known_sources.contains(run) {
            output.push_str("[Twin source]");
        } else {
            output.push_str(run);
        }
    }
    output
}

fn replace_twin_source_reference(text: &str, source: &str, display_name: &str) -> String {
    let needle = format!("{TWIN_SCHEME}://{source}");
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some(offset) = text[cursor..].find(&needle) {
        let start = cursor + offset;
        let end = start + needle.len();
        let next = text[end..].chars().next();
        if next.is_none_or(|character| character == '/' || is_source_text_boundary(character)) {
            output.push_str(&text[cursor..start]);
            output.push_str(display_name);
            if next == Some('/') {
                output.push_str(" / ");
                cursor = end + 1;
            } else {
                cursor = end;
            }
        } else {
            output.push_str(&text[cursor..end]);
            cursor = end;
        }
    }
    output.push_str(&text[cursor..]);
    output
}

fn is_source_text_boundary(character: char) -> bool {
    character.is_whitespace() || matches!(character, '`' | '\'' | '"' | ')' | ']' | '}' | ',' | ';')
}

/// Split a `twin://<authority>/<rel>` URI into its parts, or `None` when it carries a
/// different scheme (or no scheme at all). The parsing inverse of [`twin_uri`].
pub fn parse_twin_uri(uri: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = crate::asset_path::split_scheme(uri)?;
    split_twin_rel((scheme.eq_ignore_ascii_case(TWIN_SCHEME)).then_some(rest)?)
}

/// Parse a Twin-root address used by root-scoped commands, without a file path.
pub fn parse_twin_root_uri(uri: &str) -> Option<&str> {
    let (scheme, authority) = crate::asset_path::split_scheme(uri)?;
    (scheme.eq_ignore_ascii_case(TWIN_SCHEME)
        && !authority.is_empty()
        && !authority.contains(['/', '\\']))
    .then_some(authority)
}

/// Split the scheme-stripped remainder of a `twin://` address — the
/// `<authority>/<rel>` form an `AssetReader` or scheme handler receives — into
/// the Twin authority and Twin-relative path. [`parse_twin_uri`] for callers that
/// hold the full URI.
pub fn split_twin_rel(rest: &str) -> Option<(&str, &str)> {
    rest.split_once(['/', '\\'])
}

/// The key an overlay is stored under — the reader-facing relative path
/// `<authority>/<rel>`, matching what [`AssetReader::read`] receives once the
/// `twin://` scheme is stripped.
fn overlay_key(authority: &str, rel: &str) -> PathBuf {
    Path::new(authority).join(rel)
}

/// Authoritative Twin mount registry shared by the asset reader and runtime.
/// One lock owns mount admission, composed-document overlays, and stable source
/// identities. Unmount drops roots and bytes; only authority-to-logical-identity
/// metadata remains, so old handles retain diagnosable provenance without
/// retaining a closed Twin's resources or making its address readable again.
#[derive(Resource, Clone, Default)]
pub struct TwinRoots {
    registry: Arc<RwLock<TwinRootRegistry>>,
}

#[derive(Default)]
struct TwinRootRegistry {
    roots: HashMap<String, TwinMount>,
    logical_identities: HashMap<String, String>,
    overlays: HashMap<PathBuf, Arc<Vec<u8>>>,
    revision: u64,
}

/// Immutable live logical source paths for background authored-file readers. No overlay bytes
/// or retired identity metadata are retained. Publication checks the revision
/// against the live registry before consuming derived results.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TwinRootsSnapshot {
    pub revision: u64,
    logical_roots: HashMap<String, PathBuf>,
}
impl TwinRootsSnapshot {
    /// Resolve an authored logical Twin source identity in this admitted scope snapshot.
    /// Transport authorities and stale-origin rejection belong to the live
    /// registry; snapshot consumers fence publication by scope and revision.
    pub fn resolve_authored_file(
        &self,
        logical_identity: &str,
        relative: &Path,
    ) -> Result<Option<PathBuf>, TwinRootsError> {
        match self.logical_roots.get(logical_identity) {
            Some(root) => resolve_twin_relative_file(root, relative),
            None => Ok(None),
        }
    }
}

struct TwinMount {
    root: PathBuf,
    source_identity: String,
    display_name: String,
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
        .logical_identity_for_authority(authority)?;
    Ok(format!("{logical}/{relative}"))
}

#[cfg(not(target_arch = "wasm32"))]
fn canonical_root(root: &Path) -> Result<PathBuf, TwinRootsError> {
    crate::canonical_directory_root(root).map_err(|error| {
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
    let roots = crate::asset_read_roots_with_shared(root, shared);
    #[cfg(not(target_arch = "wasm32"))]
    return crate::lunco_source::resolve_native_asset(&roots, relative, None)
        .map(|resolved| resolved.map(|(_, path)| path))
        .map_err(|error| TwinRootsError::AssetResolution(error.kind(), error.to_string()));
    #[cfg(target_arch = "wasm32")]
    {
        if !crate::asset_path::is_safe_relative_components(relative) {
            return Err(TwinRootsError::AssetResolution(
                std::io::ErrorKind::InvalidInput,
                format!("unsafe relative path `{}`", relative.display()),
            ));
        }
        for root in roots {
            let path = root.join(relative);
            if path.exists() {
                return Ok(Some(path));
            }
        }
        Ok(None)
    }
}

pub(crate) fn resolve_twin_relative_file(
    root: &Path,
    relative: &Path,
) -> Result<Option<PathBuf>, TwinRootsError> {
    #[cfg(not(target_arch = "wasm32"))]
    return crate::lunco_source::resolve_native_asset(
        &crate::asset_read_roots(root),
        relative,
        Some(lunco_storage::StorageEntryKind::File),
    )
    .map(|resolved| resolved.map(|(_, path)| path))
    .map_err(|error| TwinRootsError::AssetResolution(error.kind(), error.to_string()));
    #[cfg(target_arch = "wasm32")]
    Ok(resolve_twin_relative_path(root, relative)?.filter(|path| path.is_file()))
}

pub(crate) fn resolve_twin_relative_directory(
    root: &Path,
    relative: &Path,
) -> Result<Option<PathBuf>, TwinRootsError> {
    // Domain manifests designate the admitted Twin directory itself with `.`.
    if relative == Path::new(".") {
        #[cfg(not(target_arch = "wasm32"))]
        return crate::resolve_native_asset(
            &[root.to_path_buf()],
            Path::new(""),
            Some(lunco_storage::StorageEntryKind::Directory),
        )
        .map(|resolved| resolved.map(|(_, path)| path))
        .map_err(|error| TwinRootsError::AssetResolution(error.kind(), error.to_string()));
        #[cfg(target_arch = "wasm32")]
        return Ok(Some(root.to_path_buf()));
    }
    #[cfg(not(target_arch = "wasm32"))]
    return crate::lunco_source::resolve_native_asset(
        &crate::asset_read_roots(root),
        relative,
        Some(lunco_storage::StorageEntryKind::Directory),
    )
    .map(|resolved| resolved.map(|(_, path)| path))
    .map_err(|error| TwinRootsError::AssetResolution(error.kind(), error.to_string()));
    #[cfg(target_arch = "wasm32")]
    Ok(resolve_twin_relative_path(root, relative)?.filter(|path| path.is_dir()))
}

impl TwinRoots {
    /// Capture the exact current authority, logical identity and canonical root
    /// under one registry read. A retired authority cannot produce a read scope.
    pub fn read_scope(&self, authority: &str) -> Result<TwinReadScope, TwinRootsError> {
        let registry = self
            .registry
            .read()
            .map_err(|_| TwinRootsError::RegistryPoisoned)?;
        let root = registry
            .roots
            .get(authority)
            .ok_or_else(|| TwinRootsError::UnknownAuthority(authority.to_owned()))?;
        let logical_identity = registry
            .logical_identities
            .get(authority)
            .ok_or_else(|| TwinRootsError::UnknownAuthority(authority.to_owned()))?;
        Ok(TwinReadScope {
            authority: authority.to_owned(),
            logical_identity: logical_identity.clone(),
            root: root.root.clone(),
        })
    }
    /// Capture current live logical sources without reading filesystem metadata.
    pub fn snapshot(&self) -> Result<TwinRootsSnapshot, TwinRootsError> {
        let registry = self
            .registry
            .read()
            .map_err(|_| TwinRootsError::RegistryPoisoned)?;
        Ok(TwinRootsSnapshot {
            revision: registry.revision,
            logical_roots: registry
                .roots
                .iter()
                .map(|(authority, mount)| {
                    registry
                        .logical_identities
                        .get(authority)
                        .map(|logical| (logical.clone(), mount.root.clone()))
                        .ok_or_else(|| TwinRootsError::UnknownAuthority(authority.clone()))
                })
                .collect::<Result<_, _>>()?,
        })
    }

    pub fn revision(&self) -> Result<u64, TwinRootsError> {
        Ok(self
            .registry
            .read()
            .map_err(|_| TwinRootsError::RegistryPoisoned)?
            .revision)
    }
    fn clear_overlays_for_authorities(
        overlays: &mut HashMap<PathBuf, Arc<Vec<u8>>>,
        authorities: &[String],
    ) {
        overlays.retain(|path, _| {
            path.components()
                .next()
                .and_then(|component| component.as_os_str().to_str())
                .is_none_or(|authority| !authorities.iter().any(|removed| removed == authority))
        });
    }

    /// Register the asset authority for an opened workspace Twin.
    ///
    /// The workspace event remains the normal lifecycle trigger, but startup
    /// composition can admit a Twin and immediately request its default scene
    /// in the same observer dispatch. Exposing this operation on the asset
    /// owner lets that composition root establish the `twin://` authority
    /// before any domain observer resolves the scene; repeated registration is
    /// idempotent for the same root. Authored source identity and display name
    /// remain independent.
    pub fn register_twin(&self, twin: &lunco_twin::Twin) -> Result<String, TwinRootsError> {
        self.register_authority_with_display_name(
            logical_twin_source_identity(twin),
            twin.display_name(),
            twin.root.clone(),
        )
    }

    /// Admit a Twin root and return its mount-local load authority.
    ///
    /// Repeated admission of the same live root/source identity is idempotent.
    /// A source identity cannot name two live roots. Every mount receives a
    /// fresh opaque UUID authority, including a remount of the same folder.
    /// Callers must use the returned authority for all loads and overlays.
    #[must_use = "use the RETURNED mount authority to build `twin://` load URIs"]
    pub fn register_authority(
        &self,
        logical_identity: impl Into<String>,
        root: impl Into<PathBuf>,
    ) -> Result<String, TwinRootsError> {
        let requested_identity = logical_identity.into();
        let display_name = percent_encoding::percent_decode_str(&requested_identity)
            .decode_utf8_lossy()
            .into_owned();
        self.register_authority_with_display_name(requested_identity, display_name, root)
    }

    /// Admit one source with separate provenance identity and presentation label.
    /// Twins with an authored manifest UUID and downloaded scenario caches use
    /// this path; the short form is for synthetic mounts whose identity is also
    /// their label.
    pub fn register_authority_with_display_name(
        &self,
        logical_identity: impl Into<String>,
        display_name: impl Into<String>,
        root: impl Into<PathBuf>,
    ) -> Result<String, TwinRootsError> {
        let requested_identity = logical_identity.into();
        let display_name = display_name.into();
        if display_name.trim().is_empty() {
            return Err(TwinRootsError::InvalidDisplayName);
        }
        if !crate::asset_path::is_safe_relative_path(&requested_identity)
            || requested_identity.contains(['/', '\\', ':', '#', '?'])
        {
            return Err(TwinRootsError::InvalidLogicalIdentity(requested_identity));
        }
        let root = root.into();
        let canonical = canonical_root(&root)?;
        let mut registry = self
            .registry
            .write()
            .map_err(|_| TwinRootsError::RegistryPoisoned)?;
        if let Some((authority, _)) = registry.roots.iter().find(|(_, mount)| {
            mount.root == canonical && mount.source_identity == requested_identity
        }) {
            return Ok(authority.clone());
        }
        if registry
            .logical_identities
            .contains_key(&requested_identity)
        {
            return Err(TwinRootsError::SourceIdentityConflictsWithMountAuthority);
        }
        if registry.roots.keys().any(|authority| {
            registry.logical_identities.get(authority) == Some(&requested_identity)
        }) {
            return Err(TwinRootsError::DuplicateSourceIdentity(requested_identity));
        }
        let mut authority = uuid::Uuid::new_v4().simple().to_string();
        while registry.logical_identities.contains_key(&authority)
            || registry
                .logical_identities
                .values()
                .any(|identity| identity == &authority)
            || requested_identity == authority
        {
            authority = uuid::Uuid::new_v4().simple().to_string();
        }
        let admission = registry
            .revision
            .checked_add(1)
            .ok_or(TwinRootsError::RegistryExhausted)?;
        registry.revision = admission;
        registry.roots.insert(
            authority.clone(),
            TwinMount {
                root: canonical,
                source_identity: requested_identity.clone(),
                display_name,
                admission,
            },
        );
        registry
            .logical_identities
            .insert(authority.clone(), requested_identity);
        Ok(authority)
    }

    /// Stable logical source identity of an admitted mount, including retired mounts.
    /// Identity metadata does not make a retired authority readable again.
    pub fn logical_identity_for_authority(
        &self,
        authority: &str,
    ) -> Result<String, TwinRootsError> {
        self.registry
            .read()
            .map_err(|_| TwinRootsError::RegistryPoisoned)?
            .logical_identities
            .get(authority)
            .cloned()
            .ok_or_else(|| TwinRootsError::UnknownAuthority(authority.to_string()))
    }

    /// Replace mount-local authorities in diagnostic or presentation text with
    /// their stable source identities. Retired authority metadata remains
    /// available for sanitizing late failures, but it never restores read access.
    pub fn stable_source_text(&self, text: &str) -> Result<String, TwinRootsError> {
        let registry = self
            .registry
            .read()
            .map_err(|_| TwinRootsError::RegistryPoisoned)?;
        let known_sources = registry
            .logical_identities
            .keys()
            .chain(registry.logical_identities.values())
            .cloned()
            .collect();
        let sanitized = redact_opaque_twin_authorities_except(text, &known_sources);
        let stable = registry
            .logical_identities
            .iter()
            .fold(sanitized, |text, (authority, identity)| {
                text.replace(authority, identity)
            });
        Ok(stable)
    }

    /// Human-readable source label for a live mount authority.
    pub fn display_name_for_authority(&self, authority: &str) -> Result<String, TwinRootsError> {
        let registry = self
            .registry
            .read()
            .map_err(|_| TwinRootsError::RegistryPoisoned)?;
        let mount = registry
            .roots
            .get(authority)
            .ok_or_else(|| TwinRootsError::UnknownAuthority(authority.to_owned()))?;
        Ok(mount.display_name.clone())
    }

    /// Human-readable source label for a stable identity or live mount authority.
    pub fn display_name_for_source(&self, source: &str) -> Result<String, TwinRootsError> {
        let registry = self
            .registry
            .read()
            .map_err(|_| TwinRootsError::RegistryPoisoned)?;
        if let Some(mount) = registry.roots.get(source) {
            return Ok(mount.display_name.clone());
        }
        registry
            .roots
            .values()
            .find(|mount| mount.source_identity == source)
            .map(|mount| mount.display_name.clone())
            .ok_or(TwinRootsError::SourceNotMounted)
    }

    /// Replace Twin addresses in user-facing text with their display names.
    /// Retired sources use a generic label because their display metadata is
    /// released with the mount.
    pub fn display_source_text(&self, text: &str) -> Result<String, TwinRootsError> {
        let registry = self
            .registry
            .read()
            .map_err(|_| TwinRootsError::RegistryPoisoned)?;
        let mut names = HashMap::<String, String>::new();
        for (authority, mount) in &registry.roots {
            names.insert(authority.clone(), mount.display_name.clone());
            names.insert(mount.source_identity.clone(), mount.display_name.clone());
        }
        for (authority, identity) in &registry.logical_identities {
            let display_name = names
                .get(identity)
                .cloned()
                .unwrap_or_else(|| "Twin source".to_owned());
            names
                .entry(authority.clone())
                .or_insert(display_name.clone());
            names.entry(identity.clone()).or_insert(display_name);
        }

        let mut replacements = names.into_iter().collect::<Vec<_>>();
        replacements.sort_by(|left, right| right.0.len().cmp(&left.0.len()));
        Ok(replacements
            .into_iter()
            .fold(text.to_owned(), |text, (source, display_name)| {
                replace_twin_source_reference(&text, &source, &display_name)
            }))
    }

    /// Display a native source path relative to the open Twin that owns it.
    pub fn display_path_for_native_path(
        &self,
        path: &Path,
    ) -> Result<Option<String>, TwinRootsError> {
        let registry = self
            .registry
            .read()
            .map_err(|_| TwinRootsError::RegistryPoisoned)?;
        let owners = registry
            .roots
            .values()
            .filter_map(|mount| {
                let relative = path.strip_prefix(&mount.root).ok()?;
                Some((mount.root.components().count(), mount, relative))
            })
            .collect::<Vec<_>>();
        let depth = owners.iter().map(|(depth, _, _)| *depth).max();
        let mut owners = owners
            .into_iter()
            .filter(|(owner_depth, _, _)| Some(*owner_depth) == depth);
        let Some((_, owner, relative)) = owners.next() else {
            return Ok(None);
        };
        if owners.any(|(_, candidate, _)| candidate.display_name != owner.display_name) {
            return Ok(None);
        }
        Ok(Some(format!(
            "{} / {}",
            owner.display_name,
            relative.display()
        )))
    }

    /// Current load authority for an exact logical source identity, when mounted.
    /// Used by scenario synchronization to reuse an editable local root instead
    /// of shadowing it with the downloaded copy.
    pub fn mounted_authority_for_logical(
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
                    .logical_identities
                    .get(*authority)
                    .is_some_and(|identity| identity == logical)
            })
            .cloned())
    }

    /// Serve `bytes` in place of the on-disk file at `twin://<authority>/<rel>`. The
    /// key matches the path the `AssetReader` receives (scheme stripped), so a
    /// subsequent `AssetServer` load/reload of `twin://<authority>/<rel>` reads these
    /// bytes. Used by E1b to project a document's composed source into the live
    /// world; pass the same `(authority, rel)` to [`clear_overlay`](Self::clear_overlay)
    /// to fall back to disk.
    pub fn set_overlay(
        &self,
        authority: &str,
        rel: &str,
        bytes: Arc<Vec<u8>>,
    ) -> Result<(), TwinRootsError> {
        if !crate::asset_path::is_safe_relative_path(authority)
            || !crate::asset_path::is_safe_relative_path(rel)
        {
            return Err(TwinRootsError::InvalidOverlayPath(format!(
                "{authority}/{rel}"
            )));
        }
        let mut registry = self
            .registry
            .write()
            .map_err(|_| TwinRootsError::RegistryPoisoned)?;
        if !registry.roots.contains_key(authority) {
            return Err(TwinRootsError::UnknownAuthority(authority.to_string()));
        }
        registry.overlays.insert(overlay_key(authority, rel), bytes);
        Ok(())
    }

    /// Drop the in-memory overlay for `twin://<authority>/<rel>` so reads fall back
    /// to the on-disk file again.
    pub fn clear_overlay(&self, authority: &str, rel: &str) -> Result<(), TwinRootsError> {
        if !crate::asset_path::is_safe_relative_path(authority)
            || !crate::asset_path::is_safe_relative_path(rel)
        {
            return Err(TwinRootsError::InvalidOverlayPath(format!(
                "{authority}/{rel}"
            )));
        }
        self.registry
            .write()
            .map_err(|_| TwinRootsError::RegistryPoisoned)?
            .overlays
            .remove(&overlay_key(authority, rel));
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
        authority: &str,
        relative: &Path,
    ) -> Result<Option<Arc<Vec<u8>>>, TwinRootsError> {
        if !crate::asset_path::is_safe_relative_path(authority)
            || !crate::asset_path::is_safe_relative_components(relative)
        {
            return Err(TwinRootsError::InvalidOverlayPath(format!(
                "{authority}/{}",
                crate::asset_path::slashed(relative)
            )));
        }
        self.overlay_for(&overlay_key(
            authority,
            &crate::asset_path::slashed(relative),
        ))
    }

    /// Overlay bytes registered for the reader-facing relative `path`
    /// (`<authority>/<rel>`), if any.
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
    pub fn root_for(&self, authority: &str) -> Result<Option<PathBuf>, TwinRootsError> {
        self.registry
            .read()
            .map(|registry| {
                registry
                    .roots
                    .get(authority)
                    .map(|mount| mount.root.clone())
            })
            .map_err(|_| TwinRootsError::RegistryPoisoned)
    }

    /// Return the first admitted live authority for an open Twin root.
    ///
    /// Authorities can be disambiguated when two open folders share the same
    /// authored/folder name, so consumers must resolve the assigned authority
    /// instead of reconstructing it from the manifest again.
    pub fn authority_for_root(
        &self,
        root: impl AsRef<Path>,
    ) -> Result<Option<String>, TwinRootsError> {
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
                    .map(|(authority, _)| authority.clone())
            })
    }

    /// Current load authorities, ordered by stable logical source identity.
    pub fn authorities(&self) -> Result<Vec<String>, TwinRootsError> {
        self.registry
            .read()
            .map(|registry| {
                let mut v: Vec<String> = registry.roots.keys().cloned().collect();
                v.sort_by(|left, right| {
                    registry
                        .logical_identities
                        .get(left)
                        .cmp(&registry.logical_identities.get(right))
                        .then_with(|| {
                            registry.roots[left]
                                .admission
                                .cmp(&registry.roots[right].admission)
                        })
                });
                v
            })
            .map_err(|_| TwinRootsError::RegistryPoisoned)
    }

    /// Resolve a Twin-relative file using the same authored-first, Twin-cache,
    /// shared-cache
    /// policy as the `twin://` AssetReader. Native consumers that need a concrete
    /// filesystem path must use this boundary rather than joining a Twin root
    /// themselves, because downloaded Twin assets live in `.cache`.
    pub fn resolve_file(
        &self,
        authority: &str,
        relative: &Path,
    ) -> Result<Option<PathBuf>, TwinRootsError> {
        let Some(root) = self.root_for(authority)? else {
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
        authority: &str,
        relative: &Path,
    ) -> Result<Option<PathBuf>, TwinRootsError> {
        let Some(root) = self.root_for(authority)? else {
            return Ok(None);
        };
        resolve_twin_relative_directory(&root, relative)
    }

    /// Whether a Twin-relative file or directory exists in the Twin's
    /// authored-tree, Twin-cache, shared-cache order, including a composed
    /// document overlay. Native readers only: on wasm the mounted OPFS tree
    /// cannot be probed synchronously and every candidate is reported present.
    pub fn resolve_existing(
        &self,
        authority: &str,
        relative: &Path,
    ) -> Result<bool, TwinRootsError> {
        if self
            .overlay_for(&overlay_key(
                authority,
                &crate::asset_path::slashed(relative),
            ))?
            .is_some()
        {
            return Ok(true);
        }
        let Some(root) = self.root_for(authority)? else {
            return Err(TwinRootsError::UnknownAuthority(authority.to_owned()));
        };
        #[cfg(not(target_arch = "wasm32"))]
        return Ok(resolve_twin_relative_path(&root, relative)?.is_some());
        #[cfg(target_arch = "wasm32")]
        {
            let _ = root;
            Ok(true)
        }
    }

    /// The "primary" open Twin as `(authority, root)` — the Twin with the
    /// lexicographically first logical source identity, used as the default destination for newly created or
    /// imported assets when the caller doesn't name a Twin. `None` if no Twin
    /// is open.
    pub fn primary(&self) -> Result<Option<(String, PathBuf)>, TwinRootsError> {
        let Some(authority) = self.authorities()?.into_iter().next() else {
            return Ok(None);
        };
        Ok(self.root_for(&authority)?.map(|root| (authority, root)))
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
                .ok_or(TwinRootsError::RegistryExhausted)?;
        }
        for name in &removed {
            registry.roots.remove(name);
        }
        Self::clear_overlays_for_authorities(&mut registry.overlays, &removed);
        Ok(())
    }

    /// Retire one synthetic or user-session Twin authority by its exact value,
    /// including its composed-document overlays. This is distinct from
    /// [`unregister_root`](Self::unregister_root): several authorities may
    /// intentionally point at the same directory, so a document view must not
    /// tear down an unrelated Twin merely because their roots match.
    pub fn unregister_authority(&self, authority: &str) -> Result<(), TwinRootsError> {
        let mut registry = self
            .registry
            .write()
            .map_err(|_| TwinRootsError::RegistryPoisoned)?;
        if registry.roots.contains_key(authority) {
            registry.revision = registry
                .revision
                .checked_add(1)
                .ok_or(TwinRootsError::RegistryExhausted)?;
            registry.roots.remove(authority);
        }
        Self::clear_overlays_for_authorities(&mut registry.overlays, &[authority.to_string()]);
        Ok(())
    }
}

/// Storage failures retain the requested Twin identity in diagnostics.
#[cfg(target_arch = "wasm32")]
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
async fn open_reader(full: &Path) -> Result<Box<dyn Reader>, AssetReaderError> {
    Ok(Box::new(crate::native_reader::open_asset_file(full)?))
}

#[cfg(target_arch = "wasm32")]
async fn open_reader(full: &Path) -> Result<Box<dyn Reader>, AssetReaderError> {
    lunco_storage::OpfsStorage::new()
        .open_reader(&lunco_storage::StorageHandle::File(full.to_path_buf()))
        .await
        .map(|reader| Box::new(TwinOpfsReader(reader)) as Box<dyn Reader>)
        .map_err(|error| storage_read_error(full, error))
}

#[cfg(target_arch = "wasm32")]
struct TwinOpfsReader(lunco_storage::opfs_storage::OpfsReader);

#[cfg(target_arch = "wasm32")]
impl bevy::tasks::futures_lite::io::AsyncRead for TwinOpfsReader {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.get_mut().0).poll_read(cx, buf)
    }
}

#[cfg(target_arch = "wasm32")]
impl bevy::tasks::futures_lite::io::AsyncSeek for TwinOpfsReader {
    fn poll_seek(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        pos: std::io::SeekFrom,
    ) -> std::task::Poll<std::io::Result<u64>> {
        std::pin::Pin::new(&mut self.get_mut().0).poll_seek(cx, pos)
    }
}

#[cfg(target_arch = "wasm32")]
impl Reader for TwinOpfsReader {
    fn seekable(
        &mut self,
    ) -> Result<&mut dyn bevy::asset::io::SeekableReader, bevy::asset::io::ReaderNotSeekableError>
    {
        Ok(self)
    }
}

struct SharedBytes(Arc<Vec<u8>>);

impl AsRef<[u8]> for SharedBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// Overlay readers pin the immutable revision without copying its payload.
struct OverlayReader(bevy::tasks::futures_lite::io::Cursor<SharedBytes>);

impl bevy::tasks::futures_lite::io::AsyncRead for OverlayReader {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.get_mut().0).poll_read(cx, buf)
    }
}

impl bevy::tasks::futures_lite::io::AsyncSeek for OverlayReader {
    fn poll_seek(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        pos: std::io::SeekFrom,
    ) -> std::task::Poll<std::io::Result<u64>> {
        std::pin::Pin::new(&mut self.get_mut().0).poll_seek(cx, pos)
    }
}

impl Reader for OverlayReader {
    fn seekable(
        &mut self,
    ) -> Result<&mut dyn bevy::asset::io::SeekableReader, bevy::asset::io::ReaderNotSeekableError>
    {
        Ok(self)
    }
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
/// streams `<root>/<rel>` through storage. Consumer budgets apply before any
/// full-payload allocation on either native files or browser OPFS snapshots.
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
            return Err::<Box<dyn Reader>, _>(AssetReaderError::NotFound(path.to_path_buf()));
        };
        // In-memory overlay wins over the on-disk file (E1b: a scene document's
        // composed source projected into the live world). Keyed by the exact
        // reader-facing `<name>/<rel>` path.
        if let Some(bytes) = self
            .roots
            .overlay_for(path.as_path())
            .map_err(asset_reader_error)?
        {
            return Ok(
                Box::new(OverlayReader(bevy::tasks::futures_lite::io::Cursor::new(
                    SharedBytes(bytes),
                ))) as Box<dyn Reader>,
            );
        }
        let Some(full) = self.resolve(path.as_path()).map_err(asset_reader_error)? else {
            return Err::<Box<dyn Reader>, _>(AssetReaderError::NotFound(path.to_path_buf()));
        };
        open_reader(&full).await
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
        if self
            .roots
            .overlay_for(path)
            .map_err(asset_reader_error)?
            .is_some()
        {
            return Ok(false);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(root) = self.roots.root_for(name).map_err(asset_reader_error)? else {
                return Ok(false);
            };
            return Ok(
                match resolve_twin_relative_path(&root, &relative).map_err(asset_reader_error)? {
                    Some(path) => std::fs::metadata(path)?.is_dir(),
                    None => false,
                },
            );
        }
        #[cfg(target_arch = "wasm32")]
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
    #[cfg(not(target_arch = "wasm32"))]
    fn native_resolution_edges_directory_probe_distinguishes_files_from_errors() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("entry"), b"file").unwrap();
        std::fs::create_dir_all(crate::twin_cache_dir(root.path()).join("entry")).unwrap();
        std::fs::create_dir(root.path().join("directory")).unwrap();
        let roots = TwinRoots::default();
        let authority = roots.register_authority("probe", root.path()).unwrap();
        let reader = TwinReader { roots };
        bevy::tasks::futures_lite::future::block_on(async {
            for name in ["entry", "missing"] {
                assert!(
                    !AssetReader::is_directory(&reader, &Path::new(&authority).join(name))
                        .await
                        .unwrap()
                );
            }
            assert!(
                AssetReader::is_directory(&reader, &Path::new(&authority).join("directory"))
                    .await
                    .unwrap()
            );
            reader
                .roots
                .set_overlay(&authority, "directory", Arc::new(b"overlay".to_vec()))
                .unwrap();
            assert!(
                !AssetReader::is_directory(&reader, &Path::new(&authority).join("directory"))
                    .await
                    .unwrap()
            );
        });
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn asset_resolution_contract_twin_stream() {
        use bevy::tasks::futures_lite::{
            future::block_on,
            io::{AsyncReadExt, AsyncSeekExt},
        };
        block_on(async {
            let root = tempfile::tempdir().unwrap();
            let file = root.path().join("payload.bin");
            std::fs::write(&file, b"original").unwrap();
            let roots = TwinRoots::default();
            let name = roots.register_authority("generic", root.path()).unwrap();
            let reader = TwinReader { roots };
            let path = PathBuf::from(format!("{name}/payload.bin"));
            let mut stream = AssetReader::read(&reader, &path).await.unwrap();
            // The open handle has not copied bytes. Truncating before the
            // first poll changes what the consumer receives.
            std::fs::OpenOptions::new()
                .write(true)
                .open(&file)
                .unwrap()
                .set_len(0)
                .unwrap();
            let mut byte = [0];
            assert_eq!(stream.read(&mut byte).await.unwrap(), 0);
            std::fs::write(&file, b"original").unwrap();
            assert_eq!(stream.read(&mut byte).await.unwrap(), 1);
            assert_eq!(
                stream
                    .seekable()
                    .unwrap()
                    .seek(std::io::SeekFrom::Current(0))
                    .await
                    .unwrap(),
                1
            );
            assert_eq!(
                stream
                    .seekable()
                    .unwrap()
                    .seek(std::io::SeekFrom::End(-1))
                    .await
                    .unwrap(),
                7
            );
            assert_eq!(stream.read(&mut byte).await.unwrap(), 1);
            assert_eq!(&byte, b"l");
            let bytes = Arc::new(b"overlay".to_vec());
            reader
                .roots
                .set_overlay(&name, "payload.bin", bytes.clone())
                .unwrap();
            let count = Arc::strong_count(&bytes);
            let mut overlay = AssetReader::read(&reader, &path).await.unwrap();
            assert_eq!(Arc::strong_count(&bytes), count + 1);
            assert_eq!(overlay.read(&mut byte).await.unwrap(), 1);
            assert_eq!(&byte, b"o");
            drop(overlay);
            assert_eq!(Arc::strong_count(&bytes), count);
        });
    }

    #[test]
    fn twin_mount_snapshot_revision_retires_equal_root_reopens() {
        let root = tempfile::tempdir().expect("generic root");
        let roots = TwinRoots::default();
        let empty = roots.snapshot().unwrap();
        let authority = roots.register_authority("generic", root.path()).unwrap();
        let admitted = roots.snapshot().unwrap();
        assert!(admitted.revision > empty.revision);
        assert_eq!(
            roots.register_authority("generic", root.path()).unwrap(),
            authority
        );
        assert_eq!(roots.revision().unwrap(), admitted.revision);
        roots.unregister_authority(&authority).unwrap();
        let retired = roots.snapshot().unwrap();
        assert!(retired.revision > admitted.revision);
        assert!(!retired.logical_roots.contains_key("generic"));
        assert_eq!(
            admitted.logical_roots.get("generic"),
            Some(&canonical_root(root.path()).unwrap()),
            "immutable preparation retains only its admitted path snapshot"
        );
        let reopened = roots.register_authority("generic", root.path()).unwrap();
        assert_ne!(reopened, authority);
        assert!(roots.revision().unwrap() > retired.revision);
        roots.unregister_authority(&authority).unwrap();
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
    /// receives for `twin://<authority>/<rel>` (scheme stripped) — otherwise an
    /// `AssetServer` load would miss it and read the on-disk file.
    #[test]
    fn overlay_keyed_by_reader_facing_path() {
        let roots = TwinRoots::default();
        let root = tempfile::tempdir().expect("Twin root");
        let authority = roots
            .register_authority("moonbase", root.path())
            .expect("register root");
        let bytes = Arc::new(b"#usda 1.0\n".to_vec());
        roots
            .set_overlay(&authority, "scenes/luncosim.usda", bytes.clone())
            .expect("set overlay");

        assert_eq!(
            roots
                .overlay_bytes(&authority, Path::new("scenes/luncosim.usda"))
                .expect("read named overlay")
                .as_deref(),
            Some(&*bytes),
            "native consumers use the same overlay key as the AssetReader"
        );

        // The reader receives `<authority>/scenes/luncosim.usda` (scheme stripped).
        assert_eq!(
            roots
                .overlay_for(Path::new(&format!("{authority}/scenes/luncosim.usda")))
                .expect("read overlay registry")
                .as_deref(),
            Some(&*bytes),
            "overlay hit for the exact reader-facing path"
        );
        assert_eq!(
            roots
                .overlay_for(Path::new(&format!(r"{authority}\scenes\luncosim.usda")))
                .expect("read overlay registry")
                .as_deref(),
            Some(&*bytes),
            "a Windows reader-facing path finds the slash-normalized overlay"
        );
        assert!(
            roots
                .overlay_for(Path::new(&format!("{authority}/other.usda")))
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
        for invalid in ["folder/file:stream.usda", "folder/nul\0.usda"] {
            assert!(matches!(
                roots.set_overlay("moonbase", invalid, Arc::new(b"secret".to_vec())),
                Err(TwinRootsError::InvalidOverlayPath(_))
            ));
            assert!(matches!(
                roots.clear_overlay("moonbase", invalid),
                Err(TwinRootsError::InvalidOverlayPath(_))
            ));
        }
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

    /// A source identity cannot be assigned to unrelated folders. Admission
    /// rejects the second owner instead of creating an order-dependent alias.
    #[test]
    fn duplicate_source_identity_is_rejected_without_repointing_the_first_root() {
        let roots = TwinRoots::default();
        let first_root = tempfile::tempdir().expect("first Twin root");
        let second_root = tempfile::tempdir().expect("second Twin root");

        let a = roots
            .register_authority("scenes", first_root.path())
            .expect("register first root");
        assert!(!a.contains("scenes"));
        assert_eq!(
            roots.display_name_for_authority(&a),
            Ok("scenes".to_owned())
        );
        assert_eq!(
            roots.register_authority("scenes", second_root.path()),
            Err(TwinRootsError::DuplicateSourceIdentity("scenes".to_owned()))
        );
        assert_eq!(
            roots.root_for(&a),
            Ok(Some(
                lunco_storage::canonicalize_file_path(first_root.path()).unwrap()
            )),
            "first Twin still resolves to its own folder"
        );
        assert_eq!(
            roots.logical_identity_for_authority(&a),
            Ok("scenes".to_string())
        );
        roots
            .unregister_authority(&a)
            .expect("retire first duplicate");
        let second = roots
            .register_authority("scenes", second_root.path())
            .expect("identity can be reused after its previous owner retires");
        assert_eq!(
            roots.root_for(&second),
            Ok(Some(
                lunco_storage::canonicalize_file_path(second_root.path()).unwrap()
            ))
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
            assert!(!first.contains(folder_name));
            assert_eq!(
                roots
                    .display_name_for_authority(&first)
                    .expect("display name"),
                folder_name
            );
            assert_eq!(
                roots
                    .logical_identity_for_authority(&first)
                    .expect("logical identity"),
                expected
            );
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

            roots
                .unregister_authority(&first)
                .expect("retire folder mount");
            let reopened = roots.register_twin(&twin).expect("reopen folder mount");
            assert_ne!(reopened, first);
            assert_eq!(
                roots
                    .logical_identity_for_authority(&reopened)
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
                .unregister_authority(&reopened)
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
                let mut manifest = lunco_twin::TwinManifest::new(display);
                manifest.uuid = None;
                twin.manifest = Some(manifest);
                let authority = roots
                    .register_twin(&twin)
                    .expect("manifest display-name mount");
                assert_eq!(
                    roots
                        .logical_identity_for_authority(&authority)
                        .expect("manifest logical identity"),
                    expected
                );
                assert_eq!(
                    twin.manifest.as_ref().unwrap().name,
                    display,
                    "display metadata is unchanged"
                );
                roots
                    .unregister_authority(&authority)
                    .expect("retire manifest mount");
            }
            for invalid in ["a/b", r"a\b", "a:b", "a#b", "a?b", ".", ".."] {
                assert!(
                    matches!(
                        roots.register_authority(invalid, &folder),
                        Err(TwinRootsError::InvalidLogicalIdentity(_))
                    ),
                    "low-level registration must reject {invalid:?}"
                );
            }
        });
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn authored_twin_uuid_owns_stable_source_identity_and_must_be_unique_while_mounted() {
        let first_root = tempfile::tempdir().expect("first Twin root");
        let second_root = tempfile::tempdir().expect("second Twin root");
        let lunco_twin::TwinMode::Folder(mut first) =
            lunco_twin::TwinMode::open(first_root.path()).expect("open first root")
        else {
            panic!("expected folder Twin");
        };
        let lunco_twin::TwinMode::Folder(mut second) =
            lunco_twin::TwinMode::open(second_root.path()).expect("open second root")
        else {
            panic!("expected folder Twin");
        };
        let identity =
            uuid::Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").expect("fixture UUID");
        let mut first_manifest = lunco_twin::TwinManifest::new("Roadster");
        first_manifest.uuid = Some(identity);
        first.manifest = Some(first_manifest);
        let mut second_manifest = lunco_twin::TwinManifest::new("Copied Roadster");
        second_manifest.uuid = Some(identity);
        second.manifest = Some(second_manifest);

        let roots = TwinRoots::default();
        let authority = roots.register_twin(&first).expect("mount authored Twin");
        let logical = format!("uuid-{}", identity.simple());
        assert_eq!(logical_twin_source_identity(&first), logical);
        assert_eq!(
            roots.logical_identity_for_authority(&authority),
            Ok(logical.clone())
        );
        assert_eq!(
            roots.register_twin(&second),
            Err(TwinRootsError::DuplicateSourceIdentity(logical)),
            "two live roots cannot claim one authored Twin identity"
        );

        first.manifest.as_mut().unwrap().name = "Renamed Roadster".to_owned();
        assert_eq!(
            logical_twin_source_identity(&first),
            format!("uuid-{}", identity.simple())
        );
    }

    #[test]
    fn logical_source_identity_cannot_alias_a_retired_mount_authority() {
        let first_root = tempfile::tempdir().expect("first Twin root");
        let second_root = tempfile::tempdir().expect("second Twin root");
        let roots = TwinRoots::default();
        let authority = roots
            .register_authority("public-source", first_root.path())
            .expect("mount first Twin");
        roots
            .unregister_authority(&authority)
            .expect("retire first mount");

        assert_eq!(
            roots.register_authority(authority.clone(), second_root.path()),
            Err(TwinRootsError::SourceIdentityConflictsWithMountAuthority),
            "public identities and internal leases must remain disjoint after retirement"
        );
    }

    /// Repeated admission while the same mount is live is idempotent.
    #[test]
    fn reregistering_same_root_reuses_the_live_authority() {
        let roots = TwinRoots::default();
        let root = tempfile::tempdir().expect("Twin root");

        let first = roots
            .register_authority("moonbase", root.path())
            .expect("register root");
        let again = roots
            .register_authority("moonbase", root.path())
            .expect("re-register root");

        assert_ne!(first, "moonbase");
        assert_eq!(
            roots.logical_identity_for_authority(&first).unwrap(),
            "moonbase"
        );
        assert_eq!(
            again, first,
            "live admission must reuse the existing authority"
        );
        assert_eq!(
            roots.authorities().expect("read Twin registry").len(),
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
            .register_authority("fixture", root.path())
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
        assert_eq!(
            roots
                .stable_source_text(&format!(
                    "failed to read {}",
                    twin_uri(&first, "data/payload.txt")
                ))
                .expect("sanitize late failure"),
            "failed to read twin://fixture/data/payload.txt"
        );
        assert_eq!(
            roots
                .stable_source_text(&format!(
                    "failed twin://{}/scenes/main.usda",
                    "a".repeat(32)
                ))
                .expect("redact unknown mount authority"),
            "failed twin://[Twin source]/scenes/main.usda"
        );
        let all_hex_identity = "b".repeat(32);
        roots
            .register_authority_with_display_name(
                all_hex_identity.clone(),
                "legacy source",
                root.path(),
            )
            .expect("mount all-hex logical identity");
        assert_eq!(
            roots
                .stable_source_text(&format!("twin://{all_hex_identity}/scenes/main.usda"))
                .expect("preserve all-hex stable identity"),
            format!("twin://{all_hex_identity}/scenes/main.usda")
        );

        let reopened = roots
            .register_authority("fixture", root.path())
            .expect("reopen same folder");
        assert_ne!(
            reopened, first,
            "Bevy path identities cannot survive a mount lifetime"
        );
        assert_eq!(
            roots
                .register_authority("fixture", root.path())
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
            .unregister_authority(&reopened)
            .expect("retire reopened mount");

        let next = roots
            .register_authority("fixture", replacement.path())
            .expect("mount replacement folder");
        assert_ne!(next, first);
        assert_ne!(next, reopened);
        assert_eq!(
            roots
                .logical_identity_for_authority(&next)
                .expect("replacement logical name"),
            "fixture"
        );
        assert_eq!(
            roots
                .mounted_authority_for_logical("fixture")
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
            .register_authority("fixture", host_root.path())
            .expect("host first admission");
        host.unregister_authority(&old).expect("host unmount");
        let host_name = host
            .register_authority("fixture", host_root.path())
            .expect("host reopen");
        let client_name = client
            .register_authority("fixture", client_root.path())
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
                .register_authority("fixture", first_root.path())
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
            roots.unregister_authority(&first).expect("retire first");
            let second = roots
                .register_authority("fixture", second_root.path())
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
        let name = roots
            .register_authority("luna2", twin.path())
            .expect("register root");

        assert_eq!(
            roots.resolve_file(&name, Path::new("terrain/luna2")),
            Ok(Some(
                lunco_storage::canonicalize_file_path(&cached).unwrap()
            )),
            "logical Twin paths must resolve downloaded assets through the cache"
        );

        let authored = twin.path().join("terrain/luna2");
        lunco_storage::ensure_directory_sync(authored.parent().expect("authored parent"))
            .expect("authored directory");
        lunco_storage::write_file_sync(&authored, b"authored terrain").expect("authored asset");
        assert_eq!(
            roots.resolve_file(&name, Path::new("terrain/luna2")),
            Ok(Some(
                lunco_storage::canonicalize_file_path(&authored).unwrap()
            )),
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
        let name = roots
            .register_authority("luna2", twin.path())
            .expect("register root");

        assert_eq!(
            roots.resolve_directory(&name, Path::new("terrain/luna2")),
            Ok(Some(
                lunco_storage::canonicalize_file_path(&cached).unwrap()
            )),
            "processed Twin directories must resolve through the asset boundary"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn directory_root_designator_retains_mount_ownership_and_rejects_parent_escape() {
        let twin = tempfile::tempdir().unwrap();
        let roots = TwinRoots::default();
        let name = roots
            .register_authority("native-root", twin.path())
            .unwrap();
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
        roots.unregister_authority(&name).unwrap();
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
            Some(lunco_storage::canonicalize_file_path(&shared_file).unwrap()),
            "a Twin must consume a shared logical asset without materialising a local copy"
        );
    }

    #[test]
    fn unregistering_a_root_removes_its_authority_and_overlays() {
        let roots = TwinRoots::default();
        let root = tempfile::tempdir().expect("Twin root");
        let name = roots
            .register_authority("moonbase", root.path())
            .expect("register root");
        roots
            .set_overlay(&name, "scene.usda", Arc::new(b"#usda 1.0\n".to_vec()))
            .expect("set overlay");

        roots.unregister_root(root.path()).expect("unregister root");

        assert!(roots.authorities().expect("read Twin registry").is_empty());
        assert!(roots.root_for(&name).expect("read Twin registry").is_none());
        assert!(
            roots
                .overlay_for(Path::new("moonbase/scene.usda"))
                .expect("read overlay registry")
                .is_none()
        );
    }

    #[test]
    fn unregistering_one_authority_preserves_another_authority_on_the_same_root() {
        let roots = TwinRoots::default();
        let root = tempfile::tempdir().expect("Twin root");
        let twin = roots
            .register_authority("moonbase", root.path())
            .expect("register root");
        let session = roots
            .register_authority("__viewport_1", root.path())
            .expect("register session root");
        assert_eq!(
            roots
                .authority_for_root(root.path())
                .expect("find open Twin mount"),
            Some(twin.clone())
        );
        roots
            .set_overlay(&twin, "scene.usda", Arc::new(b"twin".to_vec()))
            .expect("set Twin overlay");
        roots
            .set_overlay(&session, "scene.usda", Arc::new(b"session".to_vec()))
            .expect("set session overlay");

        roots
            .unregister_authority(&session)
            .expect("unregister session");

        assert!(roots.root_for(&twin).expect("read Twin registry").is_some());
        assert!(
            roots
                .root_for(&session)
                .expect("read Twin registry")
                .is_none()
        );
        assert!(
            roots
                .overlay_for(Path::new(&format!("{twin}/scene.usda")))
                .expect("read overlay registry")
                .is_some()
        );
        assert!(
            roots
                .overlay_for(Path::new(&format!("{session}/scene.usda")))
                .expect("read overlay registry")
                .is_none()
        );
    }

    #[test]
    fn poisoned_registry_rejects_mount_mutation_and_reads() {
        let twin = tempfile::tempdir().expect("temporary Twin root");
        let roots = TwinRoots::default();
        let name = roots
            .register_authority("moonbase", twin.path())
            .expect("register root");
        let registry = roots.registry.clone();
        std::thread::spawn(move || {
            let _guard = registry.write().expect("registry lock");
            panic!("poison registry for the transaction test");
        })
        .join()
        .expect_err("the transaction test must poison the registry lock");

        assert_eq!(
            roots.unregister_authority(&name),
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
            .register_authority("example", root.path())
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
