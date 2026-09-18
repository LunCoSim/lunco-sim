//! Journaling for named **registrations** — rhai tool libraries and mission
//! timelines. Both are *name-keyed config* (not `lunco_doc::Document`s), so the
//! `JournalOpRecorder` auto-bridge doesn't apply; this module owns their op
//! vocabularies here (the orphan rule keeps `impl OpPayload` beside the op type)
//! and records **explicitly** at the `RegisterToolLibrary` / `RegisterTimeline`
//! command chokepoints.
//!
//! Neither command rides the command bus, so before journaling they didn't sync
//! at all (only native file persistence existed). Journaling them gives a
//! registration cross-peer sync **and** cross-platform persistence for free —
//! the replay leg in the sandbox installs a peer's registration. Per
//! `NETWORKING_STATE_SYNC_TAXONOMY_DESIGN.md` (Decision A: journal domain).

#![cfg(feature = "rhai")]

use lunco_doc::DocumentId;
use lunco_doc_bevy::JournalResource;
use lunco_twin_journal::{AuthorTag, DomainKind, OpPayload};
use serde::{Deserialize, Serialize};

/// A journaled rhai tool-library registration (`RegisterToolLibrary`). Full
/// snapshot (name + source); replay re-registers, hot-replacing any prior one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolLibraryOp {
    Register { name: String, source: String },
}

impl OpPayload for ToolLibraryOp {
    fn domain(&self) -> DomainKind {
        DomainKind::ToolLibrary
    }
}

/// A journaled mission-timeline registration (`RegisterTimeline`). Full snapshot
/// (name + timeline JSON); replay stores it in the `TimelineStore`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TimelineOp {
    Register { name: String, timeline: String },
}

impl OpPayload for TimelineOp {
    fn domain(&self) -> DomainKind {
        DomainKind::Timeline
    }
}

/// A stable `DocumentId` folded from a registration *name* (FNV-1a-64), so every
/// peer keys the same-named library/timeline under the same document — later
/// re-registrations of that name are the same document's history.
fn doc_id_for(name: &str) -> DocumentId {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    DocumentId::new(h)
}

/// Record a `RegisterToolLibrary` into the journal under the name's derived doc
/// id. The op is idempotent (re-register replaces), so — like the experiment
/// delete — the inverse is the same op; undo-of-register isn't a headline
/// feature and there is no `unregister` in the vocabulary.
pub fn record_tool_library(journal: &JournalResource, name: &str, source: &str) {
    let op = ToolLibraryOp::Register {
        name: name.to_string(),
        source: source.to_string(),
    };
    journal.with_write(|j| {
        if let Err(e) = j.record_op(AuthorTag::local_user(), doc_id_for(name), &op, &op, None) {
            bevy::log::warn!("[tool-library-journal] record_op failed: {e}");
        }
    });
}

/// Record a `RegisterTimeline` into the journal under the name's derived doc id.
/// Self-inverse, same rationale as [`record_tool_library`].
pub fn record_timeline(journal: &JournalResource, name: &str, timeline: &str) {
    let op = TimelineOp::Register {
        name: name.to_string(),
        timeline: timeline.to_string(),
    };
    journal.with_write(|j| {
        if let Err(e) = j.record_op(AuthorTag::local_user(), doc_id_for(name), &op, &op, None) {
            bevy::log::warn!("[timeline-journal] record_op failed: {e}");
        }
    });
}

/// Decode a replayed tool-library op → `(name, source)`. `None` (logged) if the
/// payload isn't a `ToolLibraryOp`. Replay entry point — does **not** record.
pub fn replay_tool_library(op_json: &serde_json::Value) -> Option<(String, String)> {
    match serde_json::from_value::<ToolLibraryOp>(op_json.clone()) {
        Ok(ToolLibraryOp::Register { name, source }) => Some((name, source)),
        Err(e) => {
            bevy::log::warn!("[tool-library-journal] op payload is not a ToolLibraryOp: {e}");
            None
        }
    }
}

/// Decode a replayed timeline op → `(name, timeline)`. `None` (logged) if the
/// payload isn't a `TimelineOp`. Replay entry point — does **not** record.
pub fn replay_timeline(op_json: &serde_json::Value) -> Option<(String, String)> {
    match serde_json::from_value::<TimelineOp>(op_json.clone()) {
        Ok(TimelineOp::Register { name, timeline }) => Some((name, timeline)),
        Err(e) => {
            bevy::log::warn!("[timeline-journal] op payload is not a TimelineOp: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_library_round_trips_and_declares_domain() {
        let op = ToolLibraryOp::Register {
            name: "formation".into(),
            source: "fn pick() {}".into(),
        };
        assert_eq!(op.domain(), DomainKind::ToolLibrary);
        let json = serde_json::to_value(&op).unwrap();
        let (name, source) = replay_tool_library(&json).expect("decodes");
        assert_eq!(name, "formation");
        assert_eq!(source, "fn pick() {}");
    }

    #[test]
    fn timeline_round_trips_and_declares_domain() {
        let op = TimelineOp::Register {
            name: "descent".into(),
            timeline: "[]".into(),
        };
        assert_eq!(op.domain(), DomainKind::Timeline);
        let json = serde_json::to_value(&op).unwrap();
        let (name, timeline) = replay_timeline(&json).expect("decodes");
        assert_eq!(name, "descent");
        assert_eq!(timeline, "[]");
    }

    #[test]
    fn doc_id_is_name_stable_and_distinct() {
        assert_eq!(doc_id_for("a"), doc_id_for("a"));
        assert_ne!(doc_id_for("a"), doc_id_for("b"));
    }

    #[test]
    fn bad_payloads_rejected_softly() {
        assert!(replay_tool_library(&serde_json::json!({ "nope": 1 })).is_none());
        assert!(replay_timeline(&serde_json::json!({ "nope": 1 })).is_none());
    }
}
