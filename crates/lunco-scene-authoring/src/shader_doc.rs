//! Shaders as a journaled, synced, live-editable **domain** — the WESL twin of
//! rhai's `ScriptDocument` (`lunco-scripting`) and Modelica's document model.
//!
//! A shader edit is no longer a fire-and-forget `Assets<Shader>` poke: it flows
//! through a [`ShaderDocument`] whose [`ShaderRegistry`] carries a
//! [`JournalOpRecorder`](lunco_doc_bevy::JournalOpRecorder). So a live source edit
//! records a [`ShaderOp::SetSource`] into the canonical Twin journal
//! (`DomainKind::Shader`) — which means it **hot-reloads locally AND syncs +
//! persists** exactly like a rhai behaviour edit. The op carries the shader's
//! canonical asset **path** (cross-peer-stable, unlike the locally-minted
//! `DocumentId`), so a peer routes a replayed edit to the right shader by path —
//! no single-doc limitation.
//!
//! What this module does NOT do: touch `Assets<Shader>`. The hot-reload hook
//! (`properties::apply_shader_source_live`) lives in the command handler / replay
//! system that owns `ResMut<Assets<Shader>>`; this module is the source-of-truth
//! document + journaling seam it drives.

use bevy::prelude::*;
use lunco_doc::{Document, DocumentError, DocumentHost, DocumentId, DocumentOp};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The canonical source for one shader, keyed by its asset `path`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShaderDocument {
    pub id: u64,
    pub generation: u64,
    /// Canonical asset path (`twin://…/foo.wesl` or `lunco://…/foo.wesl`) — the
    /// stable cross-peer routing key. The asset owner resolves it to an exact
    /// private loading lease before preparing a shader.
    pub path: String,
    pub source: String,
}

/// Return the one persisted identity for a shader asset.
///
/// Engine-library shader references are commonly authored without a scheme,
/// while Bevy loads them from the registered `lunco://` source. Canonicalizing
/// at the document boundary prevents one shader from acquiring separate
/// journal documents merely because two callers used different spellings.
pub fn canonical_shader_path(path: &str) -> String {
    let path = path.trim();
    if lunco_assets_core::has_scheme(path) {
        path.to_string()
    } else {
        lunco_assets_core::engine_asset_uri(path)
    }
}

impl ShaderDocument {
    pub fn new(id: u64, path: impl Into<String>, source: impl Into<String>) -> Self {
        let path = path.into();
        Self {
            id,
            generation: 0,
            path: canonical_shader_path(&path),
            source: source.into(),
        }
    }
}

/// A journaled edit to a shader. `SetSource` carries the `path` so a replayed op
/// finds-or-creates the right document on a peer without a shared `DocumentId`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ShaderOp {
    SetSource { path: String, source: String },
}

impl DocumentOp for ShaderOp {}

impl lunco_twin_journal::OpPayload for ShaderOp {
    fn domain(&self) -> lunco_twin_journal::DomainKind {
        lunco_twin_journal::DomainKind::Shader
    }
}

impl Document for ShaderDocument {
    type Op = ShaderOp;

    fn id(&self) -> DocumentId {
        DocumentId::new(self.id)
    }

    fn generation(&self) -> u64 {
        self.generation
    }

    fn apply(&mut self, op: ShaderOp) -> Result<ShaderOp, DocumentError> {
        let inverse = match op {
            ShaderOp::SetSource { path, source } => {
                let old = self.source.clone();
                self.source = source;
                ShaderOp::SetSource { path, source: old }
            }
        };
        self.generation += 1;
        Ok(inverse)
    }
}

/// `DocumentId`-keyed shader-source store with a path index and a journal handle.
/// Mirrors the scripting registry and generic Modelica document registry.
#[derive(Resource, Default)]
pub struct ShaderRegistry {
    documents: HashMap<DocumentId, DocumentHost<ShaderDocument>>,
    by_path: HashMap<String, DocumentId>,
    owner_roots: HashMap<DocumentId, std::path::PathBuf>,
    journal: Option<lunco_doc_bevy::JournalResource>,
}

impl ShaderRegistry {
    /// Get-or-create the document id for `path` (seeding `initial_source` on
    /// first sight). Attaches a recorder when a journal is wired.
    fn document_for(
        &mut self,
        path: &str,
        initial_source: &str,
        owner_root: Option<std::path::PathBuf>,
    ) -> Result<DocumentId, String> {
        if let Some(id) = self.by_path.get(path) {
            if self.owner_roots.get(id) != owner_root.as_ref() {
                return Err("Shader document belongs to a retired Twin source".into());
            }
            return Ok(*id);
        }
        let id = DocumentId::fresh();
        self.documents.insert(
            id,
            DocumentHost::new(ShaderDocument::new(id.raw(), path, initial_source)),
        );
        self.by_path.insert(path.to_string(), id);
        if let Some(root) = owner_root {
            self.owner_roots.insert(id, root);
        }
        self.attach_recorder(id);
        Ok(id)
    }

    /// Apply a live source edit **through the `DocumentHost`** so the recorder
    /// journals a `ShaderOp::SetSource` (→ syncs + persists) and the generation
    /// bumps. The one funnel every live shader edit routes through. Returns the
    /// doc id (caller then hot-reloads `Assets<Shader>`).
    pub fn apply_source(
        &mut self,
        path: &str,
        source: String,
        roots: Option<&lunco_assets_core::TwinRoots>,
    ) -> Result<DocumentId, String> {
        let path = canonical_shader_path(path);
        let root = shader_source_root(&path, roots)?;
        let id = self.document_for(&path, &source, root)?;
        let host = self
            .documents
            .get_mut(&id)
            .ok_or("Shader document is unavailable")?;
        host.apply(ShaderOp::SetSource { path, source })
            .map_err(|error| error.to_string())?;
        Ok(id)
    }

