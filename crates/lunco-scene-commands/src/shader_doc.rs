//! Shaders as a journaled, synced, live-editable **domain** — the WGSL twin of
//! rhai's `ScriptDocument` (`lunco-scripting`) and Modelica's document model.
//!
//! A shader edit is no longer a fire-and-forget `Assets<Shader>` poke: it flows
//! through a [`ShaderDocument`] whose [`ShaderRegistry`] carries a
//! [`JournalOpRecorder`](lunco_doc_bevy::JournalOpRecorder). So a live source edit
//! records a [`ShaderOp::SetSource`] into the canonical Twin journal
//! (`DomainKind::Shader`) — which means it **hot-reloads locally AND syncs +
//! persists** exactly like a rhai behaviour edit. The op carries the shader's
//! asset **path** (cross-peer-stable, unlike the locally-minted `DocumentId`), so
//! a peer routes a replayed edit to the right shader by path — no single-doc
//! limitation.
//!
//! What this module does NOT do: touch `Assets<Shader>`. The hot-reload hook
//! (`shaders.insert(handle.id(), Shader::from_wgsl(source, path))`) lives in the
//! command handler / replay system that owns `ResMut<Assets<Shader>>`; this module
//! is the source-of-truth document + journaling seam it drives.

use bevy::prelude::*;
use lunco_doc::{Document, DocumentError, DocumentHost, DocumentId, DocumentOp};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The canonical source for one shader, keyed by its asset `path`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShaderDocument {
    pub id: u64,
    pub generation: u64,
    /// Asset path (`twin://…/foo.wgsl` or `shaders/foo.wgsl`) — the identity the
    /// Bevy asset system + every `ShaderMaterial` handle key on, and the stable
    /// cross-peer routing key.
    pub path: String,
    pub source: String,
}

impl ShaderDocument {
    pub fn new(id: u64, path: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            id,
            generation: 0,
            path: path.into(),
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
/// Mirrors `ScriptRegistry` / `ModelicaDocumentRegistry`.
#[derive(Resource, Default)]
pub struct ShaderRegistry {
    documents: HashMap<DocumentId, DocumentHost<ShaderDocument>>,
    by_path: HashMap<String, DocumentId>,
    journal: Option<lunco_doc_bevy::JournalResource>,
    next_id: u64,
}

impl ShaderRegistry {
    /// Get-or-create the document id for `path` (seeding `initial_source` on
    /// first sight). Attaches a recorder when a journal is wired.
    fn document_for(&mut self, path: &str, initial_source: &str) -> DocumentId {
        if let Some(id) = self.by_path.get(path) {
            return *id;
        }
        self.next_id += 1;
        let id = DocumentId::new(self.next_id);
        self.documents.insert(
            id,
            DocumentHost::new(ShaderDocument::new(id.raw(), path, initial_source)),
        );
        self.by_path.insert(path.to_string(), id);
        self.attach_recorder(id);
        id
    }

    /// Apply a live source edit **through the `DocumentHost`** so the recorder
    /// journals a `ShaderOp::SetSource` (→ syncs + persists) and the generation
    /// bumps. The one funnel every live shader edit routes through. Returns the
    /// doc id (caller then hot-reloads `Assets<Shader>`).
    pub fn apply_source(&mut self, path: &str, source: String) -> DocumentId {
        let id = self.document_for(path, &source);
        if let Some(host) = self.documents.get_mut(&id) {
            let _ = host.apply(ShaderOp::SetSource {
                path: path.to_string(),
                source,
            });
        }
        id
    }

    /// Apply a **replayed** op (arrived via the journal, already recorded) WITHOUT
    /// re-recording — routing by the op's `path` (find-or-create). Returns
    /// `(path, source)` for the caller to hot-reload, or `None` on a bad payload.
    pub fn apply_replayed(&mut self, op: &ShaderOp) -> Option<(String, String)> {
        let ShaderOp::SetSource { path, source } = op;
        let id = self.document_for(path, source);
        let host = self.documents.get_mut(&id)?;
        host.document_mut()
            .apply(ShaderOp::SetSource {
                path: path.clone(),
                source: source.clone(),
            })
            .ok()?;
        let d = host.document();
        Some((d.path.clone(), d.source.clone()))
    }

    /// Current source for `path`, if a document exists.
    pub fn source_of(&self, path: &str) -> Option<&str> {
        self.by_path
            .get(path)
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
        let id = reg.apply_source("shaders/foo.wgsl", "v1".into());
        assert_eq!(reg.source_of("shaders/foo.wgsl"), Some("v1"));
        let id2 = reg.apply_source("shaders/foo.wgsl", "v2".into());
        assert_eq!(id, id2, "same path reuses the doc id");
        assert_eq!(reg.source_of("shaders/foo.wgsl"), Some("v2"));
    }

    #[test]
    fn replay_routes_by_path_without_preexisting_doc() {
        // A peer that never saw this shader before still applies a replayed edit
        // (find-or-create by path) — no shared DocumentId needed.
        let mut reg = ShaderRegistry::default();
        let op = ShaderOp::SetSource {
            path: "shaders/bar.wgsl".into(),
            source: "remote".into(),
        };
        let got = reg.apply_replayed(&op).expect("routes by path");
        assert_eq!(got, ("shaders/bar.wgsl".to_string(), "remote".to_string()));
        assert_eq!(reg.source_of("shaders/bar.wgsl"), Some("remote"));
    }
}