    /// Apply a **replayed** op (arrived via the journal, already recorded) WITHOUT
    /// re-recording — routing by the op's `path` (find-or-create). Returns
    /// `(path, source)` for the caller to hot-reload, rejecting an unavailable owner.
    pub fn apply_replayed(
        &mut self,
        op: &ShaderOp,
        roots: Option<&lunco_assets_core::TwinRoots>,
    ) -> Result<(String, String), String> {
        let ShaderOp::SetSource { path, source } = op;
        let path = canonical_shader_path(path);
        let root = shader_source_root(&path, roots)?;
        let id = self.document_for(&path, source, root)?;
        let host = self
            .documents
            .get_mut(&id)
            .ok_or("Shader document is unavailable")?;
        host.document_mut()
            .apply(ShaderOp::SetSource {
                path,
                source: source.clone(),
            })
            .map_err(|error| error.to_string())?;
        let d = host.document();
        Ok((d.path.clone(), d.source.clone()))
    }

    /// Retire documents and undo/recorder state before a Twin root is reused.
    fn retire_root(&mut self, root: &std::path::Path) {
        self.owner_roots.retain(|id, owner| {
            if owner == root {
                self.documents.remove(id);
                false
            } else {
                true
            }
        });
        self.by_path.retain(|_, id| self.documents.contains_key(id));
    }

    /// Current source for `path`, if a document exists.
    pub fn source_of(&self, path: &str) -> Option<&str> {
        let path = canonical_shader_path(path);
        self.by_path
            .get(&path)
            .and_then(|id| self.documents.get(id))
            .map(|h| h.document().source.as_str())
    }

    /// Wire the journal + retro-fit a recorder onto every existing host.
    pub fn set_journal(&mut self, journal: lunco_doc_bevy::JournalResource) {
        self.journal = Some(journal);
        let ids: Vec<_> = self.documents.keys().copied().collect();
        for id in ids {
            self.attach_recorder(id);
        }
    }

    fn attach_recorder(&mut self, id: DocumentId) {
        if let Some(journal) = &self.journal {
            if let Some(host) = self.documents.get_mut(&id) {
                if !host.has_recorder() {
                    lunco_doc_bevy::attach_journal_recorder(host, journal);
                }
            }
        }
    }
}

fn shader_source_root(
    path: &str,
    roots: Option<&lunco_assets_core::TwinRoots>,
) -> Result<Option<std::path::PathBuf>, String> {
    if lunco_assets_core::parse_twin_uri(path).is_none() {
        return Ok(None);
    }
    let roots = roots.ok_or("Shader source requires a Twin registry")?;
    let load = lunco_assets_core::asset_path::load_source_uri(path, Some(roots))
        .map_err(|error| error.to_string())?;
    let address = lunco_assets_core::asset_path::anchor_of(&load);
    let (authority, _) =
        lunco_assets_core::parse_twin_uri(&address).ok_or("Shader source has no Twin owner")?;
    roots
        .root_for(authority)
        .map_err(|error| error.to_string())?
        .map(Some)
        .ok_or_else(|| "Shader source Twin is retired".into())
}

pub fn retire_shader_documents(
    closed: On<lunco_workspace::TwinClosed>,
    mut registry: ResMut<ShaderRegistry>,
) {
    registry.retire_root(&closed.root);
}

/// A3 auto-bridge: hand the journal to the `ShaderRegistry` when it appears so
/// shader edits record. Reactive (`resource_added`), runs once.
pub fn wire_shader_journal_handle(
    mut registry: ResMut<ShaderRegistry>,
    journal: Res<lunco_doc_bevy::JournalResource>,
) {
    registry.set_journal(journal.clone());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_source_bumps_generation_and_round_trips() {
        let mut reg = ShaderRegistry::default();
        let id = reg
            .apply_source("shaders/foo.wesl", "v1".into(), None)
            .unwrap();
        assert_eq!(reg.source_of("shaders/foo.wesl"), Some("v1"));
        assert_eq!(reg.source_of("lunco://shaders/foo.wesl"), Some("v1"));
        let id2 = reg
            .apply_source("lunco://shaders/foo.wesl", "v2".into(), None)
            .unwrap();
        assert_eq!(id, id2, "same path reuses the doc id");
        assert_eq!(reg.source_of("shaders/foo.wesl"), Some("v2"));
    }

    #[test]
    fn shader_document_retirement_drops_only_its_owner() {
        let mut registry = ShaderRegistry::default();
        let root = std::path::PathBuf::from("/fixture/twin");
        let first = registry
            .document_for("twin://fixture/probe.wesl", "source", Some(root.clone()))
            .unwrap();
        let engine = registry
            .apply_source("shaders/probe.wesl", "engine".into(), None)
            .unwrap();
        registry.retire_root(&root);
        assert!(!registry.documents.contains_key(&first));
        assert!(registry.documents.contains_key(&engine));
        let second = registry
            .document_for("twin://fixture/probe.wesl", "fresh", Some(root))
            .unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn replay_routes_by_path_without_preexisting_doc() {
        // A peer that never saw this shader before still applies a replayed edit
        // (find-or-create by path) — no shared DocumentId needed.
        let mut reg = ShaderRegistry::default();
        let op = ShaderOp::SetSource {
            path: "shaders/bar.wesl".into(),
            source: "remote".into(),
        };
        let got = reg.apply_replayed(&op, None).expect("routes by path");
        assert_eq!(
            got,
            ("lunco://shaders/bar.wesl".to_string(), "remote".to_string())
        );
        assert_eq!(reg.source_of("shaders/bar.wesl"), Some("remote"));
    }
}
